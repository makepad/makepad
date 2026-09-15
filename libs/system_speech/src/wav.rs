//! RIFF/WAVE in and out. The Windows, Android and Linux engines all hand back
//! WAV bytes (a WinRT stream, a file from `synthesizeToFile`, `espeak-ng
//! --stdout`); this turns them into [`SpeechAudio`] and back.

use crate::SpeechAudio;

/// Decode PCM WAV (8/16/24/32-bit integer or 32-bit float, any channel
/// count) to mono `f32`. Multi-channel input is averaged down.
pub fn decode(bytes: &[u8]) -> Result<SpeechAudio, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }
    let mut pos = 12;
    let mut format_tag = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(size).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err("fmt chunk too short".to_string());
                }
                format_tag = u16::from_le_bytes([body[0], body[1]]);
                channels = u16::from_le_bytes([body[2], body[3]]);
                sample_rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                bits = u16::from_le_bytes([body[14], body[15]]);
                // WAVE_FORMAT_EXTENSIBLE: the real tag is the sub-format GUID's first two bytes.
                if format_tag == 0xFFFE && body.len() >= 26 {
                    format_tag = u16::from_le_bytes([body[24], body[25]]);
                }
            }
            b"data" => {
                data = Some(body);
            }
            _ => {}
        }
        // Chunks are word-aligned.
        pos = body_start + size + (size & 1);
    }
    let data = data.ok_or_else(|| "no data chunk".to_string())?;
    if channels == 0 || sample_rate == 0 {
        return Err("missing fmt chunk".to_string());
    }
    let channels = channels as usize;
    let samples: Vec<f32> = match (format_tag, bits) {
        (1, 8) => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        (1, 16) => data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        (1, 24) => data
            .chunks_exact(3)
            .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        (1, 32) => data
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
            .collect(),
        (3, 32) => data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        (3, 64) => data
            .chunks_exact(8)
            .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32)
            .collect(),
        (tag, bits) => return Err(format!("unsupported wav format tag {tag} / {bits} bits")),
    };
    let mono = if channels == 1 {
        samples
    } else {
        samples
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };
    Ok(SpeechAudio { samples: mono, sample_rate })
}

/// Encode mono `f32` as 16-bit PCM WAV.
pub fn encode_pcm16_mono(audio: &SpeechAudio) -> Vec<u8> {
    let data_len = (audio.samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&audio.sample_rate.to_le_bytes());
    out.extend_from_slice(&(audio.sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in &audio.samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm16_roundtrips() {
        let audio = SpeechAudio {
            samples: vec![0.0, 0.5, -0.5, 1.0, -1.0],
            sample_rate: 22_050,
        };
        let decoded = decode(&encode_pcm16_mono(&audio)).unwrap();
        assert_eq!(decoded.sample_rate, 22_050);
        assert_eq!(decoded.samples.len(), 5);
        for (a, b) in audio.samples.iter().zip(&decoded.samples) {
            assert!((a - b).abs() < 1.0 / 32000.0, "{a} vs {b}");
        }
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        // Hand-built stereo 16-bit WAV, one frame: L=0.5, R=-0.5 -> 0.0.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36u32 + 4).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&16384i16.to_le_bytes());
        bytes.extend_from_slice(&(-16384i16).to_le_bytes());
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.samples, vec![0.0]);
        assert_eq!(decoded.sample_rate, 48_000);
    }

    #[test]
    fn rejects_non_wav() {
        assert!(decode(b"not a wav at all").is_err());
    }
}
