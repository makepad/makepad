//! Round-trip test for the low-latency hardware stream encoder/decoder
//! (`VideoStreamEncoder` / `VideoStreamDecoder`, VideoToolbox on macOS /
//! the H.264 MFTs on Windows). Runs wherever a backend exists; on Windows
//! that is a real Media Foundation pass (run it on a fleet box).

use makepad_video::{VideoStreamEncoder, VideoStreamEncoderOptions};
// the codec tests run where a stream codec exists (VideoToolbox, Media Foundation)
#[cfg(any(target_os = "macos", target_os = "windows"))]
use makepad_video::{annex_b, StreamVideoCodec, VideoStreamDecoder};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
#[cfg(any(target_os = "macos", target_os = "windows"))]
const FRAME_COUNT: usize = 30;
#[cfg(any(target_os = "macos", target_os = "windows"))]
const FPS: u32 = 30;
#[cfg(any(target_os = "macos", target_os = "windows"))]
const HNS_PER_FRAME: i64 = 10_000_000 / FPS as i64;

#[cfg(any(target_os = "macos", target_os = "windows"))]
/// A moving diagonal gradient — visually distinct frame to frame (encoder
/// motion estimation actually has something to do) and cheap to generate.
fn synthetic_frame_rgb8(frame_index: usize) -> Vec<u8> {
    sized_frame_rgb8(WIDTH as usize, HEIGHT as usize, frame_index)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn sized_frame_rgb8(width: usize, height: usize, frame_index: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 3];
    let shift = (frame_index * 4) as i32;
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 3;
            out[idx] = ((x as i32 + shift) % 256) as u8;
            out[idx + 1] = (y % 256) as u8;
            out[idx + 2] = (((x + y) as i32 + shift * 2) % 256) as u8;
        }
    }
    out
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn keyint_zero_keys_only_the_start_and_each_request() {
    // 720p for 3 s: an encoder left to its own interval keys in that (the NVIDIA MFT: about
    // every second at this size)
    const UNASKED: usize = 90;
    let (w, h) = (1280, 720);
    let mut encoder = VideoStreamEncoder::new(VideoStreamEncoderOptions {
        codec: StreamVideoCodec::H264,
        width: w as u32,
        height: h as u32,
        fps: FPS,
        bitrate_kbps: 1_500,
        keyint: 0,
        low_latency: true, full_chroma: false,
    })
    .expect("encoder creation");
    let mut keys = Vec::new();
    for index in 0..UNASKED {
        let packets = encoder.push_frame_rgb8(&sized_frame_rgb8(w, h, index), index as i64 * HNS_PER_FRAME).expect("encode");
        keys.extend(packets.iter().filter(|p| p.is_key).map(|p| p.pts_100ns / HNS_PER_FRAME));
    }
    assert_eq!(keys, [0], "keyint 0: a key frame at the start only, unasked ({})", encoder.description());
    encoder.request_keyframe();
    let forced = encoder.push_frame_rgb8(&sized_frame_rgb8(w, h, UNASKED), UNASKED as i64 * HNS_PER_FRAME).expect("forced keyframe encode");
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
        low_latency: true, full_chroma: false,
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

/// The HAL's encoder is hardware the box may not have: it comes up, or says why not.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn a_hal_stream_encoder_comes_up_or_says_why() {
    if let Err(err) = VideoStreamEncoder::new(VideoStreamEncoderOptions {
        width: WIDTH,
        height: HEIGHT,
        ..Default::default()
    }) {
        assert!(!err.context.is_empty());
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", target_os = "android")))]
#[test]
fn stream_codec_is_explicitly_unsupported_elsewhere() {
    let Err(err) = VideoStreamEncoder::new(VideoStreamEncoderOptions {
        width: WIDTH,
        height: HEIGHT,
        ..Default::default()
    }) else {
        panic!("a stream encoder came up where none is implemented")
    };
    assert!(err.context.contains("not implemented"));
}
