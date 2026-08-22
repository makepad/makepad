//! The whole conversion, end to end, on a clip this test makes itself: a
//! noise field translating a known number of pixels per frame, encoded with
//! the platform codec seam, converted, and then read back exactly the way the
//! player reads it (find the box, parse the header, unpack the planes).
//!
//! What it pins: the payload is byte-compatible with the player's parser, the
//! geometry maps onto the video the way `flow_warp::endpoint_stride` needs,
//! and the measured field really is the motion that went in — sign, scale and
//! units together.

#![cfg(all(feature = "convert", any(target_os = "macos", target_os = "windows")))]

use makepad_video::{VideoFileCodec, VideoFileDecoder, VideoFileEncoder, VideoFileEncoderOptions};
use makepad_video_flow::{
    convert_video, find_mkfl_box, parse_flow_payload, ConvertOptions, ConvertProgress, PLANES,
};
use std::path::PathBuf;

const W: usize = 320;
const H: usize = 192;
const FRAMES: usize = 20;
/// Source pixels of rightward motion per frame: 8 px = 2 grid cells, so the
/// half-way vectors are ±1 grid cell = ±4 stored quarter-cell units.
const STEP: i32 = 8;

fn hash2(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374761393) ^ (y as u32).wrapping_mul(668265263);
    h ^= h >> 13;
    h = h.wrapping_mul(1274126177);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535.0
}

fn noise(fx: f32, fy: f32, cell: f32) -> f32 {
    let (gx, gy) = ((fx / cell).floor(), (fy / cell).floor());
    let (tx, ty) = (fx / cell - gx, fy / cell - gy);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let (ix, iy) = (gx as i32, gy as i32);
    let (a, b) = (hash2(ix, iy), hash2(ix + 1, iy));
    let (c, d) = (hash2(ix, iy + 1), hash2(ix + 1, iy + 1));
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sy
}

fn frame_rgb(shift: i32) -> Vec<u8> {
    let mut out = vec![0u8; W * H * 3];
    for y in 0..H {
        for x in 0..W {
            let fx = (x as i32 + shift) as f32;
            let fy = y as f32;
            let v = 0.6 * noise(fx, fy, 20.0) + 0.4 * noise(fx, fy, 6.0);
            let px = (v * 255.0).clamp(0.0, 255.0) as u8;
            let at = (y * W + x) * 3;
            // A little colour so the encode is not a grey special case.
            out[at] = px;
            out[at + 1] = px.wrapping_add(20);
            out[at + 2] = 255 - px;
        }
    }
    out
}

fn tmp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "video-flow-convert-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_source(path: &std::path::Path) {
    let mut encoder = VideoFileEncoder::new(
        path.to_str().unwrap(),
        VideoFileEncoderOptions {
            codec: VideoFileCodec::H264,
            width: W as u32,
            height: H as u32,
            fps_num: 30,
            fps_den: 1,
            video_bitrate_bps: 20_000_000,
            audio: None,
            keyframe_only: false,
        },
    )
    .expect("source encoder");
    for i in 0..FRAMES {
        // Sampling the pattern further LEFT each frame moves the picture
        // RIGHT, which is the direction the assertions below are written in.
        encoder
            .push_frame_rgb8(&frame_rgb(-(i as i32) * STEP), None)
            .expect("push source frame");
    }
    encoder.finish().expect("finish source");
}

#[test]
fn a_converted_clip_carries_the_motion_that_went_into_it() {
    let dir = tmp_dir();
    let src = dir.join("src.mp4");
    let out = dir.join("out.mp4");
    write_source(&src);

    let mut seen: Vec<ConvertProgress> = Vec::new();
    let report = convert_video(
        &src,
        &out,
        &ConvertOptions::default(),
        &mut |p| seen.push(p),
        &|| false,
    )
    .expect("conversion");

    assert_eq!((report.width, report.height), (W as u32, H as u32));
    assert_eq!(report.scale, 1, "a small clip needs no downscale");
    assert!(report.frames >= FRAMES - 1, "decoded {} of {FRAMES}", report.frames);
    assert_eq!(report.pairs as usize, report.frames - 1);
    assert!(report.warps, "a 20-frame 320x192 clip must warp: {}", report.warp_note);
    assert_eq!((report.fps_num, report.fps_den), (30, 1));
    assert!(seen.first().map(|p| p.fraction) == Some(0.0));
    assert!(seen.last().map(|p| p.fraction) == Some(1.0));

    // Read it back exactly as the player does.
    let bytes = std::fs::read(&out).expect("read output");
    let payload = find_mkfl_box(&bytes).expect("the output carries an mkfl box");
    let (header, samples) = parse_flow_payload(payload).expect("payload parses");
    assert_eq!(header.pairs, report.pairs);
    assert_eq!((header.vid_w, header.vid_h), (W as u16, H as u16));
    assert_eq!((header.grid_w, header.grid_h), ((W / 4) as u16, (H / 4) as u16));
    assert_eq!((header.fps_num, header.fps_den), (30, 1));

    // The video half must still be an ordinary decodable mp4 of the same
    // geometry — the box is a passenger, not a corruption.
    let mut decoder = VideoFileDecoder::open(out.to_str().unwrap()).expect("decode converted");
    assert_eq!(decoder.info().width, W as u32);
    assert_eq!(decoder.info().height, H as u32);
    let mut decoded = 0usize;
    while decoder.next_frame().expect("decode frame").is_some() {
        decoded += 1;
    }
    assert!(
        decoded + 1 >= report.frames,
        "converted clip decoded {decoded} of {} frames",
        report.frames
    );

    // The middle pair's interior: +8 source px per frame is +2 grid cells,
    // so the t=0.5 vectors are -1 and +1 grid cell = -4 and +4 stored units.
    let grid_plane = header.grid_w as usize * header.grid_h as usize;
    let pair = header.pairs as usize / 2;
    let at = pair * header.pair_stride();
    let plane = |index: usize| -> &[u8] {
        &samples[at + index * grid_plane..at + (index + 1) * grid_plane]
    };
    let (gw, gh) = (header.grid_w as usize, header.grid_h as usize);
    let mean = |bytes: &[u8]| -> f32 {
        let (mut sum, mut n) = (0.0f32, 0.0f32);
        for y in 4..gh - 4 {
            for x in 4..gw - 4 {
                sum += bytes[y * gw + x] as i8 as f32;
                n += 1.0;
            }
        }
        sum / n
    };
    let f0x = mean(plane(0));
    let f0y = mean(plane(1));
    let f1x = mean(plane(2));
    let f1y = mean(plane(3));
    assert!((f0x + 4.0).abs() < 1.2, "f0x {f0x}, expected -4 quarter-cell units");
    assert!((f1x - 4.0).abs() < 1.2, "f1x {f1x}, expected +4 quarter-cell units");
    assert!(f0y.abs() < 1.2 && f1y.abs() < 1.2, "vertical drift {f0y}/{f1y}");
    assert_eq!(samples.len(), header.pairs as usize * grid_plane * PLANES);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_cancelled_conversion_leaves_nothing_behind() {
    let dir = tmp_dir();
    let src = dir.join("src.mp4");
    let out = dir.join("out.mp4");
    write_source(&src);
    let err = convert_video(
        &src,
        &out,
        &ConvertOptions::default(),
        &mut |_| {},
        &|| true,
    )
    .expect_err("a cancel must not produce a clip");
    assert_eq!(err, makepad_video_flow::ConvertError::Cancelled);
    assert!(!out.exists(), "a cancelled conversion left a partial file");
    std::fs::remove_dir_all(&dir).ok();
}
