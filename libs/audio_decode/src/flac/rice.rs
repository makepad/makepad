//! Rice residual coding, the two methods FLAC uses for subframe residuals.
//!
//! Method 0 uses 4-bit partition parameters (escape 15); method 1 uses 5-bit
//! (escape 31). A partition whose parameter is the escape codes residuals as
//! raw two's-complement integers of a width given by the next 5 bits.

use super::bits::BitReader;
use crate::error::AudioError;

/// Decode residuals into `out[predictor_order..]`. `out` is `blocksize` long;
/// the warm-up samples that occupy the front of a FIXED/LPC subframe are left
/// alone.
pub fn decode_residuals(
    r: &mut BitReader,
    predictor_order: usize,
    blocksize: usize,
    out: &mut [i64],
) -> Result<(), AudioError> {
    if out.len() < blocksize {
        return Err(AudioError::Corrupt("flac residual length"));
    }
    let method = r.read(2).ok_or(AudioError::Truncated)?;
    let param_bits = match method {
        0 => 4u32,
        1 => 5u32,
        _ => return Err(AudioError::Corrupt("flac residual method")),
    };
    let escape = (1u32 << param_bits) - 1;
    let partition_order = r.read(4).ok_or(AudioError::Truncated)? as u32;
    if partition_order > 15 {
        return Err(AudioError::Corrupt("flac partition order"));
    }
    let partitions = 1usize << partition_order;
    if blocksize % partitions != 0 {
        return Err(AudioError::Corrupt("flac partition size"));
    }
    let samples_per_partition = blocksize / partitions;
    if samples_per_partition < predictor_order && partition_order > 0 {
        // The first partition has to hold the residual after the warm-up;
        // a partition smaller than the predictor cannot.
        return Err(AudioError::Corrupt("flac partition vs predictor"));
    }

    let mut at = predictor_order;
    for p in 0..partitions {
        let mut n = samples_per_partition;
        if p == 0 {
            if n < predictor_order {
                return Err(AudioError::Corrupt("flac partition vs predictor"));
            }
            n -= predictor_order;
        }
        let param = r.read(param_bits).ok_or(AudioError::Truncated)?;
        if param == escape {
            let bits = r.read(5).ok_or(AudioError::Truncated)?;
            for _ in 0..n {
                let sample = if bits == 0 {
                    0
                } else {
                    r.read_signed_i64(bits).ok_or(AudioError::Truncated)?
                };
                if at >= blocksize {
                    return Err(AudioError::Corrupt("flac residual overrun"));
                }
                out[at] = sample;
                at += 1;
            }
        } else {
            for _ in 0..n {
                let sample = read_rice_signed(r, param)?;
                if at >= blocksize {
                    return Err(AudioError::Corrupt("flac residual overrun"));
                }
                out[at] = sample;
                at += 1;
            }
        }
    }
    if at != blocksize {
        return Err(AudioError::Corrupt("flac residual count"));
    }
    Ok(())
}

/// One Rice-coded signed residual: unary quotient, stop bit, `param` remainder
/// bits, then the zigzag map back to a signed integer.
pub fn read_rice_signed(r: &mut BitReader, param: u32) -> Result<i64, AudioError> {
    if param > 30 {
        return Err(AudioError::Corrupt("flac rice parameter"));
    }
    let q = r.read_unary().ok_or(AudioError::Truncated)?;
    let remainder = if param == 0 {
        0
    } else {
        r.read(param).ok_or(AudioError::Truncated)?
    };
    let u = (q as u64)
        .checked_shl(param)
        .and_then(|v| v.checked_add(remainder as u64))
        .ok_or(AudioError::Corrupt("flac rice overflow"))?;
    Ok(unzigzag(u))
}

fn unzigzag(u: u64) -> i64 {
    // Even: n = u/2; odd: n = -(u+1)/2. The xor form also preserves i64::MIN.
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

#[cfg(test)]
pub fn zigzag(n: i32) -> u32 {
    ((n as u32) << 1) ^ ((n >> 31) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flac::bits::BitReader;

    struct BitWriter {
        bytes: Vec<u8>,
        bit: usize,
    }

    impl BitWriter {
        fn new() -> Self {
            Self { bytes: Vec::new(), bit: 0 }
        }
        fn put(&mut self, value: u32, n: u32) {
            for i in (0..n).rev() {
                if self.bit % 8 == 0 {
                    self.bytes.push(0);
                }
                if (value >> i) & 1 == 1 {
                    let at = self.bytes.len() - 1;
                    self.bytes[at] |= 0x80 >> (self.bit % 8);
                }
                self.bit += 1;
            }
        }
        fn write_rice(&mut self, n: i32, param: u32) {
            let u = zigzag(n);
            let q = if param == 0 { u } else { u >> param };
            let rem = if param == 0 { 0 } else { u & ((1 << param) - 1) };
            for _ in 0..q {
                self.put(0, 1);
            }
            self.put(1, 1);
            if param > 0 {
                self.put(rem, param);
            }
        }
    }

    #[test]
    fn zigzag_roundtrip_extremes() {
        for n in [0i32, 1, -1, 2, -2, 127, -128, i32::MAX, i32::MIN] {
            assert_eq!(unzigzag(zigzag(n) as u64), n as i64, "n={n}");
        }
    }

    #[test]
    fn rice_roundtrip_various_params() {
        let values = [
            0i32, 1, -1, 2, -2, 7, -8, 255, -256, 1024, -1024, 32_767, -32_768,
        ];
        for param in 0..=8u32 {
            let mut w = BitWriter::new();
            for &v in &values {
                w.write_rice(v, param);
            }
            let mut r = BitReader::new(&w.bytes);
            for &v in &values {
                let got = read_rice_signed(&mut r, param).expect("rice");
                assert_eq!(got, v as i64, "param={param} v={v}");
            }
        }
    }

    #[test]
    fn rice_param_zero_is_unary_zigzag() {
        // param 0: just unary of zigzag(n), so 3 (zigzag 6) is six zeros and a 1.
        let mut w = BitWriter::new();
        w.write_rice(3, 0);
        let mut r = BitReader::new(&w.bytes);
        assert_eq!(read_rice_signed(&mut r, 0), Ok(3));
    }

    #[test]
    fn rice_value_above_i32_is_not_saturated() {
        let mut w = BitWriter::new();
        for _ in 0..4 {
            w.put(0, 1);
        }
        w.put(1, 1); // quotient 4
        w.put(0, 30);
        let mut r = BitReader::new(&w.bytes);
        assert_eq!(read_rice_signed(&mut r, 30), Ok(1i64 << 31));
    }

    #[test]
    fn residual_partitions_and_escape() {
        // Method 0, partition order 0 (one partition), rice param 15 = escape,
        // then 5-bit width 8, two residuals 1 and -1. Predictor order 0,
        // blocksize 2.
        let mut w = BitWriter::new();
        w.put(0, 2); // method 0
        w.put(0, 4); // partition order 0
        w.put(15, 4); // escape
        w.put(8, 5); // raw width
        w.put(1u32, 8); // +1
        w.put((-1i32) as u32, 8); // -1 in 8 bits
        let mut out = [0i64; 2];
        let mut r = BitReader::new(&w.bytes);
        decode_residuals(&mut r, 0, 2, &mut out).unwrap();
        assert_eq!(out, [1, -1]);
    }

    #[test]
    fn rice2_uses_five_bit_parameter() {
        let mut w = BitWriter::new();
        w.put(1, 2); // method 1 / RICE2
        w.put(0, 4); // one partition
        w.put(17, 5); // value not representable in method 0's parameter field
        for u in [2u32, 1] { // +1, -1
            w.put(1, 1); // quotient zero, stop bit
            w.put(u, 17);
        }
        let mut out = [0i64; 2];
        let mut r = BitReader::new(&w.bytes);
        decode_residuals(&mut r, 0, 2, &mut out).unwrap();
        assert_eq!(out, [1, -1]);
    }

    #[test]
    fn residual_truncated_is_an_error() {
        let mut r = BitReader::new(&[0x00]);
        let mut out = [0i64; 8];
        assert!(decode_residuals(&mut r, 0, 8, &mut out).is_err());
    }

    #[test]
    fn empty_reader_never_panics() {
        let mut r = BitReader::new(&[]);
        assert!(matches!(read_rice_signed(&mut r, 4), Err(AudioError::Truncated)));
        let mut out = [0i64; 1];
        assert!(decode_residuals(&mut r, 0, 1, &mut out).is_err());
    }
}
