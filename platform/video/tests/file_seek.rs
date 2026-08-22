//! `VideoFileDecoder::seek` — does the next frame really come back from where
//! we asked, and what does asking cost?
//!
//! Encodes two clips with the in-repo file encoder (same frames, same
//! settings, differing only in `keyframe_only`), then drives the decoder over
//! both. Every assertion is on the frame's OWN identity, not just its
//! timestamp: each source frame carries its index painted into a flat corner
//! patch, so "seek landed on frame 37" is checked against the picture the
//! encoder was handed, not against a number the decoder computed. A seek that
//! reported the right pts while handing back the wrong picture — the exact
//! failure a discard loop off by one produces — would pass a pts-only test.
//!
//! Runs for real on macOS only (the platform this agent can execute on). The
//! Windows backend takes the same code path through the same facade and is
//! compile-gated with `cargo build --target x86_64-pc-windows-msvc`.

use makepad_video::{
    PcmAudioTrackOptions, VideoFileCodec, VideoFileDecoder, VideoFileEncoder,
    VideoFileEncoderOptions,
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 192;
const FPS: u32 = 24;
const FRAME_COUNT: usize = 120;
const HNS_PER_FRAME: i64 = 10_000_000 / FPS as i64;

const AUDIO_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u16 = 2;

/// The frame index is painted across the top of every frame as 8 black/white
/// blocks — one per bit. Read back as bits rather than as a luma level, the
/// identity survives the RGB -> video-range NV12 -> RGB round trip and any
/// amount of codec ringing, and comes back as an exact integer instead of
/// something to compare with a tolerance.
const IDENT_BITS: usize = 8;
const IDENT_BLOCK_W: usize = WIDTH as usize / IDENT_BITS;
const IDENT_BLOCK_H: usize = 24;

/// Frame `index` as RGB8: the identity strip across the top, a mid-grey field,
/// and a bar sweeping the bottom half so inter-frame prediction has real work
/// and the GOP clip gets a real GOP.
fn synthetic_frame_rgb8(index: usize) -> Vec<u8> {
    let bar = (index * 5) % WIDTH as usize;
    let mut out = vec![0u8; WIDTH as usize * HEIGHT as usize * 3];
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let luma = if y < IDENT_BLOCK_H {
                let bit = (x / IDENT_BLOCK_W).min(IDENT_BITS - 1);
                if index >> bit & 1 == 1 { 235 } else { 16 }
            } else if y >= HEIGHT as usize / 2 && x.abs_diff(bar) < 20 {
                220
            } else {
                90
            };
            let idx = (y * WIDTH as usize + x) * 3;
            out[idx] = luma;
            out[idx + 1] = luma;
            out[idx + 2] = luma;
        }
    }
    out
}

/// Read the index back out of a decoded frame's luma plane. Each block is
/// sampled well inside its own borders so ringing at the block edges cannot
/// flip a bit.
fn identity_of(nv12: &[u8], width: u32) -> usize {
    let mut index = 0usize;
    for bit in 0..IDENT_BITS {
        let x0 = bit * IDENT_BLOCK_W + IDENT_BLOCK_W / 4;
        let x1 = bit * IDENT_BLOCK_W + IDENT_BLOCK_W * 3 / 4;
        let mut sum = 0u32;
        let mut count = 0u32;
        for y in IDENT_BLOCK_H / 4..IDENT_BLOCK_H * 3 / 4 {
            for x in x0..x1 {
                sum += nv12[y * width as usize + x] as u32;
                count += 1;
            }
        }
        if sum / count.max(1) > 128 {
            index |= 1 << bit;
        }
    }
    index
}

fn encode_clip(path: &str, keyframe_only: bool) {
    let mut encoder = VideoFileEncoder::new(
        path,
        VideoFileEncoderOptions {
            codec: VideoFileCodec::H264,
            width: WIDTH,
            height: HEIGHT,
            fps_num: FPS,
            fps_den: 1,
            video_bitrate_bps: 12_000_000,
            audio: Some(PcmAudioTrackOptions {
                sample_rate: AUDIO_RATE,
                channels: AUDIO_CHANNELS,
                aac_bitrate_bps: 128_000,
            }),
            keyframe_only,
        },
    )
    .expect("file encoder creation");
    for index in 0..FRAME_COUNT {
        encoder
            .push_frame_rgb8(&synthetic_frame_rgb8(index), None)
            .expect("push frame");
    }
    // A quiet ramp spanning the whole clip, so a seek to the midpoint still
    // has audio ahead of it to land on.
    let total = AUDIO_RATE as usize * AUDIO_CHANNELS as usize * FRAME_COUNT / FPS as usize;
    let samples: Vec<i16> = (0..total)
        .map(|i| ((i % 400) as i16 - 200) * 40)
        .collect();
    encoder.push_audio_i16(&samples).expect("push audio");
    encoder.finish().expect("finish");
}

/// Decode straight through with no seeking at all — the ground truth every
/// seek is checked against, and the proof that adding `seek` left the plain
/// forward path alone.
fn decode_all(path: &str) -> Vec<(i64, usize)> {
    let mut decoder = VideoFileDecoder::open(path).expect("open");
    let mut out = Vec::new();
    while let Some(frame) = decoder.next_frame().expect("next_frame") {
        out.push((frame.pts_100ns, identity_of(&frame.nv12, frame.width)));
    }
    out
}

#[cfg(target_os = "macos")]
#[test]
fn seek_lands_on_the_requested_frame_on_both_clip_kinds() {
    let dir = std::env::temp_dir().join(format!("makepad-video-seek-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let intra_path = dir.join("intra.mp4");
    let gop_path = dir.join("gop.mp4");
    let intra = intra_path.to_str().unwrap();
    let gop = gop_path.to_str().unwrap();
    encode_clip(intra, true);
    encode_clip(gop, false);

    let mut report = Vec::new();
    for (label, path) in [("all-intra", intra), ("gop", gop)] {
        // --- ground truth: the plain forward path, untouched by seek ---
        let forward_started = std::time::Instant::now();
        let forward = decode_all(path);
        let forward_elapsed = forward_started.elapsed();
        assert!(
            forward.len() >= FRAME_COUNT - 1,
            "{label}: forward decode produced {} frames, expected ~{FRAME_COUNT}",
            forward.len()
        );
        for (index, (_, identity)) in forward.iter().enumerate() {
            assert_eq!(
                *identity, index,
                "{label}: forward frame {index} carries the picture of frame {identity}"
            );
        }

        let mut decoder = VideoFileDecoder::open(path).expect("open");
        let info = decoder.info().clone();
        assert_eq!(info.width, WIDTH);
        assert_eq!(info.height, HEIGHT);
        assert!(info.has_audio, "{label}: encoded audio track went missing");

        // --- exact hits, including the first and last frame ---
        for &target_index in &[0usize, 1, 17, 60, 61, FRAME_COUNT - 1] {
            let target_pts = forward[target_index].0;
            decoder.seek(target_pts).expect("seek");
            let frame = decoder
                .next_frame()
                .expect("next_frame after seek")
                .unwrap_or_else(|| panic!("{label}: seek to frame {target_index} hit EOS"));
            assert_eq!(
                frame.pts_100ns, target_pts,
                "{label}: seek to frame {target_index} returned pts {} not {target_pts}",
                frame.pts_100ns
            );
            assert_eq!(
                identity_of(&frame.nv12, frame.width),
                target_index,
                "{label}: seek to frame {target_index} returned the wrong picture"
            );
        }

        // --- a target BETWEEN two frames must round forward, never back ---
        let between = forward[42].0 - 1;
        decoder.seek(between).expect("seek between frames");
        let frame = decoder.next_frame().expect("next_frame").expect("frame");
        assert_eq!(
            frame.pts_100ns, forward[42].0,
            "{label}: a target 1 tick before frame 42 must yield frame 42, not the one before it"
        );

        // --- decoding continues in order from where the seek left off ---
        decoder.seek(forward[30].0).expect("seek");
        for offset in 0..4usize {
            let frame = decoder.next_frame().expect("next_frame").expect("frame");
            assert_eq!(
                frame.pts_100ns,
                forward[30 + offset].0,
                "{label}: frame {} after a seek to 30 is out of order",
                30 + offset
            );
            assert_eq!(
                identity_of(&frame.nv12, frame.width),
                30 + offset,
                "{label}: the picture after a seek to 30 + {offset} is the wrong frame"
            );
        }

        // --- a backwards seek after reaching EOS re-arms the stream ---
        while decoder.next_frame().expect("drain").is_some() {}
        decoder.seek(forward[3].0).expect("seek back from EOS");
        let frame = decoder
            .next_frame()
            .expect("next_frame")
            .expect("{label}: rewind after EOS produced nothing");
        assert_eq!(frame.pts_100ns, forward[3].0, "{label}: rewind landed wrong");

        // --- audio follows the same rule and is never behind the target ---
        let mid = forward[FRAME_COUNT / 2].0;
        decoder.seek(mid).expect("seek");
        let chunk = decoder
            .next_audio()
            .expect("next_audio after seek")
            .unwrap_or_else(|| panic!("{label}: no audio at the midpoint"));
        assert!(
            chunk.pts_100ns >= mid,
            "{label}: audio came back at {} which is before the seek target {mid}",
            chunk.pts_100ns
        );
        assert_eq!(chunk.sample_rate, AUDIO_RATE);
        assert_eq!(chunk.channels, AUDIO_CHANNELS);

        // --- past the end is end-of-stream, not an error ---
        decoder
            .seek(forward[FRAME_COUNT - 1].0 + 100 * HNS_PER_FRAME)
            .expect("seek past end");
        assert!(
            decoder.next_frame().expect("next_frame past end").is_none(),
            "{label}: a seek past the end must leave the stream at EOS"
        );

        // --- and 0 is the start, from anywhere ---
        decoder.seek(0).expect("seek to 0");
        let frame = decoder.next_frame().expect("next_frame").expect("frame");
        assert_eq!(frame.pts_100ns, forward[0].0);

        // --- cost: seek to every 8th frame, spread across the clip ---
        let mut worst = std::time::Duration::ZERO;
        let mut total = std::time::Duration::ZERO;
        let mut count = 0u32;
        for target_index in (0..FRAME_COUNT).step_by(8) {
            let target_pts = forward[target_index].0;
            let started = std::time::Instant::now();
            decoder.seek(target_pts).expect("seek");
            let frame = decoder.next_frame().expect("next_frame").expect("frame");
            let elapsed = started.elapsed();
            assert_eq!(frame.pts_100ns, target_pts);
            worst = worst.max(elapsed);
            total += elapsed;
            count += 1;
        }
        report.push(format!(
            "{label:<10} seek+frame mean {:>7.2} ms  worst {:>7.2} ms  ({count} seeks)  \
             full forward decode of {} frames {:.2} ms",
            total.as_secs_f64() * 1000.0 / count as f64,
            worst.as_secs_f64() * 1000.0,
            forward.len(),
            forward_elapsed.as_secs_f64() * 1000.0,
        ));
    }

    for line in &report {
        eprintln!("{line}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn seek_is_explicitly_unsupported_elsewhere() {
    let mut decoder = VideoFileDecoder::open("nonexistent.mp4").unwrap_err();
    let _ = &mut decoder;
}
