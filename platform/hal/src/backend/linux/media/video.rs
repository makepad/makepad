//! Frames cross from decode workers to the HAL as owned leases, never native Cx textures.
use crate::{
    media::{camera_layout, read_camera_frame, yuv},
    types::format,
};
use std::{
    any::Any,
    os::fd::{AsRawFd, OwnedFd},
    sync::Arc,
};

pub struct Prepared {
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub seekable: bool,
    pub video_tracks: Vec<String>,
    pub audio_tracks: Vec<String>,
}
impl Prepared {
    pub fn new(
        width: u32,
        height: u32,
        duration: u128,
        seekable: bool,
        video_tracks: Vec<String>,
        audio_tracks: Vec<String>,
    ) -> Self {
        Self {
            width,
            height,
            duration_ms: duration as u64,
            seekable,
            video_tracks,
            audio_tracks,
        }
    }
}

pub struct DmaPlane {
    pub fd: OwnedFd,
    pub modifier: u64,
    pub offset: u64,
    pub pitch: u64,
    pub size: u64,
    /// The decoder may recycle the buffer only after the last submitted image is retired.
    pub _sample: Arc<dyn Any + Send + Sync>,
}
pub enum Storage {
    Bytes(Arc<[u8]>),
    Dma(Arc<DmaPlane>),
}
pub struct Plane {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub storage: Storage,
}
impl Plane {
    pub fn packed(
        width: u32,
        height: u32,
        format: u32,
        data: &[u8],
        stride: usize,
    ) -> Option<Self> {
        let row = crate::types::format::level_bytes(format, width, 1) as usize;
        if width == 0 || height == 0 || stride < row {
            return None;
        }
        let mut bytes = Vec::with_capacity(row.checked_mul(height as usize)?);
        for y in 0..height as usize {
            let off = y.checked_mul(stride)?;
            bytes.extend_from_slice(data.get(off..off.checked_add(row)?)?);
        }
        Some(Self {
            width,
            height,
            format,
            storage: Storage::Bytes(bytes.into()),
        })
    }
    pub fn dummy() -> Self {
        Self {
            width: 1,
            height: 1,
            format: format::R8,
            storage: Storage::Bytes(Arc::from([0u8])),
        }
    }
}
pub struct Frame {
    pub planes: Vec<Plane>,
    pub yuv: u32,
    pub matrix: f32,
}
impl Frame {
    /// Linux dma-buf POLLIN observes the producer's write fences. Poll on the decode
    /// worker and keep this sample until ready; never wait inside the HAL state lock.
    /// The sample lease then prevents recycling until all Vulkan read fences complete.
    pub fn ready(&self) -> Result<bool, String> {
        for plane in &self.planes {
            let Storage::Dma(dma) = &plane.storage else {
                continue;
            };
            let mut p = crate::backend::linux::io::PollFd {
                fd: dma.fd.as_raw_fd(),
                events: crate::backend::linux::io::READ,
                revents: 0,
            };
            let result = unsafe { crate::backend::linux::io::poll(&mut p, 1, 0) };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    return Ok(false);
                }
                return Err(error.to_string());
            }
            // POLLERR | POLLHUP | POLLNVAL: do not sample storage with a failed fence.
            if p.revents & 0x38 != 0 {
                return Err("DMA-BUF producer fence failed".into());
            }
            if p.revents & crate::backend::linux::io::READ == 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Preview conversion is worker work; application capture receives the original packet.
pub fn camera_preview(bytes: &[u8]) -> Option<Frame> {
    let frame = read_camera_frame(bytes)?;
    let (w, h) = (frame.width, frame.height);
    let first = frame.planes.first()?;
    let mut planes = Vec::new();
    let mut bits = yuv::ENABLED;
    match frame.layout {
        camera_layout::I420 | camera_layout::NV12 => {
            planes.push(Plane::packed(
                w,
                h,
                format::R8,
                first.bytes,
                first.row_stride as usize,
            )?);
            let second = frame.planes.get(1)?;
            let biplanar = frame.layout == camera_layout::NV12;
            planes.push(Plane::packed(
                w.div_ceil(2),
                h.div_ceil(2),
                if biplanar { format::RG8 } else { format::R8 },
                second.bytes,
                second.row_stride as usize,
            )?);
            if biplanar {
                planes.push(Plane::dummy());
                bits |= yuv::BIPLANAR;
            } else {
                let third = frame.planes.get(2)?;
                planes.push(Plane::packed(
                    w.div_ceil(2),
                    h.div_ceil(2),
                    format::R8,
                    third.bytes,
                    third.row_stride as usize,
                )?);
            }
        }
        camera_layout::YUY2 => {
            let mut y = vec![0; w as usize * h as usize];
            let cw = w.div_ceil(2) as usize;
            let ch = h.div_ceil(2) as usize;
            let mut u = vec![0; cw * ch];
            let mut v = vec![0; cw * ch];
            for row in 0..h as usize {
                for col in 0..w as usize {
                    y[row * w as usize + col] =
                        *first.bytes.get(row * first.row_stride as usize + col * 2)?;
                }
            }
            for row in 0..ch {
                for col in 0..cw {
                    let off = (row * 2) * first.row_stride as usize + col * 4;
                    u[row * cw + col] = *first.bytes.get(off + 1)?;
                    v[row * cw + col] = *first.bytes.get(off + 3)?;
                }
            }
            planes.push(Plane::packed(w, h, format::R8, &y, w as usize)?);
            planes.push(Plane::packed(cw as u32, ch as u32, format::R8, &u, cw)?);
            planes.push(Plane::packed(cw as u32, ch as u32, format::R8, &v, cw)?);
        }
        camera_layout::MJPEG => {
            use makepad_zune_jpeg::makepad_zune_core::{
                bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions,
            };
            let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::BGRA);
            let mut decoder = makepad_zune_jpeg::JpegDecoder::new_with_options(
                ZCursor::new(first.bytes),
                options,
            );
            decoder.decode_headers().ok()?;
            let (width, height) = decoder.dimensions()?;
            if (width, height) != (w as usize, h as usize) {
                return None;
            }
            let mut pixels = vec![0; width.checked_mul(height)?.checked_mul(4)?];
            decoder.decode_into(&mut pixels).ok()?;
            planes.push(Plane::packed(w, h, format::BGRA8, &pixels, width * 4)?);
            bits = 0;
        }
        camera_layout::RGB24 | camera_layout::GRAY => {
            let mut pixels = Vec::with_capacity(w as usize * h as usize * 4);
            for row in 0..h as usize {
                for col in 0..w as usize {
                    let off = row * first.row_stride as usize + col * first.pixel_stride as usize;
                    let r = *first.bytes.get(off)?;
                    let (g, b) = if frame.layout == camera_layout::RGB24 {
                        (*first.bytes.get(off + 1)?, *first.bytes.get(off + 2)?)
                    } else {
                        (r, r)
                    };
                    pixels.extend_from_slice(&[r, g, b, 255]);
                }
            }
            planes.push(Plane::packed(w, h, format::RGBA8, &pixels, w as usize * 4)?);
            bits = 0;
        }
        _ => return None,
    }
    Some(Frame {
        planes,
        yuv: bits,
        matrix: frame.matrix as f32,
    })
}
