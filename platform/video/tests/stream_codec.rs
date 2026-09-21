//! Round-trip test for the low-latency hardware stream encoder/decoder
//! (`VideoStreamEncoder` / `VideoStreamDecoder`, VideoToolbox on macOS /
//! the H.264 MFTs on Windows). Runs wherever a backend exists; on Windows
//! that is a real Media Foundation pass (run it on a fleet box). The
//! captured-stream test replays access units another machine's encoder
//! produced (`MAKEPAD_H264_DEBUG` dumps them next to its trace) so a
//! cross-platform wire problem can be reproduced offline.

use makepad_video::{
    annex_b, StreamVideoCodec, VideoStreamDecoder, VideoStreamEncoder, VideoStreamEncoderOptions,
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const FRAME_COUNT: usize = 30;
const FPS: u32 = 30;
const HNS_PER_FRAME: i64 = 10_000_000 / FPS as i64;

/// A moving diagonal gradient — visually distinct frame to frame (encoder
/// motion estimation actually has something to do) and cheap to generate.
fn synthetic_frame_rgb8(frame_index: usize) -> Vec<u8> {
    let mut out = vec![0u8; WIDTH as usize * HEIGHT as usize * 3];
    let shift = (frame_index * 4) as i32;
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let idx = (y * WIDTH as usize + x) * 3;
            out[idx] = ((x as i32 + shift) % 256) as u8;
            out[idx + 1] = (y % 256) as u8;
            out[idx + 2] = (((x + y) as i32 + shift * 2) % 256) as u8;
        }
    }
    out
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let sum_sq: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            let d = x as f64 - y as f64;
            d * d
        })
        .sum();
    let mse = sum_sq / a.len() as f64;
    if mse <= 0.0 {
        return 100.0;
    }
    20.0 * 255f64.log10() - 10.0 * mse.log10()
}

/// `MAKEPAD_H264_SAMPLE_DIR=<dir>` holds `*.h264` access units (one file
/// each, sorted by name = send order); every one is pushed and the decoder
/// must yield at least one picture per AU after the first two.
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn decode_captured_access_units() {
    let Ok(dir) = std::env::var("MAKEPAD_H264_SAMPLE_DIR") else {
        eprintln!("decode_captured_access_units: MAKEPAD_H264_SAMPLE_DIR unset, skipping");
        return;
    };
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("sample dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "h264"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no *.h264 access units in {dir}");
    let mut decoder = VideoStreamDecoder::new(StreamVideoCodec::H264).expect("decoder creation");
    let mut streamed = 0usize;
    let mut per_au = Vec::new();
    for (index, path) in files.iter().enumerate() {
        let au = std::fs::read(path).expect("read au");
        let frames = decoder.push_packet(&au, index as i64 * HNS_PER_FRAME).expect("decode packet").len();
        per_au.push(format!("au{index}:{frames}"));
        streamed += frames;
    }
    let flushed = decoder.flush().expect("flush").len();
    eprintln!("captured stream: {streamed} pictures while streaming + {flushed} on flush [{}]", per_au.join(" "));
    // Live use never flushes: every picture but the last one or two must
    // come out while the stream is still running.
    assert!(
        streamed + 2 >= files.len(),
        "only {streamed} of {} pictures came out while streaming",
        files.len()
    );
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn request_keyframe_forces_the_next_packet() {
    let mut encoder = VideoStreamEncoder::new(VideoStreamEncoderOptions {
        codec: StreamVideoCodec::H264,
        width: WIDTH,
        height: HEIGHT,
        fps: FPS,
        bitrate_kbps: 4_000,
        keyint: 300,
        low_latency: true,
    })
    .expect("encoder creation");
    for index in 0..4 {
        encoder.push_frame_rgb8(&synthetic_frame_rgb8(index), index as i64 * HNS_PER_FRAME).expect("encode");
    }
    encoder.request_keyframe();
    let forced = encoder.push_frame_rgb8(&synthetic_frame_rgb8(4), 4 * HNS_PER_FRAME).expect("forced keyframe encode");
    assert!(forced.iter().any(|p| p.is_key), "request_keyframe() did not force a keyframe");
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn encode_decode_round_trip_psnr_and_keyframes() {
    let mut encoder = VideoStreamEncoder::new(VideoStreamEncoderOptions {
        codec: StreamVideoCodec::H264,
        width: WIDTH,
        height: HEIGHT,
        fps: FPS,
        bitrate_kbps: 4_000,
        keyint: FRAME_COUNT as u32, // one GOP for this test, forced keyframe below exercises a second
        low_latency: true,
    })
    .expect("encoder creation");

    let sources: Vec<Vec<u8>> = (0..FRAME_COUNT).map(synthetic_frame_rgb8).collect();

    let mut packets = Vec::new();
    for (index, frame) in sources.iter().enumerate() {
        let pts = index as i64 * HNS_PER_FRAME;
        let mut produced = encoder.push_frame_rgb8(frame, pts).expect("encode frame");
        packets.append(&mut produced);
    }

    assert!(!packets.is_empty(), "encoder produced no packets at all");
    let first = &packets[0];
    assert!(first.is_key, "the very first packet must be a keyframe");
    // The Media Foundation encoder opens every access unit with an access
    // unit delimiter (NAL 9); VideoToolbox does not. Neither carries
    // meaning for the decoder, so the parameter sets are checked after it.
    let first_nals: Vec<&[u8]> = annex_b::split_annex_b(&first.data)
        .into_iter()
        .filter(|nal| annex_b::nal_unit_type(nal) != 9)
        .collect();
    assert!(!first_nals.is_empty(), "keyframe packet has no NAL units");
    assert_eq!(
        annex_b::nal_unit_type(first_nals[0]),
        annex_b::NAL_TYPE_SPS,
        "a keyframe packet must start with SPS"
    );
    assert!(
        first_nals.iter().any(|nal| annex_b::nal_unit_type(nal) == annex_b::NAL_TYPE_PPS),
        "a keyframe packet must carry a PPS"
    );
    assert!(
        first_nals.iter().any(|nal| annex_b::nal_unit_type(nal) == annex_b::NAL_TYPE_IDR),
        "a keyframe packet must carry an IDR slice"
    );

    // One more frame past the GOP so the decoder has a next access unit to
    // close the last picture with (the forced-keyframe behaviour has its
    // own test).
    encoder.request_keyframe();
    let extra_pts = FRAME_COUNT as i64 * HNS_PER_FRAME;
    let extra_frame = synthetic_frame_rgb8(FRAME_COUNT);
    let forced = encoder.push_frame_rgb8(&extra_frame, extra_pts).expect("extra frame encode");

    // Decode everything (including the extra packet) back.
    let mut decoder = VideoStreamDecoder::new(StreamVideoCodec::H264).expect("decoder creation");
    let mut decoded_by_pts = std::collections::HashMap::new();
    let mut streamed = 0usize;
    for packet in packets.iter().chain(forced.iter()) {
        for frame in decoder.push_packet(&packet.data, packet.pts_100ns).expect("decode packet") {
            streamed += 1;
            decoded_by_pts.insert(frame.pts_100ns, frame);
        }
    }
    let flushed = decoder.flush().expect("decoder flush");
    eprintln!("round trip: {streamed} pictures while streaming, {} on flush", flushed.len());
    for frame in flushed {
        decoded_by_pts.insert(frame.pts_100ns, frame);
    }

    assert!(
        decoded_by_pts.len() >= FRAME_COUNT - 1,
        "decoded {} frames, expected at least {}",
        decoded_by_pts.len(),
        FRAME_COUNT - 1
    );

    let mut checked = 0usize;
    let mut psnr_sum = 0.0;
    for (index, source) in sources.iter().enumerate() {
        let pts = index as i64 * HNS_PER_FRAME;
        let Some(decoded) = decoded_by_pts.get(&pts) else { continue };
        assert_eq!(decoded.width, WIDTH);
        assert_eq!(decoded.height, HEIGHT);
        let decoded_rgb = decoded.to_rgb8();
        assert_eq!(decoded_rgb.len(), source.len());
        let db = psnr(source, &decoded_rgb);
        psnr_sum += db;
        checked += 1;
        assert!(db > 30.0, "frame {index} PSNR {db:.2} dB too low");
    }
    assert!(checked >= FRAME_COUNT - 1, "only compared {checked} frames against source");
    eprintln!("stream_codec round trip: {checked} frames, avg PSNR {:.2} dB", psnr_sum / checked as f64);
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn stream_codec_is_explicitly_unsupported_elsewhere() {
    let err = VideoStreamEncoder::new(VideoStreamEncoderOptions {
        width: WIDTH,
        height: HEIGHT,
        ..Default::default()
    })
    .unwrap_err();
    assert!(err.context.contains("not implemented"));
}
