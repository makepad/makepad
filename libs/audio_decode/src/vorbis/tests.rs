//! Unit tests for the Vorbis headers and the lapping model. Bitstream-level
//! coverage lives next to each piece (codebook, floor, mdct, residue) and the
//! whole-file tests are in `tests/vorbis_oracle.rs`.

use super::*;
use crate::ogg::test_pages::{lacing, page};

/// Pack fields LSB-first, the way a Vorbis header is written.
fn bitstream(fields: &[(u32, u32)]) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::new();
    for &(val, n) in fields {
        for i in 0..n {
            bits.push((val >> i) & 1 == 1);
        }
    }
    let mut bytes = vec![0u8; bits.len().div_ceil(8)];
    for (i, b) in bits.iter().enumerate() {
        if *b {
            bytes[i / 8] |= 1 << (i % 8);
        }
    }
    bytes
}

fn ident_packet(channels: u32, rate: u32, bs0: u32, bs1: u32) -> Vec<u8> {
    let mut p = vec![1u8];
    p.extend_from_slice(b"vorbis");
    p.extend(bitstream(&[
        (0, 32),       // version
        (channels, 8),
        (rate, 32),
        (0, 32),       // bitrate max
        (0, 32),       // bitrate nominal
        (0, 32),       // bitrate min
        (bs0, 4),
        (bs1, 4),
        (1, 1),        // framing
    ]));
    p
}

fn comment_packet(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut p = vec![3u8];
    p.extend_from_slice(b"vorbis");
    let vendor = b"makepad-audio-decode";
    p.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    p.extend_from_slice(vendor);
    p.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
    for (k, v) in pairs {
        let text = format!("{k}={v}");
        p.extend_from_slice(&(text.len() as u32).to_le_bytes());
        p.extend_from_slice(text.as_bytes());
    }
    p
}

#[test]
fn ident_header_validates_its_fields() {
    let limits = Limits::default();
    let good = ident_packet(2, 44_100, 8, 11);
    let id = read_ident(&good, &limits).unwrap();
    assert_eq!((id.channels, id.sample_rate), (2, 44_100));
    assert_eq!((id.blocksize_0, id.blocksize_1), (256, 2048));

    // Zero channels, zero rate, blocksize_0 > blocksize_1, absurd rate.
    assert!(matches!(
        read_ident(&ident_packet(0, 44_100, 8, 11), &limits),
        Err(AudioError::BadHeader(_))
    ));
    assert!(matches!(
        read_ident(&ident_packet(2, 0, 8, 11), &limits),
        Err(AudioError::BadHeader(_))
    ));
    assert!(matches!(
        read_ident(&ident_packet(2, 44_100, 11, 8), &limits),
        Err(AudioError::BadHeader(_))
    ));
    assert!(matches!(
        read_ident(&ident_packet(2, 999_999, 8, 11), &limits),
        Err(AudioError::BadHeader(_))
    ));
    // Blocksize below 64 is refused rather than clamped.
    assert!(matches!(
        read_ident(&ident_packet(2, 44_100, 4, 11), &limits),
        Err(AudioError::BadHeader(_))
    ));
    // Channel counts above the caller's limit are a limits failure, not a
    // malformed file.
    let tight = Limits { max_channels: 1, ..Limits::default() };
    assert!(matches!(
        read_ident(&ident_packet(2, 44_100, 8, 11), &tight),
        Err(AudioError::TooLarge(_))
    ));
}

#[test]
fn ident_header_truncation_never_panics() {
    let good = ident_packet(2, 44_100, 8, 11);
    for len in 0..good.len() {
        assert!(read_ident(&good[..len], &Limits::default()).is_err());
    }
}

#[test]
fn comments_split_on_the_first_equals() {
    let mut tags = Tags::default();
    let p = comment_packet(&[
        ("TITLE", "A Song"),
        ("ARTIST", "Someone"),
        ("DESCRIPTION", "has=an=equals"),
        ("NOVALUE", ""),
        ("", "novalue"),
    ]);
    read_comments(&p, &mut tags).unwrap();
    assert_eq!(tags.title.as_deref(), Some("A Song"));
    assert_eq!(tags.artist.as_deref(), Some("Someone"));
    assert_eq!(tags.get("DESCRIPTION"), Some("has=an=equals"));
    // Empty values are dropped by Tags::push.
    assert_eq!(tags.get("NOVALUE"), None);
}

#[test]
fn comment_header_truncation_never_panics() {
    let p = comment_packet(&[("TITLE", "A Song"), ("ARTIST", "Someone")]);
    for len in 0..p.len() {
        let mut tags = Tags::default();
        // Either it parses what it can or it reports truncation; never panics.
        let _ = read_comments(&p[..len], &mut tags);
    }
}

#[test]
fn a_lying_comment_count_does_not_loop() {
    let mut p = vec![3u8];
    p.extend_from_slice(b"vorbis");
    p.extend_from_slice(&0u32.to_le_bytes()); // empty vendor
    p.extend_from_slice(&u32::MAX.to_le_bytes()); // "four billion comments"
    let mut tags = Tags::default();
    read_comments(&p, &mut tags).unwrap();
    assert!(tags.all.is_empty());
}

#[test]
fn lapped_window_is_power_complementary_for_equal_blocks() {
    let n = 256;
    let w = lapped_window(n, 64, true, true);
    for i in 0..n / 2 {
        let s = w[i] * w[i] + w[i + n / 2] * w[i + n / 2];
        assert!((s - 1.0).abs() < 1e-5, "i={i} {s}");
    }
}

#[test]
fn lapped_window_narrows_against_a_short_neighbour() {
    let n = 256;
    let w = lapped_window(n, 64, false, true);
    assert_eq!(w[0], 0.0);
    assert_eq!(w[n / 4 - 64 / 4 - 1], 0.0);
    assert!((w[n / 2] - 1.0).abs() < 1e-5);
    // The narrow slope is exactly the short block's rise.
    let short = mdct::window(64);
    for i in 0..32 {
        assert!((w[n / 4 - 16 + i] - short[i]).abs() < 1e-6, "i={i}");
    }
}

#[test]
fn lapped_windows_of_neighbouring_blocks_sum_to_one() {
    // A long block followed by a short one: over their shared region the two
    // windows must still be power complementary, which is what makes mixed
    // block sizes reconstruct.
    let (long, short) = (2048usize, 256usize);
    let wl = lapped_window(long, short, true, false);
    let ws = lapped_window(short, short, false, true);
    // Centres: c and c + (long + short) / 4.
    let spacing = (long + short) / 4;
    let overlap_start = spacing - short / 2 + long / 2; // in long-block indices
    for i in 0..short / 2 {
        let a = wl[overlap_start + i];
        let b = ws[i];
        assert!((a * a + b * b - 1.0).abs() < 1e-5, "i={i}: {a} {b}");
    }
}

/// A stream with headers but no audio: enough to exercise the header path
/// without an encoder.
fn header_only_stream() -> Vec<u8> {
    let ident = ident_packet(2, 44_100, 8, 11);
    let comment = comment_packet(&[("TITLE", "Nothing")]);
    let mut data = page(77, 0, 0, 0x02, &lacing(ident.len()), &ident);
    data.extend(page(77, 1, 0, 0, &lacing(comment.len()), &comment));
    data
}

#[test]
fn a_stream_without_a_setup_header_is_truncated() {
    let data = header_only_stream();
    assert!(matches!(VorbisDecoder::new(&data), Err(AudioError::Truncated)));
    // Tags still read, because they precede the setup header.
    let tags = read_tags(&data).unwrap();
    assert_eq!(tags.title.as_deref(), Some("Nothing"));
}

#[test]
fn probe_duration_reads_the_last_granule() {
    let ident = ident_packet(1, 48_000, 8, 11);
    let mut data = page(9, 0, 0, 0x02, &lacing(ident.len()), &ident);
    data.extend(page(9, 1, 24_000, 0x04, &[1], &[0u8]));
    let secs = probe_duration(&data).unwrap();
    assert!((secs - 0.5).abs() < 1e-9, "{secs}");
}

#[test]
fn probe_duration_refuses_a_stream_with_no_granule() {
    let ident = ident_packet(1, 48_000, 8, 11);
    let data = page(9, 0, ogg::GRANULE_NONE, 0x02, &lacing(ident.len()), &ident);
    assert!(probe_duration(&data).is_err());
}

/// A setup header whose single floor is type 0: one codebook of two length-1
/// codewords, one null time transform, then floor type 0.
#[test]
fn floor_zero_is_reported_through_the_setup_header() {
    let mut p = vec![5u8];
    p.extend_from_slice(b"vorbis");
    p.extend(bitstream(&[
        (0, 8),         // codebook count - 1
        (0x564342, 24), // codebook sync
        (1, 16),        // dimensions
        (2, 24),        // entries
        (0, 1),         // not ordered
        (0, 1),         // not sparse
        (0, 5),         // entry 0 length - 1
        (0, 5),         // entry 1 length - 1
        (0, 4),         // lookup type 0
        (0, 6),         // time transform count - 1
        (0, 16),        // the transform, which must be zero
        (0, 6),         // floor count - 1
        (0, 16),        // floor type 0
    ]));
    match read_setup(&p, 2) {
        Err(AudioError::Unsupported(what)) => assert_eq!(what, "vorbis floor 0"),
        other => panic!("floor 0 must be reported, got {:?}", other.err()),
    }
}

#[test]
fn garbage_is_refused_without_panicking() {
    assert!(decode_all(&[]).is_err());
    assert!(decode_all(b"OggS but not really vorbis").is_err());
    assert!(probe_duration(&[]).is_err());
    assert!(read_tags(&[]).is_err());
    assert!(VorbisDecoder::new(&[]).is_err());
}
