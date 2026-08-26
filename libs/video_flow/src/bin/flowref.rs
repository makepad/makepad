//! DEBUG RIG (never committed): the EYE'S REFERENCE — sine_ease rendered
//! analytically at 120 fps, many loops long, one file. What perfectly
//! smooth playback of that clip must look like; open beside the VJ app
//! and stare.

use makepad_video::{VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions};

const W: usize = 640;
const H: usize = 360;
const SQ: f32 = 64.0;
const OUT_FPS: u32 = 120;
const CLIP_FRAMES: f32 = 72.0; // the 12fps clip's frame count (2 sine periods)
const LOOPS: usize = 30;

fn span_overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| {
        format!(
            "{}/../../local/flowtest/sine_ease_reference_120fps.mp4",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    let mut enc = VideoFileEncoder::new(
        &out,
        VideoFileEncoderOptions {
            codec: VideoFileCodec::H264,
            width: W as u32,
            height: H as u32,
            fps_num: OUT_FPS,
            fps_den: 1,
            video_bitrate_bps: 12_000_000,
            audio: None,
            keyframe_only: false,
        },
    )
    .expect("encoder");
    let total = LOOPS * 72 * (OUT_FPS as usize) / 12;
    let mut px = vec![0f32; W * H];
    let mut rgb = vec![0u8; W * H * 3];
    let sq_y = (H as f32 - SQ) * 0.5;
    for k in 0..total {
        // Media time advances 12/120 = 0.1 source frames per output frame;
        // the sine is periodic every 36 source frames, so the loop seam is
        // continuous by construction.
        let m = k as f32 * 12.0 / OUT_FPS as f32;
        let phase = std::f32::consts::TAU * 2.0 * m / CLIP_FRAMES;
        let x0 = W as f32 * 0.5 - SQ * 0.5 + 96.0 * phase.sin();
        px.fill(0.0);
        let (x1, y0, y1) = (x0 + SQ, sq_y, sq_y + SQ);
        let ix0 = (x0.floor() as i64).clamp(0, W as i64) as usize;
        let ix1 = (x1.ceil() as i64).clamp(0, W as i64) as usize;
        let iy0 = (y0.floor() as i64).clamp(0, H as i64) as usize;
        let iy1 = (y1.ceil() as i64).clamp(0, H as i64) as usize;
        for y in iy0..iy1 {
            let cy = span_overlap(y as f32, y as f32 + 1.0, y0, y1);
            for x in ix0..ix1 {
                let cx = span_overlap(x as f32, x as f32 + 1.0, x0, x1);
                px[y * W + x] = (cx * cy).min(1.0);
            }
        }
        for (i, v) in px.iter().enumerate() {
            let b = (v * 255.0 + 0.5) as u8;
            rgb[i * 3] = b;
            rgb[i * 3 + 1] = b;
            rgb[i * 3 + 2] = b;
        }
        enc.push_frame_rgb8(&rgb, None).expect("push");
        if k % 2400 == 0 {
            println!("{k}/{total}");
        }
    }
    enc.finish().expect("finish");
    println!("wrote {out} — {total} frames at {OUT_FPS} fps ({LOOPS} clip-loops)");
}
