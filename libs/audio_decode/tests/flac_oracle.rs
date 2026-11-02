//! FLAC: self-contained fixtures and no-panic sweeps over mangled bytes.

use makepad_audio_decode::{decode_any, flac, AudioFormat};

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
/// a real bitstream to mangle.
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
