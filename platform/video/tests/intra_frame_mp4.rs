//! The hand-written single-frame container has to be a real mp4, not merely
//! a plausible one: a size field that lies or a chunk offset off by eight
//! produces a file that opens and then yields nothing, which is exactly the
//! failure a picture cache would not notice until the pictures were gone.
//!
//! So every case here goes all the way round — encode a known frame through
//! `encode_intra_frame_mp4`, decode it back with the ordinary decoder, and
//! look at the pixels that come out.

#![cfg(target_os = "macos")]

use makepad_video::{
    decode_first_frame_from_bytes, encode_intra_frame_mp4, nv12, VideoFileCodec, VideoFileDecoder,
};

/// A frame with structure in both directions, so a container that silently
/// swapped or truncated planes cannot pass by luck.
fn synthetic_rgb8(width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0u8; width as usize * height as usize * 3];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let at = (y * width as usize + x) * 3;
            out[at] = (x * 255 / width.max(1) as usize) as u8;
            out[at + 1] = (y * 255 / height.max(1) as usize) as u8;
            out[at + 2] = if (x / 8 + y / 8) % 2 == 0 { 220 } else { 40 };
        }
    }
    out
}

fn encode(width: u32, height: u32, codec: VideoFileCodec) -> (Vec<u8>, Vec<u8>) {
    let rgb = synthetic_rgb8(width, height);
    let mut nv12_bytes = Vec::new();
    nv12::rgb8_to_nv12(&rgb, width, height, &mut nv12_bytes);
    let mp4 = encode_intra_frame_mp4(&nv12_bytes, width, height, 30, 8_000_000, codec)
        .expect("encode_intra_frame_mp4");
    (mp4, nv12_bytes)
}

/// Mean absolute luma difference, as a share of full scale. HEVC is lossy, so
/// this is a likeness test, not an equality one — but a container that handed
/// back the wrong plane, wrong stride or wrong frame lands nowhere near.
fn luma_error(a: &[u8], b: &[u8], width: u32, height: u32) -> f64 {
    let count = width as usize * height as usize;
    let total: u64 = a[..count]
        .iter()
        .zip(b[..count].iter())
        .map(|(x, y)| x.abs_diff(*y) as u64)
        .sum();
    total as f64 / count as f64 / 255.0
}

#[test]
fn hevc_round_trips_through_the_written_container() {
    let (width, height) = (320u32, 240u32);
    let (mp4, source_nv12) = encode(width, height, VideoFileCodec::H265);

    let frame = decode_first_frame_from_bytes(&mp4).expect("decode the container we wrote");
    assert_eq!((frame.width, frame.height), (width, height));
    let error = luma_error(&frame.nv12, &source_nv12, width, height);
    assert!(error < 0.05, "decoded luma is {error:.4} off the source — not the frame we encoded");
}

#[test]
fn the_file_on_disk_opens_as_a_normal_movie() {
    let (width, height) = (256u32, 144u32);
    let (mp4, source_nv12) = encode(width, height, VideoFileCodec::H265);

    // Through a path, not bytes: this is the way the picture cache reads and
    // the way anything else on the machine would.
    let dir = std::env::temp_dir().join("makepad-intra-frame-test");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("one.mov");
    std::fs::write(&path, &mp4).expect("write");

    let mut decoder = VideoFileDecoder::open(&path.to_string_lossy()).expect("open the written file");
    let frame = decoder
        .next_frame()
        .expect("decode")
        .expect("a container with one sample must yield one frame");
    assert_eq!((frame.width, frame.height), (width, height));
    assert!(luma_error(&frame.nv12, &source_nv12, width, height) < 0.05);

    // Exactly one sample: a stsz/stts that claimed more would show up here.
    assert!(decoder.next_frame().expect("second read").is_none(), "container yielded a second frame");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn odd_shapes_and_large_frames_survive() {
    // Sizes that exercise chroma rounding and a stride the pool will pad.
    for (width, height) in [(2u32, 2u32), (1920, 1080), (66, 34)] {
        let (mp4, source_nv12) = encode(width, height, VideoFileCodec::H265);
        let frame = decode_first_frame_from_bytes(&mp4)
            .unwrap_or_else(|e| panic!("{width}x{height} did not decode: {e}"));
        assert_eq!((frame.width, frame.height), (width, height), "{width}x{height} came back reshaped");
        let error = luma_error(&frame.nv12, &source_nv12, width, height);
        assert!(error < 0.08, "{width}x{height} decoded {error:.4} off the source");
    }
}

#[test]
fn h264_writes_its_own_configuration_atom() {
    let (width, height) = (320u32, 240u32);
    let (mp4, source_nv12) = encode(width, height, VideoFileCodec::H264);
    assert!(
        mp4.windows(4).any(|w| w == b"avcC"),
        "an H.264 file must carry avcC, not hvcC"
    );
    let frame = decode_first_frame_from_bytes(&mp4).expect("decode H.264");
    assert_eq!((frame.width, frame.height), (width, height));
    assert!(luma_error(&frame.nv12, &source_nv12, width, height) < 0.05);
}
