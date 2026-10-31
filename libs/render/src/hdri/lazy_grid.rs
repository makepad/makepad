//! A grid of `C` channels whose rows are computed on their first read and kept (plan hdri-7: the nebulas' sprites).
//! The content is a function of (x, y) alone, so whichever thread fills a row the bits are the same, and a bake equals
//! its parallel bake.

use std::sync::OnceLock;

/// `C` channels on a `w` x `h` grid over (u, v) in [0, 1]², each row computed on its first read by `fill` and kept
/// (`OnceLock`, the `CloudCache` pattern), read bilinearly between texel centres ((x + 0.5) / w, (y + 0.5) / h); u wraps
/// or clamps, v clamps.
pub(crate) struct LazyGrid<const C: usize> {
    w: usize,
    h: usize,
    wrap_u: bool,
    rows: Vec<OnceLock<Box<[[f32; C]]>>>,
}

/// The grid's shape only: which rows are filled depends on what has been read, and the rows themselves are bulk.
impl<const C: usize> std::fmt::Debug for LazyGrid<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyGrid").field("w", &self.w).field("h", &self.h).field("wrap_u", &self.wrap_u).finish()
    }
}

impl<const C: usize> LazyGrid<C> {
    pub(crate) fn new(w: usize, h: usize, wrap_u: bool) -> Self {
        let (w, h) = (w.max(1), h.max(1));
        LazyGrid { w, h, wrap_u, rows: (0..h).map(|_| OnceLock::new()).collect() }
    }

    fn row(&self, y: usize, fill: &(dyn Fn(usize, usize) -> [f32; C] + Sync)) -> &[[f32; C]] {
        self.rows[y].get_or_init(|| (0..self.w).map(|x| fill(x, y)).collect())
    }

    /// The grid at (u, v), filling the two rows it reads on first use; 0 where a coordinate is not finite.
    pub(crate) fn sample(&self, u: f32, v: f32, fill: &(dyn Fn(usize, usize) -> [f32; C] + Sync)) -> [f32; C] {
        if !(u.is_finite() && v.is_finite()) {
            return [0.0; C];
        }
        let (w, h) = (self.w as f32, self.h as f32);
        let fy = (v * h - 0.5).clamp(0.0, h - 1.0);
        let fx = if self.wrap_u { u * w - 0.5 } else { (u * w - 0.5).clamp(0.0, w - 1.0) };
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let column = |x: f32| -> usize {
            if self.wrap_u { (x as i64).rem_euclid(self.w as i64) as usize } else { (x as usize).min(self.w - 1) }
        };
        let (xa, xb) = (column(x0), column(x0 + 1.0));
        let ya = y0 as usize;
        let yb = (ya + 1).min(self.h - 1);
        let (ra, rb) = (self.row(ya, fill), self.row(yb, fill));
        std::array::from_fn(|c| {
            let top = ra[xa][c] + (ra[xb][c] - ra[xa][c]) * tx;
            let bottom = rb[xa][c] + (rb[xb][c] - rb[xa][c]) * tx;
            top + (bottom - top) * ty
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lazy_grid_reads_its_fill_bilinearly_at_texel_centres() {
        let fill = |x: usize, y: usize| [x as f32, y as f32];
        let g = LazyGrid::<2>::new(8, 4, false);
        assert_eq!(g.sample(0.5 / 8.0, 0.5 / 4.0, &fill), [0.0, 0.0], "a texel centre is its texel");
        assert_eq!(g.sample(3.5 / 8.0, 2.5 / 4.0, &fill), [3.0, 2.0]);
        assert_eq!(g.sample(4.0 / 8.0, 2.0 / 4.0, &fill), [3.5, 1.5], "half way between four");
        assert_eq!(g.sample(0.0, 0.0, &fill), [0.0, 0.0], "clamped at the edges");
        assert_eq!(g.sample(1.0, 1.0, &fill), [7.0, 3.0]);
        let w = LazyGrid::<2>::new(8, 4, true);
        assert_eq!(w.sample(0.0, 0.5 / 4.0, &fill), [3.5, 0.0], "wrapped: half way between the last column and the first");
        assert_eq!(g.sample(f32::NAN, 0.5, &fill), [0.0, 0.0]);
    }

    /// Rows filled on four threads hold the bits rows filled on one do: a bake equals its parallel bake.
    #[test]
    fn rows_filled_on_many_threads_hold_the_same_bits() {
        let fill = |x: usize, y: usize| [((x * 7 + y * 13) as f32 * 0.37).sin(), (x as f32).sqrt() + y as f32];
        let (serial, parallel) = (LazyGrid::<2>::new(64, 32, true), LazyGrid::<2>::new(64, 32, true));
        let looks: Vec<(f32, f32)> = (0..2000).map(|i| ((i as f32 * 0.618_034).fract(), (i as f32 * 0.414_214).fract())).collect();
        std::thread::scope(|s| {
            for t in 0..4 {
                let (grid, looks) = (&parallel, &looks);
                s.spawn(move || looks.iter().skip(t).step_by(4).for_each(|(u, v)| { grid.sample(*u, *v, &fill); }));
            }
        });
        for (u, v) in &looks {
            assert_eq!(serial.sample(*u, *v, &fill).map(f32::to_bits), parallel.sample(*u, *v, &fill).map(f32::to_bits));
        }
    }
}
