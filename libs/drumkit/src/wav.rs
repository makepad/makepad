use std::convert::TryInto;

pub(crate) struct Wav {
    pub(crate) frames: Vec<[f32; 2]>,
    pub(crate) sample_rate: u32,
}

/// Decode mono or stereo PCM WAV into interleaved stereo f32 frames.
/// Integer PCM may be 16, 24 or 32 bit; IEEE float must be 32 bit.
pub(crate) fn decode(bytes: &[u8]) -> Result<Wav, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }
    let mut pos = 12usize;
    let mut format = None;
    let mut data = None;
    while pos.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        let end = start
            .checked_add(size)
            .ok_or_else(|| "WAV chunk size overflow".to_string())?;
        if end > bytes.len() {
            return Err(format!("truncated {:?} WAV chunk", String::from_utf8_lossy(id)));
        }
        match id {
            b"fmt " => format = Some(parse_format(&bytes[start..end])?),
            b"data" => data = Some(&bytes[start..end]),
            _ => {}
        }
        pos = end
            .checked_add(size & 1)
            .ok_or_else(|| "WAV chunk alignment overflow".to_string())?;
    }

    let (tag, channels, sample_rate, bits) =
        format.ok_or_else(|| "missing fmt chunk".to_string())?;
    let data = data.ok_or_else(|| "missing data chunk".to_string())?;
    if !(channels == 1 || channels == 2) {
        return Err(format!("unsupported WAV channel count {channels}"));
    }
    if sample_rate == 0 {
        return Err("WAV sample rate is zero".to_string());
    }
    let bytes_per_sample = match (tag, bits) {
        (1, 16) => 2,
        (1, 24) => 3,
        (1, 32) | (3, 32) => 4,
        _ => return Err(format!("unsupported WAV format tag {tag} / {bits} bits")),
    };
    let frame_bytes = bytes_per_sample * channels as usize;
    if data.len() % frame_bytes != 0 {
        return Err("WAV data is not an exact number of frames".to_string());
    }
    let read = |sample: &[u8]| -> f32 {
        match (tag, bits) {
            (1, 16) => i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0,
            (1, 24) => {
                let raw = (sample[0] as i32)
                    | ((sample[1] as i32) << 8)
                    | ((sample[2] as i32) << 16);
                let signed = if raw & 0x80_0000 != 0 { raw | !0xff_ffff } else { raw };
                signed as f32 / 8_388_608.0
            }
            (1, 32) => i32::from_le_bytes(sample.try_into().unwrap()) as f32 / 2_147_483_648.0,
            (3, 32) => {
                let value = f32::from_le_bytes(sample.try_into().unwrap());
                if value.is_finite() { value } else { 0.0 }
            }
            _ => unreachable!(),
        }
    };
    let mut frames = Vec::with_capacity(data.len() / frame_bytes);
    for frame in data.chunks_exact(frame_bytes) {
        let left = read(&frame[..bytes_per_sample]);
        let right = if channels == 1 {
            left
        } else {
            read(&frame[bytes_per_sample..2 * bytes_per_sample])
        };
        frames.push([left, right]);
    }
    Ok(Wav { frames, sample_rate })
}

fn parse_format(body: &[u8]) -> Result<(u16, u16, u32, u16), String> {
    if body.len() < 16 {
        return Err("fmt chunk too short".to_string());
    }
    let mut tag = u16::from_le_bytes([body[0], body[1]]);
    let channels = u16::from_le_bytes([body[2], body[3]]);
    let sample_rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
    let bits = u16::from_le_bytes([body[14], body[15]]);
    if tag == 0xfffe {
        if body.len() < 26 {
            return Err("WAVE_FORMAT_EXTENSIBLE fmt chunk too short".to_string());
        }
        tag = u16::from_le_bytes([body[24], body[25]]);
    }
    Ok((tag, channels, sample_rate, bits))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_wav(tag: u16, channels: u16, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&48_000u32.to_le_bytes());
        let width = u32::from(bits / 8) * u32::from(channels);
        out.extend_from_slice(&(48_000 * width).to_le_bytes());
        out.extend_from_slice(&(width as u16).to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn decodes_pcm16_mono_and_stereo() {
        let mono = make_wav(1, 1, 16, &16_384i16.to_le_bytes());
        assert_eq!(decode(&mono).unwrap().frames, [[0.5, 0.5]]);

        let mut stereo_data = Vec::new();
        stereo_data.extend_from_slice(&(-16_384i16).to_le_bytes());
        stereo_data.extend_from_slice(&8_192i16.to_le_bytes());
        let stereo = decode(&make_wav(1, 2, 16, &stereo_data)).unwrap();
        assert_eq!(stereo.frames, [[-0.5, 0.25]]);
    }

    #[test]
    fn decodes_pcm24_mono_and_stereo() {
        let mono = decode(&make_wav(1, 1, 24, &[0x00, 0x00, 0x40])).unwrap();
        assert_eq!(mono.frames, [[0.5, 0.5]]);

        let stereo = decode(&make_wav(
            1,
            2,
            24,
            &[0x00, 0x00, 0xc0, 0x00, 0x00, 0x20],
        ))
        .unwrap();
        assert_eq!(stereo.frames, [[-0.5, 0.25]]);
    }

    #[test]
    fn decodes_pcm32_and_float32() {
        let pcm = decode(&make_wav(1, 1, 32, &1_073_741_824i32.to_le_bytes())).unwrap();
        assert_eq!(pcm.frames, [[0.5, 0.5]]);
        let float = decode(&make_wav(3, 2, 32, &[0, 0, 0, 0, 0, 0, 0x80, 0xbf])).unwrap();
        assert_eq!(float.frames, [[0.0, -1.0]]);
    }
}
