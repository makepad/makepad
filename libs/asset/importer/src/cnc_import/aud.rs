use std::fmt;

const CHUNK_MARKER: u32 = 0x0000_deaf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudError {
    Truncated,
    UnsupportedCodec(u8),
    UnsupportedFlags(u8),
    InvalidChunkMarker,
    InvalidChunk,
    OutputSizeMismatch { expected: usize, actual: usize },
}

impl fmt::Display for AudError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated AUD file"),
            Self::UnsupportedCodec(codec) => write!(f, "unsupported AUD codec {codec}"),
            Self::UnsupportedFlags(flags) => write!(f, "unsupported AUD flags {flags:#04x}"),
            Self::InvalidChunkMarker => f.write_str("invalid AUD chunk marker"),
            Self::InvalidChunk => f.write_str("invalid AUD chunk"),
            Self::OutputSizeMismatch { expected, actual } => {
                write!(f, "AUD output is {actual} bytes, expected {expected}")
            }
        }
    }
}

impl std::error::Error for AudError {}

#[derive(Clone, Debug)]
pub struct Aud {
    sample_rate: u16,
    channels: u8,
    codec: u8,
    output_size: usize,
    samples: Vec<i16>,
}

impl Aud {
    pub fn parse(bytes: &[u8]) -> Result<Self, AudError> {
        if bytes.len() < 12 {
            return Err(AudError::Truncated);
        }
        let sample_rate = read_u16(bytes, 0)?;
        let data_size = read_u32(bytes, 2)? as usize;
        let output_size = read_u32(bytes, 6)? as usize;
        let flags = bytes[10];
        let codec = bytes[11];
        if flags & !3 != 0 {
            return Err(AudError::UnsupportedFlags(flags));
        }
        let channels = if flags & 1 != 0 { 2 } else { 1 };
        let data_end = 12usize.checked_add(data_size).ok_or(AudError::Truncated)?;
        if data_end > bytes.len() {
            return Err(AudError::Truncated);
        }
        let data = &bytes[12..data_end];
        let maximum_output = match codec {
            1 => data_size.checked_mul(64),
            99 => data_size.checked_mul(4),
            _ => None,
        };
        if maximum_output.is_some_and(|maximum| output_size > maximum) {
            return Err(AudError::InvalidChunk);
        }
        let samples = match codec {
            1 => {
                if flags & 2 != 0 {
                    return Err(AudError::UnsupportedFlags(flags));
                }
                decode_westwood(data, output_size)?
                    .into_iter()
                    .map(|sample| ((sample as i32 - 128) << 8) as i16)
                    .collect()
            }
            99 => {
                if flags & 2 == 0 || output_size % 2 != 0 {
                    return Err(AudError::UnsupportedFlags(flags));
                }
                decode_ima(data, output_size, channels as usize)?
            }
            codec => return Err(AudError::UnsupportedCodec(codec)),
        };
        let actual_size = match codec {
            1 => samples.len(),
            99 => samples.len().checked_mul(2).ok_or(AudError::InvalidChunk)?,
            _ => return Err(AudError::UnsupportedCodec(codec)),
        };
        if actual_size != output_size {
            return Err(AudError::OutputSizeMismatch {
                expected: output_size,
                actual: actual_size,
            });
        }
        Ok(Self {
            sample_rate,
            channels,
            codec,
            output_size,
            samples,
        })
    }

    pub fn sample_rate(&self) -> u16 {
        self.sample_rate
    }

    pub fn channels(&self) -> u8 {
        self.channels
    }

    pub fn codec(&self) -> u8 {
        self.codec
    }

    pub fn output_size(&self) -> usize {
        self.output_size
    }

    pub fn samples(&self) -> &[i16] {
        &self.samples
    }
}

fn decode_ima(data: &[u8], expected_size: usize, channels: usize) -> Result<Vec<i16>, AudError> {
    const INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
    const STEP: [i32; 89] = [
        7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45,
        50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230,
        253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
        1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327,
        3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442,
        11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
        32767,
    ];
    let mut predictor = vec![0i32; channels];
    let mut step_index = vec![0i32; channels];
    let mut output = Vec::with_capacity(expected_size / 2);
    let mut at = 0usize;
    let mut channel = 0usize;
    while at < data.len() {
        let (chunk, out_size) = next_chunk(data, &mut at)?;
        if out_size % 2 != 0 {
            return Err(AudError::InvalidChunk);
        }
        let sample_count = out_size / 2;
        let available = chunk.len().checked_mul(2).ok_or(AudError::InvalidChunk)?;
        if sample_count > available + 1 {
            return Err(AudError::InvalidChunk);
        }
        let before = output.len();
        for &byte in chunk {
            for code in [byte & 0x0f, byte >> 4] {
                if output.len() - before == sample_count {
                    break;
                }
                let state = channel;
                let step = STEP[step_index[state] as usize];
                let mut difference = step >> 3;
                if code & 1 != 0 {
                    difference += step >> 2;
                }
                if code & 2 != 0 {
                    difference += step >> 1;
                }
                if code & 4 != 0 {
                    difference += step;
                }
                if code & 8 != 0 {
                    predictor[state] -= difference;
                } else {
                    predictor[state] += difference;
                }
                predictor[state] = predictor[state].clamp(i16::MIN as i32, i16::MAX as i32);
                step_index[state] =
                    (step_index[state] + INDEX[(code & 7) as usize]).clamp(0, 88);
                output.push(predictor[state] as i16);
                channel = (channel + 1) % channels;
            }
        }
        // A number of files declare one final sample beyond the packed
        // nibble count. Preserve the current predictor for that sample; this
        // is the only interpretation that reaches their declared PCM size
        // without inventing another ADPCM code.
        if output.len() - before + 1 == sample_count {
            output.push(predictor[channel] as i16);
            channel = (channel + 1) % channels;
        }
        if output.len() - before != sample_count {
            return Err(AudError::InvalidChunk);
        }
    }
    Ok(output)
}

fn decode_westwood(data: &[u8], expected_size: usize) -> Result<Vec<u8>, AudError> {
    const TABLE2: [i16; 4] = [-2, -1, 0, 1];
    const TABLE4: [i16; 16] = [-9, -8, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 8];
    let mut output = Vec::with_capacity(expected_size);
    let mut sample = 0x80i16;
    let mut at = 0usize;
    while at < data.len() {
        let (chunk, out_size) = next_chunk(data, &mut at)?;
        let before = output.len();
        if chunk.len() == out_size {
            for &value in chunk {
                sample = value as i16;
                output.push(value);
            }
        } else {
            let mut src_at = 0usize;
            while src_at < chunk.len() && output.len() - before < out_size {
                let command = chunk[src_at];
                src_at += 1;
                let count = (command & 0x3f) as usize + 1;
                match command >> 6 {
                    0 => {
                        let values = take(chunk, &mut src_at, count)?;
                        for &value in values {
                            for shift in [0, 2, 4, 6] {
                                sample = (sample + TABLE2[((value >> shift) & 3) as usize]).clamp(0, 255);
                                output.push(sample as u8);
                            }
                        }
                    }
                    1 => {
                        let values = take(chunk, &mut src_at, count)?;
                        for &value in values {
                            for code in [value & 0x0f, value >> 4] {
                                sample = (sample + TABLE4[code as usize]).clamp(0, 255);
                                output.push(sample as u8);
                            }
                        }
                    }
                    2 if command & 0x20 != 0 => {
                        let delta = (((command & 0x1f) as i8) << 3) >> 3;
                        sample = (sample + delta as i16).clamp(0, 255);
                        output.push(sample as u8);
                    }
                    2 => {
                        let values = take(chunk, &mut src_at, count)?;
                        for &value in values {
                            sample = value as i16;
                            output.push(value);
                        }
                    }
                    3 => {
                        let end = output.len().checked_add(count).ok_or(AudError::InvalidChunk)?;
                        output.resize(end, sample as u8);
                    }
                    _ => return Err(AudError::InvalidChunk),
                }
                if output.len() - before > out_size {
                    return Err(AudError::InvalidChunk);
                }
            }
            if src_at != chunk.len() {
                return Err(AudError::InvalidChunk);
            }
        }
        if output.len() - before != out_size {
            return Err(AudError::InvalidChunk);
        }
    }
    Ok(output)
}

fn next_chunk<'a>(data: &'a [u8], at: &mut usize) -> Result<(&'a [u8], usize), AudError> {
    let size = read_u16(data, *at)? as usize;
    let out_size = read_u16(data, at.checked_add(2).ok_or(AudError::Truncated)?)? as usize;
    let marker = read_u32(data, at.checked_add(4).ok_or(AudError::Truncated)?)?;
    if marker != CHUNK_MARKER {
        return Err(AudError::InvalidChunkMarker);
    }
    let start = at.checked_add(8).ok_or(AudError::Truncated)?;
    let end = start.checked_add(size).ok_or(AudError::Truncated)?;
    let chunk = data.get(start..end).ok_or(AudError::Truncated)?;
    *at = end;
    Ok((chunk, out_size))
}

fn take<'a>(src: &'a [u8], at: &mut usize, count: usize) -> Result<&'a [u8], AudError> {
    let end = at.checked_add(count).ok_or(AudError::InvalidChunk)?;
    let values = src.get(*at..end).ok_or(AudError::InvalidChunk)?;
    *at = end;
    Ok(values)
}

fn read_u16(bytes: &[u8], at: usize) -> Result<u16, AudError> {
    let value = bytes.get(at..at.checked_add(2).ok_or(AudError::Truncated)?).ok_or(AudError::Truncated)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, AudError> {
    let value = bytes.get(at..at.checked_add(4).ok_or(AudError::Truncated)?).ok_or(AudError::Truncated)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cnc_import_aud_raw_eight_bit_chunk() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&11025u16.to_le_bytes());
        bytes.extend_from_slice(&11u32.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 1]);
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&CHUNK_MARKER.to_le_bytes());
        bytes.extend_from_slice(&[0, 128, 255]);
        let aud = Aud::parse(&bytes).unwrap();
        assert_eq!(aud.samples(), &[i16::MIN, 0, 32512]);
    }

    #[test]
    fn cnc_import_aud_ima_uses_low_nibble_first() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&22050u16.to_le_bytes());
        bytes.extend_from_slice(&9u32.to_le_bytes());
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[2, 99]);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&CHUNK_MARKER.to_le_bytes());
        bytes.push(0x11);
        assert_eq!(Aud::parse(&bytes).unwrap().samples(), &[1, 2]);
    }

    #[test]
    fn cnc_import_aud_two_bit_deltas_are_low_first() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&11025u16.to_le_bytes());
        bytes.extend_from_slice(&10u32.to_le_bytes());
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 1]);
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&CHUNK_MARKER.to_le_bytes());
        bytes.extend_from_slice(&[0, 0b00_11_10_01]);
        assert_eq!(Aud::parse(&bytes).unwrap().samples(), &[-256, -256, 0, -512]);
    }
}
