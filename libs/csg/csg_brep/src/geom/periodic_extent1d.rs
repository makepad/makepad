//! Intervals on a periodic 1D domain `[0, period]` (closed-curve parameters, angles):
//! seam-crossing intervals, full-period and empty extents, containment, disjointness
//! and interval growth across the seam.
//!
//! Upstream: `source/SMLib/inc/SmPeriodicExtent1d.h`, `source/SMLib/src/SmPeriodicExtent1d.cpp`.
//!
//! | Rust                                     | Upstream                                         |
//! |------------------------------------------|--------------------------------------------------|
//! | [`PeriodicExtent1d`]                     | `SmPeriodicExtent1d`                             |
//! | [`PeriodicExtent1d::full`]               | `SmPeriodicExtent1d(double dPeriod)`             |
//! | `Clone`/`Copy`, drop                     | copy constructor, `~SmPeriodicExtent1d`          |
//! | [`PeriodicExtent1d::from_parts`]         | direct member initialisation (no upstream fn)    |
//! | [`PeriodicExtent1d::min`], [`PeriodicExtent1d::max`] | `GetMin`, `GetMax`                   |
//! | [`PeriodicExtent1d::period`]             | `GetPeriod`                                      |
//! | [`PeriodicExtent1d::set_full_period`]    | `SetFullPeriod`                                  |
//! | [`PeriodicExtent1d::is_empty`]           | `IsEmpty`                                        |
//! | [`PeriodicExtent1d::is_full_period`]     | `IsFullPeriod`, `GetFullPeriod`                  |
//! | [`PeriodicExtent1d::crosses_seam`]       | `CrossesSeam`                                    |
//! | [`PeriodicExtent1d::map_value`]          | `MapValue`                                       |
//! | [`PeriodicExtent1d::normalize_value`]    | `NormalizeValue`                                 |
//! | [`PeriodicExtent1d::complement`]         | `Compliment`                                     |
//! | [`PeriodicExtent1d::add_interval`]       | `AddInterval`                                    |
//! | [`PeriodicExtent1d::contains_value`]     | `ContainsValue`                                  |
//! | [`PeriodicExtent1d::set_min_max`]        | `SetMinMax`                                      |
//! | [`PeriodicExtent1d::is_contained_by`]    | `IsContainedBy`                                  |
//! | [`PeriodicExtent1d::are_disjoint`]       | `AreDisjoint`                                    |
//! | [`PeriodicExtent1d::mid`]                | `GetMid`                                         |
//! | `is_contained` (private)                 | `SM_IS_CONTAINED` macro (`SmMath.h`)             |
//!
//! Deviations:
//! - `SmStatus` returns become `Result<()>`: `SM_ERR` is
//!   `SolidError::Upstream { code: 1001 }`, `SM_ERR_INVALID_INPUT` is
//!   [`SolidError::InvalidInput`]. [`PeriodicExtent1d::add_interval`] leaves the extent
//!   unchanged on error (as upstream). [`PeriodicExtent1d::set_min_max`] updates the
//!   extent even when it returns `InvalidInput`, exactly as upstream does.
//! - `SetMinMax`'s optional `double*` period is an `Option<f64>`.
//! - `SM_ASSERT*` / `SM_ASSERT_VALID_NO_STREAM` checks and `SE(...)` logging are
//!   compiled out upstream (release build) and omitted.
//! - The upstream `m_bFullPeriod` is an `int`; every assignment in this unit stores 0
//!   or 1, so it is a `bool` here.
//! - Only the functions owned by unit `geom.periodic_extent1d` are ported; the rest of
//!   the class (`Invert`, `AddValue`, `Union`, `Intersect`, `Subtract`, the 3-argument
//!   constructor, ...) is unused upstream.

use core::cmp::Ordering;

use crate::error::{Result, SolidError};
use crate::geom::tol::{self, ScaledZero};
use crate::math::BIG_DOUBLE;

/// Upstream `SM_ERR` (generic error status).
const SM_ERR: i32 = 1001;

/// An interval on the periodic domain `[0, period]` (`SmPeriodicExtent1d`).
///
/// - `min < max`: the interval `[min, max]`, not containing the seam.
/// - `min > max`: crosses the seam, `[min, period]` plus `[0, max]`.
/// - `min == max`: a single point.
/// - `min == 0`, `max == period`: the full period when [`Self::is_full_period`],
///   otherwise a point on the seam.
/// - Empty: `min == BIG_DOUBLE`, `max == -BIG_DOUBLE`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeriodicExtent1d {
    min: f64,
    max: f64,
    period: f64,
    full_period: bool,
}

/// Which of the extent and the added span cross the seam, in
/// [`PeriodicExtent1d::add_interval`] (upstream's case 2, only the extent crossing, is
/// swapped into [`SeamCase::SpanOnly`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeamCase {
    /// Case 0: neither crosses.
    Neither,
    /// Case 1: only the added span crosses.
    SpanOnly,
    /// Case 3: both cross.
    Both,
}

/// `SM_IS_CONTAINED(c, a, b)`: `c` inside or on `[a, b]` (either order), widened by
/// `SmTol::GetScaledZero(a, b)`.
fn is_contained(c: f64, a: f64, b: f64) -> bool {
    let tz = tol::scaled_zero2(&a, &b);
    ((a <= c + tz) && (c - tz <= b)) || ((b <= c + tz) && (c - tz <= a))
}

/// `SM_ERR` from `operation`.
fn sm_err(operation: &'static str) -> SolidError {
    SolidError::Upstream {
        operation,
        code: SM_ERR,
        entity: None,
    }
}

impl PeriodicExtent1d {
    /// The extent covering the whole period `[0, period]`
    /// (`SmPeriodicExtent1d(double dPeriod)`).
    #[must_use]
    pub fn full(period: f64) -> Self {
        Self {
            min: 0.0,
            max: period,
            period,
            full_period: true,
        }
    }

    /// An extent with exactly these members, unchecked (upstream code and its friend
    /// tests assign `m_dMin`, `m_dMax`, `m_dPeriod`, `m_bFullPeriod` directly).
    #[must_use]
    pub fn from_parts(min: f64, max: f64, period: f64, full_period: bool) -> Self {
        Self {
            min,
            max,
            period,
            full_period,
        }
    }

    /// Interval start (`GetMin`).
    #[must_use]
    pub fn min(&self) -> f64 {
        self.min
    }

    /// Interval end (`GetMax`).
    #[must_use]
    pub fn max(&self) -> f64 {
        self.max
    }

    /// The period (`GetPeriod`).
    #[must_use]
    pub fn period(&self) -> f64 {
        self.period
    }

    /// Sets the full-period flag when `full_period` is true; `false` is ignored
    /// (`SetFullPeriod`).
    pub fn set_full_period(&mut self, full_period: bool) {
        if full_period {
            self.full_period = true;
        }
    }

    /// True for the empty extent `[BIG_DOUBLE, -BIG_DOUBLE]` (`IsEmpty`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.min == BIG_DOUBLE && self.max == -BIG_DOUBLE
    }

    /// True when the extent covers the whole period (`IsFullPeriod`).
    #[must_use]
    pub fn is_full_period(&self) -> bool {
        self.full_period
    }

    /// True when the interval crosses the seam, `max < min` (`CrossesSeam`).
    #[must_use]
    pub fn crosses_seam(&self) -> bool {
        self.max < self.min
    }

    /// Maps `location` into the primary period (`MapValue`); same as
    /// [`Self::normalize_value`].
    #[must_use]
    pub fn map_value(&self, location: f64) -> f64 {
        self.normalize_value(location)
    }

    /// Maps `value` into `[0, period]` by at most one period, snapping values within
    /// the period's scaled zero of `0` or `period` onto them; `0` for an empty extent
    /// (`NormalizeValue`).
    #[must_use]
    pub fn normalize_value(&self, value: f64) -> f64 {
        let sz = self.period.scaled_zero();

        // clamp value to [0 period]
        let value = if value.abs() < sz {
            0.0
        } else if (value - self.period).abs() < sz {
            self.period
        } else {
            value
        };

        // for empty sets - just return 0.0
        if self.is_empty() {
            return 0.0;
        }

        // map value to Ivl:[0 period] by one period
        if value < 0.0 {
            value + self.period
        } else if value > self.period {
            value - self.period
        } else {
            value
        }
    }

    /// Replaces the extent by its complement (`Compliment`): empty becomes full, full
    /// becomes empty, a point becomes full, otherwise `min` and `max` swap.
    pub fn complement(&mut self) {
        // empty set
        if self.is_empty() {
            self.set_full();
            return;
        }

        if self.full_period {
            // Given Full Period, Return empty set
            self.full_period = false;
            self.min = BIG_DOUBLE;
            self.max = -BIG_DOUBLE;
        } else if tol::in_tol(self.max - self.min, tol::tol3d(self.period.scaled_zero())) {
            // Given Point Interval, Return Full Interval
            self.set_full();
        } else {
            core::mem::swap(&mut self.min, &mut self.max);
        }
    }

    /// Extends the extent to include the span from `from` to `to`, moving forward
    /// (increasing, across the seam if needed) when `forward`, else backward
    /// (`AddInterval`).
    ///
    /// # Errors
    ///
    /// `SolidError::Upstream { code: 1001 }` (`SM_ERR`) when `from` or `to` is outside
    /// `[0, period]`, or when the span is disjoint from the extent (the union is not an
    /// interval). The extent is unchanged then.
    pub fn add_interval(&mut self, from: f64, to: f64, forward: bool) -> Result<()> {
        const OP: &str = "geom::periodic_extent1d::PeriodicExtent1d::add_interval";
        let (mut from, mut to) = (from, to);

        // check input
        if !is_contained(from, 0.0, self.period) || !is_contained(to, 0.0, self.period) {
            return Err(sm_err(OP));
        }

        // when forward is false, swap [from to] to make all cases forward
        if !forward {
            core::mem::swap(&mut to, &mut from);
        }

        let sz = self.period.scaled_zero();
        let mut swapped = false; // remember switching case 2 into case 1

        // check for Full Period intervals
        if self.is_full_period()
            || (tol::in_tol(from, tol::tol3d(sz)) && tol::in_tol(to - self.period, tol::tol3d(sz)))
        {
            self.set_full();
            return Ok(());
        }

        // empty sets
        if self.is_empty() {
            self.full_period = (self.period - (self.max - self.min).abs()) < sz;
            if self.full_period {
                self.min = 0.0;
                self.max = self.period;
            } else {
                self.min = from;
                self.max = to;
            }
            return Ok(());
        }

        // upstream case 0) min < max && from < to; 1) min < max && from > to;
        //               2) min > max && from < to; 3) min > max && from > to
        // `!(a <= b)`, i.e. also true for NaN, as upstream's `(a <= b) ? 0 : n`.
        let not_le = |a: f64, b: f64| matches!(a.partial_cmp(&b), Some(Ordering::Greater) | None);
        let case = match (not_le(self.min, self.max), not_le(from, to)) {
            (false, false) => SeamCase::Neither,
            (false, true) => SeamCase::SpanOnly,
            (true, false) => {
                // map case 2 into 1
                core::mem::swap(&mut self.min, &mut from);
                core::mem::swap(&mut self.max, &mut to);
                swapped = true;
                SeamCase::SpanOnly
            }
            (true, true) => SeamCase::Both,
        };

        match case {
            SeamCase::Neither => {
                // disjoint cases
                if (to < self.min - sz) || (from > self.max + sz) {
                    return Err(sm_err(OP));
                }
                if from < self.min {
                    self.min = from;
                }
                if to > self.max {
                    self.max = to;
                }
            }
            SeamCase::SpanOnly => {
                // disjoint case
                if (from > self.max + sz) && (to < self.min - sz) {
                    if swapped {
                        self.min = from;
                        self.max = to;
                    }
                    return Err(sm_err(OP));
                }
                // full case
                if (from < self.max + sz) && (to > self.min - sz) {
                    self.set_full();
                    return Ok(());
                }
                if to < self.min {
                    if from < self.min {
                        self.min = from;
                    }
                    self.max = to;
                } else {
                    self.min = from;
                    if to > self.max {
                        self.max = to;
                    }
                }
            }
            SeamCase::Both => {
                if from < self.min {
                    self.min = from;
                }
                if to > self.max {
                    self.max = to;
                }
                if self.min < self.max - sz {
                    // full case
                    self.set_full();
                    return Ok(());
                }
            }
        }

        Ok(())
    }

    /// `[0, period]` with the full-period flag set.
    fn set_full(&mut self) {
        self.min = 0.0;
        self.max = self.period;
        self.full_period = true;
    }

    /// True when `value` lies in the extent, ends included, to the period's scaled
    /// zero (`ContainsValue`).
    #[must_use]
    pub fn contains_value(&self, value: f64) -> bool {
        // low work - Full Period, all points are in
        if self.is_full_period() {
            return true;
        }

        // low work - empty set, no points are in
        if self.is_empty() {
            return false;
        }

        let sz = self.period.scaled_zero();

        if (self.min - self.max).abs() < sz {
            // Pt Ivl
            return ((self.min - value).abs() < sz) || ((self.max - value).abs() < sz);
        }
        if self.min < self.max {
            // no seam Ivl
            (value > self.min - sz) && (value < self.max + sz)
        } else {
            // cross seam Ivl
            (value > self.min + sz) || (value < self.max + sz)
        }
    }

    /// Sets the interval to `[min, max]` (crossing the seam when `max < min`), with
    /// a new period when `period` is given (`SetMinMax`). When `min` and `max` agree to
    /// the scaled zero the extent becomes the point `[min, max]` if `single_point`,
    /// else the full period.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] (`SM_ERR_INVALID_INPUT`) when `min` or `max` is
    /// outside `[0, period]`. As upstream, the extent is updated anyway.
    pub fn set_min_max(
        &mut self,
        min: f64,
        max: f64,
        period: Option<f64>,
        single_point: bool,
    ) -> Result<()> {
        // local
        let period = period.unwrap_or(self.period);

        // check input - bounds must be in range:[0 period]
        let bad = min < 0.0 || min > period || max < 0.0 || max > period;

        let sz = period.scaled_zero();

        self.period = period;

        if (max - min).abs() < sz {
            if single_point {
                // degen pt inv
                self.min = min;
                self.max = max;
                self.full_period = false;
            } else {
                // Full period inv
                self.set_full();
            }
        } else {
            self.min = min;
            self.max = max;
            self.full_period = (self.period - (self.max - self.min).abs()) < sz;
        }

        if bad {
            return Err(SolidError::InvalidInput {
                operation: "geom::periodic_extent1d::PeriodicExtent1d::set_min_max",
                reason: "min and max are not both within [0, period]",
            });
        }
        Ok(())
    }

    /// True when this extent lies inside `other` (equal extents contain each other;
    /// both should have the same period) (`IsContainedBy`).
    #[must_use]
    pub fn is_contained_by(&self, other: &Self) -> bool {
        let sz = self.period.scaled_zero();

        // low work - other is FullPeriod
        if other.is_full_period() {
            return true;
        }

        // low work - other is Empty
        if other.is_empty() {
            return false;
        }

        // when this Ivl Crosses Seam
        if self.crosses_seam() {
            // only a seam crossing Ivl can contain another seam crossing Ivl
            if !other.crosses_seam() {
                return false;
            }
            return (self.max < other.max + sz) && (self.min > other.min - sz);
        }

        if other.crosses_seam() {
            // both end points in one end or the other of the split other interval
            ((self.min < other.max + sz) && (self.max < other.max + sz))
                || ((self.min > other.min - sz) && (self.max > other.min - sz))
        } else {
            other.contains_value(self.min) && other.contains_value(self.max)
        }
    }

    /// True when the two extents do not meet; touching at a point is not disjoint
    /// (`AreDisjoint`).
    #[must_use]
    pub fn are_disjoint(&self, other: &Self) -> bool {
        // low work: this or other are FullPeriods
        if self.is_full_period() || other.is_full_period() {
            return false;
        }

        // low work: either interval is the NULL set
        if self.is_empty() || other.is_empty() {
            return true;
        }

        // low work: either interval contains a boundary of the other
        if other.contains_value(self.min)
            || other.contains_value(self.max)
            || self.contains_value(other.min)
            || self.contains_value(other.max)
        {
            return false;
        }

        // the ends of this are off the other: remains this embracing the other
        if other.is_contained_by(self) {
            return false;
        }

        true
    }

    /// Midpoint of the interval, accounting for the seam; `0` when empty (`GetMid`).
    #[must_use]
    pub fn mid(&self) -> f64 {
        // empty set
        if self.is_empty() {
            return 0.0;
        }

        let seam = if self.min > self.max {
            if (self.min + self.max) < self.period {
                self.period / 2.0 // in 2nd half of period
            } else {
                -self.period / 2.0 // in 1st half of period
            }
        } else {
            0.0
        };
        ((self.min + self.max) / 2.0) + seam
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pe(min: f64, max: f64) -> PeriodicExtent1d {
        PeriodicExtent1d::from_parts(min, max, 1.0, false)
    }

    fn empty(period: f64) -> PeriodicExtent1d {
        PeriodicExtent1d::from_parts(BIG_DOUBLE, -BIG_DOUBLE, period, false)
    }

    #[test]
    fn full_and_predicates() {
        let f = PeriodicExtent1d::full(360.0);
        assert!(f.is_full_period());
        assert_eq!((f.min(), f.max(), f.period()), (0.0, 360.0, 360.0));
        assert!(!f.is_empty());
        assert!(!f.crosses_seam());
        assert!(empty(1.0).is_empty());
        assert!(pe(0.8, 0.2).crosses_seam());
        let mut p = pe(0.2, 0.4);
        p.set_full_period(false);
        assert!(!p.is_full_period());
        p.set_full_period(true);
        assert!(p.is_full_period());
    }

    #[test]
    fn normalize() {
        let e = pe(0.2, 0.4);
        assert_eq!(e.normalize_value(-0.25), 0.75);
        assert_eq!(e.normalize_value(1.25), 0.25);
        assert_eq!(e.normalize_value(1e-13), 0.0);
        assert_eq!(e.map_value(1.0 - 1e-13), 1.0);
        assert_eq!(empty(1.0).normalize_value(0.5), 0.0);
    }

    #[test]
    fn complement_cases() {
        let mut e = empty(2.0);
        e.complement();
        assert_eq!(e, PeriodicExtent1d::full(2.0));
        e.complement();
        assert!(e.is_empty());
        let mut p = pe(0.3, 0.3);
        p.complement();
        assert!(p.is_full_period());
        let mut i = pe(0.2, 0.6);
        i.complement();
        assert_eq!((i.min(), i.max()), (0.6, 0.2));
    }

    #[test]
    fn contains() {
        assert!(pe(0.2, 0.6).contains_value(0.4));
        assert!(!pe(0.2, 0.6).contains_value(0.7));
        assert!(pe(0.8, 0.2).contains_value(0.9));
        assert!(pe(0.8, 0.2).contains_value(0.1));
        assert!(!pe(0.8, 0.2).contains_value(0.5));
        assert!(pe(0.5, 0.5).contains_value(0.5));
        assert!(!empty(1.0).contains_value(0.5));
    }

    #[test]
    fn add_interval_cases() {
        let mut e = pe(0.2, 0.4);
        e.add_interval(0.3, 0.6, true).unwrap();
        assert_eq!((e.min(), e.max()), (0.2, 0.6));
        // disjoint: error, unchanged
        let mut e = pe(0.2, 0.4);
        assert!(e.add_interval(0.6, 0.7, true).is_err());
        assert_eq!(e, pe(0.2, 0.4));
        // seam crossing
        let mut e = pe(0.2, 0.4);
        e.add_interval(0.9, 0.3, true).unwrap();
        assert_eq!((e.min(), e.max()), (0.9, 0.4));
        // case 2 mapped to 1, disjoint restores
        let mut e = pe(0.8, 0.1);
        assert!(e.add_interval(0.3, 0.5, true).is_err());
        assert_eq!(e, pe(0.8, 0.1));
        // out of range
        assert!(pe(0.2, 0.4).add_interval(1.5, 0.3, true).is_err());
        // full
        let mut e = pe(0.2, 0.4);
        e.add_interval(0.0, 1.0, true).unwrap();
        assert!(e.is_full_period());
        // empty extent becomes full (upstream quirk)
        let mut e = empty(1.0);
        e.add_interval(0.2, 0.3, true).unwrap();
        assert!(e.is_full_period());
    }

    #[test]
    fn set_min_max_cases() {
        let mut e = pe(0.1, 0.2);
        e.set_min_max(0.5, 0.5, None, true).unwrap();
        assert_eq!(e, pe(0.5, 0.5));
        e.set_min_max(0.5, 0.5, Some(2.0), false).unwrap();
        assert_eq!(e, PeriodicExtent1d::full(2.0));
        let r = e.set_min_max(-0.5, 0.5, None, false);
        assert!(matches!(r, Err(SolidError::InvalidInput { .. })));
        assert_eq!((e.min(), e.max()), (-0.5, 0.5));
    }

    #[test]
    fn containment_and_disjoint() {
        assert!(pe(0.3, 0.4).is_contained_by(&pe(0.2, 0.6)));
        assert!(!pe(0.3, 0.7).is_contained_by(&pe(0.2, 0.6)));
        assert!(pe(0.9, 0.1).is_contained_by(&pe(0.8, 0.2)));
        assert!(pe(0.85, 0.9).is_contained_by(&pe(0.8, 0.2)));
        assert!(!pe(0.8, 0.2).is_contained_by(&pe(0.1, 0.9)));
        assert!(pe(0.1, 0.2).are_disjoint(&pe(0.3, 0.4)));
        assert!(!pe(0.1, 0.3).are_disjoint(&pe(0.3, 0.4)));
        assert!(!pe(0.1, 0.9).are_disjoint(&pe(0.3, 0.4)));
        assert!(pe(0.1, 0.9).are_disjoint(&empty(1.0)));
    }

    #[test]
    fn mid_cases() {
        assert_eq!(pe(0.2, 0.6).mid(), 0.4);
        assert!((pe(0.8, 0.1).mid() - 0.95).abs() < 1e-15);
        assert!((pe(0.9, 0.2).mid() - 0.05).abs() < 1e-15);
        assert_eq!(empty(1.0).mid(), 0.0);
    }
}
