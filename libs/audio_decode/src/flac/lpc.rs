//! Fixed-order and FIR LPC restoration.
//!
//! Warm-up samples occupy `out[..order]`; residuals occupy `out[order..]`.
//! Restoration writes the decoded samples in place. Samples are `i64` so the
//! 33-bit stereo side channel is representable; predictors accumulate in
//! `i128` so hostile coefficients cannot wrap.

use crate::error::AudioError;

/// FLAC fixed predictors, orders 0–4. Order 0 is a no-op (the residual *is*
/// the signal).
pub fn restore_fixed(out: &mut [i64], order: usize) -> Result<(), AudioError> {
    if order == 0 || out.len() <= order {
        return Ok(());
    }
    match order {
        1 => {
            for i in 1..out.len() {
                out[i] = add_prediction(out[i], out[i - 1] as i128)?;
            }
        }
        2 => {
            for i in 2..out.len() {
                let pred = out[i - 1] as i128 * 2 - out[i - 2] as i128;
                out[i] = add_prediction(out[i], pred)?;
            }
        }
        3 => {
            for i in 3..out.len() {
                let pred = out[i - 1] as i128 * 3 - out[i - 2] as i128 * 3
                    + out[i - 3] as i128;
                out[i] = add_prediction(out[i], pred)?;
            }
        }
        4 => {
            for i in 4..out.len() {
                let pred = out[i - 1] as i128 * 4 - out[i - 2] as i128 * 6
                    + out[i - 3] as i128 * 4 - out[i - 4] as i128;
                out[i] = add_prediction(out[i], pred)?;
            }
        }
        _ => return Err(AudioError::Corrupt("flac fixed predictor order")),
    }
    Ok(())
}

/// FIR LPC: `decoded[i] = residual[i] + ((sum_j coeff[j] * decoded[i-j-1]) >> shift)`.
/// A negative `shift` is a left shift, per the spec's signed 5-bit field.
pub fn restore_lpc(out: &mut [i64], coeffs: &[i32], shift: i32) -> Result<(), AudioError> {
    let order = coeffs.len();
    if order == 0 || out.len() <= order {
        return Ok(());
    }
    for i in order..out.len() {
        let mut sum: i128 = 0;
        for (j, &c) in coeffs.iter().enumerate() {
            sum += c as i128 * out[i - j - 1] as i128;
        }
        let pred = if shift >= 0 {
            sum >> shift
        } else {
            sum.checked_shl((-shift) as u32)
                .ok_or(AudioError::Corrupt("flac lpc overflow"))?
        };
        out[i] = add_prediction(out[i], pred)?;
    }
    Ok(())
}

fn add_prediction(residual: i64, prediction: i128) -> Result<i64, AudioError> {
    let sample = residual as i128 + prediction;
    i64::try_from(sample).map_err(|_| AudioError::Corrupt("flac predictor overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_order1_is_prefix_sum() {
        let mut s = [3, 1, 1, 1, 1];
        restore_fixed(&mut s, 1).unwrap();
        assert_eq!(s, [3, 4, 5, 6, 7]);
    }

    #[test]
    fn fixed_order2_linear() {
        // Warm-up 1, 2; residuals 0; the order-2 predictor is 2*s[n-1]-s[n-2],
        // which continues an arithmetic sequence.
        let mut s = [1, 2, 0, 0, 0];
        restore_fixed(&mut s, 2).unwrap();
        assert_eq!(s, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn fixed_orders3_and4_continue_polynomials() {
        let mut quadratic = [1, 4, 9, 0, 0];
        restore_fixed(&mut quadratic, 3).unwrap();
        assert_eq!(quadratic, [1, 4, 9, 16, 25]);

        let mut cubic = [1, 8, 27, 64, 0, 0];
        restore_fixed(&mut cubic, 4).unwrap();
        assert_eq!(cubic, [1, 8, 27, 64, 125, 216]);
    }

    #[test]
    fn fixed_order0_is_verbatim_residual() {
        let mut s = [9, -4, 1];
        restore_fixed(&mut s, 0).unwrap();
        assert_eq!(s, [9, -4, 1]);
    }

    #[test]
    fn lpc_order2_shift0_known_coeffs() {
        // Same arithmetic sequence with explicit LPC coeffs [2, -1] and shift 0.
        let mut s = [1, 2, 0, 0, 0];
        restore_lpc(&mut s, &[2, -1], 0).unwrap();
        assert_eq!(s, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn lpc_with_shift() {
        // coeffs [2, -2] at shift 1 is the same predictor as [1, -1] at shift 0:
        // pred = (2*s[n-1] + (-2)*s[n-2]) >> 1 = s[n-1] - s[n-2].
        let mut s = [4, 7, 0, 0];
        restore_lpc(&mut s, &[2, -2], 1).unwrap();
        // i=2: (2*7 + (-2)*4) >> 1 = (14-8)>>1 = 3; 0+3 = 3
        // i=3: (2*3 + (-2)*7) >> 1 = (6-14)>>1 = -4; 0-4 = -4
        assert_eq!(s, [4, 7, 3, -4]);
    }

    #[test]
    fn lpc_negative_shift_lefts() {
        let mut s = [1, 0];
        restore_lpc(&mut s, &[1], -1).unwrap();
        // pred = 1 * s[0] << 1 = 2; 0+2 = 2
        assert_eq!(s, [1, 2]);
    }

    #[test]
    fn empty_and_short_are_noops() {
        let mut empty: [i64; 0] = [];
        restore_lpc(&mut empty, &[1], 0).unwrap();
        restore_fixed(&mut empty, 1).unwrap();
        let mut one = [7];
        restore_lpc(&mut one, &[1, 2], 0).unwrap();
        assert_eq!(one, [7]);
    }
}
