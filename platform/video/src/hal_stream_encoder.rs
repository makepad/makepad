//! Linux and Android backend of [`crate::VideoStreamEncoder`]: the HAL's hardware stream encoder
//! (`makepad_hal::stream_encode`). Linux: the GPU's H.264/H.265 encoder through Vulkan Video on
//! the HAL's own Vulkan device; pictures come from a shared surface (a registered DMA-BUF holding
//! NV12 packed into BGRA8 texels, `SurfaceFormat::PackedNv12`) without a pixel
//! crossing the CPU, or from CPU NV12. Android: the phone's MediaCodec encoder, CPU NV12 only;
//! it may keep one picture back, so a push can return no packet (and the next one the picture
//! before it): a packet is a key frame when it begins with its parameter sets.

use crate::stream_encoder::{EncodeTimings, EncodedPacket, EncoderCaps, QpGrid, StreamEncoderOs, StreamTile, StreamTuning, StreamVideoCodec, VideoStreamEncoderOptions};
use crate::surface::{GpuCompletion, SurfaceHandle};
#[cfg(target_os = "linux")]
use crate::surface::{SurfaceFormat, SurfaceKind};
use crate::VideoFileError;
use makepad_hal::stream_encode as hal;
#[cfg(target_os = "linux")]
use std::os::fd::IntoRawFd;

fn last_error() -> VideoFileError {
    let mut buf = vec![0u8; 1024];
    let n = hal::hal_stream_encoder_error(&mut buf) as usize;
    buf.truncate(n.min(1024));
    VideoFileError::new(String::from_utf8_lossy(&buf).into_owned())
}

pub(crate) struct HalStreamEncoder {
    id: u64,
    hevc: bool,
    key_wanted: bool,
    /// The next texture picture is a refinement (Linux's).
    #[cfg(target_os = "linux")]
    refine_wanted: bool,
    keyint: u32,
    since_key: u32,
    out: Vec<u8>,
}

impl HalStreamEncoder {
    /// On encode engine `engine` (`EncoderCaps::engines`).
    fn new(options: &VideoStreamEncoderOptions, engine: u32, tuning: StreamTuning) -> Result<Self, VideoFileError> {
        if cfg!(target_os = "linux") && options.width % 4 != 0 {
            return Err(VideoFileError::new(format!("stream width {} must be a multiple of 4", options.width)));
        }
        let flags = if options.full_chroma { hal::FULL_CHROMA } else { 0 } | if options.codec == StreamVideoCodec::Hevc { hal::HEVC } else { 0 } | if tuning.constant_rate { hal::CBR } else { 0 } | if tuning.sharp { hal::SHARP } else { 0 };
        let id = hal::hal_stream_encoder_open(options.width, options.height, options.fps, options.bitrate_kbps, engine, flags, tuning.quality, tuning.buffer_pictures);
        if id == 0 {
            return Err(last_error());
        }
        Ok(Self { id, hevc: options.codec == StreamVideoCodec::Hevc, key_wanted: true, #[cfg(target_os = "linux")] refine_wanted: false, keyint: options.keyint, since_key: 0, out: Vec::new() })
    }

    fn info(&self) -> String {
        let mut buf = vec![0u8; 512];
        let n = hal::hal_stream_encoder_info(self.id, &mut buf) as usize;
        buf.truncate(n.min(512));
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn flags(&mut self) -> (u32, bool) {
        let key = self.key_wanted || (self.keyint > 0 && self.since_key + 1 >= self.keyint);
        self.key_wanted = false;
        self.since_key = if key { 0 } else { self.since_key + 1 };
        (if key { hal::KEY } else { 0 }, key)
    }

    fn take(&mut self, bytes: i64, key: bool, pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        if bytes < 0 {
            // a failed picture: the encoder restarts the stream with a key frame
            self.key_wanted = true;
            return Err(last_error());
        }
        self.out.resize(bytes as usize, 0);
        let n = hal::hal_stream_encoder_read(self.id, &mut self.out);
        if n < 0 {
            return Err(VideoFileError::new("stream encoder output lost"));
        }
        let data = std::mem::take(&mut self.out);
        #[cfg(target_os = "android")]
        {
            // MediaCodec may hand the picture over a call late: none now, and a packet's key-ness
            // is what it begins with (the HAL puts the parameter sets in front of each key frame)
            let _ = key;
            if data.is_empty() {
                return Ok(Vec::new());
            }
            let is_key = makepad_hal::stream_pack::starts_with_parameter_set(&data, self.hevc);
            return Ok(vec![EncodedPacket { data, pts_100ns, is_key }]);
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = self.hevc;
            Ok(vec![EncodedPacket { data, pts_100ns, is_key: key }])
        }
    }
}

impl StreamEncoderOs for HalStreamEncoder {
    fn caps() -> EncoderCaps {
        #[cfg(target_os = "linux")]
        return EncoderCaps { engines: hal::hal_stream_encoder_engines().max(1), tiles: true, full_chroma: true, hevc: true };
        #[cfg(target_os = "android")]
        return EncoderCaps { engines: 1, tiles: false, full_chroma: false, hevc: true };
    }

    fn max_size(codec: StreamVideoCodec, full_chroma: bool) -> Option<[u32; 2]> {
        #[cfg(target_os = "linux")]
        {
            let v = hal::hal_stream_encoder_max_size(if full_chroma { hal::FULL_CHROMA } else { 0 } | if codec == StreamVideoCodec::Hevc { hal::HEVC } else { 0 });
            return (v != 0).then(|| [(v >> 32) as u32, v as u32]);
        }
        #[cfg(target_os = "android")]
        {
            let _ = (codec, full_chroma);
            return None;
        }
    }

    fn open(options: &VideoStreamEncoderOptions, tile: StreamTile, tuning: StreamTuning) -> Result<Self, VideoFileError> {
        Self::new(options, tile.engine, tuning)
    }

    fn encode_nv12(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        let (flags, key) = self.flags();
        let bytes = hal::hal_stream_encoder_nv12(self.id, nv12, flags);
        self.take(bytes, key, pts_100ns)
    }

    /// The tile of a packed NV12 DMA-BUF picture; the GPU waits for `ready`'s sync_file.
    fn encode_surface(&mut self, surface: &SurfaceHandle, tile: StreamTile, ready: &GpuCompletion, pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        #[cfg(target_os = "linux")]
        {
            if surface.kind != SurfaceKind::DmaBuf || surface.format != SurfaceFormat::PackedNv12 {
                return Err(VideoFileError::new(format!("the stream encoder takes packed NV12 DMA-BUFs, not {surface:?}")));
            }
            // the frame is on the queue (its sync_file published): the encode waits on the GPU
            ready.wait(std::time::Duration::from_secs(2)).map_err(VideoFileError::new)?;
            let sync = ready.sync_file().map_or(-1, |fd| fd.into_raw_fd());
            let (flags, key) = self.flags();
            let refine = if std::mem::take(&mut self.refine_wanted) && !key { hal::REFINE } else { 0 };
            let bytes = hal::hal_stream_encoder_texture(self.id, surface.id, tile.source_height, tile.x0, tile.y0, sync, flags | refine);
            return self.take(bytes, key, pts_100ns);
        }
        #[cfg(target_os = "android")]
        {
            let _ = (surface, tile, ready, pts_100ns);
            return Err(VideoFileError::new("the phone's stream encoder takes CPU pictures"));
        }
    }

    fn request_keyframe(&mut self) {
        self.key_wanted = true;
    }

    fn request_refinement(&mut self) {
        #[cfg(target_os = "linux")]
        {
            self.refine_wanted = true;
        }
    }

    fn timings(&self) -> EncodeTimings {
        EncodeTimings { encode_us: (hal::hal_stream_encoder_last_gpu_ns(self.id) / 1000) as u32, ..Default::default() }
    }

    fn set_bitrate_kbps(&mut self, kbps: u32) {
        hal::hal_stream_encoder_bitrate(self.id, kbps);
    }

    fn set_max_qp(&mut self, qp: u32) {
        hal::hal_stream_encoder_max_qp(self.id, qp);
    }

    /// (columns, rows, texel px, delta range) of the QP delta map, when the encoder takes one.
    fn qp_grid(&self) -> Option<QpGrid> {
        let mut v = [0i32; 6];
        hal::hal_stream_encoder_qp_grid(self.id, &mut v).then(|| QpGrid { cols: v[0] as u32, rows: v[1] as u32, texel: [v[2] as u32, v[3] as u32], min: v[4], max: v[5] })
    }

    fn set_qp_map(&mut self, deltas: &[i8]) -> bool {
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(deltas.as_ptr().cast(), deltas.len()) };
        hal::hal_stream_encoder_qp_map(self.id, bytes)
    }

    fn description(&self) -> String {
        self.info()
    }
}

impl Drop for HalStreamEncoder {
    fn drop(&mut self) {
        hal::hal_stream_encoder_close(self.id);
    }
}
