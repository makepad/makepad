//! Round-trip test for the low-latency hardware stream encoder/decoder
//! (`VideoStreamEncoder` / `VideoStreamDecoder`, VideoToolbox on macOS /
//! an H.264 MFT on Windows). Only run for real on macOS (the platform this
//! agent can execute on) — see the `#[cfg]` gate below. The Windows backend
//! is compile-checked only (`cargo check -p makepad-video --target
//! x86_64-pc-windows-msvc`), never exercised by this test.

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

#[cfg(target_os = "macos")]
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
    let first_nals = annex_b::split_annex_b(&first.data);
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

    // request_keyframe() must force the NEXT pushed frame to be a keyframe,
    // even mid-GOP.
    encoder.request_keyframe();
    let extra_pts = FRAME_COUNT as i64 * HNS_PER_FRAME;
    let extra_frame = synthetic_frame_rgb8(FRAME_COUNT);
    let forced = encoder.push_frame_rgb8(&extra_frame, extra_pts).expect("forced keyframe encode");
    assert!(forced.iter().any(|p| p.is_key), "request_keyframe() did not force a keyframe");

    // Decode everything (including the forced-keyframe packet) back.
    let mut decoder = VideoStreamDecoder::new(StreamVideoCodec::H264).expect("decoder creation");
    let mut decoded_by_pts = std::collections::HashMap::new();
    for packet in packets.iter().chain(forced.iter()) {
        for frame in decoder.push_packet(&packet.data, packet.pts_100ns).expect("decode packet") {
            decoded_by_pts.insert(frame.pts_100ns, frame);
        }
    }
    for frame in decoder.flush().expect("decoder flush") {
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
