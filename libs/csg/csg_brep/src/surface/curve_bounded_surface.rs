//! Curve bounded (trimmed) surfaces: a basis surface with 3D and UV boundary curves,
//! used for IGES I/O. Evaluation passes every call along to the basis surface.
//!
//!
//! Upstream: `source/SMLib/src/SmCurveBoundedSurface.cpp`,
//! `source/SMLib/inc/SmCurveBoundedSurface.h`.
//!
//! | Rust                                                  | Upstream                                       |
//! |-------------------------------------------------------|------------------------------------------------|
//! | [`CurveBoundedSurface`]                               | `SmCurveBoundedSurface`                        |
//! | [`CurveBoundedSurface::new`]                          | `SmCurveBoundedSurface(rBasisSurface, cp3DBoundaries, cpUVBoundaries, bImplicitOuter)` |
//! | `Drop` (implicit)                                     | `~SmCurveBoundedSurface`                       |
//! | [`CurveBoundedSurface::approx_eq`]                    | `operator==`                                   |
//! | [`CurveBoundedSurface::natural_uv_domain`]            | `GetNaturalUVDomain`                           |
//! | [`CurveBoundedSurface::evaluate_normal`]              | `EvaluateNormal`                               |
//! | [`CurveBoundedSurface::evaluate`]                     | `Evaluate`                                     |
//! | [`CurveBoundedSurface::evaluate_simple`]              | `EvaluateSimple`                               |
//! | [`CurveBoundedSurface::evaluate_normal_at_singularity`] | `EvaluateNormalAtSingularity`                |
//! | [`CurveBoundedSurface::evaluate_geometric`]           | `EvaluateGeometric`                            |
//! | [`CurveBoundedSurface::evaluate_normal_section`]      | `EvaluateNormalSection`                        |
//! | [`CurveBoundedSurface::evaluate_point`]               | `EvaluatePoint`                                |
//! | [`CurveBoundedSurface::evaluate_step`]                | `EvaluateSTEP`                                 |
//! | [`CurveBoundedSurface::evaluate_step_point`]          | `EvaluateSTEPPoint`                            |
//! | [`CurveBoundedSurface::set_context`]                  | `SetContext`                                   |
//! | [`TYPE`], [`TYPE_STRING`], [`CLASS_STRING`], [`CurveBoundedSurface::type_code`], [`CurveBoundedSurface::type_string`], [`CurveBoundedSurface::class_string`] | `GetType`, `GetTypeString`, `GetClassString` (`SM_COMMON`) |
//! | [`is_kind_of`], [`CurveBoundedSurface::is_kind_of`]   | `IsKindOf`                                     |
//! | [`CurveBoundedSurface::read_from_db`]                 | `ReadFromDB`                                   |
//!
//! Deviations:
//! - **Nesting.** `new` returns `InvalidInput` when the basis or a boundary curve would nest
//!   deeper than [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING)
//!   (32; upstream fixtures nest at most 3), which bounds the recursive dispatch,
//!   cloning and dropping of nested geometry (no limit upstream).
//! - Compound evaluation copies carry a depth counter checked by `Surface` against the same bound.
//! - **Ownership.** The surface owns its basis surface and boundary curves, as
//!   upstream (whose destructor deletes them): they are moved into
//!   [`CurveBoundedSurface::new`] (a missing upstream array is an empty `Vec`), and the
//!   boundary arrays hold curves, never null pointers. The accessors
//!   ([`CurveBoundedSurface::basis`], ...) are not upstream (the members are private).
//! - **Object model.** The surface is the [`SurfaceKind::CurveBounded`] kind of
//!   [`Surface`]; the `SmSurface` virtuals it does not override run the base-class
//!   defaults there. Its own overrides pass the call along to the basis surface.
//! - **Unported callees.** The basis surface's `EvaluateNormalAtSingularity`
//!   (`surface.surface_p5`) is not in the [`Surface`] dispatch yet: that call returns
//!   [`SolidError::Unported`]. The other basis-surface calls (`EvaluateSimple`,
//!   `EvaluateSTEP`, `EvaluateSTEPPoint`, `operator==`, `EvaluateGeometric`,
//!   `EvaluateNormalSection`) are wired.
//! - **`operator==`** keeps upstream's loop bound: the UV boundaries are compared over
//!   the number of 3D boundaries. Reading past the UV arrays (undefined upstream) ends
//!   the comparison with the elements compared so far.
//! - **`SetContext`** has nothing to set: the kernel is passed to the calls that need
//!   it.
//! - **`ReadFromDB`** keeps upstream's bugs: the UV boundaries are read with the type
//!   of the last 3D boundary (`0`, not a curve type, when there is none), and the
//!   implicit-outer flag read from the stream is dropped (the surface gets `FALSE`).
//!   A failed ASCII extraction of a count or the flag stores `0` (C++11 `operator>>`).
//!   Not upstream: the read is bounded by [`DatabaseReader::enter_nested`] (a basis
//!   surface can be another curve bounded surface) and each boundary count by the
//!   bytes left in the stream ([`DatabaseReader::ensure_available`]).

use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::EFF_ZERO;
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::surface::p2::{
    self as surface_p2, check_geometry_nesting, GeometricProperties, NestedGeometry, Surface,
    SurfaceKind,
};
use crate::topology::database_io::{DatabaseReader, FileType};

const OP_NEW: &str = "surface::curve_bounded_surface::CurveBoundedSurface::new";
const OP_READ: &str = "surface::curve_bounded_surface::CurveBoundedSurface::read_from_db";

/// `SmCurveBoundedSurface_TYPE`.
pub const TYPE: i64 = surface_p2::CURVE_BOUNDED_SURFACE_TYPE;
/// `GetTypeString()` of `SmCurveBoundedSurface`.
pub const TYPE_STRING: &str = "SmCurveBoundedSurface_TYPE";
/// `GetClassString()` of `SmCurveBoundedSurface`.
pub const CLASS_STRING: &str = "SmCurveBoundedSurface";

/// `SmCurveBoundedSurface::IsKindOf`: `SmCurveBoundedSurface_TYPE` or a base type.
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    TYPE == t || surface_p2::is_kind_of(t)
}

/// The outputs of [`CurveBoundedSurface::evaluate_normal_section`]
/// (`EvaluateNormalSection`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalSection {
    /// `rTangentPlaneDirection`: the unit direction projected into the tangent plane.
    pub tangent_plane_direction: Vec3,
    /// `rdCurvatureValue`: the curvature of the surface curve in that direction.
    pub curvature: f64,
    /// `rSurfaceNormalVector`: the unit surface normal.
    pub normal: Vec3,
}

/// A trimmed surface (`SmCurveBoundedSurface`): a basis surface bounded by 3D curves
/// and their UV counterparts.
#[derive(Debug, Clone)]
pub struct CurveBoundedSurface {
    /// `m_pBasisSurface`.
    basis: Box<Surface>,
    /// `m_v3DBoundaries`.
    boundaries_3d: Vec<Curve>,
    /// `m_vUVBoundaries`.
    boundaries_uv: Vec<Curve>,
    /// `m_bImplicitOuter`.
    implicit_outer: bool,
}

impl From<CurveBoundedSurface> for Surface {
    fn from(s: CurveBoundedSurface) -> Self {
        Self::new(SurfaceKind::CurveBounded(s))
    }
}

impl CurveBoundedSurface {
    /// Exact evaluation copy; the caller validates the complete geometry graph first.
    pub(crate) fn clone_exact_bounded(&self) -> Self {
        Self {
            basis: Box::new(self.basis.clone_exact_bounded()),
            boundaries_3d: self
                .boundaries_3d
                .iter()
                .map(Curve::clone_exact_bounded)
                .collect(),
            boundaries_uv: self
                .boundaries_uv
                .iter()
                .map(Curve::clone_exact_bounded)
                .collect(),
            implicit_outer: self.implicit_outer,
        }
    }

    /// The basis surface, mutable, for [`Surface::detach_nurb_cells`].
    pub(crate) fn nested_surfaces_mut(&mut self) -> Vec<&mut Surface> {
        vec![self.basis.as_mut()]
    }

    /// An evaluation copy retaining exact basis data, with Surface's `MAX_GEOMETRY_NESTING` bound.
    pub(crate) fn clone_for_evaluation(&self, depth: usize) -> Result<Self> {
        Ok(Self {
            basis: Box::new(self.basis.clone_for_evaluation_depth(depth)?),
            boundaries_3d: self
                .boundaries_3d
                .iter()
                .map(|c| c.clone_for_evaluation_depth(depth))
                .collect::<Result<_>>()?,
            boundaries_uv: self
                .boundaries_uv
                .iter()
                .map(|c| c.clone_for_evaluation_depth(depth))
                .collect::<Result<_>>()?,
            implicit_outer: self.implicit_outer,
        })
    }

    /// A surface owning `basis` and the boundary curves (the constructor; the curves
    /// are assumed valid).
    ///
    /// # Errors
    ///
    /// `InvalidInput` when the basis or a boundary curve nests deeper than
    /// [`MAX_GEOMETRY_NESTING`](crate::surface::surface::p2::MAX_GEOMETRY_NESTING) - 1.
    pub fn new(
        basis: Surface,
        boundaries_3d: Vec<Curve>,
        boundaries_uv: Vec<Curve>,
        implicit_outer: bool,
    ) -> Result<Self> {
        check_geometry_nesting(NestedGeometry::Surface(&basis), 1, OP_NEW)?;
        for c in boundaries_3d.iter().chain(&boundaries_uv) {
            check_geometry_nesting(NestedGeometry::Curve(c), 1, OP_NEW)?;
        }
        Ok(Self {
            basis: Box::new(basis),
            boundaries_3d,
            boundaries_uv,
            implicit_outer,
        })
    }

    /// The basis surface.
    #[must_use]
    pub fn basis(&self) -> &Surface {
        &self.basis
    }

    /// The 3D boundary curves.
    #[must_use]
    pub fn boundaries_3d(&self) -> &[Curve] {
        &self.boundaries_3d
    }

    /// The UV boundary curves.
    #[must_use]
    pub fn boundaries_uv(&self) -> &[Curve] {
        &self.boundaries_uv
    }

    /// Whether the outer boundary is implicit (the basis surface's boundary).
    #[must_use]
    pub const fn implicit_outer(&self) -> bool {
        self.implicit_outer
    }

    // ---- type queries ---------------------------------------------------------------

    /// `SmCurveBoundedSurface_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        TYPE
    }

    /// `"SmCurveBoundedSurface_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        TYPE_STRING
    }

    /// `"SmCurveBoundedSurface"` (`GetClassString`).
    #[must_use]
    pub const fn class_string(&self) -> &'static str {
        CLASS_STRING
    }

    /// True for `SmCurveBoundedSurface_TYPE` and its base types (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        is_kind_of(t)
    }

    /// Sets the context of the surface and its curves (`SetContext`): nothing to set.
    pub const fn set_context(&self) {}

    // ---- equality -------------------------------------------------------------------

    /// `operator==`: the same object, or the base-class test and then equal basis
    /// surfaces, boundary counts, implicit-outer flags and boundary curves.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`]: `SmSurface::operator==` (`surface.surface_p1`) for
    /// a different object; those of [`Curve::approx_eq`].
    pub fn approx_eq(&self, k: &Kernel, other: &Surface) -> Result<bool> {
        // low work
        if let SurfaceKind::CurveBounded(o) = other.kind() {
            if core::ptr::eq(self, o) {
                return Ok(true);
            }
        }

        // first check the base
        let mut rtn = surface_base_eq(self, other);

        if rtn {
            // OK to cast
            let SurfaceKind::CurveBounded(o) = other.kind() else {
                return Ok(false);
            };

            // check equivalence of these objects
            rtn = (core::ptr::eq(&raw const *self.basis, &raw const *o.basis)
                || surface_eq(k, &self.basis, &o.basis)?)
                && self.boundaries_3d.len() == o.boundaries_3d.len()
                && self.boundaries_uv.len() == o.boundaries_uv.len()
                && self.implicit_outer == o.implicit_outer;

            // check the array elements
            if rtn {
                for (a, b) in self.boundaries_3d.iter().zip(&o.boundaries_3d) {
                    if !rtn {
                        break;
                    }
                    rtn &= core::ptr::eq(a, b) || a.approx_eq(k, b)?;
                }

                // upstream bound: the number of 3D boundaries
                for ii in 0..self.boundaries_3d.len() {
                    if !rtn {
                        break;
                    }
                    let (Some(a), Some(b)) = (self.boundaries_uv.get(ii), o.boundaries_uv.get(ii))
                    else {
                        break;
                    };
                    rtn &= core::ptr::eq(a, b) || a.approx_eq(k, b)?;
                }
            }
        }

        // all done
        Ok(rtn)
    }

    // ---- evaluation: passed along to the basis surface ------------------------------

    /// The basis surface's natural parameter domain (`GetNaturalUVDomain`).
    #[must_use]
    pub fn natural_uv_domain(&self) -> Extent2d {
        // (kernel-free: a lazy analytic answers its NURB-less base's empty domain)
        self.basis.natural_uv_domain_if_built()
    }

    /// The basis surface's unit normal at `uv` (`EvaluateNormal`).
    ///
    /// # Errors
    ///
    /// Those of the basis surface's [`Surface::evaluate_normal`].
    pub fn evaluate_normal(
        &self,
        k: &Kernel,
        uv: Point2,
        u_side: Side,
        v_side: Side,
    ) -> Result<Vec3> {
        // pass the call along
        self.basis.evaluate_normal(k, uv, u_side, v_side)
    }

    /// The basis surface's point and partial derivatives at `uv` (`Evaluate`).
    ///
    /// # Errors
    ///
    /// Those of the basis surface's [`Surface::evaluate`].
    pub fn evaluate(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        flags: EvaluateFlags,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.basis
            .evaluate(k, uv, highest_u_deriv, highest_v_deriv, flags)
    }

    /// The basis surface's derivatives at `uv` without zero-tangent adjustments
    /// (`EvaluateSimple`).
    ///
    /// # Errors
    ///
    /// Those of the basis surface's evaluation.
    pub fn evaluate_simple(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        u_side: Side,
        v_side: Side,
        only_upper_half: bool,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.basis.evaluate_simple(
            k,
            uv,
            highest_u_deriv,
            highest_v_deriv,
            EvaluateFlags {
                u_side,
                v_side,
                only_upper_half,
                non_zero_tangents: false,
            },
        )
    }

    /// The basis surface's normal at a singular `uv` (`EvaluateNormalAtSingularity`;
    /// upstream's default `d3dTol` is [`EFF_ZERO`]).
    ///
    /// # Errors
    ///
    /// Those of [`Surface::evaluate_normal_at_singularity`] of the basis surface.
    pub fn evaluate_normal_at_singularity(
        &self,
        k: &Kernel,
        uv: Point2,
        tol_3d: f64,
    ) -> Result<Vec3> {
        self.basis().evaluate_normal_at_singularity(k, uv, tol_3d)
    }

    /// The default `d3dTol` of [`Self::evaluate_normal_at_singularity`].
    pub const SINGULARITY_TOL: f64 = EFF_ZERO;

    /// The basis surface's curvatures, fundamental forms and principal directions at
    /// `uv` (`EvaluateGeometric`).
    ///
    /// # Errors
    ///
    /// Those of the basis surface's [`Surface::evaluate_geometric`].
    pub fn evaluate_geometric(
        &self,
        k: &Kernel,
        uv: Point2,
        u_side: Side,
        v_side: Side,
    ) -> Result<GeometricProperties> {
        self.basis.evaluate_geometric(k, uv, u_side, v_side)
    }

    /// The basis surface's normal section in `direction` at `uv`
    /// (`EvaluateNormalSection`).
    ///
    /// # Errors
    ///
    /// Those of the basis surface's [`Surface::evaluate_normal_section`].
    pub fn evaluate_normal_section(
        &self,
        k: &Kernel,
        uv: Point2,
        u_side: Side,
        v_side: Side,
        direction: Vec3,
    ) -> Result<NormalSection> {
        let section = self
            .basis
            .evaluate_normal_section(k, uv, u_side, v_side, direction)?;
        Ok(NormalSection {
            tangent_plane_direction: section.tangent,
            curvature: section.curvature,
            normal: section.normal,
        })
    }

    /// The basis surface's point at `uv` (`EvaluatePoint`).
    ///
    /// # Errors
    ///
    /// Those of the basis surface's [`Surface::evaluate_point`].
    pub fn evaluate_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        self.basis.evaluate_point(k, uv)
    }

    /// The basis surface's derivatives at `uv` in its STEP parameterisation
    /// (`EvaluateSTEP`).
    ///
    /// # Errors
    ///
    /// Those of the basis surface's evaluation.
    pub fn evaluate_step(
        &self,
        k: &Kernel,
        uv: Point2,
        highest_u_deriv: usize,
        highest_v_deriv: usize,
        u_side: Side,
        v_side: Side,
        only_upper_half: bool,
    ) -> Result<Vec<Vec<Vec3>>> {
        self.basis.evaluate_step(
            k,
            uv,
            highest_u_deriv,
            highest_v_deriv,
            EvaluateFlags {
                u_side,
                v_side,
                only_upper_half,
                non_zero_tangents: true,
            },
        )
    }

    /// The basis surface's point at `uv` in its STEP parameterisation
    /// (`EvaluateSTEPPoint`).
    ///
    /// # Errors
    ///
    /// Those of the basis surface's evaluation.
    pub fn evaluate_step_point(&self, k: &Kernel, uv: Point2) -> Result<Point3> {
        self.basis.evaluate_step_point(k, uv)
    }

    // ---- database -------------------------------------------------------------------

    /// Reads a curve bounded surface written by `WriteToDB` (`ReadFromDB`): the
    /// boundary counts and implicit-outer flag, the basis surface (type header and
    /// record), then every 3D and every UV boundary curve (type and dimension header
    /// and record).
    ///
    /// # Errors
    ///
    /// Those of the stream reads, of [`Surface::read_from_db`] and of
    /// [`Curve::read_from_db`]; [`SolidError::InvalidInput`] for an unreadable type
    /// header (undefined upstream), nesting too deep or a boundary count larger than
    /// the input.
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        db.enter_nested(OP_READ)?;
        let result = read_body(k, db, version);
        db.leave_nested();
        result
    }
}

/// `SmSurface::operator==(crOther)` on `this` (the base-class test).
fn surface_base_eq(this: &CurveBoundedSurface, other: &Surface) -> bool {
    // (the same object was handled by the caller; the same type)
    this.type_code() == other.type_code()
}

/// `*a == *b` for two surfaces (the virtual `operator==`).
fn surface_eq(k: &Kernel, a: &Surface, b: &Surface) -> Result<bool> {
    a.approx_eq(k, b)
}

/// An ASCII count or flag: `0` when the extraction fails (C++11 `operator>>`).
fn ascii_ulong(db: &mut DatabaseReader) -> u64 {
    let v = db
        .in_stream_mut()
        .and_then(crate::topology::database_io::InputStream::extract_ulong)
        .unwrap_or(0);
    db.go_to_next_line();
    v
}

/// A boundary count as a length, bounded by the bytes left (each curve takes at
/// least one).
fn boundary_count(db: &DatabaseReader, count: u64) -> Result<usize> {
    let n = usize::try_from(count)
        .map_err(|_| SolidError::invalid_input(OP_READ, "boundary count too large"))?;
    db.ensure_available(n, 1, OP_READ)?;
    Ok(n)
}

/// A curve type header (`ReadType(type, &dim)`).
fn read_curve_type(db: &mut DatabaseReader) -> Result<(i64, u64)> {
    let header = db.read_type(true)?;
    let ty = header
        .type_code
        .ok_or_else(|| SolidError::invalid_input(OP_READ, "curve type not read"))?;
    let dim = header
        .dim
        .ok_or_else(|| SolidError::invalid_input(OP_READ, "curve dimension not read"))?;
    Ok((ty, dim))
}

/// The body of [`CurveBoundedSurface::read_from_db`].
fn read_body(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<CurveBoundedSurface> {
    // file type
    let ascii = db.file_type() == FileType::Ascii;

    // locals
    let (l3d_boundaries_size, luv_boundaries_size) = if ascii {
        let a = ascii_ulong(db);
        let b = ascii_ulong(db);
        let _implicit_outer = db
            .in_stream_mut()
            .and_then(crate::topology::database_io::InputStream::extract_int)
            .unwrap_or(0);
        db.go_to_next_line();
        (a, b)
    } else {
        let a = db.read_ulong()?;
        let b = db.read_ulong()?;
        let _implicit_outer = db.read_boolean()?;
        (a, b)
    };

    // surface locals
    let mut s3d_curves = Vec::new();
    let mut suv_curves = Vec::new();
    let mut l3d_type: i64 = 0;

    // basis surface
    if ascii {
        db.go_to_next_line();
    }
    let basis_type = db
        .read_type(false)?
        .type_code
        .ok_or_else(|| SolidError::invalid_input(OP_READ, "basis surface type not read"))?;
    let basis_surface = Surface::read_from_db(k, basis_type, db, version)?;

    // for every 3d curve
    let n3d = boundary_count(db, l3d_boundaries_size)?;
    for _ in 0..n3d {
        let (ty, l3d_dim) = read_curve_type(db)?;
        l3d_type = ty;
        s3d_curves.push(Curve::read_from_db(k, l3d_type, db, l3d_dim, version)?);
    }

    // for every UV curve (read with the type of the last 3D curve, as upstream)
    let nuv = boundary_count(db, luv_boundaries_size)?;
    for _ in 0..nuv {
        let (_luv_type, luv_dim) = read_curve_type(db)?;
        suv_curves.push(Curve::read_from_db(k, l3d_type, db, luv_dim, version)?);
    }

    // all done (the implicit-outer flag read above is not passed on)
    CurveBoundedSurface::new(basis_surface, s3d_curves, suv_curves, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::curve::p2::BSPLINE_CURVE_TYPE;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::b_spline_surface::BSplineSurface;

    fn patch() -> BSplineSurface {
        BSplineSurface::create_bilinear_surface(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(0.0, 3.0, 0.0),
            Point3::new(2.0, 3.0, 1.0),
        )
        .unwrap()
    }

    fn line_curve(a: Point3, b: Point3) -> Curve {
        Curve::from(BSplineCurve::create_line_segment(3, a, b).unwrap())
    }

    fn cbs() -> CurveBoundedSurface {
        CurveBoundedSurface::new(
            Surface::from(patch()),
            vec![line_curve(
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(2.0, 0.0, 0.0),
            )],
            vec![line_curve(
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
            )],
            true,
        )
        .unwrap()
    }

    #[test]
    fn types() {
        let s = Surface::from(cbs());
        assert_eq!(s.type_code(), TYPE);
        assert_eq!(s.type_string(), TYPE_STRING);
        assert_eq!(s.class_string(), CLASS_STRING);
        assert!(s.is_kind_of(TYPE).unwrap());
        assert!(s.is_kind_of(surface_p2::TYPE).unwrap());
        assert!(!s.is_kind_of(BSplineSurface::TYPE).unwrap());
        assert!(!is_kind_of(BSPLINE_CURVE_TYPE));
    }

    #[test]
    fn passes_evaluation_to_basis() {
        let k = &Kernel::default();
        let b = patch();
        let c = cbs();
        let s = Surface::from(c.clone());
        assert_eq!(s.natural_uv_domain_if_built(), b.natural_uv_domain());
        let uv = Point2::new(0.25, 0.5);
        assert_eq!(
            s.evaluate_point(k, uv).unwrap(),
            b.evaluate_point(uv).unwrap()
        );
        let f = EvaluateFlags::default();
        assert_eq!(
            s.evaluate(k, uv, 1, 1, f).unwrap(),
            b.evaluate(uv, 1, 1, f).unwrap()
        );
        // base-class defaults for what the class does not override
        assert_eq!(
            s.degree(crate::nurbs::b_spline_surface::SurfParamType::U),
            surface_p2::base_degree()
        );
        assert!(s.copy(&Kernel::default()).is_err());
        assert_eq!(
            c.evaluate_normal(k, uv, Side::Left, Side::Left).unwrap(),
            Surface::from(b.clone())
                .evaluate_normal(k, uv, Side::Left, Side::Left)
                .unwrap()
        );
        assert_eq!(
            c.evaluate_step_point(k, uv).unwrap(),
            b.evaluate_point(uv).unwrap()
        );
    }

    #[test]
    fn equality() {
        let k = &Kernel::default();
        let c = cbs();
        let s = Surface::from(c.clone());
        let SurfaceKind::CurveBounded(inner) = s.kind() else {
            panic!("kind");
        };
        assert!(inner.approx_eq(k, &s).unwrap());
        assert!(c.approx_eq(k, &s).unwrap());
        assert_eq!(c.boundaries_3d().len(), 1);
        assert_eq!(c.boundaries_uv().len(), 1);
        assert!(c.implicit_outer());
        c.set_context();
    }

    fn surface_text() -> String {
        // bilinear patch, version 30 layout
        "1 1\n1 1\n0\n0 0 0\n0 3 0\n2 0 0\n2 3 1\n0\n0\n1\n1\n0\n0\n1\n1\n".to_string()
    }

    fn curve_text(dim: u64) -> String {
        let pts = if dim == 2 {
            "0 0\n1 0\n"
        } else {
            "0 0 0\n2 0 0\n"
        };
        format!(
            "Object Type {BSPLINE_CURVE_TYPE}\nCurve Dim {dim}\n1\n1\n0\n{dim}\n{pts}0\n0\n1\n1\n"
        )
    }

    fn reader(text: &str) -> DatabaseReader {
        let mut db = DatabaseReader::new();
        db.open_for_read(text.as_bytes().to_vec(), FileType::Ascii);
        db
    }

    #[test]
    fn read_ascii() {
        let k = Kernel::default();
        let text = format!(
            "1 n3d\n1 nuv\n1 implicit\n basis\nObject Type {}\n{}{}{}",
            BSplineSurface::TYPE,
            surface_text(),
            curve_text(3),
            curve_text(2)
        );
        let s = CurveBoundedSurface::read_from_db(&k, &mut reader(&text), 30).unwrap();
        assert_eq!(s.boundaries_3d().len(), 1);
        assert_eq!(s.boundaries_uv().len(), 1);
        assert_eq!(s.boundaries_uv()[0].dim(), 2);
        assert!(!s.implicit_outer());
        assert_eq!(s.basis().type_code(), BSplineSurface::TYPE);

        // through the surface dispatch
        let r = Surface::read_from_db(&k, TYPE, &mut reader(&text), 30).unwrap();
        assert_eq!(r.type_code(), TYPE);
    }

    #[test]
    fn read_uv_without_3d_uses_type_zero() {
        let k = Kernel::default();
        let text = format!(
            "0\n1\n0\n\nObject Type {}\n{}{}",
            BSplineSurface::TYPE,
            surface_text(),
            curve_text(2)
        );
        let e = CurveBoundedSurface::read_from_db(&k, &mut reader(&text), 30).unwrap_err();
        assert!(!e.is_unported());
    }

    #[test]
    fn read_rejects_huge_counts_and_deep_nesting() {
        let k = Kernel::default();
        let text = format!(
            "99999999\n0\n0\n\nObject Type {}\n{}",
            BSplineSurface::TYPE,
            surface_text()
        );
        assert!(CurveBoundedSurface::read_from_db(&k, &mut reader(&text), 30).is_err());

        let deep = format!("0\n0\n0\n\nObject Type {TYPE}\n").repeat(200);
        assert!(CurveBoundedSurface::read_from_db(&k, &mut reader(&deep), 30).is_err());
    }
}
