//! Polar boxes: bounds on a set of directions, expressed as a rectangle of two
//! projection angles about a polar axis on the unit sphere (used to bound tangent and
//! normal fields of curves and surfaces).
//!
//! Upstream: `source/SMLib/src/SmPolarBox.cpp`, `source/SMLib/inc/SmPolarBox.h`.
//!
//! | Rust                                         | Upstream                                     |
//! |----------------------------------------------|----------------------------------------------|
//! | [`PolarBoxState`]                            | `SmPolarBoxState`                            |
//! | [`PolarBox`]                                 | `SmPolarBox`                                 |
//! | [`PolarBox::new`], `Default`                 | `SmPolarBox()`                               |
//! | [`PolarBox::from_vector`]                    | `SmPolarBox(const SmVector3d&)`              |
//! | [`PolarBox::assign`]                         | `operator=`                                  |
//! | `Drop` (implicit)                            | `~SmPolarBox`                                |
//! | [`PolarBox::reset`]                          | `ReSet`                                      |
//! | [`PolarBox::domain`]                         | `GetDomain`                                  |
//! | [`PolarBox::state`]                          | `GetPolarBoxState`                           |
//! | [`PolarBox::basis`]                          | `GetBasis`                                   |
//! | [`PolarBox::compute_polar_coord`]            | `ComputePolarCoord`                          |
//! | [`PolarBox::polar_area`]                     | `GetPolarArea`                               |
//! | [`PolarBox::is_perpendicular_disjoint_with_cone`] | `IsPerpendicularDisjointWithCone`       |
//! | [`PolarBox::is_disjoint_with_cone`]          | `IsDisjointWithCone`                         |
//! | [`PolarBox::are_disjoint`]                   | `AreDisjoint`                                |
//! | [`PolarBox::cross`]                          | `Cross`                                      |
//! | [`PolarBox::evaluate_normalized`]            | `EvaluateNormalized`                         |
//! | [`PolarBox::add_vector`]                     | `AddVector3d`                                |
//! | [`PolarBox::boundary_vectors`]               | `GetBoundaryVectors`                         |
//! | [`PolarBox::has_perpendicular_to_vector`]    | `HasPerpendicularToVector`                   |
//! | [`PolarBox::union`], [`PolarBox::union_assign`] | `Union` (`union_assign`: result aliases `this`) |
//! | [`PolarBox::expand_absolute`]                | `ExpandAbsolute`                             |
//!
//! `IsContainedBy`, `AreEqual`, `HasLargerSpan`, `Dump`, `Draw`, `AssertValid`,
//! `TestTransform` and the type-name getters are not reached by in-scope code
//! (`harness/callgraph`) and are not ported.
//!
//! Deviations:
//! - `SmStatus` results that are always `SM_SUCCESS` (`AddVector3d`, `Cross`,
//!   `ExpandAbsolute`) become `()` / plain values. Out-params become return values
//!   (`Cross` and `Union` return the result box, `GetBoundaryVectors` a `Vec`).
//! - `Cross`/`Union` write into a caller-provided box and, on their early
//!   `SM_PS_UNBOUNDED` exits (or when `Cross` adds no vector), leave its basis and
//!   domain as they were. The Rust versions start from [`PolarBox::new`]; callers that
//!   need upstream's "keep the stale fields" behaviour copy the result over the old
//!   box with [`PolarBox::assign`] (which, like `operator=`, keeps the basis for
//!   uninitialised and unbounded boxes). `p.Union(o, p)` (result aliasing `this`, as
//!   in `SmCompositeCurve`) is [`PolarBox::union_assign`].
//! - `GetBoundaryVectors` on an uninitialised or unbounded box returns
//!   `SM_ERR_INVALID_INPUT` and leaves its count unset; `Union` then reads that unset
//!   count (undefined behaviour). [`PolarBox::union`] returns the error instead.
//! - `ComputePolarCoord` on an uninitialised or unbounded box returns the
//!   uninitialised point `SmPoint2d()` (and logs `SM_ERR`);
//!   [`PolarBox::compute_polar_coord`] returns `SM_ERR` ([`SolidError::Upstream`]),
//!   internal callers use the `(SM_UNDEF_DOUBLE, SM_UNDEF_DOUBLE)` point as upstream.
//! - `GetBasis(lIndex)` returns a pointer to one basis vector;
//!   [`PolarBox::basis`] returns all three.
//! - `MakeUnitOrthoVectors` fails here only for vectors whose squared length
//!   overflows (the unit vector underflows to zero); `set_ortho_basis` then
//!   leaves the partial basis upstream's out-params hold.
//! - `SM_ASSERT`, `SM_DEBUG_CODE` blocks and the error-log side effect of `SE(...)`
//!   are compiled out upstream (release build) and omitted.
//! - `EvaluateNormalized` (`SM_PS_REGION`) uses `sin_cos` wrapped in
//!   `core::hint::black_box` where the reference build fuses sin/cos into sincos;
//!   glibc sincos differs from sin by 1 ulp for some arguments, and without the
//!   barrier LLVM sinks `sin` past the 90-degree early return and does not fuse
//!   (issue #357). Pure codegen hint: same values on targets without sincos (wasm32).

use crate::error::{Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{acos, Vec3};
use crate::math::{EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};

/// State of a [`PolarBox`] (`SmPolarBoxState`), ordered from most degenerate to most
/// general (upstream relies on this order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PolarBoxState {
    /// No vector registered yet (`SM_PS_UNINITIALIZED`).
    #[default]
    Uninitialized,
    /// The `(0, 0)` vector registered with a preset basis; only seen while
    /// [`PolarBox::add_vector`] is given an optional basis (`SM_PS_POINT_WITH_BASIS`).
    PointWithBasis,
    /// All vectors map to one point on the unit sphere (`SM_PS_SINGLE_POINT`).
    SinglePoint,
    /// All vectors map to an arc in the `basis[0]`/`basis[2]` plane (`SM_PS_ARC`).
    Arc,
    /// The vectors map to a region of the unit sphere (`SM_PS_REGION`).
    Region,
    /// No bound could be found: some vector is more than 90 degrees from `basis[0]`
    /// (`SM_PS_UNBOUNDED`).
    Unbounded,
}

impl PolarBoxState {
    /// The upstream enum value (`SM_PS_UNINITIALIZED` = 0 .. `SM_PS_UNBOUNDED` = 5).
    #[must_use]
    pub fn code(self) -> i32 {
        match self {
            Self::Uninitialized => 0,
            Self::PointWithBasis => 1,
            Self::SinglePoint => 2,
            Self::Arc => 3,
            Self::Region => 4,
            Self::Unbounded => 5,
        }
    }

    fn is_point(self) -> bool {
        matches!(self, Self::SinglePoint | Self::PointWithBasis)
    }

    /// Uninitialised or unbounded: no bound on the directions is known.
    fn is_unknown(self) -> bool {
        matches!(self, Self::Uninitialized | Self::Unbounded)
    }
}

/// Bounding box of polar coordinates on the unit sphere (`SmPolarBox`).
///
/// `basis[0]` is the polar axis (the first vector added), the `basis[0]`/`basis[2]`
/// plane contains the second independent vector, and `basis[1]` is perpendicular to
/// both. The domain's x range bounds the angle of a vector's projection onto the
/// `basis[0]`/`basis[2]` plane (counter-clockwise about `basis[1]`, positive towards
/// `-basis[2]`), its y range the angle of the projection onto the
/// `basis[0]`/`basis[1]` plane (counter-clockwise about `basis[2]`, positive towards
/// `+basis[1]`); see the upstream header for the full derivation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolarBox {
    basis: [Vec3; 3],
    domain: Extent2d,
    state: PolarBoxState,
}

impl Default for PolarBox {
    fn default() -> Self {
        Self::new()
    }
}

impl PolarBox {
    /// An empty, uninitialised box (`SmPolarBox()`): basis vectors
    /// [`Vec3::UNDEF`], domain [`Extent2d::new`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            basis: [Vec3::UNDEF; 3],
            domain: Extent2d::new(),
            state: PolarBoxState::Uninitialized,
        }
    }

    /// The single-point box of `v` (`SmPolarBox(const SmVector3d&)`); uninitialised
    /// when `v` has (near) zero length.
    #[must_use]
    pub fn from_vector(v: Vec3) -> Self {
        let mut b = Self::new();
        if v.length_squared() < EFF_ZERO_SQ {
            b.state = PolarBoxState::Uninitialized;
        } else {
            b.state = PolarBoxState::SinglePoint;
            b.set_domain(0.0, 0.0, 0.0, 0.0);
            b.set_ortho_basis(v);
        }
        b
    }

    /// Copies `other` into `self` (`operator=`): state and domain always, the basis
    /// only for point, arc and region boxes.
    pub fn assign(&mut self, other: &Self) {
        self.state = other.state;
        self.domain = other.domain;
        match self.state {
            PolarBoxState::PointWithBasis
            | PolarBoxState::Region
            | PolarBoxState::Arc
            | PolarBoxState::SinglePoint => self.basis = other.basis,
            PolarBoxState::Uninitialized | PolarBoxState::Unbounded => {}
        }
    }

    /// Takes the box back to [`PolarBoxState::Uninitialized`], keeping its basis and
    /// domain (`ReSet`).
    pub fn reset(&mut self) {
        self.state = PolarBoxState::Uninitialized;
    }

    /// The angle domain (`GetDomain`).
    #[must_use]
    pub fn domain(&self) -> Extent2d {
        self.domain
    }

    /// The state (`GetPolarBoxState`).
    #[must_use]
    pub fn state(&self) -> PolarBoxState {
        self.state
    }

    /// The three basis vectors (`GetBasis` for indices 0, 1, 2).
    #[must_use]
    pub fn basis(&self) -> &[Vec3; 3] {
        &self.basis
    }

    /// `m_sPolarDomain.SetMinMax(..)` with its status ignored (as upstream).
    fn set_domain(&mut self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) {
        // The only failure (min > max) leaves the domain unchanged, as upstream.
        let _ = self.domain.set_min_max_coords(min_x, min_y, max_x, max_y);
    }

    /// `m_aBasis[0] = v; m_aBasis[0].MakeUnitOrthoVectors(NULL, m_aBasis[0],
    /// m_aBasis[1], m_aBasis[2])` with the status ignored.
    fn set_ortho_basis(&mut self, v: Vec3) {
        match v.make_unit_ortho_vectors(None) {
            Ok(frame) => self.basis = frame,
            // upstream's out-params are the basis members: keep what it wrote
            Err(_) => {
                for (slot, w) in self.basis.iter_mut().zip(v.unit_ortho_vectors_on_failure()) {
                    if let Some(w) = w {
                        *slot = w;
                    }
                }
            }
        }
    }

    /// The polar angles of `v` relative to the box basis: x is the counter-clockwise
    /// angle from `basis[0]` of `v` projected onto the `basis[0]`/`basis[2]` plane as
    /// seen from `basis[1]`, y the same for the `basis[0]`/`basis[1]` plane seen from
    /// `basis[2]`; both in `[-PI, PI]` (`ComputePolarCoord`). The point need not lie in
    /// the domain.
    ///
    /// # Errors
    ///
    /// `SM_ERR` ([`SolidError::Upstream`]) for uninitialised and unbounded boxes
    /// (upstream returns `SmPoint2d()`).
    pub fn compute_polar_coord(&self, v: Vec3) -> Result<Point2> {
        if self.state.is_unknown() {
            return Err(SolidError::sm_err("geom::polar_box::compute_polar_coord"));
        }
        Ok(self.polar_coord_raw(v))
    }

    /// Upstream `ComputePolarCoord` including its uninitialised-point result.
    fn polar_coord(&self, v: Vec3) -> Point2 {
        self.compute_polar_coord(v).unwrap_or(Vec2::UNDEF)
    }

    fn polar_coord_raw(&self, v: Vec3) -> Point2 {
        let [b0, b1, b2] = self.basis;

        // project v onto the v0/v2 plane
        let proj1 = b1.cross(v).cross(b1);
        let angle1 = if proj1.length_squared() < EFF_ZERO_SQ {
            // v is parallel to basis[1]
            0.0
        } else {
            projected_angle(b0, b1, proj1.unitize_or_z())
        };

        // project v onto the v0/v1 plane
        let proj2 = b2.cross(v).cross(b2);
        let angle2 = if proj2.length_squared() < EFF_ZERO_SQ {
            // v is parallel to basis[2]
            0.0
        } else {
            projected_angle(b0, b2, proj2.unitize_or_z())
        };

        Vec2::new(angle1, angle2)
    }

    /// Area of the unit sphere covered by the box (`GetPolarArea`): `0` for point and
    /// arc boxes, `4 PI` for unbounded boxes, `0` for uninitialised boxes.
    #[must_use]
    pub fn polar_area(&self) -> f64 {
        match self.state {
            // points and arcs cover no area; uninitialised boxes report 0 too
            PolarBoxState::PointWithBasis
            | PolarBoxState::SinglePoint
            | PolarBoxState::Arc
            | PolarBoxState::Uninitialized => 0.0,
            PolarBoxState::Region => {
                let [b0, b1, b2] = self.basis;
                let a_min = self.domain.min();
                let a_max = self.domain.max();

                let ca1_min = a_min.x.cos();
                let sa1_min = a_min.x.sin();
                let ca2_min = a_min.y.cos();
                let sa2_min = a_min.y.sin();

                let ca1_max = a_max.x.cos();
                let sa1_max = a_max.x.sin();
                let ca2_max = a_max.y.cos();
                let sa2_max = a_max.y.sin();

                // normal of each great circle plane
                let a1_min_norm = -sa1_min * b0 - ca1_min * b2;
                let a1_max_norm = -sa1_max * b0 - ca1_max * b2;
                let a2_min_norm = -sa2_min * b0 + ca2_min * b1;
                let a2_max_norm = -sa2_max * b0 + ca2_max * b1;

                // `AngleBetween` reports 0 when it fails (status ignored upstream).
                let b00 = a1_min_norm.angle_between(a2_min_norm).unwrap_or(0.0);
                let b01 = a1_min_norm.angle_between(-a2_max_norm).unwrap_or(0.0);
                let b11 = a1_max_norm.angle_between(a2_max_norm).unwrap_or(0.0);
                let b10 = a1_max_norm.angle_between(-a2_min_norm).unwrap_or(0.0);

                2.0 * PI - b00 - b01 - b11 - b10
            }
            PolarBoxState::Unbounded => 4.0 * PI,
        }
    }

    /// True when the box is disjoint from the cone of half-angle `cone_angle` (radians)
    /// about the direction `cone_vec × basis[0] × cone_vec` (perpendicular to
    /// `cone_vec`); false when that direction degenerates
    /// (`IsPerpendicularDisjointWithCone`).
    #[must_use]
    pub fn is_perpendicular_disjoint_with_cone(&self, cone_vec: Vec3, cone_angle: f64) -> bool {
        if self.state == PolarBoxState::Unbounded {
            return false;
        }
        let perp = cone_vec.cross(self.basis[0]).cross(cone_vec);
        if perp.length_squared() < EFF_ZERO_SQ {
            return false;
        }
        self.is_disjoint_with_cone(perp, cone_angle)
    }

    /// True when no direction of the box (or its opposite) lies within `cone_angle`
    /// radians of `cone_vec` (`IsDisjointWithCone`).
    #[must_use]
    pub fn is_disjoint_with_cone(&self, cone_vec: Vec3, cone_angle: f64) -> bool {
        if self.state == PolarBoxState::Unbounded {
            return false;
        }

        // ensure the cone vector points the same way as the box
        let dot = self.basis[0].dot(cone_vec);
        let cone = if dot < 0.0 { -cone_vec } else { cone_vec };

        // only a single vector is known for this box
        if self.state.is_point() {
            let Ok(mut ang) = cone.angle_between(self.basis[0]) else {
                return false;
            };
            if ang > PI / 2.0 {
                ang = PI - ang;
            }
            return ang > cone_angle;
        }

        let polar = self.polar_coord(cone);
        let mut domain = self.domain;
        // expand the domain by the cone angle and test whether the cone vector is in it
        domain.expand_absolute(cone_angle);
        !domain.contains_point(polar, 0.0)
    }

    /// True when the two boxes certainly bound disjoint direction sets, treating
    /// each direction and its opposite alike; false when that cannot be shown
    /// (`AreDisjoint`). Uninitialised boxes give false (upstream also logs `SM_ERR`).
    #[must_use]
    pub fn are_disjoint(&self, other: &Self) -> bool {
        if self.state == PolarBoxState::Uninitialized || other.state == PolarBoxState::Uninitialized
        {
            return false;
        }

        // an unbounded box meets everything
        if self.state == PolarBoxState::Unbounded || other.state == PolarBoxState::Unbounded {
            return false;
        }

        // both are single vectors
        if self.state.is_point() && other.state.is_point() {
            let dot = self.basis[0].dot(other.basis[0]);
            // (parallel or anti-parallel) ? FALSE : TRUE, with NaN giving TRUE
            return !matches!(
                dot.abs().partial_cmp(&(1.0 - EFF_ZERO_SQRT)),
                Some(core::cmp::Ordering::Greater)
            );
        }

        // angle between the two polar axes
        // clamp to [-1, 1] (NaN stays NaN, as upstream's two tests)
        let dot = self.basis[0].dot(other.basis[0]).clamp(-1.0, 1.0);
        let mut angle_between_basis = acos(dot);
        if dot < 0.0 {
            angle_between_basis = PI - angle_between_basis;
        }

        let mut max_angle1 = 0.0;
        let mut max_angle2 = 0.0;
        if !self.state.is_point() {
            max_angle1 = self.domain.size().length();
        }
        if !other.state.is_point() {
            max_angle2 = other.domain.size().length();
        }

        // the axes are further apart than both spans together
        if angle_between_basis > max_angle1 + max_angle2 + EFF_ZERO_SQRT {
            return true;
        }

        // axes closer than 11 degrees: give up
        if angle_between_basis < PI / 16.0 {
            return false;
        }

        // which side of the cones to compare
        let mut sign = 1.0;
        let cent_this = self.evaluate_normalized(0.5, 0.5);
        let cent_other = other.evaluate_normalized(0.5, 0.5);
        if cent_this.dot(cent_other) < 0.0 {
            sign = -1.0;
        }

        // project other's corners into this box's angles and test the domains
        if !self.state.is_point() {
            if let Some(disjoint) = Self::corners_disjoint(self, other, sign) {
                return disjoint;
            }
        }
        // and this box's corners into other's angles
        if !other.state.is_point() {
            if let Some(disjoint) = Self::corners_disjoint(other, self, sign) {
                return disjoint;
            }
        }

        // cannot decide
        false
    }

    /// One half of `AreDisjoint`: `Some(false)` when a corner of `from` (after the
    /// first) maps into `onto`'s domain, `Some(true)` when the domain of all mapped
    /// corners (grown by `SM_EFF_ZERO_SQRT`) misses `onto`'s domain, `None` otherwise.
    fn corners_disjoint(onto: &Self, from: &Self, sign: f64) -> Option<bool> {
        // `from` is neither uninitialised nor unbounded, so this cannot fail.
        let vecs = from.boundary_vectors().unwrap_or_default();
        let (first, rest) = vecs.split_first()?;
        let mut domain = Extent2d::from_point(onto.polar_coord(*first * sign));
        for v in rest {
            let polar = onto.polar_coord(*v * sign);
            if onto.domain.contains_point(polar, 0.0) {
                return Some(false);
            }
            domain.add_point(polar);
        }
        domain.expand_absolute(EFF_ZERO_SQRT);
        if domain.are_disjoint(&onto.domain, 0.0) {
            Some(true)
        } else {
            None
        }
    }

    /// A box bounding the directions of `a × b` for every `a` in `self` and `b` in
    /// `other` (`Cross`), e.g. the normal box of a surface from its partial-derivative
    /// boxes. Unbounded when either input is uninitialised or unbounded.
    #[must_use]
    pub fn cross(&self, other: &Self) -> Self {
        let mut out = Self::new();
        // an uninitialised or unbounded input gives an unbounded result
        if self.state.is_unknown() || other.state.is_unknown() {
            out.state = PolarBoxState::Unbounded;
            return out;
        }

        // cross the centre vectors first so the result stays reasonably centred
        let cross_vec = self.basis[0].cross(other.basis[0]);
        if cross_vec.length_squared() > EFF_ZERO_SQ {
            out.add_vector(cross_vec, None);
            // two single vectors: done
            if self.state.is_point() && other.state.is_point() {
                return out;
            }
            // second basis vector from this box's axis
            out.basis[1] = self.basis[0];
            out.basis[2] = out.basis[0].cross(out.basis[1]);
            out.state = PolarBoxState::Arc;
            out.domain = Extent2d::from_point(Vec2::new(0.0, 0.0));
        }

        // neither box is uninitialised or unbounded here, so these cannot fail
        let this_vecs = self.boundary_vectors().unwrap_or_default();
        let other_vecs = other.boundary_vectors().unwrap_or_default();

        for a in &this_vecs {
            for b in &other_vecs {
                let c = a.cross(*b);
                if c.length_squared() > EFF_ZERO_SQ {
                    out.add_vector(c, None);
                }
            }
        }
        out
    }

    /// The direction at normalised domain coordinates `(alpha, beta)`
    /// (`EvaluateNormalized`): `basis[0]` for point boxes, a unit vector for arcs
    /// (`alpha` only) and regions, and the zero vector for uninitialised and unbounded
    /// boxes or region angles of 90 degrees or more.
    #[must_use]
    pub fn evaluate_normalized(&self, alpha: f64, beta: f64) -> Vec3 {
        let [b0, b1, b2] = self.basis;
        match self.state {
            PolarBoxState::PointWithBasis | PolarBoxState::SinglePoint => b0,
            PolarBoxState::Arc => {
                // all vectors lie in the v0/v2 plane
                let p = self.domain.evaluate(alpha, beta);
                p.x.cos() * b0 - p.x.sin() * b2
            }
            PolarBoxState::Region => {
                // rebuild the vector from its projection angles onto the v0/v2
                // (angle 1) and v0/v1 (angle 2) planes
                let p = self.domain.evaluate(alpha, beta);

                // the reference build (GCC -O2) fuses each pair into glibc sincos, which
                // can differ from sin by 1 ulp. black_box keeps both halves of each pair
                // in one block so LLVM fuses them too instead of sinking sin past the
                // early return below (plan/findings.md, issue #357)
                let (sa1, ca1) = core::hint::black_box(p.x.sin_cos());
                let (sa2, ca2) = core::hint::black_box(p.y.sin_cos());

                // no unique vector at 90 degrees
                if ca1 <= EFF_ZERO || ca2 <= EFF_ZERO {
                    return Vec3::new(0.0, 0.0, 0.0);
                }

                // the v0/v2 projection plus the v1 component of the v0/v1 projection
                // scaled to the same v0 component
                let v = ca1 * b0 - sa1 * b2 + sa2 * ca1 / ca2 * b1;
                v.unitize_or_z()
            }
            PolarBoxState::Uninitialized | PolarBoxState::Unbounded => Vec3::new(0.0, 0.0, 0.0),
        }
    }

    /// Grows the box to contain `v` (`AddVector3d`); zero vectors are ignored.
    ///
    /// The first vector sets `basis[0]` (state [`PolarBoxState::SinglePoint`]); the
    /// next independent one fixes `basis[1]`/`basis[2]` (state
    /// [`PolarBoxState::Arc`]); one out of the `basis[0]`/`basis[2]` plane makes a
    /// [`PolarBoxState::Region`]; one at 90 degrees or more from `basis[0]` makes the
    /// box [`PolarBoxState::Unbounded`]. `opt_basis2` (assumed unit length) is only
    /// used for the first vector: `basis[2]` is then set to it and `basis[0]` to the
    /// projection of `v` onto its normal plane.
    pub fn add_vector(&mut self, v: Vec3, opt_basis2: Option<Vec3>) {
        // zero vector
        if v.length_squared() < EFF_ZERO_SQ {
            return;
        }

        match self.state {
            PolarBoxState::Uninitialized => {
                self.state = PolarBoxState::SinglePoint;
                self.set_domain(0.0, 0.0, 0.0, 0.0);

                match opt_basis2 {
                    None => self.set_ortho_basis(v),
                    Some(basis2) => {
                        // basis[0] = projection of v onto the plane normal to basis2,
                        // basis[2] = basis2, basis[1] = basis[2] x basis[0]
                        self.basis[0] = basis2.cross(v).cross(basis2);
                        self.basis[2] = basis2;
                        let proj_length2 = self.basis[0].length_squared();
                        if proj_length2 > EFF_ZERO_SQ {
                            self.basis[0] = self.basis[0].unitize_or_z();
                            self.basis[2] = basis2;
                            self.basis[1] = self.basis[2].cross(self.basis[0]);
                            self.state = PolarBoxState::PointWithBasis;
                            self.add_vector(v, None);
                        } else {
                            self.set_ortho_basis(v);
                        }
                    }
                }
            }

            PolarBoxState::PointWithBasis | PolarBoxState::SinglePoint => {
                let cross = v.cross(self.basis[0]);
                let length = cross.length();
                let dot = self.basis[0].dot(v);

                if length < EFF_ZERO {
                    // parallel to the axis; opposite direction: unbounded
                    if dot < EFF_ZERO {
                        self.state = PolarBoxState::Unbounded;
                    }
                } else {
                    self.set_domain(0.0, 0.0, 0.0, 0.0);

                    if self.state == PolarBoxState::SinglePoint {
                        // v1 = unit(v x v0), v2 = unit(v0 x v1): v lies in the v0/v2
                        // plane. A preset basis (PointWithBasis) is kept.
                        self.basis[1] = cross.unitize_or_z();
                        self.basis[2] = self.basis[0].cross(self.basis[1]).unitize_or_z();
                    }

                    if dot < EFF_ZERO {
                        self.state = PolarBoxState::Unbounded;
                    } else {
                        self.state = PolarBoxState::Arc;
                        let angles = self.polar_coord(v);
                        self.domain.add_point(angles);
                        if angles.y.abs() > EFF_ZERO_SQRT {
                            self.state = PolarBoxState::Region;
                        }
                    }
                }
            }

            PolarBoxState::Arc => {
                let dot = self.basis[0].dot(v);
                if dot < EFF_ZERO {
                    // opposite direction: unbounded
                    self.state = PolarBoxState::Unbounded;
                } else {
                    let angles = self.polar_coord(v);
                    // out of the v0/v2 plane: becomes a region
                    if angles.y.abs() > EFF_ZERO_SQRT {
                        self.state = PolarBoxState::Region;
                    }
                    self.domain.add_point(angles);
                }
            }

            PolarBoxState::Region => {
                let dot = self.basis[0].dot(v);
                let scaled_zero = EFF_ZERO * (1.0 + v.max_dimension());
                if dot < scaled_zero {
                    // opposite direction: unbounded
                    self.state = PolarBoxState::Unbounded;
                } else {
                    let angles = self.polar_coord(v);
                    self.domain.add_point(angles);

                    // relabel arcs as arcs
                    if self.domain.x_length().abs() < EFF_ZERO_SQRT
                        && self.state != PolarBoxState::Unbounded
                    {
                        let tmp = -self.basis[1];
                        self.basis[1] = self.basis[2];
                        self.basis[2] = tmp;
                        // Upstream re-sets the domain to its own bounds (no transpose).
                        let (lo, hi) = (self.domain.min(), self.domain.max());
                        self.set_domain(lo.x, lo.y, hi.x, hi.y);
                        self.state = PolarBoxState::Arc;
                    }
                }
            }

            PolarBoxState::Unbounded => {}
        }
    }

    /// The 1 (point), 2 (arc) or 4 (region, counter-clockwise from the domain minimum)
    /// corner directions of the box (`GetBoundaryVectors`).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for uninitialised and unbounded boxes.
    pub fn boundary_vectors(&self) -> Result<Vec<Vec3>> {
        match self.state {
            PolarBoxState::Uninitialized | PolarBoxState::Unbounded => {
                Err(SolidError::InvalidInput {
                    operation: "geom::polar_box::boundary_vectors",
                    reason: "uninitialized or unbounded polar box",
                })
            }
            PolarBoxState::SinglePoint | PolarBoxState::PointWithBasis => Ok(vec![self.basis[0]]),
            PolarBoxState::Arc => Ok(vec![
                self.evaluate_normalized(0.0, 0.0),
                self.evaluate_normalized(1.0, 1.0),
            ]),
            PolarBoxState::Region => Ok(vec![
                self.evaluate_normalized(0.0, 0.0),
                self.evaluate_normalized(1.0, 0.0),
                self.evaluate_normalized(1.0, 1.0),
                self.evaluate_normalized(0.0, 1.0),
            ]),
        }
    }

    /// True when the box may contain a direction perpendicular to `test`
    /// (`HasPerpendicularToVector`): always for uninitialised and unbounded boxes,
    /// otherwise when a corner is (nearly) perpendicular to `test` or the corners lie
    /// on both sides of its normal plane.
    #[must_use]
    pub fn has_perpendicular_to_vector(&self, test: Vec3) -> bool {
        const TOL: f64 = EFF_ZERO_SQRT * 100.0;

        let Ok(vectors) = self.boundary_vectors() else {
            // uninitialised or unbounded: every vector is in the box
            return true;
        };
        let unit_test = test.unitize_or_z();

        if let [only] = vectors.as_slice() {
            return only.dot(unit_test).abs() < TOL;
        }

        // the corners have a perpendicular between them unless all dot products have
        // the same sign
        let mut sign = 0_i32;
        for v in &vectors {
            if v.dot(unit_test).abs() < TOL {
                return true;
            }
            if v.dot(test) > TOL {
                if sign < 0 {
                    return true;
                } else if sign == 0 {
                    sign = 1;
                }
            }
            if v.dot(test) < -TOL {
                if sign > 0 {
                    return true;
                } else if sign == 0 {
                    sign = -1;
                }
            }
        }
        false
    }

    /// A box containing both boxes, centred on the sum of their polar axes (`Union`
    /// with a separate result box). Unbounded when either is unbounded or their axes
    /// are 90 degrees or more apart.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] when the boxes pass the axis tests but one is
    /// uninitialised (upstream reads an unset corner count here).
    pub fn union(&self, other: &Self) -> Result<Self> {
        let mut out = Self::new();
        if let Some(mid) = Self::union_mid(self, other) {
            out.add_vector(mid, None);
            Self::union_finish(&mut out, self.state, &self.boundary_vectors()?, other)?;
        } else {
            out.state = PolarBoxState::Unbounded;
        }
        Ok(out)
    }

    /// `self.Union(other, self)`: [`PolarBox::union`] with the result written over
    /// `self` while upstream still reads `self` (as `SmCompositeCurve` calls it). After
    /// the reset, `self` contributes only the mid vector, so the original extent of
    /// `self` is not kept.
    ///
    /// # Errors
    ///
    /// As [`PolarBox::union`] (`self` may then be left partly updated).
    pub fn union_assign(&mut self, other: &Self) -> Result<()> {
        if let Some(mid) = Self::union_mid(self, other) {
            self.reset();
            self.add_vector(mid, None);
            let this_vecs = self.boundary_vectors()?;
            let state = self.state;
            Self::union_finish(self, state, &this_vecs, other)?;
        } else {
            self.state = PolarBoxState::Unbounded;
        }
        Ok(())
    }

    /// `Union`'s early exits: `None` when the result is unbounded, else the mid vector.
    fn union_mid(a: &Self, b: &Self) -> Option<Vec3> {
        if a.state == PolarBoxState::Unbounded || b.state == PolarBoxState::Unbounded {
            return None;
        }
        // guess a mid vector
        let mid = a.basis[0] + b.basis[0];
        let dot = a.basis[0].dot(b.basis[0]);
        // the mid vector vanishes or the axes oppose one another
        if mid.length_squared() < EFF_ZERO_SQ || dot < EFF_ZERO {
            return None;
        }
        Some(mid)
    }

    /// `Union` after the mid vector is in `out`: adds both boxes' corners and keeps
    /// the more general state.
    fn union_finish(
        out: &mut Self,
        this_state: PolarBoxState,
        this_vecs: &[Vec3],
        other: &Self,
    ) -> Result<()> {
        let other_vecs = other.boundary_vectors()?;
        for v in this_vecs {
            out.add_vector(*v, None);
        }
        for v in &other_vecs {
            out.add_vector(*v, None);
        }
        // the result is at least as general as either input
        if out.state < this_state {
            out.state = this_state;
        }
        if out.state < other.state {
            out.state = other.state;
        }
        Ok(())
    }

    /// Grows the domain by `expansion` radians on every side without changing the
    /// state, except that a box reaching `±(PI - SM_EFF_ZERO)` becomes unbounded
    /// (`ExpandAbsolute`). Arcs grow in the first angle only; point, uninitialised and
    /// unbounded boxes are unchanged.
    pub fn expand_absolute(&mut self, expansion: f64) {
        let near_pi = PI - EFF_ZERO;

        match self.state {
            PolarBoxState::Arc => {
                // only the first angle
                let mut lo = self.domain.min();
                let mut hi = self.domain.max();
                lo.x -= expansion;
                hi.x += expansion;
                // status ignored upstream: an inverted range leaves the domain as is
                let _ = self.domain.set_min_max(lo, hi);
                if lo.x <= -near_pi || hi.x >= near_pi {
                    self.state = PolarBoxState::Unbounded;
                }
            }
            PolarBoxState::Region => {
                self.domain.expand_absolute(expansion);
                let (lo, hi) = (self.domain.min(), self.domain.max());
                if lo.x <= -near_pi || lo.y <= -near_pi || hi.x >= near_pi || hi.y >= near_pi {
                    self.state = PolarBoxState::Unbounded;
                }
            }
            PolarBoxState::Uninitialized
            | PolarBoxState::PointWithBasis
            | PolarBoxState::SinglePoint
            | PolarBoxState::Unbounded => {}
        }
    }
}

/// Signed angle from `b0` of the unit projection `proj` about `axis`
/// (`ComputePolarCoord`'s inlined `CCWAngleBetween`).
fn projected_angle(b0: Vec3, axis: Vec3, proj: Vec3) -> f64 {
    let dot = proj.dot(b0);
    // keep `acos` in range and skip it near 0 and PI
    let mut angle = if dot > 1.0 - EFF_ZERO {
        0.0
    } else if dot < -1.0 + EFF_ZERO {
        PI
    } else {
        acos(dot)
    };
    // counter-clockwise when viewed from the tip of `axis`
    let cross = b0.cross(proj);
    if cross.dot(axis) < 0.0 {
        angle = -angle;
    }
    angle
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    #[test]
    fn states_progress() {
        let mut b = PolarBox::new();
        assert_eq!(b.state(), PolarBoxState::Uninitialized);
        b.add_vector(v(0.0, 0.0, 0.0), None);
        assert_eq!(b.state(), PolarBoxState::Uninitialized);
        b.add_vector(v(0.0, 0.0, 2.0), None);
        assert_eq!(b.state(), PolarBoxState::SinglePoint);
        assert_eq!(b.basis()[0], v(0.0, 0.0, 1.0));
        b.add_vector(v(0.0, 0.0, 5.0), None);
        assert_eq!(b.state(), PolarBoxState::SinglePoint);
        b.add_vector(v(1.0, 0.0, 1.0), None);
        assert_eq!(b.state(), PolarBoxState::Arc);
        let d = b.domain();
        assert!((d.x_length() - PI / 4.0).abs() < 1e-12);
        b.add_vector(v(0.0, 1.0, 1.0), None);
        assert_eq!(b.state(), PolarBoxState::Region);
        assert!(b.polar_area() > 0.0);
        b.add_vector(v(0.0, 0.0, -1.0), None);
        assert_eq!(b.state(), PolarBoxState::Unbounded);
        assert_eq!(b.polar_area(), 4.0 * PI);
        assert!(b.compute_polar_coord(v(1.0, 0.0, 0.0)).is_err());
    }

    #[test]
    fn polar_coord_round_trip() {
        let mut b = PolarBox::from_vector(v(1.0, 1.0, 1.0));
        b.add_vector(v(1.0, 0.2, 0.9), None);
        b.add_vector(v(0.8, 1.1, 0.7), None);
        assert_eq!(b.state(), PolarBoxState::Region);
        for (a, c) in [(0.0, 0.0), (0.3, 0.7), (1.0, 1.0)] {
            let dir = b.evaluate_normalized(a, c);
            let p = b.compute_polar_coord(dir).unwrap_or(Vec2::UNDEF);
            let q = b.domain().evaluate(a, c);
            assert!((p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9);
        }
        assert_eq!(b.boundary_vectors().map(|v| v.len()).ok(), Some(4));
    }

    #[test]
    fn opposite_is_unbounded() {
        let mut b = PolarBox::from_vector(v(1.0, 0.0, 0.0));
        b.add_vector(v(-1.0, 0.0, 0.0), None);
        assert_eq!(b.state(), PolarBoxState::Unbounded);
        assert!(!b.are_disjoint(&PolarBox::from_vector(v(0.0, 1.0, 0.0))));
        assert!(b.has_perpendicular_to_vector(v(1.0, 0.0, 0.0)));
    }

    #[test]
    fn disjoint_points_and_cones() {
        let a = PolarBox::from_vector(v(1.0, 0.0, 0.0));
        let b = PolarBox::from_vector(v(0.0, 1.0, 0.0));
        assert!(a.are_disjoint(&b));
        assert!(!a.are_disjoint(&PolarBox::from_vector(v(-2.0, 0.0, 0.0))));
        assert!(a.is_disjoint_with_cone(v(0.0, 1.0, 0.0), 0.1));
        assert!(!a.is_disjoint_with_cone(v(1.0, 0.05, 0.0), 0.1));
        assert!(!a.is_perpendicular_disjoint_with_cone(v(0.0, 1.0, 0.0), 0.1));
    }

    #[test]
    fn cross_and_union() {
        let mut du = PolarBox::from_vector(v(1.0, 0.0, 0.0));
        du.add_vector(v(1.0, 0.1, 0.0), None);
        let dv = PolarBox::from_vector(v(0.0, 1.0, 0.0));
        let n = du.cross(&dv);
        assert!(n.state() >= PolarBoxState::SinglePoint);
        let nd = n.evaluate_normalized(0.5, 0.5);
        assert!(nd.z > 0.9);

        let u = du.union(&dv);
        assert!(u.is_ok());
        let unb = PolarBox::from_vector(v(1.0, 0.0, 0.0))
            .union(&PolarBox::from_vector(v(-1.0, 0.0, 0.0)))
            .map(|b| b.state());
        assert_eq!(unb.ok(), Some(PolarBoxState::Unbounded));

        let mut stale = PolarBox::from_vector(v(1.0, 0.0, 0.0));
        stale.reset();
        assert!(stale.union(&dv).is_ok());
        assert!(stale
            .union(&PolarBox::from_vector(v(1.0, 0.1, 0.0)))
            .is_err());

        let mut p = du;
        assert!(p
            .union_assign(&PolarBox::from_vector(v(1.0, 0.0, 0.2)))
            .is_ok());
        assert_eq!(p.state(), PolarBoxState::Arc);
        assert!(p.domain().x_length() > 0.0);
    }

    #[test]
    fn expand_absolute_unbounds() {
        let mut b = PolarBox::from_vector(v(1.0, 0.0, 0.0));
        b.add_vector(v(1.0, 0.5, 0.0), None);
        assert_eq!(b.state(), PolarBoxState::Arc);
        b.expand_absolute(0.1);
        assert_eq!(b.state(), PolarBoxState::Arc);
        b.expand_absolute(3.0);
        assert_eq!(b.state(), PolarBoxState::Unbounded);
    }

    #[test]
    fn optional_basis() {
        let mut b = PolarBox::new();
        b.add_vector(v(1.0, 0.0, 1.0), Some(v(0.0, 0.0, 1.0)));
        assert_eq!(b.basis()[2], v(0.0, 0.0, 1.0));
        assert_eq!(b.state(), PolarBoxState::Arc);
        let mut c = PolarBox::new();
        c.assign(&b);
        assert_eq!(c, b);
        let mut d = PolarBox::new();
        d.add_vector(v(0.0, 0.0, 3.0), Some(v(0.0, 0.0, 1.0)));
        assert_eq!(d.state(), PolarBoxState::SinglePoint);
    }
}
