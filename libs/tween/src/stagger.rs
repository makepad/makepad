//! Stagger: spreading one tween's start over many targets (GSAP `stagger`,
//! computed by GSAP's `utils.distribute`).
//!
//! Delays are computed once, when the tween is built, into a caller-provided
//! buffer; nothing here allocates.

use crate::easing::Easing;
use crate::{round7, splitmix64, BIG};

/// How the total stagger span is given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spread {
    /// GSAP `each`: seconds between neighbouring targets (the whole span is
    /// `each * (n - 1)` in one dimension, whatever `from` is).
    Each(f64),
    /// GSAP `amount`: the whole span in seconds, divided among the targets.
    Amount(f64),
    /// `each`, but never more than `amount` in total (the span shrinks to
    /// fit when there are many targets). Not GSAP.
    EachWithin {
        /// Preferred seconds between neighbours.
        each: f64,
        /// Upper bound of the whole span.
        amount: f64,
    },
}

/// Where the stagger starts: GSAP `from`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StaggerFrom {
    /// A target index (GSAP numeric `from`, default 0).
    Index(u32),
    /// The first target (GSAP `"start"`).
    Start,
    /// The middle (GSAP `"center"`).
    Center,
    /// The two ends inwards (GSAP `"edges"`).
    Edges,
    /// The last target (GSAP `"end"`).
    End,
    /// A point as a ratio of the grid (GSAP `[x, y]` or a decimal in 0..1).
    Ratio(f64, f64),
    /// A shuffled order (GSAP `"random"`), deterministic for a given seed.
    Random(u64),
}

/// A grid layout of the targets: GSAP `grid: [rows, cols]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaggerGrid {
    /// Rows.
    pub rows: u32,
    /// Columns (targets wrap after this many).
    pub cols: u32,
}

/// Measure grid distances along one axis only: GSAP `axis`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaggerAxis {
    /// Columns only (`"x"`).
    X,
    /// Rows only (`"y"`).
    Y,
}

/// A stagger: GSAP's `stagger` value or object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stagger {
    /// `each` / `amount`.
    pub spread: Spread,
    /// `from`.
    pub from: StaggerFrom,
    /// `grid`.
    pub grid: Option<StaggerGrid>,
    /// `axis`.
    pub axis: Option<StaggerAxis>,
    /// `ease`: distributes the delays along a curve.
    pub ease: Option<Easing>,
    /// Added to every delay (GSAP `base`, seconds).
    pub base: f64,
    /// `repeat` of each sub-tween (GSAP `stagger: {repeat}`).
    pub repeat: Option<i32>,
    /// `yoyo` of each sub-tween.
    pub yoyo: Option<bool>,
    /// `repeatDelay` of each sub-tween.
    pub repeat_delay: Option<f64>,
}

/// The per-call geometry shared by [`Stagger::delays`] and [`Stagger::delay`].
struct Layout {
    wrap: f64,
    ox: f64,
    oy: f64,
}

impl Stagger {
    const fn with_spread(spread: Spread) -> Self {
        Self {
            spread,
            from: StaggerFrom::Index(0),
            grid: None,
            axis: None,
            ease: None,
            base: 0.0,
            repeat: None,
            yoyo: None,
            repeat_delay: None,
        }
    }

    /// GSAP `stagger: s` / `{each: s}`.
    pub const fn each(s: f64) -> Self {
        Self::with_spread(Spread::Each(s))
    }

    /// GSAP `stagger: {amount: s}`.
    pub const fn amount(s: f64) -> Self {
        Self::with_spread(Spread::Amount(s))
    }

    /// `each`, shrunk so the whole span stays within `amount`.
    pub const fn each_within(each: f64, amount: f64) -> Self {
        Self::with_spread(Spread::EachWithin { each, amount })
    }

    /// Sets `from`.
    pub const fn from(self, f: StaggerFrom) -> Self {
        Self { from: f, ..self }
    }

    /// Sets `grid: [rows, cols]` (GSAP's argument order).
    pub const fn grid(self, rows: u32, cols: u32) -> Self {
        Self {
            grid: Some(StaggerGrid { rows, cols }),
            ..self
        }
    }

    /// Sets `axis`.
    pub const fn axis(self, a: StaggerAxis) -> Self {
        Self {
            axis: Some(a),
            ..self
        }
    }

    /// Sets `ease`.
    pub const fn ease(self, e: Easing) -> Self {
        Self {
            ease: Some(e),
            ..self
        }
    }

    /// Sets `base`.
    pub const fn base(self, b: f64) -> Self {
        Self { base: b, ..self }
    }

    /// Sets the `repeat` of each sub-tween.
    pub const fn each_repeat(self, n: i32) -> Self {
        Self {
            repeat: Some(n),
            ..self
        }
    }

    /// Sets the `yoyo` of each sub-tween.
    pub const fn each_yoyo(self, b: bool) -> Self {
        Self {
            yoyo: Some(b),
            ..self
        }
    }

    /// Sets the `repeatDelay` of each sub-tween.
    pub const fn each_repeat_delay(self, s: f64) -> Self {
        Self {
            repeat_delay: Some(s),
            ..self
        }
    }

    /// The effective seconds between neighbours for `n` targets: `each`,
    /// the `EachWithin` budget applied, or `amount / (n - 1)`.
    pub fn step(&self, n: u32) -> f64 {
        match self.spread {
            Spread::Each(e) => e,
            Spread::EachWithin { each, amount } => {
                let s = each.max(0.0);
                if n < 2 {
                    s
                } else {
                    s.min(amount / (n - 1) as f64)
                }
            }
            Spread::Amount(a) => {
                if n < 2 {
                    0.0
                } else {
                    a / (n - 1) as f64
                }
            }
        }
    }

    /// Writes the start delay of each of `n` targets into `out`
    /// (`out.len() == n`), as GSAP `distribute` does. `out` doubles as the
    /// distance scratch, so nothing is allocated.
    pub fn delays(&self, n: u32, out: &mut [f64]) {
        assert_eq!(
            out.len(),
            n as usize,
            "Stagger::delays: out.len() must equal n"
        );
        if let Some((step, order)) = self.fast_step(n) {
            for (j, d) in out.iter_mut().enumerate() {
                *d = step * order.order(j as u32, n) as f64;
            }
            return;
        }
        let lay = self.layout(n);
        let (mut min, mut max) = (BIG, 0.0f64);
        for (j, d) in out.iter_mut().enumerate() {
            *d = self.distance(&lay, j as u32);
            if *d > max {
                max = *d;
            }
            if *d < min {
                min = *d;
            }
        }
        if let StaggerFrom::Random(seed) = self.from {
            for k in (1..n).rev() {
                out.swap(k as usize, shuffle_pick(seed, k) as usize);
            }
        }
        let (b, v, ease, invert) = self.span(n, lay.wrap);
        for d in out.iter_mut() {
            *d = finish(*d, min, max - min, b, v, ease, invert);
        }
    }

    /// The start delay of target `i` of `n`: bit-identical to `delays()[i]`,
    /// in O(n) time and O(1) memory.
    pub fn delay(&self, i: u32, n: u32) -> f64 {
        if let Some((step, order)) = self.fast_step(n) {
            return step * order.order(i, n) as f64;
        }
        let lay = self.layout(n);
        let (mut min, mut max) = (BIG, 0.0f64);
        for j in 0..n {
            let d = self.distance(&lay, j);
            if d > max {
                max = d;
            }
            if d < min {
                min = d;
            }
        }
        let mut q = i;
        if let StaggerFrom::Random(seed) = self.from {
            // Undo the Fisher-Yates swaps on one index, last swap first.
            for k in 1..n {
                let j = shuffle_pick(seed, k);
                if q == k {
                    q = j;
                } else if q == j {
                    q = k;
                }
            }
        }
        let (b, v, ease, invert) = self.span(n, lay.wrap);
        finish(self.distance(&lay, q), min, max - min, b, v, ease, invert)
    }

    /// The 1-D shortcut `delay = step * order` (no rounding), taken when the
    /// general formula reduces to it: no grid, axis, ease or base, `each`
    /// spacing, and `from` the first or last target.
    fn fast_step(&self, n: u32) -> Option<(f64, Order)> {
        if self.grid.is_some() || self.axis.is_some() || self.ease.is_some() || self.base != 0.0 {
            return None;
        }
        if matches!(self.spread, Spread::Amount(_)) {
            return None;
        }
        let order = match self.from {
            StaggerFrom::Index(0) | StaggerFrom::Start => Order::Forward,
            StaggerFrom::End => Order::Backward,
            _ => return None,
        };
        Some((self.step(n), order))
    }

    /// The wrap width and the origin, as GSAP `distribute` computes them.
    fn layout(&self, n: u32) -> Layout {
        let n = n as f64;
        let wrap = match self.grid {
            Some(g) => g.cols as f64,
            None => BIG,
        };
        let ratio = match self.from {
            StaggerFrom::Start | StaggerFrom::Random(_) => Some((0.0, 0.0)),
            StaggerFrom::Center | StaggerFrom::Edges => Some((0.5, 0.5)),
            StaggerFrom::End => Some((1.0, 1.0)),
            StaggerFrom::Ratio(x, y) => Some((x, y)),
            StaggerFrom::Index(_) => None,
        };
        let k = match self.from {
            StaggerFrom::Index(k) => k as f64,
            _ => 0.0,
        };
        let ox = match ratio {
            Some((rx, _)) => wrap.min(n) * rx - 0.5,
            None => k % wrap,
        };
        let oy = match (self.grid, ratio) {
            (None, _) => 0.0,
            (Some(_), Some((_, ry))) => n * ry / wrap - 0.5,
            (Some(_), None) => (k / wrap).floor(),
        };
        Layout { wrap, ox, oy }
    }

    /// The distance of target `j` from the origin (GSAP uses `sqrt(x² + y²)`).
    #[inline]
    fn distance(&self, lay: &Layout, j: u32) -> f64 {
        let j = j as f64;
        let x = j % lay.wrap - lay.ox;
        let y = lay.oy - (j / lay.wrap).floor();
        match self.axis {
            None => (x * x + y * y).sqrt(),
            Some(StaggerAxis::X) => x.abs(),
            Some(StaggerAxis::Y) => y.abs(),
        }
    }

    /// GSAP's span `v` (negated for `edges`), the base `b` it is added to, and
    /// the ease (to be inverted when the span is negative).
    fn span(&self, n: u32, wrap: f64) -> (f64, f64, Option<Easing>, bool) {
        let nf = n as f64;
        let mut v = match self.spread {
            Spread::Amount(a) => a,
            _ => {
                let lines = if wrap > nf {
                    nf - 1.0
                } else {
                    match self.axis {
                        None => wrap.max(nf / wrap),
                        Some(StaggerAxis::Y) => nf / wrap,
                        Some(StaggerAxis::X) => wrap,
                    }
                };
                self.step(n) * lines
            }
        };
        if self.from == StaggerFrom::Edges {
            v = -v;
        }
        let b = if v < 0.0 { self.base - v } else { self.base };
        (b, v, self.ease, v < 0.0)
    }
}

/// Target order for the 1-D fast path.
#[derive(Clone, Copy)]
enum Order {
    Forward,
    Backward,
}

impl Order {
    #[inline]
    fn order(self, j: u32, n: u32) -> u32 {
        match self {
            Order::Forward => j,
            Order::Backward => n.saturating_sub(1).saturating_sub(j),
        }
    }
}

/// The swap partner of index `k` in the seeded Fisher-Yates shuffle.
#[inline]
fn shuffle_pick(seed: u64, k: u32) -> u32 {
    let r = splitmix64(seed.wrapping_add((k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)));
    (r % (k as u64 + 1)) as u32
}

/// `round7(b + ease(normalised distance) * v)`, GSAP `distribute`'s last line
/// (a zero range normalises to 0; the ease is inverted, `1 - e(1 - u)`, when
/// the span is negative).
#[inline]
fn finish(d: f64, min: f64, range: f64, b: f64, v: f64, ease: Option<Easing>, invert: bool) -> f64 {
    let u = if range > 0.0 { (d - min) / range } else { 0.0 };
    let e = match ease {
        None => u,
        Some(e) if invert => 1.0 - e.map(1.0 - u),
        Some(e) => e.map(u),
    };
    round7(b + e * v)
}
