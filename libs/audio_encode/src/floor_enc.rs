//! Floor value encoding: from desired final Y values to the raw packet
//! values the decoder's prediction scheme will reconstruct exactly.
//!
//! The decoder predicts each point from the tightest bracket among earlier
//! points and folds the coded value around that prediction, switching to an
//! absolute form when the delta leaves the symmetric window. This module is
//! the exact inverse of that arithmetic (`floor.rs::synthesize` in the
//! decoder); the tests drive the decoder over every (predicted, desired)
//! pair, so the two cannot disagree anywhere in the domain.

use crate::setup::{FLOOR_INTERIOR, FLOOR_POINTS, FLOOR_RANGE};
use makepad_audio_decode::vorbis::floor::inverse_db as decoder_inverse_db;

pub struct FloorFitter {
    x: [i32; FLOOR_POINTS],
    lo: [usize; FLOOR_POINTS],
    hi: [usize; FLOOR_POINTS],
    /// Point indices in ascending X order.
    sorted: [usize; FLOOR_POINTS],
    /// The decoder's inverse-dB table, cached: `synthesize` looks amplitudes
    /// up instead of raising 10 to a power per bin, which was the single
    /// hottest thing in the whole encoder.
    inverse_db: [f32; 256],
}

impl Default for FloorFitter {
    fn default() -> Self {
        Self::new()
    }
}

impl FloorFitter {
    pub fn new() -> FloorFitter {
        let mut x = [0i32; FLOOR_POINTS];
        x[0] = 0;
        x[1] = 512;
        for (i, &v) in FLOOR_INTERIOR.iter().enumerate() {
            x[i + 2] = v as i32;
        }
        let mut lo = [0usize; FLOOR_POINTS];
        let mut hi = [0usize; FLOOR_POINTS];
        for i in 2..FLOOR_POINTS {
            // Tightest bracket among earlier list entries, as the decoder.
            let (mut l, mut h) = (0usize, 1usize);
            for j in 0..i {
                if x[j] < x[i] && x[j] > x[l] {
                    l = j;
                }
                if x[j] > x[i] && x[j] < x[h] {
                    h = j;
                }
            }
            lo[i] = l;
            hi[i] = h;
        }
        let mut sorted: [usize; FLOOR_POINTS] = std::array::from_fn(|i| i);
        sorted.sort_by_key(|&i| x[i]);
        let inverse_db = std::array::from_fn(|i| decoder_inverse_db(i as i32));
        FloorFitter { x, lo, hi, sorted, inverse_db }
    }

    /// The floor curve the decoder will synthesize from `vals` — the same
    /// integer walk as the decoder's `Floor1::synthesize`, with the
    /// inverse-dB exponential looked up from its cached table. The tests
    /// assert exact equality against the decoder over random inputs.
    pub fn synthesize(&self, vals: &[i32], out: &mut [f32]) {
        let n = out.len();
        let mut final_y = [0i32; FLOOR_POINTS];
        let mut step2 = [false; FLOOR_POINTS];
        step2[0] = true;
        step2[1] = true;
        final_y[0] = vals[0];
        final_y[1] = vals[1];
        for i in 2..FLOOR_POINTS {
            let lo = self.lo[i];
            let hi = self.hi[i];
            let predicted =
                render_point(self.x[lo], final_y[lo], self.x[hi], final_y[hi], self.x[i]);
            let val = vals[i];
            let highroom = FLOOR_RANGE - predicted;
            let lowroom = predicted;
            let room = if highroom < lowroom { highroom * 2 } else { lowroom * 2 };
            if val != 0 {
                step2[lo] = true;
                step2[hi] = true;
                step2[i] = true;
                final_y[i] = if val >= room {
                    if highroom > lowroom {
                        val - lowroom + predicted
                    } else {
                        predicted - val + highroom - 1
                    }
                } else if val & 1 == 1 {
                    predicted - (val + 1) / 2
                } else {
                    predicted + val / 2
                };
            } else {
                step2[i] = false;
                final_y[i] = predicted;
            }
        }
        // Draw the surviving points in X order (multiplier 2).
        let mut lx = 0usize;
        let mut ly = final_y[0] * 2;
        let mut hx = 0usize;
        let mut hy = ly;
        let mut drawn_any = false;
        for &i in &self.sorted {
            if !step2[i] {
                continue;
            }
            hy = final_y[i] * 2;
            hx = (self.x[i] as usize).min(n);
            if hx > lx {
                self.render_line(lx, ly, hx, hy, out);
                drawn_any = true;
            }
            lx = hx;
            ly = hy;
        }
        if !drawn_any {
            out.fill(self.inv_db(ly));
        } else if hx < n {
            let v = self.inv_db(hy);
            out[hx..].fill(v);
        }
    }

    #[inline]
    fn inv_db(&self, y: i32) -> f32 {
        self.inverse_db[y.clamp(0, 255) as usize]
    }

    /// The spec's integer Bresenham, identical to the decoder's.
    fn render_line(&self, x0: usize, y0: i32, x1: usize, y1: i32, out: &mut [f32]) {
        let n = out.len();
        let dy = y1 - y0;
        let adx = (x1 - x0) as i32;
        if adx == 0 {
            return;
        }
        let mut ady = dy.abs();
        let base = dy / adx;
        let sy = if dy < 0 { base - 1 } else { base + 1 };
        ady -= base.abs() * adx;
        let mut y = y0;
        let mut err = 0i32;
        if x0 < n {
            out[x0] = self.inv_db(y);
        }
        for slot in out.iter_mut().take(x1.min(n)).skip(x0 + 1) {
            err += ady;
            if err >= adx {
                err -= adx;
                y += sy;
            } else {
                y += base;
            }
            *slot = self.inv_db(y);
        }
    }

    /// Raw packet values for the desired final Y sequence. The decoder will
    /// reconstruct `desired` exactly.
    pub fn encode(&self, desired: &[i32], vals: &mut [i32]) {
        debug_assert!(desired.len() >= FLOOR_POINTS && vals.len() >= FLOOR_POINTS);
        vals[0] = desired[0];
        vals[1] = desired[1];
        for i in 2..FLOOR_POINTS {
            let predicted = render_point(
                self.x[self.lo[i]],
                desired[self.lo[i]],
                self.x[self.hi[i]],
                desired[self.hi[i]],
                self.x[i],
            );
            vals[i] = encode_val(desired[i], predicted);
        }
    }
}

/// The spec's integer point interpolation, identical to the decoder's.
fn render_point(x0: i32, y0: i32, x1: i32, y1: i32, x: i32) -> i32 {
    let dy = y1 - y0;
    let adx = x1 - x0;
    if adx == 0 {
        return y0;
    }
    let ady = dy.abs();
    let err = ady.saturating_mul(x - x0);
    let off = err / adx;
    if dy < 0 {
        y0 - off
    } else {
        y0 + off
    }
}

/// Inverse of the decoder's value fold: the raw value whose decode against
/// `predicted` lands on `desired`. Both in `[0, FLOOR_RANGE)`.
fn encode_val(desired: i32, predicted: i32) -> i32 {
    debug_assert!((0..FLOOR_RANGE).contains(&desired));
    debug_assert!((0..FLOOR_RANGE).contains(&predicted));
    let highroom = FLOOR_RANGE - predicted;
    let lowroom = predicted;
    let room = highroom.min(lowroom) * 2;
    let d = desired - predicted;
    if d == 0 {
        return 0;
    }
    if d > 0 {
        if 2 * d < room {
            2 * d
        } else {
            d + lowroom
        }
    } else {
        let a = -d;
        if 2 * a - 1 < room {
            2 * a - 1
        } else {
            a + highroom - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use crate::huffman::HuffBook;
    use crate::setup::{decoder_floor, FLOOR_Y_BITS, HALF};
    use makepad_audio_decode::vorbis::bits::BitReader;
    use makepad_audio_decode::vorbis::codebook::Codebook;
    use makepad_audio_decode::vorbis::floor::inverse_db;

    /// The decoder's fold, copied verbatim, as the local oracle for the
    /// exhaustive inverse test.
    fn decode_val(val: i32, predicted: i32) -> i32 {
        let highroom = FLOOR_RANGE - predicted;
        let lowroom = predicted;
        let room = if highroom < lowroom { highroom * 2 } else { lowroom * 2 };
        if val >= room {
            if highroom > lowroom {
                val - lowroom + predicted
            } else {
                predicted - val + highroom - 1
            }
        } else if val & 1 == 1 {
            predicted - (val + 1) / 2
        } else {
            predicted + val / 2
        }
    }

    #[test]
    fn every_desired_predicted_pair_round_trips() {
        for predicted in 0..FLOOR_RANGE {
            for desired in 0..FLOOR_RANGE {
                let val = encode_val(desired, predicted);
                assert!(
                    (0..FLOOR_RANGE).contains(&val),
                    "val {val} out of range for d={desired} p={predicted}"
                );
                if desired != predicted {
                    assert_ne!(val, 0, "nonzero delta must not encode as 0");
                }
                assert_eq!(
                    decode_val(val, predicted),
                    desired,
                    "d={desired} p={predicted} val={val}"
                );
            }
        }
    }

    /// End to end: fit, write a floor packet with a flat Huffman book, decode
    /// it with the real decoder, synthesize both sides, compare curves.
    #[test]
    fn a_floor_packet_survives_the_real_decoder() {
        let fitter = FloorFitter::new();
        let floor1 = decoder_floor();
        // A jagged but in-range desired curve.
        let desired: Vec<i32> =
            (0..FLOOR_POINTS).map(|i| ((i * 37 + 11) % 100 + 14) as i32).collect();
        let mut vals = vec![0i32; FLOOR_POINTS];
        fitter.encode(&desired, &mut vals);

        // Uniform-count book over the raw value alphabet.
        let counts = vec![1u64; FLOOR_RANGE as usize];
        let book = HuffBook::build(&counts);

        // The packet fragment: nonzero flag, y0, y1, then coded values.
        let mut w = BitWriter::new();
        w.push(1, 1);
        w.push(vals[0] as u32, FLOOR_Y_BITS);
        w.push(vals[1] as u32, FLOOR_Y_BITS);
        for &v in &vals[2..] {
            let cw = book.codes[v as usize];
            assert!(cw.len > 0);
            w.push(cw.bits, cw.len);
        }
        let bytes = w.finish();

        // Decoder-side codebook from the same lengths.
        let mut bw = BitWriter::new();
        bw.push(0x564342, 24);
        bw.push(1, 16);
        bw.push(book.lengths.len() as u32, 24);
        bw.push(0, 1);
        bw.push(1, 1);
        for &l in &book.lengths {
            if l == 0 {
                bw.push(0, 1);
            } else {
                bw.push(1, 1);
                bw.push(l as u32 - 1, 5);
            }
        }
        bw.push(0, 4);
        let book_bytes = bw.finish();
        let mut br = BitReader::new(&book_bytes);
        let mut budget = 1 << 22;
        let cb = Codebook::read(&mut br, &mut budget).unwrap();

        let mut r = BitReader::new(&bytes);
        let mut y = vec![0i32; FLOOR_POINTS];
        let books = vec![cb];
        assert!(floor1.decode(&mut r, &books, &mut y).unwrap());
        assert_eq!(y, vals, "decoded raw values");

        // Curves: decoder synthesize of the decoded values vs the desired
        // final Y turned into amplitudes directly.
        let mut curve = vec![0f32; HALF];
        floor1.synthesize(&y, &mut curve);
        // Check the curve passes through every desired point (points are in
        // list order; x=512 is past the buffer).
        let x_of = |i: usize| -> usize {
            match i {
                0 => 0,
                1 => 512,
                _ => FLOOR_INTERIOR[i - 2] as usize,
            }
        };
        for i in 0..FLOOR_POINTS {
            let x = x_of(i);
            if x >= HALF {
                continue;
            }
            let want = inverse_db(desired[i] * 2);
            let got = curve[x];
            assert!(
                (got - want).abs() <= want * 1e-4,
                "point {i} at bin {x}: curve {got} vs desired {want}"
            );
        }
    }
}

#[cfg(test)]
mod synth_tests {
    use super::*;
    use crate::setup::{decoder_floor, HALF};

    #[test]
    fn fast_synthesize_is_bit_identical_to_the_decoder() {
        let fitter = FloorFitter::new();
        let floor1 = decoder_floor();
        let mut s = 0x1234_5678_9abc_def0u64;
        let mut rng = move |m: i32| -> i32 {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            (s.wrapping_mul(0x2545F4914F6CDD1D) >> 33) as i32 % m
        };
        for _ in 0..2000 {
            let vals: Vec<i32> = (0..FLOOR_POINTS).map(|_| rng(FLOOR_RANGE)).collect();
            let mut ours = vec![0f32; HALF];
            let mut theirs = vec![0f32; HALF];
            fitter.synthesize(&vals, &mut ours);
            floor1.synthesize(&vals, &mut theirs);
            assert_eq!(ours, theirs, "vals {vals:?}");
        }
        // And the all-zero / degenerate shapes.
        let vals = vec![0i32; FLOOR_POINTS];
        let mut ours = vec![0f32; HALF];
        let mut theirs = vec![0f32; HALF];
        fitter.synthesize(&vals, &mut ours);
        floor1.synthesize(&vals, &mut theirs);
        assert_eq!(ours, theirs);
    }
}
