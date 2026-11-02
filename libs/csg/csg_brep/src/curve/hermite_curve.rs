//! Cubic Hermite curves: a cubic on `[0, 1]` from two end points and their first
//! derivatives, used inside the numerical algorithms (marching, fillets).
//!
//! Upstream: `source/SMLib/inc/SmHermiteCurve.h`, `source/SMLib/src/SmHermiteCurve.cpp`.
//!
//! | Rust                                       | Upstream                                         |
//! |--------------------------------------------|--------------------------------------------------|
//! | [`HermiteCurve`]                           | `SmHermiteCurve`                                 |
//! | [`HermiteCurve::new`]                      | `SmHermiteCurve(SmPoint3d, SmVector3d, SmPoint3d, SmVector3d, ULONG)` |
//! | [`HermiteCurve::from_2d`]                  | `SmHermiteCurve(SmPoint2d, SmVector2d, SmPoint2d, SmVector2d, ULONG)` |
//! | [`HermiteCurve::empty`], `Default`         | `SmHermiteCurve(ULONG lDim = 3)`                 |
//! | `Drop` (implicit)                          | `SmHermiteCurve::~SmHermiteCurve`                |
//! | [`HermiteCurve::approx_eq`]                | `SmHermiteCurve::operator==`                     |
//! | [`HermiteCurve::calculate_continuities`]   | `SmHermiteCurve::CalculateContinuities`          |
//! | [`HermiteCurve::calculate_bounding_box`]   | `SmHermiteCurve::CalculateBoundingBox`           |
//! | [`HermiteCurve::chord_height`]             | `SmHermiteCurve::ComputeChordHeight`             |
//! | [`HermiteCurve::bezier_points`]            | `SmHermiteCurve::GetBezierPoints`                |
//! | [`HermiteCurve::natural_interval`]         | `SmHermiteCurve::GetNaturalInterval`             |
//! | [`HermiteCurve::knots`]                    | `SmHermiteCurve::GetKnots`                       |
//! | [`HermiteCurve::evaluate`]                 | `SmHermiteCurve::Evaluate`                       |
//! | [`HermiteCurve::evaluate_point`]           | `SmHermiteCurve::EvaluatePoint`                  |
//! | [`HermiteCurve::read_from_db`]             | `SmHermiteCurve::ReadFromDB`                     |
//! | [`HermiteCurve::write_to_db`]              | `SmHermiteCurve::WriteToDB`                      |
//! | [`HermiteCurve::TYPE`], [`HermiteCurve::type_code`], [`HermiteCurve::type_string`], [`HermiteCurve::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`HermiteCurve::is_kind_of`]               | `SmHermiteCurve::IsKindOf`                       |
//!
//! Deviations:
//! - **Curve base.** `SmHermiteCurve` derives from `SmCurve` (units `curve.curve_*`).
//!   The only base state the class uses is the image-space dimension, kept here as
//!   [`HermiteCurve::dim`] (the base constructor `SmCurve(ULONG)` sets it; its
//!   `Notify` call has no context to reach). `SmCurve::operator==` (same type, same
//!   dimension) is the dimension test in [`HermiteCurve::approx_eq`], the type test
//!   being the Rust type. [`HermiteCurve::is_kind_of`] adds `SmHermiteCurve_TYPE` to
//!   `SmCurve::IsKindOf` ([`crate::curve::curve::p2::is_kind_of`]). As a
//!   [`crate::curve::curve::Curve`] kind the class gets the `SmCurve` base defaults.
//! - Out-params become return values; arguments upstream marks `NotUsed`
//!   (`CalculateContinuities`' angle tolerance, `CalculateBoundingBox`'s interval
//!   and expand flag, `Evaluate`'s `bFromLeft`) are dropped. `CalculateBoundingBox`
//!   keeps its optional in/out boxes as `Option<&mut _>`: the pseudo box keeps its
//!   previous intervals when a basis is set, so its prior state matters.
//! - `Evaluate` returns `num_derivs + 1` vectors. Upstream zeroes derivatives
//!   `4..num_derivs` (exclusive) and leaves entry `num_derivs` (for
//!   `num_derivs >= 4`) as the caller had it; the port returns zero there too
//!   (`plan/findings.md`).
//! - `ReadFromDB` always builds a new curve (upstream's "fill the curve passed in"
//!   branch and its `IsKindOf` check have no counterpart; its type, dimension,
//!   context and version arguments are unused upstream and dropped). The new curve
//!   has dimension 3 and uninitialised members ([`Vec3::UNDEF`], the
//!   `SmVector3d` member initialisers), so an ASCII value whose extraction is
//!   skipped stays `SM_UNDEF_DOUBLE`. A short binary read is an
//!   error (upstream: undefined values, see `topology::database_io`).
//! - `WriteToDB` is not in the plan's unit list (the write side has no unit); it is
//!   ported here, next to its reader, for `topology::geometry_write`. Its version
//!   argument is unused upstream and kept for the dispatcher.
//! - `CalculateBoundingBox` ignores a failure of `MakeUnitOrthoVectors`, as upstream
//!   (a chord whose squared length overflows); upstream then passes an uninitialised
//!   second axis and a zero third axis to `SetBasis`, which fails on the zero
//!   determinant, so the port leaves the pseudo box's basis unchanged.
//! - The offset constructor, `ComputeAngleSpannedInDegrees`,
//!   `GetBSplineControlPolygon`, `AssertValid`, `GetMemoryUsed`,
//!   `GetRootCurve` and `Dump` are not part of this unit.

use crate::curve::curve::p2::HERMITE_CURVE_TYPE;
use crate::error::Result;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::segment_point_distance;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::polar_box::PolarBox;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::tol::is_zero;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{Point3, Vec3};
use crate::math::{max2, EFF_ZERO, EFF_ZERO_SQ};
use crate::topology::database_io::{DatabaseReader, DatabaseWriter, FileType};

/// A cubic Hermite curve on `[0, 1]` (`SmHermiteCurve`):
///
/// ```text
/// H(t) = (2t³ - 3t² + 1) P1 + (t³ - 2t² + t) D1 + (-2t³ + 3t²) P2 + (t³ - t²) D2
///      = P1 + t (D1 + t (C + t D))           (power basis)
/// ```
#[derive(Debug, Clone, Copy)]
pub struct HermiteCurve {
    /// Image-space dimension (`SmCurve::m_lDim`).
    dim: u64,
    /// Initial point (`m_vP1`).
    p1: Point3,
    /// Initial derivative (`m_vD1`).
    d1: Vec3,
    /// Final point (`m_vP2`).
    p2: Point3,
    /// Final derivative (`m_vD2`).
    d2: Vec3,
    /// `C` of the power basis (`m_vC`).
    c: Vec3,
    /// `D` of the power basis (`m_vD`).
    d: Vec3,
}

/// `SmHermiteCurve()`: [`HermiteCurve::empty`] of dimension 3.
impl Default for HermiteCurve {
    fn default() -> Self {
        Self::empty(3)
    }
}

impl HermiteCurve {
    /// `SmHermiteCurve_TYPE` (`CURV_BASE_TYPE + 34`).
    pub const TYPE: i64 = HERMITE_CURVE_TYPE;

    /// The curve from `p1` with derivative `d1` to `p2` with derivative `d2`, of
    /// image dimension `dim` (upstream default 3).
    #[must_use]
    pub fn new(p1: Point3, d1: Vec3, p2: Point3, d2: Vec3, dim: u64) -> Self {
        // Store C and D of power form
        let c = -3.0 * p1 - 2.0 * d1 + 3.0 * p2 - d2;
        let d = 2.0 * p1 + d1 - 2.0 * p2 + d2;
        Self {
            dim,
            p1,
            d1,
            p2,
            d2,
            c,
            d,
        }
    }

    /// The curve of 2D points and derivatives (`z = 0`), of image dimension `dim`
    /// (upstream default 2).
    #[must_use]
    pub fn from_2d(p1: Point2, d1: Vec2, p2: Point2, d2: Vec2, dim: u64) -> Self {
        Self::new(
            Vec3::from(p1),
            Vec3::from(d1),
            Vec3::from(p2),
            Vec3::from(d2),
            dim,
        )
    }

    /// The empty curve built for reading (`SmHermiteCurve(ULONG lDim)`): every
    /// vector uninitialised ([`Vec3::UNDEF`]).
    #[must_use]
    pub const fn empty(dim: u64) -> Self {
        Self {
            dim,
            p1: Vec3::UNDEF,
            d1: Vec3::UNDEF,
            p2: Vec3::UNDEF,
            d2: Vec3::UNDEF,
            c: Vec3::UNDEF,
            d: Vec3::UNDEF,
        }
    }

    /// Image-space dimension (`GetDim`).
    #[must_use]
    pub const fn dim(&self) -> u64 {
        self.dim
    }

    /// Initial point.
    #[must_use]
    pub const fn start_point(&self) -> Point3 {
        self.p1
    }

    /// Initial first derivative.
    #[must_use]
    pub const fn start_derivative(&self) -> Vec3 {
        self.d1
    }

    /// Final point.
    #[must_use]
    pub const fn end_point(&self) -> Point3 {
        self.p2
    }

    /// Final first derivative.
    #[must_use]
    pub const fn end_derivative(&self) -> Vec3 {
        self.d2
    }

    /// `C` and `D` of the power basis `P1 + t (D1 + t (C + t D))`.
    #[must_use]
    pub const fn power_basis(&self) -> (Vec3, Vec3) {
        (self.c, self.d)
    }

    /// Upstream's tolerant equality (`operator==`): the same curve object, or equal
    /// dimensions and every defining vector equal under [`Vec3::approx_eq`].
    #[must_use]
    pub fn approx_eq(&self, other: &Self) -> bool {
        // low work
        if std::ptr::eq(self, other) {
            return true;
        }

        // first check the base (SmCurve::operator==: same type, same dimension)
        if self.dim != other.dim {
            return false;
        }

        // check equivalence of these objects
        self.p1.approx_eq(other.p1)
            && self.d1.approx_eq(other.d1)
            && self.p2.approx_eq(other.p2)
            && self.d2.approx_eq(other.d2)
            && self.c.approx_eq(other.c)
            && self.d.approx_eq(other.d)
    }

    /// Minimum continuity of the curve and the continuity at each of its two unique
    /// knots (`CalculateContinuities`): `CInfinity` and `[Discontinuous;
    /// 2]`.
    #[must_use]
    pub fn calculate_continuities(&self) -> (ContinuityType, Vec<ContinuityType>) {
        (
            ContinuityType::CInfinity,
            vec![ContinuityType::Discontinuous, ContinuityType::Discontinuous],
        )
    }

    /// Bounding boxes of the Bezier control polygon (`CalculateBoundingBox`; never
    /// expanded).
    ///
    /// `normal_box` is replaced by the axis-aligned box of the four control points.
    /// `pseudo_box` gets the basis "chord, then start derivative" (reset to the
    /// default box when the chord is zero) and grows its intervals to contain the
    /// points. `polar_box` is reset and given the three polygon legs.
    ///
    /// # Errors
    ///
    /// None (kept fallible for the callers).
    pub fn calculate_bounding_box(
        &self,
        normal_box: Option<&mut Extent3d>,
        mut pseudo_box: Option<&mut PseudoBox>,
        polar_box: Option<&mut PolarBox>,
    ) -> Result<()> {
        // Compute Bezier polygon for Hermite and use points to compute bounding
        // boxes.
        let pnts = self.bezier_points();
        let [p0, p1, p2, p3] = pnts;

        // Compute the basis vectors of the PseudoBox using the control polygon
        if let Some(pb) = pseudo_box.as_deref_mut() {
            let v1 = p3 - p0;

            // Note pseudoBox principle direction is set from beg to end points.
            // 2nd axis is selected by the begin point tangent.
            // When the curve is closed the pseudo box gets default directions
            if is_zero(v1.length_squared()) {
                // use default basis vectors
                *pb = PseudoBox::new();
            } else {
                // Upstream ignores both statuses. `MakeUnitOrthoVectors` only fails here
                // when the chord unitizes to the zero vector (its squared length
                // overflows): it then leaves `sBasis2` uninitialised and `sBasis3` the
                // zero vector, so `SetBasis` sees a zero determinant, fails, and the box
                // keeps its basis.
                if let Ok([b1, b2, b3]) = v1.make_unit_ortho_vectors(Some(self.d1)) {
                    let _ = pb.set_basis(b1, b2, b3);
                }
            }
        }

        // load the bounding boxes
        if let Some(nb) = normal_box {
            *nb = Extent3d::from_point(p0);
            nb.add_point(p1);
            nb.add_point(p2);
            nb.add_point(p3);
        }
        if let Some(pb) = pseudo_box {
            for p in pnts {
                pb.add_point(p);
            }
        }
        if let Some(pb) = polar_box {
            pb.reset();
            pb.add_vector(p1 - p0, None);
            pb.add_vector(p2 - p1, None);
            pb.add_vector(p3 - p2, None);
        }
        Ok(())
    }

    /// Position and the first `num_derivs` derivatives at `param`
    /// (`Evaluate`); derivatives above the third are zero.
    ///
    /// With `non_zero_tangents`, a first derivative shorter than
    /// `1.1 * EFF_ZERO * (1 + max |coordinate of the position|)` is replaced by the
    /// second derivative scaled to that length (negated in the upper half of the
    /// domain), when the second derivative is longer than that.
    #[must_use]
    pub fn evaluate(&self, param: f64, num_derivs: usize, non_zero_tangents: bool) -> Vec<Vec3> {
        // Derivatives above the third stay zero.
        let mut out = vec![Vec3::ZERO; num_derivs.saturating_add(1)];

        // Compute a, b, c, d of power basis - note we speeded this
        // up by computing these and storing them
        let a = self.p1;
        let b = self.d1;
        let c = self.c;
        let d = self.d;

        let u = param;

        // Compute position
        if let Some(o) = out.get_mut(0) {
            *o = a + u * (b + u * (c + u * d));
        }
        if let Some(o) = out.get_mut(1) {
            *o = b + u * (2.0 * c + 3.0 * u * d);
        }
        if let Some(o) = out.get_mut(2) {
            *o = 2.0 * c + 6.0 * u * d;
        }
        if let Some(o) = out.get_mut(3) {
            *o = 6.0 * d;
        }

        // When asked, try to find a direction for zero-length first derivatives
        // based on the 2nd derivative value
        // (This is the same trick as is used for surface evaluations.)
        if non_zero_tangents {
            let pos = out.first().copied().unwrap_or_default();
            let scaled_zero = EFF_ZERO * (1.0 + pos.max_dimension());
            let new_len = 1.1 * scaled_zero;

            // see if the 1st order derivative is smaller than new_len (this gives a
            // continuous modified function)
            let tangent = out.get(1).copied();
            if let Some(tangent) = tangent {
                if tangent.length_squared() < new_len * new_len {
                    // make sure 2nd order derivatives are available
                    if num_derivs < 2 {
                        // recurse so the next section sets the 1st derivative
                        let vals = self.evaluate(param, 2, true);

                        // save the 1st derivative
                        if let (Some(o), Some(v)) = (out.get_mut(1), vals.get(1)) {
                            *o = *v;
                        }
                    } else {
                        // fix the zero 1st derivative function here
                        let second = out.get(2).copied().unwrap_or_default();
                        let len = second.length();
                        if len > new_len {
                            let mut fixed = (new_len / len) * second;

                            // If we're at the 'top' of the domain, i.e., the 'good'
                            // parameter is entering a singularity instead of leaving
                            // it, then the degenerate derivative is shrinking, so its
                            // change (the 2nd derivative) is in the opposite direction
                            // of the derivative.
                            if param > self.natural_interval().mid() {
                                fixed *= -1.0;
                            }
                            if let Some(o) = out.get_mut(1) {
                                *o = fixed;
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// The point at `param` (`EvaluatePoint`).
    #[must_use]
    pub fn evaluate_point(&self, param: f64) -> Point3 {
        let a = self.p1;
        let b = self.d1;
        let c = self.c;
        let d = self.d;
        let u = param;
        a + u * (b + u * (c + u * d))
    }

    /// Largest distance from the chord to the inner Bezier control points, or the
    /// control polygon length for a closed curve (`ComputeChordHeight`).
    #[must_use]
    pub fn chord_height(&self) -> f64 {
        let pnt1 = self.p1 + self.d1 / 3.0;
        let pnt2 = self.p2 - self.d2 / 3.0;
        if self.p1.distance_squared(self.p2) < EFF_ZERO_SQ {
            self.p1.distance(pnt1) + pnt1.distance(pnt2) + pnt2.distance(self.p2)
        } else {
            let (dist, _) = segment_point_distance(self.p1, self.p2, pnt1);
            let (dist2, _) = segment_point_distance(self.p1, self.p2, pnt2);
            max2(dist, dist2)
        }
    }

    /// The four Bezier control points of the curve (`GetBezierPoints`).
    #[must_use]
    pub fn bezier_points(&self) -> [Point3; 4] {
        [
            self.p1,
            self.p1 + self.d1 / 3.0,
            self.p2 - self.d2 / 3.0,
            self.p2,
        ]
    }

    /// `[0, 1]` (`GetNaturalInterval`).
    #[must_use]
    pub fn natural_interval(&self) -> Extent1d {
        Extent1d::from_min_max(0.0, 1.0)
    }

    /// The unique knots of the equivalent cubic B-spline and their multiplicities
    /// (`GetKnots`): `0` and `1`, each of multiplicity 3, keeping only those inside
    /// `interval` when one is given.
    #[must_use]
    pub fn knots(&self, interval: Option<&Extent1d>) -> (Vec<f64>, Vec<u64>) {
        let mut knots = Vec::new();
        let mut mults = Vec::new();
        let ivl = self.natural_interval();

        // interval start
        if interval.is_none_or(|i| i.contains_value(ivl.min(), 0.0)) {
            knots.push(ivl.min());
            mults.push(3);
        }

        // interval end
        if interval.is_none_or(|i| i.contains_value(ivl.max(), 0.0)) {
            knots.push(ivl.max());
            mults.push(3);
        }
        (knots, mults)
    }

    /// Reads a curve written by `WriteToDB` (`ReadFromDB`): points, derivatives and
    /// the power basis, three doubles each (one line each in ASCII files).
    ///
    /// # Errors
    ///
    /// A failed binary read.
    pub fn read_from_db(db: &mut DatabaseReader) -> Result<Self> {
        // init output object
        let mut h = Self::empty(3);
        if db.file_type() == FileType::Ascii {
            for v in [
                &mut h.p1, &mut h.d1, &mut h.p2, &mut h.d2, &mut h.c, &mut h.d,
            ] {
                if let Some(s) = db.in_stream_mut() {
                    for coord in [&mut v.x, &mut v.y, &mut v.z] {
                        if let Some(x) = s.extract_double() {
                            *coord = x;
                        }
                    }
                }
                db.go_to_next_line();
            }
        } else {
            for v in [
                &mut h.p1, &mut h.d1, &mut h.p2, &mut h.d2, &mut h.c, &mut h.d,
            ] {
                v.x = db.read_double()?;
                v.y = db.read_double()?;
                v.z = db.read_double()?;
            }
        }
        Ok(h)
    }

    /// Writes the curve (`WriteToDB`): points, derivatives and the power basis, three
    /// doubles each (one labelled line each in ASCII files). `version` is unused, as
    /// upstream.
    ///
    /// # Errors
    ///
    /// None (upstream's `WriteDouble` statuses cannot fail on the in-memory writer).
    pub fn write_to_db(&self, db: &mut DatabaseWriter, version: u64) -> Result<()> {
        let _ = version;
        let rows = [
            (self.p1, " SmHermiteCurve Initial point        \n"),
            (self.d1, " SmHermiteCurve Initial direction    \n"),
            (self.p2, " SmHermiteCurve Final point          \n"),
            (self.d2, " SmHermiteCurve Final direction      \n"),
            (self.c, " SmHermiteCurve C of power basis eqn \n"),
            (self.d, " SmHermiteCurve D of power basis eqn \n"),
        ];
        if db.file_type() == FileType::Ascii {
            for (v, label) in rows {
                db.out_reals(&[v.x, v.y, v.z]).out_str(label);
            }
        } else {
            for (v, _) in rows {
                db.write_doubles(&[v.x, v.y, v.z]);
            }
        }
        // all done
        Ok(())
    }

    /// `SmHermiteCurve_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        Self::TYPE
    }

    /// `"SmHermiteCurve_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        "SmHermiteCurve_TYPE"
    }

    /// `"SmHermiteCurve"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        "SmHermiteCurve"
    }

    /// True for `SmHermiteCurve_TYPE` and its base types (`IsKindOf`).
    ///
    /// # Errors
    ///
    /// None (kept fallible for callers written against the unported base).
    pub fn is_kind_of(&self, t: i64) -> Result<bool> {
        Ok(t == Self::TYPE || crate::curve::curve::p2::is_kind_of(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> HermiteCurve {
        HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::new(3.0, 3.0, 0.0),
            Vec3::new(0.0, 3.0, 0.0),
            3,
        )
    }

    #[test]
    fn write_then_read_round_trips() {
        let h = sample();
        for ft in [FileType::Ascii, FileType::Binary] {
            let mut w = DatabaseWriter::open_for_write(ft);
            h.write_to_db(&mut w, 30).unwrap();
            let bytes = w.into_bytes();
            if ft == FileType::Ascii {
                let text = String::from_utf8(bytes.clone()).unwrap();
                assert_eq!(text.lines().count(), 6);
                assert!(text.starts_with(
                    "0.0000000000000000 0.0000000000000000 0.0000000000000000 SmHermiteCurve Initial point        \n"
                ));
            } else {
                assert_eq!(bytes.len(), 18 * 8);
            }
            let mut r = DatabaseReader::new();
            r.open_for_read(bytes, ft);
            let back = HermiteCurve::read_from_db(&mut r).unwrap();
            assert_eq!(back.start_point(), h.start_point());
            assert_eq!(back.end_derivative(), h.end_derivative());
            assert_eq!(back.power_basis(), h.power_basis());
        }
    }

    #[test]
    fn interpolates_ends() {
        let h = sample();
        assert_eq!(h.evaluate_point(0.0), h.start_point());
        assert_eq!(h.evaluate_point(1.0), h.end_point());
        let e = h.evaluate(1.0, 1, false);
        assert_eq!(e[1], h.end_derivative());
        let e = h.evaluate(0.0, 5, false);
        assert_eq!(e.len(), 6);
        assert_eq!(e[1], h.start_derivative());
        assert_eq!(e[4], Vec3::ZERO);
        assert_eq!(e[5], Vec3::ZERO);
    }

    #[test]
    fn bezier_and_bbox() {
        let h = sample();
        let b = h.bezier_points();
        assert_eq!(b[1], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(b[2], Vec3::new(3.0, 2.0, 0.0));
        let mut e = Extent3d::new();
        h.calculate_bounding_box(Some(&mut e), None, None).unwrap();
        assert_eq!(e.min(), Vec3::ZERO);
        assert_eq!(e.max(), Vec3::new(3.0, 3.0, 0.0));
    }

    #[test]
    fn overflowing_chord_keeps_the_pseudo_box_basis() {
        // fuzz: a control point at `-1e300` overflows the chord's squared length;
        // upstream ignores the `MakeUnitOrthoVectors` failure (its `SetBasis` then
        // fails on a zero third axis) and the box keeps its basis
        let h = HermiteCurve::new(
            Vec3::new(0.0, 0.0, -1e300),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1e300),
            Vec3::new(1.0, 0.0, 0.0),
            3,
        );
        let mut pb = PseudoBox::new();
        let before = pb;
        h.calculate_bounding_box(None, Some(&mut pb), None).unwrap();
        assert_eq!(pb.basis(), before.basis());
    }

    #[test]
    fn zero_tangent_is_fixed() {
        // D1 = 0: the tangent at 0 is replaced by a tiny vector along H''(0).
        let h = HermiteCurve::new(Vec3::ZERO, Vec3::ZERO, Vec3::X, Vec3::X, 3);
        let e = h.evaluate(0.0, 1, true);
        assert!(e[1].length() > 0.0 && e[1].x > 0.0);
        assert_eq!(h.evaluate(0.0, 1, false)[1], Vec3::ZERO);
    }

    #[test]
    fn knots_and_kind() {
        let h = sample();
        assert_eq!(h.knots(None), (vec![0.0, 1.0], vec![3, 3]));
        let ivl = Extent1d::from_min_max(0.5, 2.0);
        assert_eq!(h.knots(Some(&ivl)), (vec![1.0], vec![3]));
        assert!(h.is_kind_of(HermiteCurve::TYPE).unwrap());
        assert!(!h.is_kind_of(1).unwrap());
        assert!(h.is_kind_of(14001).unwrap());
        assert!(h.approx_eq(&sample()));
        assert!(!h.approx_eq(&HermiteCurve::empty(3)));
    }

    #[test]
    fn chord_height_of_closed_curve_is_polygon_length() {
        let h = HermiteCurve::new(
            Vec3::ZERO,
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::ZERO,
            Vec3::new(0.0, -3.0, 0.0),
            3,
        );
        let expected = 1.0 + 2f64.sqrt() + 1.0;
        assert!((h.chord_height() - expected).abs() < 1e-15);
    }

    #[test]
    fn read_ascii() {
        let text = "1 2 3 a\n4 5 6\n7 8 9\n1 1 1\n2 2 2\n3 3 3\n";
        let mut db = DatabaseReader::new();
        db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
        let h = HermiteCurve::read_from_db(&mut db).unwrap();
        assert_eq!(h.start_point(), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(h.power_basis().1, Vec3::new(3.0, 3.0, 3.0));
        assert_eq!(h.dim(), 3);
    }
}
