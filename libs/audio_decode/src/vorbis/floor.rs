//! Floor decode: the spectral envelope. Only floor type 1 is implemented —
//! type 0 (LSP) is essentially unused by modern encoders, and reporting it is
//! honest where guessing would produce noise.
//!
//! Adapted from the same author's sandbox decoder. The per-packet work here is
//! allocation-free: the point sort and the neighbour brackets depend only on
//! the header, so they are computed once at setup, and the curve is drawn into
//! a caller-owned buffer.

use super::bits::{ilog, BitReader};
use super::codebook::Codebook;
use crate::error::AudioError;

/// A floor 1 configuration cannot have more points than this: 31 partitions
/// (5 bits) of at most 8 dimensions each, plus the two implicit end points.
pub const MAX_POINTS: usize = 2 + 31 * 8;

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
    range: i32,
    /// Bits per Y value, from the multiplier.
    y_bits: u32,
    x_list: Vec<u32>,
    /// Point indices in increasing X order (constant per stream).
    sorted: Vec<u16>,
    /// Tightest bracket among the earlier points (constant per stream).
    neighbour_lo: Vec<u16>,
    neighbour_hi: Vec<u16>,
}

impl Floor {
    pub fn read(r: &mut BitReader, codebook_count: usize) -> Result<Floor, AudioError> {
        let kind = r.read(16).ok_or(AudioError::Truncated)?;
        match kind {
            1 => Ok(Floor::Type1(Floor1::read(r, codebook_count)?)),
            0 => Err(AudioError::Unsupported("vorbis floor 0")),
            _ => Err(AudioError::Corrupt("floor type")),
        }
    }
}

impl Floor1 {
    /// Number of Y values one packet carries for this floor.
    pub fn points(&self) -> usize {
        self.x_list.len()
    }

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
                    return Err(AudioError::Corrupt("floor masterbook"));
                }
                class_masterbooks[c] = mb as u8;
            }
            let n = 1usize << class_subclasses[c];
            let mut books = Vec::with_capacity(n);
            for _ in 0..n {
                let b = r.read(8).ok_or(AudioError::Truncated)? as i32 - 1;
                if b >= codebook_count as i32 {
                    return Err(AudioError::Corrupt("floor subclass book"));
                }
                books.push(b);
            }
            subclass_books[c] = books;
        }
        let multiplier = r.read(2).ok_or(AudioError::Truncated)? + 1;
        let rangebits = r.read(4).ok_or(AudioError::Truncated)?;
        let mut x_list = vec![0u32, 1u32 << rangebits];
        for &c in &partition_class_list {
            let dim = *class_dimensions.get(c as usize).ok_or(AudioError::Corrupt("floor class"))?;
            for _ in 0..dim {
                x_list.push(r.read(rangebits).ok_or(AudioError::Truncated)?);
            }
        }
        if x_list.len() > MAX_POINTS {
            return Err(AudioError::Corrupt("floor point count"));
        }
        let range = match multiplier {
            1 => 256i32,
            2 => 128,
            3 => 86,
            _ => 64,
        };
        let mut sorted: Vec<u16> = (0..x_list.len() as u16).collect();
        sorted.sort_by_key(|&i| x_list[i as usize]);
        let mut neighbour_lo = vec![0u16; x_list.len()];
        let mut neighbour_hi = vec![0u16; x_list.len()];
        for i in 2..x_list.len() {
            let (lo, hi) = neighbors(&x_list, i);
            neighbour_lo[i] = lo as u16;
            neighbour_hi[i] = hi as u16;
        }
        Ok(Floor1 {
            partition_class_list,
            class_dimensions,
            class_subclasses,
            class_masterbooks,
            subclass_books,
            multiplier,
            range,
            y_bits: ilog(range - 1),
            x_list,
            sorted,
            neighbour_lo,
            neighbour_hi,
        })
    }

    /// Decode this packet's floor into `y` (which must hold [`Floor1::points`]
    /// values). `false` means "this channel is silent in this packet".
    pub fn decode(
        &self,
        r: &mut BitReader,
        books: &[Codebook],
        y: &mut [i32],
    ) -> Result<bool, AudioError> {
        if y.len() < self.x_list.len() {
            return Err(AudioError::Corrupt("floor scratch"));
        }
        let nonzero = r.read_bit().ok_or(AudioError::Truncated)?;
        if !nonzero {
            return Ok(false);
        }
        y[0] = r.read(self.y_bits).ok_or(AudioError::Truncated)? as i32;
        y[1] = r.read(self.y_bits).ok_or(AudioError::Truncated)? as i32;
        let mut offset = 2usize;
        for &cls in &self.partition_class_list {
            let cls = cls as usize;
            let cdim = self.class_dimensions[cls] as usize;
            let cbits = self.class_subclasses[cls] as u32;
            let csub = (1i32 << cbits) - 1;
            let mut cval = 0i32;
            if cbits > 0 {
                let book = self.class_masterbooks[cls] as usize;
                let cb = books.get(book).ok_or(AudioError::Corrupt("floor masterbook"))?;
                cval = cb.decode(r)? as i32;
            }
            for j in 0..cdim {
                let idx = (cval & csub) as usize;
                let book = *self
                    .subclass_books
                    .get(cls)
                    .and_then(|b| b.get(idx))
                    .ok_or(AudioError::Corrupt("floor subclass"))?;
                cval >>= cbits;
                let slot = offset + j;
                if slot >= self.x_list.len() {
                    return Err(AudioError::Corrupt("floor overrun"));
                }
                y[slot] = if book >= 0 {
                    let cb =
                        books.get(book as usize).ok_or(AudioError::Corrupt("floor subclass"))?;
                    cb.decode(r)? as i32
                } else {
                    0
                };
            }
            offset += cdim;
        }
        Ok(true)
    }

    /// Turn decoded Y values into the linear floor curve over `out.len()` bins.
    pub fn synthesize(&self, y: &[i32], out: &mut [f32]) {
        let n = out.len();
        let values = self.x_list.len().min(y.len());
        if values < 2 || n == 0 {
            out.fill(0.0);
            return;
        }
        let mut final_y = [0i32; MAX_POINTS];
        let mut step2 = [false; MAX_POINTS];
        step2[0] = true;
        step2[1] = true;
        final_y[0] = y[0];
        final_y[1] = y[1];

        for i in 2..values {
            let lo = self.neighbour_lo[i] as usize;
            let hi = self.neighbour_hi[i] as usize;
            let predicted = render_point(
                self.x_list[lo] as i32,
                final_y[lo],
                self.x_list[hi] as i32,
                final_y[hi],
                self.x_list[i] as i32,
            );
            let val = y[i];
            let highroom = self.range - predicted;
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

        // Walk the points in X order, drawing line segments between the ones
        // that survived step 2.
        let mut lx = 0usize;
        let mut ly = final_y[0] * self.multiplier as i32;
        let mut hx = 0usize;
        let mut hy = ly;
        let mut drawn_any = false;
        for &si in &self.sorted {
            let i = si as usize;
            if i >= values || !step2[i] {
                continue;
            }
            hy = final_y[i] * self.multiplier as i32;
            hx = (self.x_list[i] as usize).min(n);
            if hx > lx {
                render_line(lx, ly, hx, hy, out);
                drawn_any = true;
            }
            lx = hx;
            ly = hy;
        }
        // Flat-line the tail past the last defined point.
        if !drawn_any {
            out.fill(inverse_db(ly));
        } else if hx < n {
            let v = inverse_db(hy);
            out[hx..].fill(v);
        }
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
    let err = ady.saturating_mul(x - x0);
    let off = err / adx;
    if dy < 0 {
        y0 - off
    } else {
        y0 + off
    }
}

/// The spec's integer line draw, in inverse-dB space.
fn render_line(x0: usize, y0: i32, x1: usize, y1: i32, out: &mut [f32]) {
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
        out[x0] = inverse_db(y);
    }
    for slot in out.iter_mut().take(x1.min(n)).skip(x0 + 1) {
        err += ady;
        if err >= adx {
            err -= adx;
            y += sy;
        } else {
            y += base;
        }
        *slot = inverse_db(y);
    }
}

/// The spec's 256-entry inverse-dB table, generated rather than pasted.
///
/// Each step is 7/256 of a decade, which reproduces the published endpoints
/// (1.0649863e-07 at 0, 1.0 at 255) to seven digits — the table in the spec is
/// this exponential rounded to `f32`, so generating it costs no accuracy.
pub fn inverse_db(x: i32) -> f32 {
    let i = x.clamp(0, 255) as f64;
    (10f64).powf((i - 255.0) * (7.0 / 256.0)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_db_matches_the_published_table() {
        // The published floor1_inverse_dB_table is this exponential rounded to
        // f32; its first two and last entries pin both ends of the ramp.
        for (i, want) in [(0i32, 1.0649863e-07f64), (1, 1.1341951e-07), (255, 1.0)] {
            let got = inverse_db(i) as f64;
            assert!((got / want - 1.0).abs() < 1e-6, "index {i}: {got:e} vs {want:e}");
        }
        // Every step is the same 7/256-of-a-decade ratio, and the table rises.
        let step = 10f64.powf(7.0 / 256.0);
        for i in 1..256 {
            assert!(inverse_db(i) > inverse_db(i - 1));
            let ratio = inverse_db(i) as f64 / inverse_db(i - 1) as f64;
            assert!((ratio / step - 1.0).abs() < 1e-5, "step at {i}: {ratio}");
        }
    }

    #[test]
    fn inverse_db_clamps_out_of_range_indices() {
        assert_eq!(inverse_db(-50), inverse_db(0));
        assert_eq!(inverse_db(9999), inverse_db(255));
    }

    #[test]
    fn render_point_interpolates_linearly() {
        assert_eq!(render_point(0, 0, 10, 100, 5), 50);
        assert_eq!(render_point(0, 100, 10, 0, 5), 50);
        // Degenerate span returns the start.
        assert_eq!(render_point(4, 7, 4, 90, 4), 7);
    }

    #[test]
    fn render_line_fills_the_span_monotonically() {
        let mut out = vec![0f32; 16];
        render_line(0, 0, 16, 255, &mut out);
        for i in 1..16 {
            assert!(out[i] >= out[i - 1], "not monotonic at {i}");
        }
        assert!(out[15] > out[0]);
    }

    #[test]
    fn render_line_respects_the_output_bound() {
        let mut out = vec![0f32; 8];
        render_line(0, 0, 100, 255, &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn render_line_is_the_spec_bresenham() {
        // Reference: the spec's own loop, written out.
        fn reference(x0: usize, y0: i32, x1: usize, y1: i32, n: usize) -> Vec<f32> {
            let mut v = vec![0f32; n];
            let dy = y1 - y0;
            let adx = (x1 - x0) as i32;
            let mut ady = dy.abs();
            let base = dy / adx;
            let sy = if dy < 0 { base - 1 } else { base + 1 };
            ady -= base.abs() * adx;
            let mut y = y0;
            let mut err = 0;
            if x0 < n {
                v[x0] = inverse_db(y);
            }
            #[allow(clippy::needless_range_loop)]
            for x in (x0 + 1)..x1.min(n) {
                err += ady;
                if err >= adx {
                    err -= adx;
                    y += sy;
                } else {
                    y += base;
                }
                v[x] = inverse_db(y);
            }
            v
        }
        for &(x0, y0, x1, y1) in &[
            (0usize, 10i32, 40usize, 200i32),
            (3, 200, 39, 5),
            (0, 0, 64, 255),
            (10, 128, 11, 129),
        ] {
            let mut got = vec![0f32; 64];
            render_line(x0, y0, x1, y1, &mut got);
            assert_eq!(got, reference(x0, y0, x1, y1, 64), "line {x0},{y0} -> {x1},{y1}");
        }
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
        // 16 bits of zero is floor type 0.
        match Floor::read(&mut r, 1) {
            Err(AudioError::Unsupported(what)) => assert_eq!(what, "vorbis floor 0"),
            other => panic!("floor type 0 should be reported: {other:?}", other = other.err()),
        }
    }

    #[test]
    fn floor_type_nine_is_corrupt() {
        let mut data = [0u8; 8];
        data[0] = 9;
        let mut r = BitReader::new(&data);
        assert!(matches!(Floor::read(&mut r, 1), Err(AudioError::Corrupt(_))));
    }

    /// Build a minimal floor 1 header: one partition, one class of one
    /// dimension with no subclasses and no book, so the Y values decode
    /// straight from the packet.
    fn tiny_floor() -> Floor1 {
        let fields: &[(u32, u32)] = &[
            (1, 16),  // floor type 1
            (1, 5),   // partitions
            (0, 4),   // partition 0 uses class 0
            (0, 3),   // class 0 dimensions - 1
            (0, 2),   // class 0 subclasses
            (0, 8),   // subclass book + 1 == 0, i.e. "no book"
            (1, 2),   // multiplier - 1 == 1, so multiplier 2, range 128
            (5, 4),   // rangebits: x values are 5 bits, top point at 32
            (16, 5),  // x_list[2]
        ];
        let mut bits: Vec<bool> = Vec::new();
        for &(v, n) in fields {
            for i in 0..n {
                bits.push((v >> i) & 1 == 1);
            }
        }
        let mut bytes = vec![0u8; bits.len().div_ceil(8) + 4];
        for (i, b) in bits.iter().enumerate() {
            if *b {
                bytes[i / 8] |= 1 << (i % 8);
            }
        }
        let mut r = BitReader::new(&bytes);
        match Floor::read(&mut r, 1).unwrap() {
            Floor::Type1(f) => f,
        }
    }

    #[test]
    fn floor_header_parses_its_points() {
        let f = tiny_floor();
        assert_eq!(f.x_list, vec![0, 32, 16]);
        assert_eq!(f.points(), 3);
        assert_eq!(f.multiplier, 2);
        assert_eq!(f.range, 128);
        // Sorted order is by X: 0, 16, 32.
        assert_eq!(f.sorted, vec![0, 2, 1]);
    }

    #[test]
    fn curve_interpolates_between_its_points() {
        let f = tiny_floor();
        // Y at x=0 is 20, at x=32 is 100; the middle point says "predicted".
        let y = [20i32, 100, 0];
        let mut curve = vec![0f32; 32];
        f.synthesize(&y, &mut curve);
        // Endpoints, in the multiplier-scaled dB domain.
        assert_eq!(curve[0], inverse_db(40));
        // Monotonically rising towards the top point.
        for i in 1..32 {
            assert!(curve[i] >= curve[i - 1], "dip at {i}");
        }
        assert!(curve[31] > curve[0]);
        // Everything is a sane amplitude.
        assert!(curve.iter().all(|&v| (0.0..=1.0).contains(&v)));
    }

    #[test]
    fn a_silent_packet_says_so() {
        let f = tiny_floor();
        let data = [0u8; 4]; // leading bit zero means "no floor"
        let mut r = BitReader::new(&data);
        let mut y = [0i32; 8];
        assert!(!f.decode(&mut r, &[], &mut y).unwrap());
    }

    #[test]
    fn curve_fills_a_buffer_shorter_than_the_x_range() {
        let f = tiny_floor();
        let y = [100i32, 20, 0];
        let mut curve = vec![0f32; 8];
        f.synthesize(&y, &mut curve);
        assert!(curve.iter().all(|&v| v.is_finite() && v > 0.0));
    }
}
