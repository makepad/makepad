//! Floor decode: the spectral envelope. Only floor type 1 is implemented —
//! type 0 (LSP) is essentially unused by modern encoders, and reporting it is
//! honest where guessing would produce noise.

use super::codebook::Codebook;
use crate::bitread::{ilog, BitReader};
use crate::AudioError;

pub enum Floor {
    Type1(Floor1),
}

pub struct Floor1 {
    partition_class_list: Vec<u8>,
    class_dimensions: Vec<u8>,
    class_subclasses: Vec<u8>,
    class_masterbooks: Vec<u8>,
    subclass_books: Vec<Vec<i32>>,
    multiplier: u32,
    x_list: Vec<u32>,
}

impl Floor {
    pub fn read(r: &mut BitReader, codebook_count: usize) -> Result<Floor, AudioError> {
        let kind = r.read(16).ok_or(AudioError::Truncated)?;
        match kind {
            1 => Ok(Floor::Type1(Floor1::read(r, codebook_count)?)),
            0 => Err(AudioError::Unsupported("vorbis floor type 0")),
            _ => Err(AudioError::Malformed),
        }
    }
}

impl Floor1 {
    fn read(r: &mut BitReader, codebook_count: usize) -> Result<Floor1, AudioError> {
        let partitions = r.read(5).ok_or(AudioError::Truncated)? as usize;
        let mut partition_class_list = Vec::with_capacity(partitions);
        let mut max_class: i32 = -1;
        for _ in 0..partitions {
            let c = r.read(4).ok_or(AudioError::Truncated)? as u8;
            max_class = max_class.max(c as i32);
            partition_class_list.push(c);
        }
        let n_classes = (max_class + 1).max(0) as usize;
        let mut class_dimensions = vec![0u8; n_classes];
        let mut class_subclasses = vec![0u8; n_classes];
        let mut class_masterbooks = vec![0u8; n_classes];
        let mut subclass_books = vec![Vec::new(); n_classes];
        for c in 0..n_classes {
            class_dimensions[c] = (r.read(3).ok_or(AudioError::Truncated)? + 1) as u8;
            class_subclasses[c] = r.read(2).ok_or(AudioError::Truncated)? as u8;
            if class_subclasses[c] != 0 {
                let mb = r.read(8).ok_or(AudioError::Truncated)? as usize;
                if mb >= codebook_count {
                    return Err(AudioError::Malformed);
                }
                class_masterbooks[c] = mb as u8;
            }
            let n = 1usize << class_subclasses[c];
            let mut books = Vec::with_capacity(n);
            for _ in 0..n {
                let b = r.read(8).ok_or(AudioError::Truncated)? as i32 - 1;
                if b >= codebook_count as i32 {
                    return Err(AudioError::Malformed);
                }
                books.push(b);
            }
            subclass_books[c] = books;
        }
        let multiplier = r.read(2).ok_or(AudioError::Truncated)? + 1;
        let rangebits = r.read(4).ok_or(AudioError::Truncated)?;
        let mut x_list = vec![0u32, 1u32 << rangebits.min(31)];
        for &c in &partition_class_list {
            let dim = class_dimensions[c as usize];
            for _ in 0..dim {
                x_list.push(r.read(rangebits).ok_or(AudioError::Truncated)?);
            }
        }
        if x_list.len() > 1024 {
            return Err(AudioError::Malformed);
        }
        Ok(Floor1 {
            partition_class_list,
            class_dimensions,
            class_subclasses,
            class_masterbooks,
            subclass_books,
            multiplier,
            x_list,
        })
    }

    /// Decode the per-packet floor. `None` means "this channel is silent".
    pub fn decode(
        &self,
        r: &mut BitReader,
        books: &[Codebook],
    ) -> Result<Option<Vec<i32>>, AudioError> {
        let nonzero = r.read_bit().ok_or(AudioError::Truncated)?;
        if !nonzero {
            return Ok(None);
        }
        let range = match self.multiplier {
            1 => 256,
            2 => 128,
            3 => 86,
            _ => 64,
        };
        let bits = ilog(range - 1);
        let mut y = vec![0i32; self.x_list.len()];
        y[0] = r.read(bits).ok_or(AudioError::Truncated)? as i32;
        y[1] = r.read(bits).ok_or(AudioError::Truncated)? as i32;
        let mut offset = 2usize;
        for &cls in &self.partition_class_list {
            let cls = cls as usize;
            let cdim = self.class_dimensions[cls] as usize;
            let cbits = self.class_subclasses[cls] as u32;
            let csub = (1i32 << cbits) - 1;
            let mut cval = 0i32;
            if cbits > 0 {
                let book = self.class_masterbooks[cls] as usize;
                let cb = books.get(book).ok_or(AudioError::Malformed)?;
                cval = cb.decode(r)? as i32;
            }
            for j in 0..cdim {
                let idx = (cval & csub) as usize;
                let book = *self
                    .subclass_books
                    .get(cls)
                    .and_then(|b| b.get(idx))
                    .ok_or(AudioError::Malformed)?;
                cval >>= cbits;
                let slot = offset + j;
                if slot >= y.len() {
                    return Err(AudioError::Malformed);
                }
                y[slot] = if book >= 0 {
                    let cb = books.get(book as usize).ok_or(AudioError::Malformed)?;
                    cb.decode(r)? as i32
                } else {
                    0
                };
            }
            offset += cdim;
        }
        Ok(Some(y))
    }

    /// Turn decoded Y values into the linear floor curve over `n` bins.
    pub fn synthesize(&self, y: &[i32], n: usize) -> Vec<f32> {
        let range = match self.multiplier {
            1 => 256i32,
            2 => 128,
            3 => 86,
            _ => 64,
        };
        let values = self.x_list.len();
        let mut final_y = vec![0i32; values];
        let mut step2 = vec![false; values];
        step2[0] = true;
        step2[1] = true;
        final_y[0] = y[0];
        final_y[1] = y[1];

        for i in 2..values {
            let (lo, hi) = neighbors(&self.x_list, i);
            let predicted = render_point(
                self.x_list[lo] as i32,
                final_y[lo],
                self.x_list[hi] as i32,
                final_y[hi],
                self.x_list[i] as i32,
            );
            let val = y[i];
            let highroom = range - predicted;
            let lowroom = predicted;
            let room = if highroom < lowroom {
                highroom * 2
            } else {
                lowroom * 2
            };
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

        // Walk the points in X order, drawing line segments between the ones
        // that survived step 2.
        let mut order: Vec<usize> = (0..values).collect();
        order.sort_by_key(|&i| self.x_list[i]);

        let mut curve = vec![0f32; n];
        let mut lx = 0usize;
        let mut ly = final_y[0] * self.multiplier as i32;
        let mut hx = 0usize;
        let mut hy = ly;
        let mut drawn_any = false;
        for &i in &order {
            if !step2[i] {
                continue;
            }
            hy = final_y[i] * self.multiplier as i32;
            hx = (self.x_list[i] as usize).min(n);
            if hx > lx {
                render_line(lx, ly, hx, hy, &mut curve, n);
                drawn_any = true;
            }
            lx = hx;
            ly = hy;
        }
        // Flat-line the tail past the last defined point.
        if !drawn_any {
            for c in curve.iter_mut() {
                *c = inverse_db(ly);
            }
        } else if hx < n {
            let v = inverse_db(hy);
            for c in curve.iter_mut().skip(hx) {
                *c = v;
            }
        }
        curve
    }
}

/// Nearest X below and above `i` among the entries before `i`.
fn neighbors(x: &[u32], i: usize) -> (usize, usize) {
    let xi = x[i];
    let mut lo = 0usize;
    let mut hi = 1usize;
    let mut lo_val = 0u32;
    let mut hi_val = u32::MAX;
    let mut lo_set = false;
    let mut hi_set = false;
    for (j, &xj) in x.iter().enumerate().take(i) {
        if xj < xi && (!lo_set || xj > lo_val) {
            lo = j;
            lo_val = xj;
            lo_set = true;
        }
        if xj > xi && (!hi_set || xj < hi_val) {
            hi = j;
            hi_val = xj;
            hi_set = true;
        }
    }
    (lo, hi)
}

fn render_point(x0: i32, y0: i32, x1: i32, y1: i32, x: i32) -> i32 {
    let dy = y1 - y0;
    let adx = x1 - x0;
    if adx == 0 {
        return y0;
    }
    let ady = dy.abs();
    let err = ady * (x - x0);
    let off = err / adx;
    if dy < 0 {
        y0 - off
    } else {
        y0 + off
    }
}

fn render_line(x0: usize, y0: i32, x1: usize, y1: i32, out: &mut [f32], n: usize) {
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
        out[x0] = inverse_db(y);
    }
    for x in (x0 + 1)..x1.min(n) {
        err += ady;
        if err >= adx {
            err -= adx;
            y += sy;
        } else {
            y += base;
        }
        out[x] = inverse_db(y);
    }
}

/// The spec's 256-entry inverse-dB table, generated rather than pasted.
///
/// Each step is 7/256 of a decade, which reproduces the published endpoints
/// (1.0649863e-07 at 0, 1.0 at 255) to within ~3e-6 relative — about -110 dB,
/// far below anything audible.
pub fn inverse_db(x: i32) -> f32 {
    let i = x.clamp(0, 255) as f64;
    (10f64).powf((i - 255.0) * (7.0 / 256.0)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_db_matches_the_published_endpoints() {
        let first = inverse_db(0);
        let last = inverse_db(255);
        assert!(
            (first as f64 / 1.0649863e-07 - 1.0).abs() < 1e-3,
            "first {first:e}"
        );
        assert!((last - 1.0).abs() < 1e-6, "last {last}");
        // Monotonic across the whole table.
        for i in 1..256 {
            assert!(inverse_db(i) > inverse_db(i - 1));
        }
    }

    #[test]
    fn inverse_db_clamps_out_of_range_indices() {
        assert_eq!(inverse_db(-50), inverse_db(0));
        assert_eq!(inverse_db(9999), inverse_db(255));
    }

    #[test]
    fn render_point_interpolates_linearly() {
        // Straight line from (0,0) to (10,100): midpoint is 50.
        assert_eq!(render_point(0, 0, 10, 100, 5), 50);
        // Descending.
        assert_eq!(render_point(0, 100, 10, 0, 5), 50);
        // Degenerate span returns the start.
        assert_eq!(render_point(4, 7, 4, 90, 4), 7);
    }

    #[test]
    fn render_line_fills_the_span_monotonically() {
        let mut out = vec![0f32; 16];
        render_line(0, 0, 16, 255, &mut out, 16);
        for i in 1..16 {
            assert!(out[i] >= out[i - 1], "not monotonic at {i}");
        }
        assert!(out[15] > out[0]);
    }

    #[test]
    fn render_line_respects_the_output_bound() {
        // x1 past the end must not write out of range.
        let mut out = vec![0f32; 8];
        render_line(0, 0, 100, 255, &mut out, 8);
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn neighbors_picks_the_tightest_bracket() {
        let x = [0u32, 128, 32, 64];
        // For index 3 (x=64): low is 32 (idx 2), high is 128 (idx 1).
        assert_eq!(neighbors(&x, 3), (2, 1));
    }

    #[test]
    fn floor_type_zero_is_reported_not_guessed() {
        let data = [0u8; 8];
        let mut r = BitReader::new(&data);
        // 16 bits of zero == floor type 0.
        let e = match Floor::read(&mut r, 1) {
            Err(e) => e,
            Ok(_) => panic!("floor type 0 should not parse"),
        };
        assert!(matches!(e, AudioError::Unsupported(_)));
    }
}

