//! Vorbis: a tiny committed fixture and a no-panic sweep over mangled copies.
//! These checks are self-contained and run without a local audio corpus.

use makepad_audio_decode::{decode_any, vorbis, AudioError, AudioFormat};

const FIXTURE: &[u8] = include_bytes!("../testdata/button-press.ogg");

// -- the committed fixture -------------------------------------------------

#[test]
fn fixture_decodes_to_the_expected_audio() {
    let audio = vorbis::decode_all(FIXTURE).unwrap();
    assert_eq!(audio.rate, 48_000);
    assert_eq!(audio.channels, 1);
    // Exactly the granule position of the last page, which is also the frame
    // count CoreAudio reports for this file ("13347 valid frames").
    assert_eq!(audio.frames(), 13_347);

    // Level and shape, measured against the CoreAudio decode of the same file.
    // afconvert writes its WAV 128 frames in (CoreAudio trims that much as
    // priming), and dropping those 128 frames reproduces its RMS to six digits
    // and its peak exactly.
    let pcm = &audio.pcm_interleaved_f32;
    let rms = |s: &[f32]| (s.iter().map(|&v| (v * v) as f64).sum::<f64>() / s.len() as f64).sqrt();
    assert!((rms(pcm) - 0.007_074).abs() < 1e-5, "rms {}", rms(pcm));
    assert!((rms(&pcm[128..]) - 0.007_107).abs() < 1e-5, "trimmed rms {}", rms(&pcm[128..]));
    let peak = pcm.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
    assert!((peak - 0.118_989).abs() < 1e-4, "peak {peak}");

    // A button click: quiet, then a burst a third of the way in, then decay.
    let eighth = pcm.len() / 8;
    let loudest = (0..8).max_by(|&a, &b| {
        rms(&pcm[a * eighth..(a + 1) * eighth])
            .partial_cmp(&rms(&pcm[b * eighth..(b + 1) * eighth]))
            .unwrap()
    });
    assert_eq!(loudest, Some(3));
    assert!(rms(&pcm[..eighth]) < 0.002);
    assert!(rms(&pcm[7 * eighth..]) < 0.002);
}

#[test]
fn fixture_probe_matches_the_decode() {
    let secs = vorbis::probe_duration(FIXTURE).unwrap();
    let audio = vorbis::decode_all(FIXTURE).unwrap();
    assert!((secs - audio.duration_secs()).abs() < 1e-9, "{secs} vs {}", audio.duration_secs());
    // 13347 frames at 48 kHz, and afinfo agrees to six digits.
    assert!((secs - 0.278_062_5).abs() < 1e-6, "{secs}");
}

#[test]
fn probe_does_not_decode_a_hundred_megabytes() {
    // The fixture's pages repeated to 100 MB: a duration probe must read the
    // first page and the last one, not the 100 MB in between.
    let mut big: Vec<u8> = Vec::with_capacity(100 << 20);
    while big.len() < 100 << 20 {
        big.extend_from_slice(FIXTURE);
    }
    let started = std::time::Instant::now();
    let secs = vorbis::probe_duration(&big).unwrap();
    let elapsed = started.elapsed();
    assert!((secs - 0.278_062_5).abs() < 1e-6, "{secs}");
    assert!(elapsed.as_millis() < 500, "probe took {elapsed:?} on 100 MB");
    eprintln!("probe of 100 MB: {elapsed:?}");
}

#[test]
fn fixture_sniffs_and_decodes_through_the_crate_entry_points() {
    assert_eq!(makepad_audio_decode::sniff(FIXTURE), Some(AudioFormat::OggVorbis));
    let a = decode_any(FIXTURE).unwrap();
    let b = vorbis::decode_all(FIXTURE).unwrap();
    assert_eq!(a, b);
    assert!((makepad_audio_decode::probe_duration(FIXTURE).unwrap() - 0.278_062_5).abs() < 1e-6);
}

#[test]
fn fixture_tags_are_readable() {
    let tags = vorbis::read_tags(FIXTURE).unwrap();
    // This file carries only an encoder string, but the header must parse.
    assert!(tags.all.len() < 8);
    assert_eq!(tags, makepad_audio_decode::read_tags(FIXTURE).unwrap());
}

#[test]
fn streaming_blocks_concatenate_to_the_whole_decode() {
    let whole = vorbis::decode_all(FIXTURE).unwrap();
    let mut decoder = vorbis::VorbisDecoder::new(FIXTURE).unwrap();
    assert_eq!(decoder.rate(), 48_000);
    assert_eq!(decoder.channels(), 1);
    let mut pieces = Vec::new();
    let mut blocks = 0usize;
    while let Some(block) = decoder.next_block().unwrap() {
        assert!(!block.is_empty());
        pieces.extend_from_slice(block);
        blocks += 1;
    }
    assert!(blocks > 4, "expected several blocks, got {blocks}");
    assert_eq!(pieces, whole.pcm_interleaved_f32);
}

#[test]
fn limits_are_enforced() {
    use makepad_audio_decode::Limits;
    let err = vorbis::decode_all_limited(FIXTURE, Limits::with_max_frames(100));
    assert!(matches!(err, Err(AudioError::TooLarge(_))), "{err:?}");
    let tight = Limits { max_channels: 0, ..Limits::default() };
    assert!(matches!(
        vorbis::decode_all_limited(FIXTURE, tight),
        Err(AudioError::TooLarge(_))
    ));
    // A limit that fits decodes as usual.
    assert!(vorbis::decode_all_limited(FIXTURE, Limits::with_max_frames(20_000)).is_ok());
}

// -- totality: truncation and bit flips ------------------------------------

/// Tiny LCG, so the mangling is reproducible without a dependency.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 16
    }
}

/// How many mangled copies each fuzz test tries. The default keeps the suite
/// under a second; `MAKEPAD_VORBIS_FUZZ=20000` is the sweep to run when the
/// decoder changes.
fn fuzz_iterations() -> usize {
    std::env::var("MAKEPAD_VORBIS_FUZZ").ok().and_then(|v| v.parse().ok()).unwrap_or(200)
}

fn exercise(bytes: &[u8]) {
    // Whatever comes back, it must be a value or an error, never a panic, and
    // any samples must be finite.
    if let Ok(audio) = vorbis::decode_all(bytes) {
        assert!(audio.pcm_interleaved_f32.iter().all(|v| v.is_finite()));
        assert!(audio.channels > 0 && audio.rate > 0);
    }
    let _ = vorbis::probe_duration(bytes);
    let _ = vorbis::read_tags(bytes);
    if let Ok(mut d) = vorbis::VorbisDecoder::new(bytes) {
        let mut guard = 0;
        while let Ok(Some(block)) = d.next_block() {
            assert!(block.iter().all(|v| v.is_finite()));
            guard += 1;
            assert!(guard < 100_000, "block loop did not terminate");
        }
    }
}

#[test]
fn truncation_at_every_sixteenth_never_panics() {
    for i in 0..=16 {
        let cut = FIXTURE.len() * i / 16;
        exercise(&FIXTURE[..cut]);
    }
    // And a few odd byte counts around the header boundaries.
    for cut in [1usize, 2, 27, 28, 29, 30, 57, 58, 100, 2000] {
        exercise(&FIXTURE[..cut.min(FIXTURE.len())]);
    }
}

#[test]
fn flipped_bytes_never_panic() {
    let mut rng = Lcg(0x5EED_1234_ABCD_0001);
    for _ in 0..fuzz_iterations() {
        let mut bytes = FIXTURE.to_vec();
        let flips = 1 + (rng.next() % 4) as usize;
        for _ in 0..flips {
            let at = (rng.next() as usize) % bytes.len();
            bytes[at] ^= (rng.next() % 255 + 1) as u8;
        }
        exercise(&bytes);
    }
}

/// Ogg's CRC, so a mangled fixture can be handed to the decoder with valid page
/// checksums. Without this the page reader rejects every flipped page and the
/// codec itself never sees corrupt data.
fn ogg_crc(bytes: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut r = (i as u32) << 24;
        for _ in 0..8 {
            r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04c1_1db7 } else { r << 1 };
        }
        *slot = r;
    }
    let mut crc = 0u32;
    for &b in bytes {
        crc = (crc << 8) ^ table[(((crc >> 24) as u8) ^ b) as usize];
    }
    crc
}

/// Recompute every page checksum in a (possibly mangled) stream.
fn repair_checksums(bytes: &mut [u8]) {
    let mut at = 0usize;
    while at + 27 <= bytes.len() {
        if &bytes[at..at + 4] != b"OggS" {
            at += 1;
            continue;
        }
        let n_segments = bytes[at + 26] as usize;
        let body = at + 27 + n_segments;
        if body > bytes.len() {
            break;
        }
        let body_len: usize = bytes[at + 27..body].iter().map(|&b| b as usize).sum();
        let Some(end) = body.checked_add(body_len).filter(|&e| e <= bytes.len()) else {
            break;
        };
        bytes[at + 22..at + 26].fill(0);
        let crc = ogg_crc(&bytes[at..end]);
        bytes[at + 22..at + 26].copy_from_slice(&crc.to_le_bytes());
        at = end;
    }
}

#[test]
fn flipped_bytes_behind_valid_checksums_never_panic() {
    // The interesting fuzz: the container still checks out, so the mangled
    // bytes reach the codebooks, the floor and the residue.
    let mut rng = Lcg(0xFACE_B00C_0000_0001);
    for _ in 0..fuzz_iterations() {
        let mut bytes = FIXTURE.to_vec();
        let flips = 1 + (rng.next() % 4) as usize;
        for _ in 0..flips {
            // Past the page header of the first page, so the stream stays
            // findable.
            let at = 58 + (rng.next() as usize) % (bytes.len() - 58);
            bytes[at] ^= (rng.next() % 255 + 1) as u8;
        }
        repair_checksums(&mut bytes);
        exercise(&bytes);
    }
}

#[test]
fn truncation_behind_valid_checksums_never_panics() {
    for i in 0..=64 {
        let cut = FIXTURE.len() * i / 64;
        let mut bytes = FIXTURE[..cut].to_vec();
        repair_checksums(&mut bytes);
        exercise(&bytes);
    }
}

#[test]
fn mangled_headers_never_panic() {
    // The first 200 bytes hold the identification header and the start of the
    // comment header: the fields most likely to steer an allocation.
    let mut rng = Lcg(0xC0FF_EE00_1234_5678);
    for _ in 0..fuzz_iterations() {
        let mut bytes = FIXTURE.to_vec();
        let at = (rng.next() as usize) % 200;
        bytes[at] ^= (rng.next() % 255 + 1) as u8;
        exercise(&bytes);
    }
}

#[test]
fn a_truncated_setup_header_is_not_an_allocation() {
    // Cut inside the setup header: the codebook counts still say "thousands of
    // entries", and the decoder must refuse rather than reserve for them.
    for cut in (60..1200).step_by(7) {
        exercise(&FIXTURE[..cut]);
    }
}
