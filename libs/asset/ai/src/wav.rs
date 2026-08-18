//! Minimal WAV (RIFF) encoding: mono f32 PCM in, 16-bit PCM WAV bytes out.
//! The speech backend's artifact format — universally decodable, no deps.

/// Encodes mono f32 samples (nominal range -1..=1, clamped) as a 16-bit PCM
/// WAV file.
pub fn encode_wav_pcm16_mono(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);

    // RIFF header.
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    // fmt chunk: PCM, mono, 16-bit.
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * block_align as u32;
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());

    // data chunk.
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Encodes planar stereo f32 samples (`left`, `right` equal length, nominal
/// range -1..=1, clamped) as an interleaved 16-bit PCM WAV file.
pub fn encode_wav_pcm16_stereo(left: &[f32], right: &[f32], sample_rate: u32) -> Vec<u8> {
    let frames = left.len().min(right.len());
    let data_len = (frames * 2 * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    let channels: u16 = 2;
    let bits_per_sample: u16 = 16;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * block_align as u32;
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());

    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..frames {
        for sample in [left[frame], right[frame]] {
            let value = (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

/// Decodes a PCM RIFF/WAV file to mono f32 samples + sample rate. Accepts
/// 16-bit / 24-bit / 32-bit integer PCM and 32-bit float, mono or stereo
/// (stereo is averaged to mono). Compressed formats error. Used for
/// reference-voice input (indextts voice cloning).
pub fn decode_wav_to_mono_f32(bytes: &[u8]) -> Result<(Vec<f32>, u32), String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }
    let mut pos = 12usize;
    let mut format: Option<(u16, u16, u32, u16)> = None; // (tag, channels, rate, bits)
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]])
            as usize;
        let body_start = pos + 8;
        let body_end = (body_start + size).min(bytes.len());
        match id {
            b"fmt " => {
                let body = &bytes[body_start..body_end];
                if body.len() < 16 {
                    return Err("truncated fmt chunk".to_string());
                }
                let tag = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                let bits = u16::from_le_bytes([body[14], body[15]]);
                format = Some((tag, channels, rate, bits));
            }
            b"data" => data = Some(&bytes[body_start..body_end]),
            _ => {}
        }
        pos = body_start + size + (size & 1); // chunks are 2-byte aligned
    }
    let (tag, channels, rate, bits) = format.ok_or("missing fmt chunk")?;
    let data = data.ok_or("missing data chunk")?;
    if channels == 0 || channels > 2 {
        return Err(format!("unsupported channel count {channels}"));
    }
    // WAVE_FORMAT_EXTENSIBLE (0xFFFE) carries the real format in the
    // extension; PCM extensible files still decode fine by bit depth.
    let decode_int = matches!(tag, 1 | 0xFFFE);
    let frame_bytes = channels as usize * (bits as usize / 8);
    if frame_bytes == 0 {
        return Err("zero-size frames".to_string());
    }
    let frames = data.len() / frame_bytes;
    let mut out = Vec::with_capacity(frames);
    for frame in 0..frames {
        let mut acc = 0f32;
        for ch in 0..channels as usize {
            let at = frame * frame_bytes + ch * (bits as usize / 8);
            let value = match (tag, bits) {
                (3, 32) => f32::from_le_bytes([
                    data[at], data[at + 1], data[at + 2], data[at + 3],
                ]),
                (_, 16) if decode_int => {
                    i16::from_le_bytes([data[at], data[at + 1]]) as f32 / 32768.0
                }
                (_, 24) if decode_int => {
                    let v = ((data[at + 2] as i32) << 24
                        | (data[at + 1] as i32) << 16
                        | (data[at] as i32) << 8)
                        >> 8;
                    v as f32 / 8_388_608.0
                }
                (_, 32) if decode_int => {
                    i32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
                        as f32
                        / 2_147_483_648.0
                }
                _ => return Err(format!("unsupported wav format tag {tag} bits {bits}")),
            };
            acc += value;
        }
        out.push(acc / channels as f32);
    }
    if out.is_empty() {
        return Err("empty wav data".to_string());
    }
    Ok((out, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_round_trips_encode() {
        let samples = vec![0.0, 0.25, -0.25, 0.9, -0.9];
        let wav = encode_wav_pcm16_mono(&samples, 22_050);
        let (decoded, rate) = decode_wav_to_mono_f32(&wav).unwrap();
        assert_eq!(rate, 22_050);
        assert_eq!(decoded.len(), samples.len());
        for (a, b) in decoded.iter().zip(&samples) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn decode_stereo_averages_to_mono() {
        let wav = encode_wav_pcm16_stereo(&[0.5, 0.5], &[-0.5, 0.5], 16_000);
        let (decoded, rate) = decode_wav_to_mono_f32(&wav).unwrap();
        assert_eq!(rate, 16_000);
        assert_eq!(decoded.len(), 2);
        assert!(decoded[0].abs() < 1e-3);
        assert!((decoded[1] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn decode_rejects_junk() {
        assert!(decode_wav_to_mono_f32(b"not a wav").is_err());
    }

    #[test]
    fn stereo_header_and_sizes() {
        let wav = encode_wav_pcm16_stereo(&[0.0, 1.0], &[-1.0, 0.5], 44_100);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 2 * 2 * 2);
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            44_100
        );
        // Interleaved order: L0 R0 L1 R1.
        let r0 = i16::from_le_bytes([wav[46], wav[47]]);
        assert_eq!(r0, -32767);
    }

    #[test]
    fn header_and_sizes() {
        let wav = encode_wav_pcm16_mono(&[0.0, 0.5, -0.5, 1.0, -1.0], 24_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 5 * 2);
        // riff size = total - 8
        assert_eq!(
            u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]) as usize,
            wav.len() - 8
        );
        // sample rate at offset 24
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            24_000
        );
        // 16-bit mono PCM
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1);
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        // Clamped full-scale values.
        let last = i16::from_le_bytes([wav[wav.len() - 2], wav[wav.len() - 1]]);
        assert_eq!(last, -32767);
    }
}
