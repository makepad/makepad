//! Streaming conversion of float ggml model files into resident block weights.
use super::quant::f16_to_f32;
use makepad_ai_loader::{quant::TensorType, quant_encode};
use std::io::{self, Read, Write};

#[derive(Clone, Copy, Debug)]
pub enum WeightQuantization {
    Q8_0,
    Q5_0,
    Q5K,
    Q4K,
}

impl WeightQuantization {
    fn tensor_type(self) -> u32 {
        match self { Self::Q8_0 => 8, Self::Q5_0 => 6, Self::Q5K => 13, Self::Q4K => 12 }
    }

    fn block_bytes(self) -> usize {
        match self { Self::Q8_0 => 34, Self::Q5_0 => 22, Self::Q5K => 176, Self::Q4K => 144 }
    }

    fn block_elements(self) -> usize {
        match self { Self::Q8_0 | Self::Q5_0 => 32, Self::Q5K | Self::Q4K => 256 }
    }

    fn for_row(self, cols: u32) -> Self {
        if cols % self.block_elements() as u32 == 0 { self } else { Self::Q8_0 }
    }

    pub(super) fn block(self, input: &[f32], out: &mut [u8]) -> io::Result<()> {
        match self {
            Self::Q8_0 | Self::Q5K | Self::Q4K => {
                let ty = match self { Self::Q8_0 => TensorType::Q8_0, Self::Q5K => TensorType::Q5K,
                    Self::Q4K => TensorType::Q4K, Self::Q5_0 => unreachable!() };
                quant_encode::encode(ty, input, &mut out[..self.block_bytes()]).map_err(|e| invalid(&e))?;
            }
            Self::Q5_0 => {
                let mut extreme = 0.0f32;
                for &v in input {
                    if v.abs() > extreme.abs() { extreme = v; }
                }
                // Signed scale assigns the largest magnitude to -16. The
                // remaining codes span -15..15, stored with a bias of 16.
                let d = extreme / -16.0;
                let inv = if d == 0.0 { 0.0 } else { 1.0 / d };
                out[..2].copy_from_slice(&scale_half(d).to_le_bytes());
                let mut high = 0u32;
                for j in 0..16 {
                    let lo = (input[j] * inv + 16.5).floor().clamp(0.0, 31.0) as u8;
                    let hi = (input[j + 16] * inv + 16.5).floor().clamp(0.0, 31.0) as u8;
                    out[6 + j] = (lo & 15) | ((hi & 15) << 4);
                    high |= u32::from(lo >> 4) << j;
                    high |= u32::from(hi >> 4) << (j + 16);
                }
                out[2..6].copy_from_slice(&high.to_le_bytes());
            }
        }
        Ok(())
    }
}

// Round a finite scale to nearest-even half, including subnormal scales.
// Keep conversion local to the file writer; inference paths are unchanged.
fn scale_half(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 255) as i32 - 127 + 15;
    let fraction = bits & 0x7fffff;
    if exponent >= 31 { return sign | 0x7c00; }
    if exponent < -10 { return sign; }
    let (mantissa, shift, base) = if exponent <= 0 {
        (fraction | 0x800000, (14 - exponent) as u32, 0)
    } else { (fraction, 13, (exponent as u32) << 10) };
    let rounded = (mantissa + ((1 << (shift - 1)) - 1) + ((mantissa >> shift) & 1)) >> shift;
    sign | (base + rounded) as u16
}

#[derive(Debug, Default)]
pub struct QuantizationStats {
    pub tensors: usize,
    pub matrices: usize,
    pub input_weight_bytes: u64,
    pub output_weight_bytes: u64,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    r.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn positive(v: u32) -> io::Result<u64> {
    if v == 0 || v > i32::MAX as u32 { return Err(invalid("invalid ggml dimension")); }
    Ok(u64::from(v))
}

fn copy_exact(r: &mut impl Read, w: &mut impl Write, bytes: u64) -> io::Result<()> {
    if io::copy(&mut r.take(bytes), w)? != bytes {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated ggml file"));
    }
    Ok(())
}

/// Quantise only 2-D `.weight` tensors whose contiguous dimension is a
/// multiple of 32. Embeddings, convolutions, norms, biases, filters and
/// vocabulary retain their original bytes and tensor types. File ordering
/// and names are unchanged. K formats require 256-wide row multiples;
/// eligible shorter rows use Q8_0. Memory use is independent of tensor size.
pub fn quantize_ggml(r: &mut impl Read, w: &mut impl Write, format: WeightQuantization) -> io::Result<QuantizationStats> {
    let mut header = [0; 48];
    r.read_exact(&mut header)?;
    if u32::from_le_bytes(header[..4].try_into().unwrap()) != 0x67676d6c {
        return Err(invalid("expected a ggml Whisper model"));
    }
    let ftype = u32::from_le_bytes(header[44..].try_into().unwrap());
    if ftype % 16384 != 1 {
        return Err(invalid("expected an f16 model; refusing to requantise weights"));
    }
    header[44..].copy_from_slice(&(2 * 16384 + format.tensor_type()).to_le_bytes());
    w.write_all(&header)?;
    let mel = read_u32(r)?;
    let fft = read_u32(r)?;
    let filter_bytes = positive(mel)?.checked_mul(positive(fft)?).and_then(|v| v.checked_mul(4))
        .ok_or_else(|| invalid("mel filter size overflow"))?;
    w.write_all(&mel.to_le_bytes())?;
    w.write_all(&fft.to_le_bytes())?;
    copy_exact(r, w, filter_bytes)?;
    let vocab = read_u32(r)?;
    positive(vocab)?;
    w.write_all(&vocab.to_le_bytes())?;
    for _ in 0..vocab {
        let len = read_u32(r)?;
        w.write_all(&len.to_le_bytes())?;
        copy_exact(r, w, u64::from(len))?;
    }
    let mut stats = QuantizationStats::default();
    loop {
        // EOF is valid only between tensors, never in a header or payload.
        let mut first = [0; 1];
        if r.read(&mut first)? == 0 { break; }
        let mut rank_bytes = [0; 4];
        rank_bytes[0] = first[0];
        r.read_exact(&mut rank_bytes[1..])?;
        let rank = u32::from_le_bytes(rank_bytes);
        if !(1..=3).contains(&rank) { return Err(invalid("unsupported Whisper tensor rank")); }
        let name_len = read_u32(r)?;
        let ty = read_u32(r)?;
        let width = match ty { 0 => 4, 1 => 2, _ => return Err(invalid("input contains non-float tensors")) };
        let mut dims = Vec::new();
        let mut elements = 1u64;
        for _ in 0..rank {
            let d = read_u32(r)?;
            elements = elements.checked_mul(positive(d)?).ok_or_else(|| invalid("tensor size overflow"))?;
            dims.push(d);
        }
        let mut name = vec![0; name_len as usize];
        r.read_exact(&mut name)?;
        let name_str = std::str::from_utf8(&name).map_err(|_| invalid("invalid tensor name"))?;
        let quantise = rank == 2 && dims[0] % 32 == 0
            && name_str.ends_with(".weight") && !name_str.contains("embedding");
        let tensor_format = format.for_row(dims[0]);
        w.write_all(&rank_bytes)?;
        w.write_all(&name_len.to_le_bytes())?;
        w.write_all(&if quantise { tensor_format.tensor_type() } else { ty }.to_le_bytes())?;
        for d in dims { w.write_all(&d.to_le_bytes())?; }
        w.write_all(&name)?;
        let input_bytes = elements.checked_mul(width).ok_or_else(|| invalid("tensor byte size overflow"))?;
        let output_bytes = if quantise {
            let be = tensor_format.block_elements();
            let mut raw = [0; 1024];
            let mut input = [0.0; 256];
            let mut output = [0; 176];
            for _ in 0..elements / be as u64 {
                r.read_exact(&mut raw[..be * width as usize])?;
                for (i, value) in input[..be].iter_mut().enumerate() {
                    *value = if ty == 1 {
                        f16_to_f32(u16::from_le_bytes(raw[i * 2..i * 2 + 2].try_into().unwrap()))
                    } else {
                        f32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap())
                    };
                    if !value.is_finite() { return Err(invalid("non-finite matrix weight")); }
                }
                tensor_format.block(&input[..be], &mut output)?;
                w.write_all(&output[..tensor_format.block_bytes()])?;
            }
            stats.matrices += 1;
            elements / be as u64 * tensor_format.block_bytes() as u64
        } else {
            copy_exact(r, w, input_bytes)?;
            input_bytes
        };
        stats.tensors += 1;
        stats.input_weight_bytes += input_bytes;
        stats.output_weight_bytes += output_bytes;
    }
    if stats.matrices == 0 { return Err(invalid("model has no eligible matrices")); }
    w.flush()?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::whisper::quant::{dequantize_q4_k, dequantize_q5_k, dequantize_q5_0, dequantize_q8_0};

    #[test]
    fn blocks_round_trip_through_existing_dequantisers() {
        for format in [WeightQuantization::Q8_0, WeightQuantization::Q5_0, WeightQuantization::Q5K, WeightQuantization::Q4K] {
            for scale in [0.0, 0.0002, 0.07, -0.07, 200.0] {
                let input: Vec<_> = (0..format.block_elements()).map(|i| scale * ((i % 32) as f32 - 17.0)).collect();
                let mut block = [0; 176];
                format.block(&input, &mut block).unwrap();
                let mut output = vec![0.0; input.len()];
                let fraction = match format {
                    WeightQuantization::Q8_0 => { dequantize_q8_0(&block, &mut output); 0.005 },
                    WeightQuantization::Q5_0 => { dequantize_q5_0(&block, &mut output); 0.034 },
                    WeightQuantization::Q5K => { dequantize_q5_k(&block, &mut output); 0.034 },
                    WeightQuantization::Q4K => { dequantize_q4_k(&block, &mut output); 0.07 },
                };
                let max = input.iter().copied().map(f32::abs).fold(0.0, f32::max);
                for (a, b) in input.into_iter().zip(output) {
                    assert!((a - b).abs() <= max * fraction + 1e-6, "{format:?}: {a} vs {b}");
                }
            }
        }
    }

    fn fixture(cols: usize) -> Vec<u8> {
        let mut file = Vec::new();
        for v in [0x67676d6c_u32, 1, 1, 32, 1, 1, 1, 32, 1, 1, 1, 1, 1, 1, 0x3f000000, 1, 2] {
            file.extend_from_slice(&v.to_le_bytes());
        }
        file.extend_from_slice(&[0xff, 0x80]); // vocabulary bytes are opaque
        for (name, dims) in [
            ("encoder.blocks.0.attn.query.weight", &[cols, 3][..]),
            ("decoder.token_embedding.weight", &[32, 2][..]),
            ("encoder.positional_embedding", &[32, 2][..]),
            ("encoder.conv1.weight", &[3, 32, 2][..]),
            ("encoder.ln_post.weight", &[32][..]),
            ("decoder.blocks.0.attn.query.bias", &[32][..]),
            ("decoder.blocks.0.attn.query.weight", &[31, 2][..]),
        ] {
            for v in [dims.len() as u32, name.len() as u32, 1] { file.extend_from_slice(&v.to_le_bytes()); }
            for d in dims { file.extend_from_slice(&(*d as u32).to_le_bytes()); }
            file.extend_from_slice(name.as_bytes());
            for i in 0..dims.iter().product::<usize>() {
                file.extend_from_slice(&scale_half((i as f32 - 32.0) / 100.0).to_le_bytes());
            }
        }
        file
    }

    #[test]
    fn file_metadata_and_ineligible_tensors_are_preserved() {
        for cols in [32, 256, 384] {
            let input = fixture(cols);
            for format in [WeightQuantization::Q8_0, WeightQuantization::Q5_0, WeightQuantization::Q5K, WeightQuantization::Q4K] {
                let mut output = Vec::new();
                let stats = quantize_ggml(&mut input.as_slice(), &mut output, format).unwrap();
                assert_eq!((stats.tensors, stats.matrices), (7, 1));
                assert_eq!(&input[..44], &output[..44]);
                assert_eq!(u32::from_le_bytes(output[44..48].try_into().unwrap()), 32768 + format.tensor_type());
                let name = "encoder.blocks.0.attn.query.weight";
                let start = 70;
                let payload = start + 20 + name.len();
                assert_eq!(&input[48..start + 8], &output[48..start + 8]);
                assert_eq!(u32::from_le_bytes(output[start + 8..start + 12].try_into().unwrap()), format.for_row(cols as u32).tensor_type());
                assert_eq!(&input[start + 12..payload], &output[start + 12..payload]);
                let actual = format.for_row(cols as u32);
                let output_bytes = 3 * cols / actual.block_elements() * actual.block_bytes();
                assert_eq!(&input[payload + 3 * cols * 2..], &output[payload + output_bytes..]);
                assert_eq!(input.len() as u64 - output.len() as u64, stats.input_weight_bytes - stats.output_weight_bytes);
            }
        }

    }

    #[test]
    fn truncated_and_already_quantised_files_are_rejected() {
        let input = fixture(32);
        for len in [0, 47, 69, 71, input.len() - 1] {
            assert!(quantize_ggml(&mut &input[..len], &mut Vec::new(), WeightQuantization::Q5_0).is_err());
        }
        let mut output = Vec::new();
        quantize_ggml(&mut input.as_slice(), &mut output, WeightQuantization::Q8_0).unwrap();
        assert!(quantize_ggml(&mut output.as_slice(), &mut Vec::new(), WeightQuantization::Q5_0).is_err());
    }
}
