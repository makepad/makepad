//! Representative video thumbnail + measured duration, via the platform's
//! standalone hardware decode seam (no Cx, no window — the same
//! `VideoFileDecoder` the VJ's slot players use).

use crate::thumbs::{encode_jpeg_bgra, placeholder_bgra_512, THUMB_DIM};
use makepad_platform::video_file::{nv12, VideoFileDecoder};

pub struct VideoProbe {
    /// 512×512 JPEG (first frame letterboxed, or the honest placeholder
    /// when decode fails).
    pub thumbnail_jpeg: Vec<u8>,
    pub duration_ms: u32,
    pub width: u32,
    pub height: u32,
    /// False when the placeholder was used.
    pub real_frame: bool,
}

/// Decode the first frame of an mp4 and letterbox it into a 512×512 JPEG.
pub fn probe_video(path: &std::path::Path) -> Result<VideoProbe, String> {
    let path_text = path.to_str().ok_or("non-utf8 path")?;
    let mut decoder = VideoFileDecoder::open(path_text).map_err(|e| e.to_string())?;
    let info = decoder.info().clone();
    let duration_ms =
        ((info.duration_100ns.max(0) as u64) / 10_000).min(u32::MAX as u64) as u32;
    let frame = decoder.next_frame().map_err(|e| e.to_string())?;
    let (bgra, real_frame) = match frame {
        Some(frame) => {
            let mut rgb = Vec::new();
            nv12::nv12_to_rgb8(&frame.nv12, frame.width, frame.height, &mut rgb);
            (
                letterbox_512(&rgb, frame.width as usize, frame.height as usize),
                true,
            )
        }
        None => (placeholder_bgra_512(), false),
    };
    let thumbnail_jpeg = encode_jpeg_bgra(&bgra, THUMB_DIM, THUMB_DIM)?;
    Ok(VideoProbe {
        thumbnail_jpeg,
        duration_ms,
        width: info.width,
        height: info.height,
        real_frame,
    })
}

/// Nearest-neighbour letterbox of an RGB8 frame into a 512×512 BGRA tile.
fn letterbox_512(rgb: &[u8], width: usize, height: usize) -> Vec<u32> {
    const BG: u32 = 0xff00_0000;
    let mut out = vec![BG; THUMB_DIM * THUMB_DIM];
    if width == 0 || height == 0 || rgb.len() < width * height * 3 {
        return out;
    }
    let scale = (THUMB_DIM as f64 / width as f64).min(THUMB_DIM as f64 / height as f64);
    let draw_w = ((width as f64 * scale) as usize).max(1).min(THUMB_DIM);
    let draw_h = ((height as f64 * scale) as usize).max(1).min(THUMB_DIM);
    let off_x = (THUMB_DIM - draw_w) / 2;
    let off_y = (THUMB_DIM - draw_h) / 2;
    for y in 0..draw_h {
        let src_y = (y * height / draw_h).min(height - 1);
        for x in 0..draw_w {
            let src_x = (x * width / draw_w).min(width - 1);
            let s = (src_y * width + src_x) * 3;
            let pixel = 0xff00_0000
                | ((rgb[s] as u32) << 16)
                | ((rgb[s + 1] as u32) << 8)
                | rgb[s + 2] as u32;
            out[(off_y + y) * THUMB_DIM + off_x + x] = pixel;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_platform::video_file::{
        VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
    };

    #[test]
    fn first_frame_thumb_roundtrips_through_a_real_mp4() {
        // Encode a tiny real mp4 with the platform encoder, then probe it.
        let dir = std::env::temp_dir().join(format!("vj_worker_thumb_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("probe.mp4");
        let (w, h) = (128u32, 64u32);
        let mut encoder = VideoFileEncoder::new(
            path.to_str().unwrap(),
            VideoFileEncoderOptions {
                codec: VideoFileCodec::H264,
                width: w,
                height: h,
                fps_num: 24,
                fps_den: 1,
                video_bitrate_bps: 4_000_000,
                audio: None,
                ..Default::default()
            },
        )
        .expect("encoder");
        // Bright frames so the decoded pixels are clearly non-black.
        let rgb = vec![200u8; (w * h * 3) as usize];
        for _ in 0..12 {
            encoder.push_frame_rgb8(&rgb, None).expect("frame");
        }
        encoder.finish().expect("finish");

        let probe = probe_video(&path).expect("probe");
        assert!(probe.real_frame);
        assert_eq!((probe.width, probe.height), (w, h));
        assert!(probe.duration_ms > 200, "measured duration, got {}", probe.duration_ms);
        assert_eq!(
            crate::thumbs::jpeg_dims(&probe.thumbnail_jpeg),
            Some((THUMB_DIM as u32, THUMB_DIM as u32))
        );
    }
}
