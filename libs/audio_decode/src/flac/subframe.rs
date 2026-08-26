//! Subframes: CONSTANT, VERBATIM, FIXED (orders 0–4), LPC (orders 1–32),
//! plus the wasted-bits shift that any of them may carry.

use super::bits::BitReader;
use super::lpc::{restore_fixed, restore_lpc};
use super::rice;
use crate::error::AudioError;

pub fn decode_subframe(
    r: &mut BitReader,
    blocksize: usize,
    bps: u8,
    out: &mut [i64],
) -> Result<(), AudioError> {
    if out.len() < blocksize {
        return Err(AudioError::Corrupt("flac subframe length"));
    }
    if r.read_bit().ok_or(AudioError::Truncated)? {
        return Err(AudioError::Corrupt("flac subframe padding"));
    }
    let kind = r.read(6).ok_or(AudioError::Truncated)?;
    let wasted_flag = r.read_bit().ok_or(AudioError::Truncated)?;
    let wasted = if wasted_flag {
        let k = r.read_unary().ok_or(AudioError::Truncated)?.saturating_add(1);
        if k >= bps as u32 {
            return Err(AudioError::Corrupt("flac wasted bits"));
        }
        k
    } else {
        0
    };
    let coded_bps = (bps as u32).saturating_sub(wasted);
    if coded_bps == 0 || coded_bps > 33 {
        return Err(AudioError::Corrupt("flac subframe bps"));
    }

    match kind {
        0 => constant(r, blocksize, coded_bps, out)?,
        1 => verbatim(r, blocksize, coded_bps, out)?,
        0b001000..=0b001100 => {
            let order = (kind - 0b001000) as usize;
            fixed(r, blocksize, coded_bps, order, out)?;
        }
        0b100000..=0b111111 => {
            let order = (kind - 0b011111) as usize; // 01xxxx → order = xxxx+1
            lpc(r, blocksize, coded_bps, order, out)?;
        }
        _ => return Err(AudioError::Corrupt("flac subframe type")),
    }

    if wasted > 0 {
        for s in out.iter_mut().take(blocksize) {
            *s = i64::try_from((*s as i128) << wasted)
                .map_err(|_| AudioError::Corrupt("flac wasted bits overflow"))?;
        }
    }
    if out.iter().take(blocksize).any(|&s| !fits_signed(s, bps as u32)) {
        return Err(AudioError::Corrupt("flac subframe sample range"));
    }
    Ok(())
}

fn fits_signed(sample: i64, bits: u32) -> bool {
    let limit = 1i64 << (bits - 1);
    (-limit..limit).contains(&sample)
}

fn constant(r: &mut BitReader, blocksize: usize, bps: u32, out: &mut [i64]) -> Result<(), AudioError> {
    let sample = r.read_signed_i64(bps).ok_or(AudioError::Truncated)?;
    for s in out.iter_mut().take(blocksize) {
        *s = sample;
    }
    Ok(())
}

fn verbatim(r: &mut BitReader, blocksize: usize, bps: u32, out: &mut [i64]) -> Result<(), AudioError> {
    for s in out.iter_mut().take(blocksize) {
        *s = r.read_signed_i64(bps).ok_or(AudioError::Truncated)?;
    }
    Ok(())
}

fn warmup(r: &mut BitReader, order: usize, bps: u32, out: &mut [i64]) -> Result<(), AudioError> {
    for s in out.iter_mut().take(order) {
        *s = r.read_signed_i64(bps).ok_or(AudioError::Truncated)?;
    }
    Ok(())
}

fn fixed(
    r: &mut BitReader,
    blocksize: usize,
    bps: u32,
    order: usize,
    out: &mut [i64],
) -> Result<(), AudioError> {
    if order > blocksize {
        return Err(AudioError::Corrupt("flac fixed order exceeds block size"));
    }
    warmup(r, order, bps, out)?;
    rice::decode_residuals(r, order, blocksize, out)?;
    restore_fixed(&mut out[..blocksize], order)?;
    Ok(())
}

fn lpc(
    r: &mut BitReader,
    blocksize: usize,
    bps: u32,
    order: usize,
    out: &mut [i64],
) -> Result<(), AudioError> {
    if !(1..=32).contains(&order) {
        return Err(AudioError::Corrupt("flac lpc order"));
    }
    if order > blocksize {
        return Err(AudioError::Corrupt("flac lpc order exceeds block size"));
    }
    warmup(r, order, bps, out)?;
    let precision = r.read(4).ok_or(AudioError::Truncated)? + 1;
    if precision == 16 {
        // 0b1111 is reserved.
        return Err(AudioError::Corrupt("flac lpc precision"));
    }
    let shift = r.read_signed(5).ok_or(AudioError::Truncated)?;
    let mut coeffs = [0i32; 32];
    for c in coeffs.iter_mut().take(order) {
        *c = r.read_signed(precision).ok_or(AudioError::Truncated)?;
    }
    rice::decode_residuals(r, order, blocksize, out)?;
    restore_lpc(&mut out[..blocksize], &coeffs[..order], shift)?;
    Ok(())
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
        fn put_signed(&mut self, value: i32, n: u32) {
            let mask = if n == 32 { u32::MAX } else { (1u32 << n) - 1 };
            self.put((value as u32) & mask, n);
        }
    }

    #[test]
    fn constant_subframe() {
        let mut w = BitWriter::new();
        w.put(0, 1); // padding
        w.put(0, 6); // CONSTANT
        w.put(0, 1); // no wasted
        w.put_signed(-7, 16);
        let mut r = BitReader::new(&w.bytes);
        let mut out = [0i64; 4];
        decode_subframe(&mut r, 4, 16, &mut out).unwrap();
        assert_eq!(out, [-7, -7, -7, -7]);
    }

    #[test]
    fn verbatim_and_wasted_bits() {
        let mut w = BitWriter::new();
        w.put(0, 1);
        w.put(1, 6); // VERBATIM
        w.put(1, 1); // wasted
        w.put(1, 1); // unary 0 → 1 wasted bit, so coded at 15
        w.put_signed(3, 15);
        w.put_signed(-1, 15);
        let mut r = BitReader::new(&w.bytes);
        let mut out = [0i64; 2];
        decode_subframe(&mut r, 2, 16, &mut out).unwrap();
        assert_eq!(out, [6, -2]);
    }

    #[test]
    fn reserved_type_is_corrupt() {
        let mut w = BitWriter::new();
        w.put(0, 1);
        w.put(0b000010, 6);
        w.put(0, 1);
        let mut r = BitReader::new(&w.bytes);
        let mut out = [0i64; 1];
        assert!(matches!(decode_subframe(&mut r, 1, 16, &mut out), Err(AudioError::Corrupt(_))));
    }
}
