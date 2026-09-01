//! Constrained spring-and-rod horizontal spacing.
//!
//! The spacing problem has two inputs that must not be conflated: semantic
//! time (how much distance a rhythmic interval would naturally receive) and
//! ink geometry (how little distance is physically possible). Each onset
//! column therefore carries a *spring* — a natural length derived from its
//! duration, with separate stretch and compression flexibilities — and a
//! *rod* — a hard minimum from measured ink extents.
//!
//! Under a system-wide force `F` a column's width is
//!
//! ```text
//! x_i(F) = max(m_i, l_i + F * c_i_stretch)    F >= 0
//! x_i(F) = max(m_i, l_i + F * c_i_shrink)     F <  0
//! ```
//!
//! and justification solves `sum_i x_i(F) = target`. The `max` clamps make
//! the total width a *monotone piecewise-linear* function of `F`, so this is
//! a root-find over breakpoints, not a plain linear solve. [`solve_spacing`]
//! finds the exact root by sorting the clamp-release breakpoints and walking
//! segments — O(n log n), deterministic, no iteration-count tolerance.
//!
//! After the force solve, [`regularize_equal_durations`] runs a small
//! bounded quadratic projection that pulls the whitespace of rhythmically
//! equivalent neighbour columns together — the classic fix for polyphony
//! giving nominally equal notes unequal gaps — without ever violating rods,
//! changing the total width, or moving any column more than a style cap.

use crate::sp::Sp;
use crate::style::SpacingStyle;

/// Duration-to-space quanta curve.
///
/// `duration` is a fraction of a whole note (0.25 = quarter). Above the
/// reference duration `d0` the curve is logarithmic — each doubling adds one
/// quantum, so long notes gain constant extra white space instead of
/// proportional space. Below `d0` it is linear, which keeps very short
/// values from collapsing toward zero or negative widths:
///
/// ```text
/// q(d) = S + log2(d / d0)     d >= d0
/// q(d) = S + d / d0 - 1       0 < d < d0
/// ```
///
/// The two branches meet with value `S` at `d0` (the linear branch is the
/// tangent-free continuation that stays positive down to `d = 0`).
pub fn duration_quanta(duration: f64, style: &SpacingStyle) -> f64 {
    let d0 = style.reference_duration;
    let s = style.shortest_duration_space;
    if duration >= d0 {
        s + (duration / d0).log2()
    } else {
        s + duration / d0 - 1.0
    }
}

/// Natural (unforced) width for a column: collision headroom plus the
/// duration term `increment * q(d)`.
pub fn natural_length(duration: f64, headroom: Sp, style: &SpacingStyle) -> Sp {
    headroom + style.spacing_increment * duration_quanta(duration, style)
}

/// Stem direction of the ink dominating a column, for optical corrections.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StemDir {
    /// Stem points up (attached right of the head, rising).
    Up,
    /// Stem points down (attached left of the head, falling).
    Down,
}

/// Optical spacing correction between two adjacent columns, from their stem
/// directions and the vertical offset between their noteheads.
///
/// Opposing stems change how wide a gap *looks*: up-then-down leans the ink
/// apart (add space), down-then-up leans it together (remove space). For
/// same-direction stems a small correction proportional to relative head
/// height applies, capped at `optical_same_max`. The result is clamped to
/// `optical_clamp` and must be added to the *natural* length only — never to
/// the rod minimum, so legality is unaffected. Callers should skip the
/// correction when a flag or right-side accidental dominates the silhouette.
///
/// `head_delta` is the second head's y minus the first's, in staff spaces
/// (positive = second head lower).
pub fn optical_correction(
    first: StemDir,
    second: StemDir,
    head_delta: Sp,
    style: &SpacingStyle,
) -> Sp {
    let raw = match (first, second) {
        (StemDir::Up, StemDir::Down) => style.optical_up_down,
        (StemDir::Down, StemDir::Up) => style.optical_down_up,
        (StemDir::Up, StemDir::Up) | (StemDir::Down, StemDir::Down) => {
            // Same direction: lean follows relative head height, gently.
            let capped = head_delta.clamp(-style.optical_same_max, style.optical_same_max);
            capped * 0.5
        }
    };
    raw.clamp(-style.optical_clamp, style.optical_clamp)
}

/// One onset column of the spring-and-rod chain.
#[derive(Clone, PartialEq, Debug)]
pub struct SpacingColumn {
    /// Spring natural length `l`: the width this column wants at zero force.
    pub natural: Sp,
    /// Rod minimum `m`: the hard floor from measured ink silhouettes.
    pub minimum: Sp,
    /// Stretch flexibility `c+` (width gained per unit positive force).
    pub stretch_flex: f64,
    /// Shrink flexibility `c-` (width lost per unit negative force).
    pub shrink_flex: f64,
    /// Collision-only headroom `h`: the part of the natural length that
    /// exists to clear ink rather than to express duration. The
    /// regularizer compares `x - h` (pure whitespace) between columns.
    pub headroom: Sp,
    /// Equivalence key for the equal-duration regularizer. Columns with the
    /// same class in a caller-supplied pair are pulled toward equal
    /// whitespace. `None` opts out.
    pub duration_class: Option<u32>,
}

impl SpacingColumn {
    /// Build a column from a duration, measured headroom and rod minimum,
    /// with the default flexibilities: stretch flexibility is the natural
    /// length itself (longer notes absorb more stretch), shrink flexibility
    /// is the compressible surplus `natural - minimum`; both floored by
    /// style minima.
    pub fn from_duration(
        duration: f64,
        headroom: Sp,
        minimum: Sp,
        style: &SpacingStyle,
    ) -> SpacingColumn {
        let natural = natural_length(duration, headroom, style);
        let stretch_flex = natural.0.max(style.min_stretch_flex);
        let shrink_flex = (natural.0 - minimum.0).max(style.min_shrink_flex);
        // Duration classes quantize the fraction-of-whole-note exactly:
        // 1/1024 resolution covers every practical notated value.
        let class = (duration * 1024.0).round() as u32;
        SpacingColumn {
            natural,
            minimum,
            stretch_flex,
            shrink_flex,
            headroom,
            duration_class: Some(class),
        }
    }

    /// Width of this column under force `f`.
    pub fn width_at(&self, f: f64) -> Sp {
        let flex = if f >= 0.0 { self.stretch_flex } else { self.shrink_flex };
        Sp(self.minimum.0.max(self.natural.0 + f * flex))
    }

    /// The smallest width any force can reach: the rod if the column can
    /// shrink at all, otherwise its zero-force width.
    fn floor(&self) -> f64 {
        if self.shrink_flex > 0.0 {
            self.minimum.0
        } else {
            self.minimum.0.max(self.natural.0)
        }
    }
}

/// How well the solved widths meet the target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpacingFit {
    /// The target width was met exactly.
    Exact,
    /// The target is below the reachable minimum; widths sit at their
    /// floors and the line is overfull.
    Overfull,
    /// The target is above the reachable maximum (no stretch flexibility
    /// anywhere); widths sit at their ceiling and the line is underfull.
    Underfull,
}

/// Result of a spacing solve.
#[derive(Clone, PartialEq, Debug)]
pub struct SpacingSolution {
    /// The force that produced these widths (clamped at the last breakpoint
    /// when the target is unreachable).
    pub force: f64,
    /// Solved column widths, in column order.
    pub widths: Vec<Sp>,
    /// Total width at zero force, `sum max(m, l)`.
    pub natural_width: Sp,
    /// The smallest reachable total width.
    pub minimum_width: Sp,
    /// Whether the target was met.
    pub fit: SpacingFit,
}

impl SpacingSolution {
    /// Sum of the solved widths.
    pub fn total(&self) -> Sp {
        self.widths.iter().copied().sum()
    }
}

/// Solve the monotone piecewise-linear equation `sum_i x_i(F) = target`.
///
/// Exact algorithm: collect the breakpoints where a column's rod clamp
/// engages or releases — `(m - l) / c_stretch` on the stretch side for
/// columns whose rod exceeds their natural length, `(m - l) / c_shrink` on
/// the shrink side for columns with compressible surplus — sort them, and
/// walk linear segments from `F = 0` toward the target, updating the active
/// slope at each breakpoint. O(n log n), deterministic (total-order sort
/// with index tie-break), and immune to the convergence-tolerance issues a
/// bisection would have on flat segments.
pub fn solve_spacing(columns: &[SpacingColumn], target: Sp) -> SpacingSolution {
    let n = columns.len();
    let natural_width = Sp(columns.iter().map(|c| c.minimum.0.max(c.natural.0)).sum());
    let minimum_width = Sp(columns.iter().map(|c| c.floor()).sum());
    if n == 0 {
        return SpacingSolution {
            force: 0.0,
            widths: Vec::new(),
            natural_width,
            minimum_width,
            fit: if target.0.abs() < 1e-12 { SpacingFit::Exact } else { SpacingFit::Underfull },
        };
    }

    let finish = |force: f64, fit: SpacingFit| SpacingSolution {
        force,
        widths: columns.iter().map(|c| c.width_at(force)).collect(),
        natural_width,
        minimum_width,
        fit,
    };

    if (target.0 - natural_width.0).abs() < 1e-12 {
        return finish(0.0, SpacingFit::Exact);
    }

    if target.0 > natural_width.0 {
        // Stretch side. Columns with l >= m are active from F = 0; columns
        // whose rod exceeds their natural length join at (m - l) / c+.
        let mut slope: f64 = columns
            .iter()
            .filter(|c| c.natural.0 >= c.minimum.0)
            .map(|c| c.stretch_flex)
            .sum();
        let mut events: Vec<(f64, f64, usize)> = columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.minimum.0 > c.natural.0 && c.stretch_flex > 0.0)
            .map(|(i, c)| ((c.minimum.0 - c.natural.0) / c.stretch_flex, c.stretch_flex, i))
            .collect();
        events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
        let mut f = 0.0;
        let mut x = natural_width.0;
        for (fe, flex, _) in events {
            if slope > 0.0 && x + slope * (fe - f) >= target.0 {
                return finish(f + (target.0 - x) / slope, SpacingFit::Exact);
            }
            x += slope * (fe - f);
            f = fe;
            slope += flex;
        }
        if slope > 0.0 {
            return finish(f + (target.0 - x) / slope, SpacingFit::Exact);
        }
        finish(f, SpacingFit::Underfull)
    } else {
        // Shrink side. Columns with l > m compress from F = 0 until their
        // rod engages at (m - l) / c- (negative); the rest are already at
        // their floor for all F < 0.
        let mut slope: f64 = columns
            .iter()
            .filter(|c| c.natural.0 > c.minimum.0)
            .map(|c| c.shrink_flex)
            .sum();
        let mut events: Vec<(f64, f64, usize)> = columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.natural.0 > c.minimum.0 && c.shrink_flex > 0.0)
            .map(|(i, c)| ((c.minimum.0 - c.natural.0) / c.shrink_flex, c.shrink_flex, i))
            .collect();
        // Walk downward: most-recent (largest, least negative) first.
        events.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.2.cmp(&b.2)));
        let mut f = 0.0;
        let mut x = natural_width.0;
        for (fe, flex, _) in events {
            if slope > 0.0 && x - slope * (f - fe) <= target.0 {
                return finish(f - (x - target.0) / slope, SpacingFit::Exact);
            }
            x -= slope * (f - fe);
            f = fe;
            slope -= flex;
        }
        // All shrinkable columns clamped: at the floor.
        finish(f, SpacingFit::Overfull)
    }
}

/// Equal-duration whitespace regularizer.
///
/// After the force solve, rhythmically equivalent neighbour columns can
/// still show unequal *whitespace* (`x - headroom`) whenever a rod clamped
/// one of them — the well-known polyphonic irregularity. This projects the
/// solved widths onto a visually regular configuration by minimizing
///
/// ```text
/// sum_i (x_i - x_i_solved)^2
///   + lambda * sum_(i,j in pairs) ((x_i - h_i) - (x_j - h_j))^2
/// ```
///
/// subject to `x_i >= m_i`, `|x_i - x_i_solved| <= cap`, and
/// `sum x_i` unchanged. Callers supply `pairs` only between columns in
/// comparable contexts (same notated duration, same beam/voice
/// neighbourhood, no barline/explicit-spacing boundary between them);
/// [`SpacingColumn::duration_class`] exists to build such pairs.
///
/// Solved by projected gradient descent with an exact projection onto the
/// box-plus-sum constraint set (water-filling by bisection on the dual
/// variable). Fixed iteration counts, fixed order: deterministic.
pub fn regularize_equal_durations(
    solution: &mut SpacingSolution,
    columns: &[SpacingColumn],
    pairs: &[(usize, usize)],
    style: &SpacingStyle,
) {
    let n = solution.widths.len();
    assert_eq!(n, columns.len());
    if n == 0 || pairs.is_empty() {
        return;
    }
    let lambda = style.regularize_lambda;
    let cap = style.regularize_cap.0;
    let xhat: Vec<f64> = solution.widths.iter().map(|w| w.0).collect();
    let total: f64 = xhat.iter().sum();
    let lo: Vec<f64> = (0..n).map(|i| columns[i].minimum.0.max(xhat[i] - cap)).collect();
    let hi: Vec<f64> = (0..n).map(|i| xhat[i] + cap).collect();
    let h: Vec<f64> = columns.iter().map(|c| c.headroom.0).collect();

    let mut degree = vec![0usize; n];
    for &(a, b) in pairs {
        assert!(a < n && b < n && a != b, "regularizer pair out of range");
        degree[a] += 1;
        degree[b] += 1;
    }
    let max_degree = degree.iter().copied().max().unwrap_or(0) as f64;
    // Lipschitz bound of the gradient: Hessian is 2I + 2*lambda*Laplacian,
    // and the graph Laplacian norm is at most twice the max degree.
    let lip = 2.0 + 4.0 * lambda * max_degree;
    let step = 1.0 / lip;

    let mut x = xhat.clone();
    let mut grad = vec![0.0f64; n];
    for _ in 0..300 {
        for i in 0..n {
            grad[i] = 2.0 * (x[i] - xhat[i]);
        }
        for &(a, b) in pairs {
            let d = (x[a] - h[a]) - (x[b] - h[b]);
            grad[a] += 2.0 * lambda * d;
            grad[b] -= 2.0 * lambda * d;
        }
        for i in 0..n {
            x[i] -= step * grad[i];
        }
        project_box_sum(&mut x, &lo, &hi, total);
    }
    for i in 0..n {
        solution.widths[i] = Sp(x[i]);
    }
}

/// Exact Euclidean projection of `v` onto `{x : lo <= x <= hi, sum x = total}`,
/// by bisection on the shared dual variable. The set is non-empty whenever
/// `sum lo <= total <= sum hi`, which holds by construction in the
/// regularizer (the solved widths themselves are feasible).
fn project_box_sum(v: &mut [f64], lo: &[f64], hi: &[f64], total: f64) {
    let n = v.len();
    let mut t_lo = f64::INFINITY;
    let mut t_hi = f64::NEG_INFINITY;
    for i in 0..n {
        t_lo = t_lo.min(v[i] - hi[i]);
        t_hi = t_hi.max(v[i] - lo[i]);
    }
    // f(t) = sum clamp(v - t, lo, hi) decreases from sum(hi) at t_lo to
    // sum(lo) at t_hi; bisect to the t giving `total`.
    let mut a = t_lo - 1.0;
    let mut b = t_hi + 1.0;
    for _ in 0..100 {
        let mid = 0.5 * (a + b);
        let s: f64 = (0..n).map(|i| (v[i] - mid).clamp(lo[i], hi[i])).sum();
        if s >= total {
            a = mid;
        } else {
            b = mid;
        }
    }
    let t = 0.5 * (a + b);
    for i in 0..n {
        v[i] = (v[i] - t).clamp(lo[i], hi[i]);
    }
}

#[cfg(test)]
mod spacing_tests {
    use super::*;
    use crate::testutil::Lcg;

    fn style() -> SpacingStyle {
        SpacingStyle::default()
    }

    #[test]
    fn duration_curve_hand_values() {
        let s = style();
        // Eighth note (the reference): exactly S quanta.
        assert!((duration_quanta(0.125, &s) - 2.0).abs() < 1e-12);
        // Quarter: one doubling above the reference -> S + 1.
        assert!((duration_quanta(0.25, &s) - 3.0).abs() < 1e-12);
        // Half: two doublings -> S + 2.
        assert!((duration_quanta(0.5, &s) - 4.0).abs() < 1e-12);
        // Whole: three doublings -> S + 3.
        assert!((duration_quanta(1.0, &s) - 5.0).abs() < 1e-12);
        // Sixteenth: linear branch, S + 0.5 - 1.
        assert!((duration_quanta(0.0625, &s) - 1.5).abs() < 1e-12);
        // Natural length adds headroom plus increment * quanta.
        let nl = natural_length(0.25, Sp(0.8), &s);
        assert!((nl.0 - (0.8 + 1.2 * 3.0)).abs() < 1e-12);
    }

    #[test]
    fn duration_curve_is_continuous_and_monotone() {
        let s = style();
        // Continuity at the reference duration.
        let below = duration_quanta(0.125 - 1e-9, &s);
        let above = duration_quanta(0.125 + 1e-9, &s);
        assert!((below - above).abs() < 1e-6);
        // Monotone over a sweep of durations.
        let mut last = f64::NEG_INFINITY;
        for i in 1..2000 {
            let d = i as f64 / 1000.0;
            let q = duration_quanta(d, &s);
            assert!(q > last, "duration curve not monotone at d = {}", d);
            last = q;
        }
        // Positive down to tiny durations (the linear branch's purpose).
        assert!(duration_quanta(1.0 / 1024.0, &s) > 1.0);
    }

    fn col(l: f64, m: f64, cp: f64, cm: f64) -> SpacingColumn {
        SpacingColumn {
            natural: Sp(l),
            minimum: Sp(m),
            stretch_flex: cp,
            shrink_flex: cm,
            headroom: Sp(0.0),
            duration_class: None,
        }
    }

    #[test]
    fn solve_hand_worked_stretch() {
        // l = [3,4], flex+ = [3,4]; target 10.5 needs 3.5 over natural 7 at
        // combined slope 7 -> F = 0.5 -> widths [4.5, 6.0].
        let cols = [col(3.0, 1.0, 3.0, 2.0), col(4.0, 1.0, 4.0, 3.0)];
        let sol = solve_spacing(&cols, Sp(10.5));
        assert_eq!(sol.fit, SpacingFit::Exact);
        assert!((sol.force - 0.5).abs() < 1e-12);
        assert!((sol.widths[0].0 - 4.5).abs() < 1e-12);
        assert!((sol.widths[1].0 - 6.0).abs() < 1e-12);
    }

    #[test]
    fn solve_hand_worked_shrink_no_clamp() {
        // l = [3,4], m = [1,1], flex- = [2,3]; target 6 needs -1 at slope 5
        // -> F = -0.2 -> widths [2.6, 3.4].
        let cols = [col(3.0, 1.0, 3.0, 2.0), col(4.0, 1.0, 4.0, 3.0)];
        let sol = solve_spacing(&cols, Sp(6.0));
        assert_eq!(sol.fit, SpacingFit::Exact);
        assert!((sol.force - -0.2).abs() < 1e-12);
        assert!((sol.widths[0].0 - 2.6).abs() < 1e-12);
        assert!((sol.widths[1].0 - 3.4).abs() < 1e-12);
    }

    #[test]
    fn solve_hand_worked_shrink_with_rod_clamp() {
        // Column 0 has a tall rod (m = 2.9 under l = 3.0, flex- = 0.1): it
        // clamps at F = -1. Before the clamp the slope is 3.1.
        let cols = [col(3.0, 2.9, 3.0, 0.1), col(4.0, 0.5, 4.0, 3.0)];
        // Target 5.0: root before the clamp. F = -2/3.1.
        let sol = solve_spacing(&cols, Sp(5.0));
        assert_eq!(sol.fit, SpacingFit::Exact);
        assert!((sol.force - (-2.0 / 3.1)).abs() < 1e-12);
        assert!((sol.total().0 - 5.0).abs() < 1e-12);
        assert!(sol.widths[0].0 >= 2.9 - 1e-12);
        // Target 3.5: past the clamp; column 0 pinned at its rod, column 1
        // alone carries the remaining shrink at slope 3.
        let sol = solve_spacing(&cols, Sp(3.5));
        assert_eq!(sol.fit, SpacingFit::Exact);
        assert!((sol.widths[0].0 - 2.9).abs() < 1e-12);
        assert!((sol.widths[1].0 - 0.6).abs() < 1e-12);
        assert!((sol.force - (-1.0 - 0.4 / 3.0)).abs() < 1e-12);
        // Target 3.5 with column 1's rod raised to 1.0: both rods clamp
        // (floor = 3.9), so 3.5 is genuinely overfull.
        let cols = [col(3.0, 2.9, 3.0, 0.1), col(4.0, 1.0, 4.0, 3.0)];
        let sol = solve_spacing(&cols, Sp(3.5));
        assert_eq!(sol.fit, SpacingFit::Overfull);
        assert!((sol.total().0 - 3.9).abs() < 1e-12);
    }

    #[test]
    fn solve_overfull_and_underfull() {
        let cols = [col(3.0, 2.0, 0.0, 1.0), col(4.0, 3.0, 0.0, 1.0)];
        // No stretch flexibility anywhere: target above natural is
        // underfull, widths stay at natural.
        let sol = solve_spacing(&cols, Sp(12.0));
        assert_eq!(sol.fit, SpacingFit::Underfull);
        assert!((sol.total().0 - 7.0).abs() < 1e-12);
        // Below the rod floor: overfull, widths sit exactly on the rods.
        let sol = solve_spacing(&cols, Sp(4.0));
        assert_eq!(sol.fit, SpacingFit::Overfull);
        assert!((sol.widths[0].0 - 2.0).abs() < 1e-12);
        assert!((sol.widths[1].0 - 3.0).abs() < 1e-12);
    }

    /// The monotonicity claim behind the exact solver: total width is a
    /// non-decreasing function of force, across every clamp configuration.
    #[test]
    fn total_width_is_monotone_in_force() {
        let mut rng = Lcg::new(0x5eed_0001);
        for _case in 0..200 {
            let n = 1 + (rng.next_u64() % 12) as usize;
            let cols: Vec<SpacingColumn> = (0..n)
                .map(|_| {
                    let l = 0.5 + 4.5 * rng.next_f64();
                    // Rods sometimes above the natural length (dense ink).
                    let m = (0.2 + rng.next_f64() * 1.3) * l;
                    let cp = if rng.next_f64() < 0.15 { 0.0 } else { 3.0 * rng.next_f64() };
                    let cm = if rng.next_f64() < 0.15 { 0.0 } else { 2.0 * rng.next_f64() };
                    col(l, m, cp, cm)
                })
                .collect();
            let mut last = f64::NEG_INFINITY;
            for step in -60..=60 {
                let f = step as f64 * 0.1;
                let total: f64 = cols.iter().map(|c| c.width_at(f).0).sum();
                assert!(
                    total >= last - 1e-9,
                    "width sum decreased between forces (case with {} cols)",
                    n
                );
                last = total;
                // Rods always honoured.
                for c in &cols {
                    assert!(c.width_at(f).0 >= c.minimum.0 - 1e-12);
                }
            }
        }
    }

    /// Round-trip property: for any reachable target the solver hits it
    /// exactly; widths honour rods; and re-solving the achieved width is
    /// stable.
    #[test]
    fn solve_round_trip_on_random_instances() {
        let mut rng = Lcg::new(0x5eed_0002);
        for _case in 0..300 {
            let n = 1 + (rng.next_u64() % 10) as usize;
            let cols: Vec<SpacingColumn> = (0..n)
                .map(|_| {
                    let l = 0.5 + 4.5 * rng.next_f64();
                    let m = (0.2 + rng.next_f64() * 1.3) * l;
                    let cp = if rng.next_f64() < 0.1 { 0.0 } else { 3.0 * rng.next_f64() };
                    let cm = if rng.next_f64() < 0.1 { 0.0 } else { 2.0 * rng.next_f64() };
                    col(l, m, cp, cm)
                })
                .collect();
            let floor: f64 = cols
                .iter()
                .map(|c| if c.shrink_flex > 0.0 { c.minimum.0 } else { c.minimum.0.max(c.natural.0) })
                .sum();
            let natural: f64 = cols.iter().map(|c| c.minimum.0.max(c.natural.0)).sum();
            let has_stretch = cols.iter().any(|c| c.stretch_flex > 0.0);
            let hi = if has_stretch { natural * 2.0 + 5.0 } else { natural };
            let target = floor + (hi - floor) * rng.next_f64();
            let sol = solve_spacing(&cols, Sp(target));
            assert_eq!(sol.fit, SpacingFit::Exact, "reachable target not met");
            assert!((sol.total().0 - target).abs() < 1e-9 * (1.0 + target.abs()));
            for (c, w) in cols.iter().zip(&sol.widths) {
                assert!(w.0 >= c.minimum.0 - 1e-9);
            }
            // Determinism: identical input, bit-identical output.
            let sol2 = solve_spacing(&cols, Sp(target));
            assert_eq!(sol.force.to_bits(), sol2.force.to_bits());
            for (a, b) in sol.widths.iter().zip(&sol2.widths) {
                assert_eq!(a.0.to_bits(), b.0.to_bits());
            }
        }
    }

    #[test]
    fn regularizer_hand_worked_unconstrained() {
        // min (x1-3)^2 + (x2-4)^2 + 2((x1)-(x2))^2 with x1+x2 = 7 has its
        // optimum at x1 = 3.4, x2 = 3.6 (stationary point by substitution).
        let mut style = style();
        style.regularize_cap = Sp(10.0); // cap out of the way
        let cols = [col(3.0, 0.5, 3.0, 2.0), col(4.0, 0.5, 4.0, 3.0)];
        let mut sol = SpacingSolution {
            force: 0.0,
            widths: vec![Sp(3.0), Sp(4.0)],
            natural_width: Sp(7.0),
            minimum_width: Sp(1.0),
            fit: SpacingFit::Exact,
        };
        regularize_equal_durations(&mut sol, &cols, &[(0, 1)], &style);
        assert!((sol.widths[0].0 - 3.4).abs() < 1e-6, "got {}", sol.widths[0].0);
        assert!((sol.widths[1].0 - 3.6).abs() < 1e-6, "got {}", sol.widths[1].0);
        assert!((sol.total().0 - 7.0).abs() < 1e-9);
    }

    #[test]
    fn regularizer_respects_cap_minimum_and_total() {
        // Same pull as above but the default 0.25sp cap binds: each column
        // moves exactly 0.25 toward the other.
        let s = style();
        let cols = [col(3.0, 0.5, 3.0, 2.0), col(4.0, 0.5, 4.0, 3.0)];
        let mut sol = SpacingSolution {
            force: 0.0,
            widths: vec![Sp(3.0), Sp(4.0)],
            natural_width: Sp(7.0),
            minimum_width: Sp(1.0),
            fit: SpacingFit::Exact,
        };
        regularize_equal_durations(&mut sol, &cols, &[(0, 1)], &s);
        assert!((sol.widths[0].0 - 3.25).abs() < 1e-6);
        assert!((sol.widths[1].0 - 3.75).abs() < 1e-6);
        assert!((sol.total().0 - 7.0).abs() < 1e-9);

        // A binding rod stops the regularizer short of the cap.
        let cols = [col(3.0, 0.5, 3.0, 2.0), col(4.0, 3.9, 4.0, 3.0)];
        let mut sol = SpacingSolution {
            force: 0.0,
            widths: vec![Sp(3.0), Sp(4.0)],
            natural_width: Sp(7.0),
            minimum_width: Sp(4.4),
            fit: SpacingFit::Exact,
        };
        regularize_equal_durations(&mut sol, &cols, &[(0, 1)], &s);
        assert!(sol.widths[1].0 >= 3.9 - 1e-9);
        assert!((sol.total().0 - 7.0).abs() < 1e-9);
    }

    /// The polyphonic-regularity property: after rods distort a chain of
    /// equal-duration columns, regularization reduces the pairwise
    /// whitespace spread without changing the total or violating rods.
    /// (Provable: gradient projection descends the objective, whose value
    /// at the start is exactly `lambda * spread`, so the final spread can
    /// never exceed the initial one.)
    #[test]
    fn regularizer_reduces_whitespace_variance() {
        let mut rng = Lcg::new(0x5eed_0003);
        let s = style();
        for _case in 0..100 {
            let n = 4 + (rng.next_u64() % 6) as usize;
            let cols: Vec<SpacingColumn> = (0..n)
                .map(|_| {
                    let h = rng.next_f64() * 1.5;
                    let l = 2.4 + h;
                    // Occasional oversized rod distorts the force solution.
                    let m = if rng.next_f64() < 0.4 { l + rng.next_f64() } else { l * 0.5 };
                    SpacingColumn {
                        natural: Sp(l),
                        minimum: Sp(m),
                        stretch_flex: l,
                        shrink_flex: (l - m).max(0.05),
                        headroom: Sp(h),
                        duration_class: Some(8),
                    }
                })
                .collect();
            let natural: f64 = cols.iter().map(|c| c.minimum.0.max(c.natural.0)).sum();
            let target = natural * 1.1;
            let mut sol = solve_spacing(&cols, Sp(target));
            let pairs: Vec<(usize, usize)> = (0..n - 1).map(|i| (i, i + 1)).collect();
            let spread = |w: &[Sp]| {
                let ws: Vec<f64> = w.iter().zip(&cols).map(|(x, c)| x.0 - c.headroom.0).collect();
                pairs.iter().map(|&(a, b)| (ws[a] - ws[b]) * (ws[a] - ws[b])).sum::<f64>()
            };
            let before = spread(&sol.widths);
            regularize_equal_durations(&mut sol, &cols, &pairs, &s);
            let after = spread(&sol.widths);
            assert!(after <= before + 1e-9, "regularizer increased whitespace spread");
            assert!((sol.total().0 - target).abs() < 1e-6);
            for (c, w) in cols.iter().zip(&sol.widths) {
                assert!(w.0 >= c.minimum.0 - 1e-9);
            }
        }
    }

    #[test]
    fn optical_corrections_clamp() {
        let s = style();
        assert_eq!(optical_correction(StemDir::Up, StemDir::Down, Sp(0.0), &s), Sp(0.15));
        assert_eq!(optical_correction(StemDir::Down, StemDir::Up, Sp(0.0), &s), Sp(-0.10));
        let same = optical_correction(StemDir::Up, StemDir::Up, Sp(5.0), &s);
        assert!(same.0.abs() <= s.optical_same_max.0 + 1e-12);
        // The total is always inside the clamp.
        for delta in [-9.0, -0.1, 0.0, 0.1, 9.0] {
            for (a, b) in [
                (StemDir::Up, StemDir::Down),
                (StemDir::Down, StemDir::Up),
                (StemDir::Up, StemDir::Up),
                (StemDir::Down, StemDir::Down),
            ] {
                let c = optical_correction(a, b, Sp(delta), &s);
                assert!(c.0.abs() <= s.optical_clamp.0 + 1e-12);
            }
        }
    }
}
