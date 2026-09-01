//! Round-trip for the no-temp-file first-frame path: a single-intra-frame
//! HEVC (and H.264) file written by `VideoFileEncoder` is read back as BYTES
//! and decoded straight from RAM via `decode_first_frame_from_bytes` —
//! demux + VideoToolbox stream session on macOS, a Media Foundation byte
//! stream on Windows. Only run for real on macOS (the platform this agent
//! can execute on); the Windows backend is compile-checked only.

#![cfg(target_os = "macos")]

use makepad_video::{
    decode_first_frame_from_bytes, mp4_first_frame, nv12, VideoFileCodec, VideoFileEncoder,
    VideoFileEncoderOptions,
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

fn synthetic_frame_rgb8() -> Vec<u8> {
    let mut out = vec![0u8; WIDTH as usize * HEIGHT as usize * 3];
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let idx = (y * WIDTH as usize + x) * 3;
            out[idx] = (x % 256) as u8;
            out[idx + 1] = (y % 256) as u8;
            out[idx + 2] = ((x + y) % 256) as u8;
        }
    }
    out
}

fn one_frame_file(codec: VideoFileCodec) -> Vec<u8> {
    let dir = std::env::temp_dir().join("makepad-video-first-frame-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("one-{codec:?}.mov"));
    let mut encoder = VideoFileEncoder::new(
        path.to_str().unwrap(),
        VideoFileEncoderOptions {
            codec,
            width: WIDTH,
            height: HEIGHT,
            fps_num: 30,
            fps_den: 1,
            video_bitrate_bps: 4_000_000,
            audio: None,
            keyframe_only: true,
        },
    )
    .unwrap();
    encoder.push_frame_rgb8(&synthetic_frame_rgb8(), None).unwrap();
    encoder.finish().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    bytes
}

fn assert_decodes(codec: VideoFileCodec) {
    let bytes = one_frame_file(codec);
    // The demux itself must find exactly this codec.
    let au = mp4_first_frame::first_access_unit(&bytes).unwrap();
    match codec {
        VideoFileCodec::H264 => {
            assert!(matches!(au.codec, makepad_video::StreamVideoCodec::H264))
        }
        VideoFileCodec::H265 => {
            assert!(matches!(au.codec, makepad_video::StreamVideoCodec::Hevc))
        }
    }

    let frame = decode_first_frame_from_bytes(&bytes).unwrap();
    assert_eq!((frame.width, frame.height), (WIDTH, HEIGHT));
    assert_eq!(frame.nv12.len(), nv12::nv12_frame_size(WIDTH, HEIGHT));

    // Not a pixel-exact roundtrip (lossy encode), but the decoded picture
    // must resemble the gradient, not noise or black: mean absolute error
    // on the Y plane against the source's own NV12 conversion.
    let mut src_nv12 = Vec::new();
    nv12::rgb8_to_nv12(&synthetic_frame_rgb8(), WIDTH, HEIGHT, &mut src_nv12);
    let y_len = WIDTH as usize * HEIGHT as usize;
    let mae: f64 = frame.nv12[..y_len]
        .iter()
        .zip(&src_nv12[..y_len])
        .map(|(&a, &b)| (a as f64 - b as f64).abs())
        .sum::<f64>()
        / y_len as f64;
    assert!(mae < 8.0, "{codec:?} first frame MAE {mae} — decoded picture does not match source");
}

#[test]
fn hevc_first_frame_decodes_from_bytes() {
    assert_decodes(VideoFileCodec::H265);
}

#[test]
fn h264_first_frame_decodes_from_bytes() {
    assert_decodes(VideoFileCodec::H264);
}
