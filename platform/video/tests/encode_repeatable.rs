//! Probe: the same frames encoded twice, how alike are the files (edit
//! exports should be byte-reproducible). Ignored: it runs the hardware
//! encoder. Found (2026-09-30, M-series): H.264 coded frames come out
//! identical run to run, with or without an interleaved audio track, even
//! with four probes at once; the files still differ in the movie header's
//! creation and modification times (mvhd, tkhd, mdhd) and in two bytes of
//! Apple's user-data SEI on the first frame (a per-session value). H.265
//! differs throughout. And an edit export under heavy load (four renders at
//! once) once diverged from frame 30 on: the writer's inputs run with
//! expectsMediaDataInRealTime (the synchronous push API needs it), and a
//! real-time VideoToolbox session may decide rate control by timing.

#![cfg(target_os = "macos")]

use makepad_video::{PcmAudioTrackOptions, VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions};

const W: u32 = 640;
const H: u32 = 360;

fn frame(k: usize) -> Vec<u8> {
    let mut out = vec![0u8; W as usize * H as usize * 3];
    for y in 0..H as usize {
        for x in 0..W as usize {
            let i = (y * W as usize + x) * 3;
            let v = ((x as f32 * 0.05 + k as f32 * 0.3).sin() * (y as f32 * 0.07 - k as f32 * 0.2).cos() * 127.0 + 128.0) as u8;
            out[i] = v;
            out[i + 1] = ((x + k * 3) % 256) as u8;
            out[i + 2] = ((y * 2 + k) % 256) as u8;
        }
    }
    out
}

fn encode(codec: VideoFileCodec, tag: &str) -> Vec<u8> {
    encode_with(codec, tag, false)
}

/// With `audio`, a second of sound leads the pictures, as an edit export
/// interleaves them.
fn encode_with(codec: VideoFileCodec, tag: &str, audio: bool) -> Vec<u8> {
    let dir = std::env::temp_dir().join("makepad-video-repeatable");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{tag}.mp4"));
    let mut e = VideoFileEncoder::new(
        path.to_str().unwrap(),
        VideoFileEncoderOptions {
            codec,
            width: W,
            height: H,
            fps_num: 30,
            fps_den: 1,
            video_bitrate_bps: 2_000_000,
            audio: audio.then_some(PcmAudioTrackOptions { sample_rate: 48_000, channels: 2, aac_bitrate_bps: 192_000 }),
            keyframe_only: false,
        },
    )
    .unwrap();
    let pcm = |from: usize, to: usize| -> Vec<i16> {
        (from..to).flat_map(|i| { let v = ((i as f32 * 0.0575).sin() * 9000.0) as i16; [v, v] }).collect()
    };
    for k in 0..90usize {
        e.push_frame_rgb8(&frame(k), Some(k as i64 * 10_000_000 / 30)).unwrap();
        if audio {
            let (a, b) = if k == 0 { (0, 48_000) } else { (48_000 + (k - 1) * 1600, 48_000 + k * 1600) };
            e.push_audio_i16(&pcm(a, b)).unwrap();
        }
    }
    e.finish().unwrap();
    std::fs::read(&path).unwrap()
}

/// Top-level boxes and where two files first differ.
fn first_difference(a: &[u8], b: &[u8]) -> Option<(usize, String)> {
    let at = a.iter().zip(b).position(|(x, y)| x != y).or((a.len() != b.len()).then(|| a.len().min(b.len())))?;
    let mut off = 0usize;
    let mut path = String::new();
    while off + 8 <= a.len() {
        let size = u32::from_be_bytes(a[off..off + 4].try_into().unwrap()) as usize;
        let name = String::from_utf8_lossy(&a[off + 4..off + 8]).to_string();
        if size < 8 {
            break;
        }
        if at < off + size {
            path = format!("{name} at {off}+{}", at - off);
            break;
        }
        off += size;
    }
    Some((at, path))
}

#[test]
#[ignore]
fn the_same_frames_encode_to_the_same_file() {
    for (codec, audio) in [(VideoFileCodec::H264, false), (VideoFileCodec::H264, true), (VideoFileCodec::H265, false)] {
        let first = encode_with(codec, &format!("{codec:?}-{audio}-0"), audio);
        for run in 1..4 {
            let again = encode_with(codec, &format!("{codec:?}-{audio}-{run}"), audio);
            println!("{codec:?} audio {audio} run {run}: {} vs {} bytes, first difference {:?}", first.len(), again.len(), first_difference(&first, &again));
        }
    }
}
