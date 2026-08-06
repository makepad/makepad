//! RIFF/WAVE decoder: PCM 8/16/24/32-bit integer and 32/64-bit float.
//!
//! Total on malformed input. Every length in the file is attacker-controlled,
//! so nothing here sizes an allocation from a header field without first
//! checking it against the bytes actually present.

use crate::{AudioError, Pcm};

/// Chunk headers we understand; everything else is skipped by length.
const RIFF: &[u8; 4] = b"RIFF";
const WAVE: &[u8; 4] = b"WAVE";
const FMT: &[u8; 4] = b"fmt ";
const DATA: &[u8; 4] = b"data";

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn u32le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]))
}

/// Decode a WAV file into interleaved f32 samples in [-1, 1].
pub fn decode(bytes: &[u8]) -> Result<Pcm, AudioError> {
    if bytes.len() < 12 {
        return Err(AudioError::Truncated);
    }
    if &bytes[0..4] != RIFF || &bytes[8..12] != WAVE {
        return Err(AudioError::NotWav);
    }

    let mut pos = 12usize;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut format = 0u16;
    let mut data_range: Option<(usize, usize)> = None;

    // Walk chunks. A chunk claiming a size past EOF is a truncated file, not
    // a reason to index out of bounds.
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32le(bytes, pos + 4).ok_or(AudioError::Truncated)? as usize;
        let body = pos + 8;
        // Chunks are word-aligned; the pad byte is not counted in `size`.
        let next = body
            .checked_add(size)
            .ok_or(AudioError::Malformed)?
            .checked_add(size & 1)
            .ok_or(AudioError::Malformed)?;

        if id == FMT {
            if size < 16 || body + 16 > bytes.len() {
                return Err(AudioError::Truncated);
            }
            format = u16le(bytes, body).ok_or(AudioError::Truncated)?;
            channels = u16le(bytes, body + 2).ok_or(AudioError::Truncated)?;
            sample_rate = u32le(bytes, body + 4).ok_or(AudioError::Truncated)?;
            bits = u16le(bytes, body + 14).ok_or(AudioError::Truncated)?;
            if format == FORMAT_EXTENSIBLE {
                // The real format lives in the GUID's first two bytes.
                if size >= 26 && body + 26 <= bytes.len() {
                    format = u16le(bytes, body + 24).ok_or(AudioError::Truncated)?;
                } else {
                    return Err(AudioError::Malformed);
                }
            }
        } else if id == DATA {
            // Clamp rather than trust: a bogus huge size still yields the
            // bytes that exist, which is what every real player does.
            let end = body.saturating_add(size).min(bytes.len());
            if body > bytes.len() {
                return Err(AudioError::Truncated);
            }
            data_range = Some((body, end));
        }

        if next <= pos {
            // Zero-size chunk loop guard.
            return Err(AudioError::Malformed);
        }
        pos = next;
    }

    let (start, end) = data_range.ok_or(AudioError::Malformed)?;
    if channels == 0 || channels > 8 {
        return Err(AudioError::UnsupportedChannels(channels));
    }
    if sample_rate == 0 || sample_rate > 384_000 {
        return Err(AudioError::UnsupportedRate(sample_rate));
    }

    let raw = &bytes[start..end];
    let samples = decode_samples(raw, format, bits)?;

    Ok(Pcm {
        channels: channels as usize,
        sample_rate,
        samples,
    })
}

fn decode_samples(raw: &[u8], format: u16, bits: u16) -> Result<Vec<f32>, AudioError> {
    match (format, bits) {
        (FORMAT_PCM, 8) => {
            // 8-bit WAV is unsigned, biased at 128.
            Ok(raw
                .iter()
                .map(|&b| (b as f32 - 128.0) / 128.0)
                .collect())
        }
        (FORMAT_PCM, 16) => Ok(raw
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect()),
        (FORMAT_PCM, 24) => Ok(raw
            .chunks_exact(3)
            .map(|c| {
                // Sign-extend 24 -> 32.
                let v = ((c[2] as i32) << 24) | ((c[1] as i32) << 16) | ((c[0] as i32) << 8);
                (v >> 8) as f32 / 8_388_608.0
            })
            .collect()),
        (FORMAT_PCM, 32) => Ok(raw
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
            .collect()),
        (FORMAT_FLOAT, 32) => Ok(raw
            .chunks_exact(4)
            .map(|c| {
                let v = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                if v.is_finite() {
                    v
                } else {
                    0.0
                }
            })
            .collect()),
        (FORMAT_FLOAT, 64) => Ok(raw
            .chunks_exact(8)
            .map(|c| {
                let v = f64::from_le_bytes([
                    c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7],
                ]);
                if v.is_finite() {
                    v as f32
                } else {
                    0.0
                }
            })
            .collect()),
        _ => Err(AudioError::UnsupportedFormat { format, bits }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid WAV around the given sample bytes.
    fn wav(format: u16, bits: u16, channels: u16, rate: u32, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(RIFF);
        v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        v.extend_from_slice(WAVE);
        v.extend_from_slice(FMT);
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&format.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes()); // byte rate (unused)
        v.extend_from_slice(&0u16.to_le_bytes()); // block align (unused)
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(DATA);
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn decodes_16_bit_stereo() {
        let data: Vec<u8> = [0i16, 16384, -16384, 32767]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let pcm = decode(&wav(FORMAT_PCM, 16, 2, 44100, &data)).unwrap();
        assert_eq!(pcm.channels, 2);
        assert_eq!(pcm.sample_rate, 44100);
        assert_eq!(pcm.samples.len(), 4);
        assert!((pcm.samples[0] - 0.0).abs() < 1e-6);
        assert!((pcm.samples[1] - 0.5).abs() < 1e-3);
        assert!((pcm.samples[2] + 0.5).abs() < 1e-3);
    }

    #[test]
    fn decodes_every_supported_bit_depth() {
        // 8-bit unsigned: 128 is silence.
        let p8 = decode(&wav(FORMAT_PCM, 8, 1, 22050, &[128, 255, 0])).unwrap();
        assert!(p8.samples[0].abs() < 1e-6);
        assert!(p8.samples[1] > 0.9);
        assert!(p8.samples[2] < -0.9);

        // 24-bit: 0x400000 is +0.5.
        let d24 = [0x00u8, 0x00, 0x40, 0x00, 0x00, 0xC0];
        let p24 = decode(&wav(FORMAT_PCM, 24, 1, 44100, &d24)).unwrap();
        assert!((p24.samples[0] - 0.5).abs() < 1e-3, "{}", p24.samples[0]);
        assert!((p24.samples[1] + 0.5).abs() < 1e-3, "{}", p24.samples[1]);

        // 32-bit int.
        let d32: Vec<u8> = [0i32, 1_073_741_824]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let p32 = decode(&wav(FORMAT_PCM, 32, 1, 48000, &d32)).unwrap();
        assert!((p32.samples[1] - 0.5).abs() < 1e-6);

        // f32.
        let df: Vec<u8> = [0.25f32, -0.75]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let pf = decode(&wav(FORMAT_FLOAT, 32, 1, 48000, &df)).unwrap();
        assert!((pf.samples[0] - 0.25).abs() < 1e-6);
        assert!((pf.samples[1] + 0.75).abs() < 1e-6);

        // f64.
        let d64: Vec<u8> = [0.5f64].iter().flat_map(|v| v.to_le_bytes()).collect();
        let p64 = decode(&wav(FORMAT_FLOAT, 64, 1, 48000, &d64)).unwrap();
        assert!((p64.samples[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn non_finite_float_samples_become_silence() {
        let d: Vec<u8> = [f32::NAN, f32::INFINITY, 0.5]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let p = decode(&wav(FORMAT_FLOAT, 32, 1, 48000, &d)).unwrap();
        assert_eq!(p.samples[0], 0.0);
        assert_eq!(p.samples[1], 0.0);
        assert!(p.samples.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn rejects_junk_without_panicking() {
        assert!(decode(&[]).is_err());
        assert!(decode(b"not a wav file at all").is_err());
        assert!(decode(b"RIFF____WAVE").is_err()); // no fmt/data
    }

    #[test]
    fn oversized_data_chunk_is_clamped_not_trusted() {
        let mut w = wav(FORMAT_PCM, 16, 1, 44100, &[0, 0, 0, 0]);
        // Rewrite the data chunk size to claim 4 GiB.
        let n = w.len();
        w[n - 8..n - 4].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        let p = decode(&w).expect("clamps rather than allocating from the header");
        assert_eq!(p.samples.len(), 2);
    }

    #[test]
    fn zero_channels_or_rate_refused() {
        assert!(decode(&wav(FORMAT_PCM, 16, 0, 44100, &[0, 0])).is_err());
        assert!(decode(&wav(FORMAT_PCM, 16, 1, 0, &[0, 0])).is_err());
    }

    #[test]
    fn unsupported_depth_reports_rather_than_guesses() {
        let e = decode(&wav(FORMAT_PCM, 12, 1, 44100, &[0, 0])).unwrap_err();
        assert!(matches!(e, AudioError::UnsupportedFormat { .. }));
    }
}
