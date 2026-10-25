//! Linux backend of [`crate::VideoStreamEncoder`]: the GPU's hardware H.264 encoder through
//! Vulkan Video, on the HAL's own Vulkan device (`makepad_hal::stream_encode`). Pictures come
//! from a shared texture (a registered DMA-BUF holding NV12 packed into BGRA8 texels, see
//! [`crate::GpuPictureMaker::nv12_picture`]) without a pixel crossing the CPU, or from CPU NV12.

use crate::stream_encoder::{EncodedPacket, GpuReady, StreamTuning, StreamVideoCodec, VideoStreamEncoderOptions};
use crate::VideoFileError;
use makepad_hal::stream_encode as hal;
use std::os::fd::IntoRawFd;

fn last_error() -> VideoFileError {
    let mut buf = vec![0u8; 1024];
    let n = hal::hal_stream_encoder_error(&mut buf) as usize;
    buf.truncate(n.min(1024));
    VideoFileError::new(String::from_utf8_lossy(&buf).into_owned())
}

pub(crate) struct LinuxStreamEncoder {
    id: u64,
    key_wanted: bool,
    refine_wanted: bool,
    keyint: u32,
    since_key: u32,
    out: Vec<u8>,
}

impl LinuxStreamEncoder {
    /// On encode engine `engine` (`crate::VideoStreamEncoder::engines`).
    pub fn new(options: &VideoStreamEncoderOptions, engine: u32, tuning: StreamTuning) -> Result<Self, VideoFileError> {
        if options.width % 4 != 0 {
            return Err(VideoFileError::new(format!("stream width {} must be a multiple of 4", options.width)));
        }
        let flags = if options.full_chroma { hal::FULL_CHROMA } else { 0 } | if options.codec == StreamVideoCodec::Hevc { hal::HEVC } else { 0 } | if tuning.constant_rate { hal::CBR } else { 0 } | if tuning.sharp { hal::SHARP } else { 0 };
        let id = hal::hal_stream_encoder_open(options.width, options.height, options.fps, options.bitrate_kbps, engine, flags, tuning.quality, tuning.buffer_pictures);
        if id == 0 {
            return Err(last_error());
        }
        Ok(Self { id, key_wanted: true, refine_wanted: false, keyint: options.keyint, since_key: 0, out: Vec::new() })
    }

    pub fn description(&self) -> String {
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
        Ok(vec![EncodedPacket { data: std::mem::take(&mut self.out), pts_100ns, is_key: key }])
    }

    pub fn push_frame_nv12(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<Vec<EncodedPacket>, VideoFileError> {
        let (flags, key) = self.flags();
        let bytes = hal::hal_stream_encoder_nv12(self.id, nv12, flags);
        self.take(bytes, key, pts_100ns)
    }

    /// Its size of the `source_height` tall picture in `handle`, from column `x0`, row `y0`.
    pub fn push_shared_texture(&mut self, handle: u64, ready: GpuReady, pts_100ns: i64, source_height: u32, x0: u32, y0: u32) -> Result<Vec<EncodedPacket>, VideoFileError> {
        let sync = match ready {
            GpuReady::Done => -1,
            GpuReady::SyncFile(fd) => fd.into_raw_fd(),
        };
        let (flags, key) = self.flags();
        let refine = if std::mem::take(&mut self.refine_wanted) && !key { hal::REFINE } else { 0 };
        let bytes = hal::hal_stream_encoder_texture(self.id, handle, source_height, x0, y0, sync, flags | refine);
        self.take(bytes, key, pts_100ns)
    }

    pub fn request_keyframe(&mut self) {
        self.key_wanted = true;
    }

    pub fn request_refinement(&mut self) {
        self.refine_wanted = true;
    }

    pub fn last_gpu_us(&self) -> u32 {
        (hal::hal_stream_encoder_last_gpu_ns(self.id) / 1000) as u32
    }

    pub fn set_bitrate_kbps(&mut self, kbps: u32) {
        hal::hal_stream_encoder_bitrate(self.id, kbps);
    }
}

impl Drop for LinuxStreamEncoder {
    fn drop(&mut self) {
        hal::hal_stream_encoder_close(self.id);
    }
}
