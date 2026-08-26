//! FLAC: a no-panic sweep over mangled bytes, and — when `afconvert` is on
//! PATH — a sample-exact comparison against a FLAC it produced from a
//! synthetic WAV.
//!
//! ```text
//! afconvert -f flac -d flac in.wav out.flac
//! afconvert -f WAVE -d LEI16 in.flac out.wav
//! ```

use makepad_audio_decode::{decode_any, flac, AudioError, AudioFormat, Limits};
use std::path::PathBuf;
use std::process::Command;

// -- afconvert oracle ------------------------------------------------------

fn afconvert_ok() -> bool {
    Command::new("afconvert").arg("-help").output().is_ok()
}

fn write_wav_i16(path: &std::path::Path, rate: u32, channels: u16, pcm: &[i16]) {
    let data_bytes = pcm.len() * 2;
    let mut w = Vec::with_capacity(44 + data_bytes);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36u32 + data_bytes as u32).to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    let byte_rate = rate * channels as u32 * 2;
    w.extend_from_slice(&byte_rate.to_le_bytes());
    w.extend_from_slice(&(channels * 2).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for s in pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, w).expect("write wav");
}

/// Two seconds of stereo 44.1 kHz: inharmonic sines plus a short burst, so a
/// real encoder will emit FIXED/LPC/Rice rather than a CONSTANT stream.
fn test_signal() -> Vec<i16> {
    let rate = 44_100.0;
    let n = 44_100 * 2;
    let mut pcm = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / rate;
        let left = (2.0 * std::f64::consts::PI * 440.0 * t).sin() * 0.45
            + (2.0 * std::f64::consts::PI * 880.0 * t).sin() * 0.12;
        let right = (2.0 * std::f64::consts::PI * 659.25 * t).sin() * 0.40
            + (2.0 * std::f64::consts::PI * 220.0 * t).sin() * 0.18;
        let burst = if (i as i32 - 20_000).unsigned_abs() < 64 { 0.2 } else { 0.0 };
        pcm.push((left * 30_000.0) as i16);
        pcm.push(((right + burst) * 30_000.0) as i16);
    }
    pcm
}

fn oracle_dir() -> PathBuf {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    let dir = base.join("flac-oracle");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[test]
fn oracle_matches_afconvert_sample_exact() {
    if !afconvert_ok() {
        eprintln!("afconvert not available; skipping the FLAC oracle comparison");
        return;
    }
    let dir = oracle_dir();
    let wav_path = dir.join("signal.wav");
    let flac_path = dir.join("signal.flac");
    let pcm = test_signal();
    write_wav_i16(&wav_path, 44_100, 2, &pcm);
    // afconvert refuses to replace an existing destination.
    let _ = std::fs::remove_file(&flac_path);

    let status = Command::new("afconvert")
        .args(["-f", "flac", "-d", "flac"])
        .arg(&wav_path)
        .arg(&flac_path)
        .status()
        .expect("spawn afconvert");
    assert!(status.success(), "afconvert -f flac failed: {status}");
    let bytes = std::fs::read(&flac_path).expect("read flac");
    assert_eq!(makepad_audio_decode::sniff(&bytes), Some(AudioFormat::Flac));

    let audio = flac::decode_all(&bytes).expect("decode flac");
    assert_eq!(audio.rate, 44_100);
    assert_eq!(audio.channels, 2);
    assert_eq!(audio.frames(), pcm.len() / 2, "frame count");

    let mut mismatches = 0usize;
    let mut first = None;
    for (i, (&want, &got_f)) in pcm.iter().zip(audio.pcm_interleaved_f32.iter()).enumerate() {
        let got = (got_f * 32768.0).round() as i32;
        if got != want as i32 {
            mismatches += 1;
            if first.is_none() {
                first = Some((i, want, got, got_f));
            }
        }
    }
    if mismatches > 0 {
        panic!(
            "{mismatches} sample mismatches; first at {:?} of {}",
            first,
            pcm.len()
        );
    }

    let probed = flac::probe_duration(&bytes).unwrap();
    assert!((probed - audio.duration_secs()).abs() < 1e-9, "{probed} vs {}", audio.duration_secs());
    assert_eq!(decode_any(&bytes).unwrap(), audio);

    let mut stream = flac::FlacDecoder::new(&bytes).unwrap();
    let mut pieces = Vec::new();
    let mut block_sizes = std::collections::BTreeSet::new();
    while let Some(frame) = stream.next_frame().unwrap() {
        block_sizes.insert(frame.pcm.len() / frame.channels as usize);
        pieces.extend_from_slice(frame.pcm);
    }
    assert_eq!(pieces, audio.pcm_interleaved_f32);
    assert!(block_sizes.len() > 1, "oracle stream should include a short final block");

    let err = flac::decode_all_limited(&bytes, Limits::with_max_frames(100));
    assert!(matches!(err, Err(AudioError::TooLarge(_))), "{err:?}");
}

// -- totality: truncation and bit flips ------------------------------------

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 16
    }
}

fn exercise(bytes: &[u8]) {
    if let Ok(audio) = flac::decode_all(bytes) {
        assert!(audio.pcm_interleaved_f32.iter().all(|v| v.is_finite()));
        assert!(audio.channels > 0 && audio.rate > 0);
    }
    let _ = flac::probe_duration(bytes);
    let _ = flac::read_tags(bytes);
    let _ = decode_any(bytes);
    if let Ok(mut d) = flac::FlacDecoder::new(bytes) {
        let mut guard = 0;
        while let Ok(Some(frame)) = d.next_frame() {
            assert!(frame.pcm.iter().all(|v| v.is_finite()));
            guard += 1;
            assert!(guard < 100_000, "frame loop did not terminate");
        }
    }
}

/// Tiny valid FLAC (CONSTANT, 8 samples mono 16-bit) so the hostile sweep has
/// a real bitstream to mangle, even when afconvert is absent.
fn tiny_flac() -> Vec<u8> {
    // Built by the in-crate encoder shape: fLaC + STREAMINFO + one CONSTANT
    // frame of silence. CRC-8/16 are computed the same way as the decoder.
    fn crc8(data: &[u8]) -> u8 {
        let mut crc = 0u8;
        for &b in data {
            crc ^= b;
            for _ in 0..8 {
                crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
            }
        }
        crc
    }
    fn crc16(data: &[u8]) -> u16 {
        let mut crc = 0u16;
        for &b in data {
            crc ^= (b as u16) << 8;
            for _ in 0..8 {
                crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
            }
        }
        crc
    }
    let mut file = b"fLaC".to_vec();
    let mut info = [0u8; 34];
    info[0..2].copy_from_slice(&8u16.to_be_bytes());
    info[2..4].copy_from_slice(&8u16.to_be_bytes());
    let mut w = 0u64;
    w |= 8_000u64 << 44;
    w |= 0u64 << 41; // 1 channel
    w |= 15u64 << 36; // 16 bps
    w |= 8; // 8 samples
    info[10..18].copy_from_slice(&w.to_be_bytes());
    // MD5 left at zero: "not calculated". The hostile sweep only needs a
    // structurally valid file; STREAMINFO MD5 is covered by the crate tests.
    file.push(0x80); // last, STREAMINFO
    file.push(0);
    file.push(0);
    file.push(34);
    file.extend_from_slice(&info);

    let mut frame = vec![0xFF, 0xF8];
    // blocksize 8 = 256*2^(n-8) is 256 for n=8; use uncommon 8-bit: 0110, (8-1).
    frame.push(0x60);
    frame.push(0x00); // mono, bps from STREAMINFO
    frame.push(0x00); // frame number 0
    frame.push(7); // blocksize-1
    let c = crc8(&frame);
    frame.push(c);
    // CONSTANT 0, 16-bit, 8 samples: padding+type+wasted + 16-bit sample, then pad.
    // 1+6+1+16 = 24 bits → 3 bytes: 0x00, 0x00, 0x00
    frame.extend_from_slice(&[0x00, 0x00, 0x00]);
    let fc = crc16(&frame);
    frame.extend_from_slice(&fc.to_be_bytes());
    file.extend_from_slice(&frame);
    file
}

#[test]
fn tiny_fixture_decodes() {
    let bytes = tiny_flac();
    let audio = flac::decode_all(&bytes).expect("tiny flac");
    assert_eq!(audio.rate, 8_000);
    assert_eq!(audio.channels, 1);
    assert_eq!(audio.frames(), 8);
    assert!(audio.pcm_interleaved_f32.iter().all(|&v| v == 0.0));
    assert_eq!(makepad_audio_decode::sniff(&bytes), Some(AudioFormat::Flac));
}

#[test]
fn truncation_never_panics() {
    let fixture = tiny_flac();
    for cut in 0..fixture.len() {
        assert!(flac::decode_all(&fixture[..cut]).is_err(), "prefix {cut} decoded successfully");
        exercise(&fixture[..cut]);
    }
    for cut in [0usize, 1, 2, 3, 4, 5, 8, 10, 20, 38, 42, 50] {
        exercise(&fixture[..cut.min(fixture.len())]);
    }
    exercise(&[]);
    exercise(&[0xFF; 64]);
    exercise(b"fLaC\xff\xff\xff\xff");
    exercise(b"not a flac at all, really quite long garbage padding");
}

#[test]
fn flipped_bytes_never_panic() {
    let fixture = tiny_flac();
    let mut rng = Lcg(0x5EED_1234_ABCD_0001);
    for _ in 0..200 {
        let mut bytes = fixture.clone();
        let flips = 1 + (rng.next() % 4) as usize;
        for _ in 0..flips {
            let at = (rng.next() as usize) % bytes.len();
            bytes[at] ^= (rng.next() % 255 + 1) as u8;
        }
        exercise(&bytes);
    }
}

#[test]
fn random_garbage_never_panics() {
    let mut rng = Lcg(0xC0FF_EE00_1234_5678);
    for _ in 0..50 {
        let n = 1 + (rng.next() % 4096) as usize;
        let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        exercise(&bytes);
    }
}
