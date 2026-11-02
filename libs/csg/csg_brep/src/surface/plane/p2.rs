//! `SmPlane`, part 2: the offset plane, transformation, trimming to a domain, reading
//! from a database stream, recognising a plane in a B-spline surface, the plane/plane
//! intersection (and its `GlobalSurfaceIntersect` dispatch), and trimming curves,
//! lines and ellipses to a plane domain.
//!
//! Upstream: `source/SMLib/src/SmPlane.cpp`, `source/SMLib/inc/SmPlane.h`.
//!
//! | Rust                                       | Upstream                                          |
//! |--------------------------------------------|---------------------------------------------------|
//! | [`Plane::create_offset_surface`]           | `CreateOffsetSurface`                             |
//! | `global_surface_intersect_into` (via [`Surface::global_surface_intersect`]) | `GlobalSurfaceIntersect` |
//! | [`Plane::intersect_with_plane`]            | `IntersectWithPlane`                              |
//! | [`Plane::trim_curve_to_plane_domain`]      | `TrimCurveToPlaneDomain`                          |
//! | [`Plane::is_nurb_surface_plane`]           | `IsNurbSurfacePlane` (static)                     |
//! | [`Plane::transform`]                       | `Transform`                                       |
//! | [`Plane::trim_with_domain`]                | `TrimWithDomain`                                  |
//! | [`Plane::read_from_db`]                    | `ReadFromDB` (static)                             |
//! | [`Plane::write_to_db`]                     | `WriteToDB`                                       |
//! | [`Plane::make_new_plane_line`]             | `MakeNewPlaneLine`                                |
//! | [`Plane::make_new_plane_ellipse`]          | `MakeNewPlaneEllipse`                             |
//!
//! Deviations:
//! - **Arguments.** Out-params become return values; `SmContext` arguments are
//!   dropped. `GlobalSurfaceIntersect`'s and `IntersectWithPlane`'s optional output
//!   arrays are always filled ([`SurfaceIntersection`]; a plane/plane intersection has
//!   no UV curves, so those entries are `None`, upstream's `NULL`). The arguments upstream
//!   marks `NotUsed` (`CreateOffsetSurface`'s tolerance, `IntersectWithPlane`'s edge
//!   flags and angle tolerance, `ReadFromDB`'s type) are dropped.
//! - **Zone tolerance.** The plane boxes of `IntersectWithPlane` are expanded by the
//!   tolerance of the plane's face ([`Surface::face`], `SmSurface::GetFace`), else
//!   `SM_ZONE_TOL_3D`. A bare [`Plane`] has no owner (the owner lives on [`Surface`]), so
//!   it uses `SM_ZONE_TOL_3D`; through [`Surface::global_surface_intersect`] a plane with
//!   a face uses the face tolerance its B-rep cached on the surface
//!   ([`Surface::owner_zone_tol_3d`]), and returns [`SolidError::Unported`] there
//!   without it (`SmFace::GetTolerance`, unit `topology.face_p2`).
//! - **NURB.** `IntersectWithPlane`'s parallel case tests whether both planes hold a
//!   NURB (`m_pNurb`), as upstream. The port's read-only queries (e.g.
//!   `natural_uv_domain`, the boxes) make a missing NURB and keep it on the plane as
//!   upstream does through a `const_cast` (see the `surface::plane::p1` deviations),
//!   so a plane reaches this test with a NURB exactly when upstream's would.
//! - **Statuses.** Where upstream ignores a status and would go on with stale or
//!   uninitialised values (curve evaluations and copies in `TrimCurveToPlaneDomain`,
//!   the test-point evaluation of `IsNurbSurfacePlane`), the port propagates the error.
//!   A `GlobalCurveIntersect` failure in `TrimCurveToPlaneDomain` (status ignored)
//!   contributes no solutions. `IsNurbSurfacePlane` returns a status through its
//!   `SmBoolean` (`SER`): upstream's "`TRUE` without plane" is an `Err` here.
//!   `SmMatrix::SolveLinearSystem`'s near-singular warning, which upstream returns
//!   through `SER`, is [`SolidError::Upstream`] with `SM_ERR_WARNING`.
//! - **Recognisers** after the plane in `GlobalSurfaceIntersect`: the surface of
//!   revolution's and the cone's (`SmCone::IntersectWithPlane`, also for a cylinder)
//!   special cases run; the sphere and extrusion need surface kinds not in
//!   [`SurfaceKind`] yet. Against a surface of revolution, a plane
//!   with a face returns [`SolidError::Unported`] (`SmFace::GetTolerance`) even where
//!   the special case would not reach the plane's box. The general
//!   `SmSurface::GlobalSurfaceIntersect` runs `SmSurfaceIntersector::DoIntersection`
//!   (`intersect.surface_intersector_p5`), which returns its unported callees'
//!   [`SolidError::Unported`].

use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::Curve;
use crate::curve::ellipse::{Ellipse, EllipseOptions};
use crate::curve::line::{BoundedLineOptions, Line};
use crate::error::{ignore_status, ignored, Result, SolidError, SM_ERR_WARNING};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p2::{line_coplanar_ellipse_intersect, CoplanarEllipse};
use crate::geom::global_solver::{GlobalSolve, GlobalSolver, SolverOperation};
use crate::geom::matrix::Matrix;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::solution_array::{
    ObjectKind, SolutionArray, SolutionObject, SolutionType, SortKey,
};
use crate::geom::tol::{are_same, is_zero, is_zero_to_tol, ZONE_TOL_3D};
use crate::geom::vector2d::{Point2, Vec2};
use crate::geom::vector3d::{sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, min2, EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::{BSplineSurface, BoundingBoxRequest};
use crate::nurbs::knots::Side;
use crate::surface::surf_of_revolution::SurfOfRevolution;
use crate::surface::surface::p1::{evaluate_1st_derivatives, SurfaceIntersectContext};
use crate::surface::surface::p2::{
    base_global_surface_intersect_into, SurfaceIntersection, TsectCurveType,
};
use crate::surface::surface::p7::base_evaluate_normal;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::topology::database_io::{DatabaseReader, DatabaseWriter, FileType, InputStream};

use super::p1::{is_grazing_intersection, Plane};

const OP_PP: &str = "surface::plane::p2::Plane::intersect_with_plane";
const OP_NURB: &str = "surface::plane::p2::Plane::is_nurb_surface_plane";
const OP_XFORM: &str = "surface::plane::p2::Plane::transform";
const OP_TRIM: &str = "surface::plane::p2::Plane::trim_with_domain";

/// A curve handle of `TrimCurveToPlaneDomain`'s intersections: an id (`0` the curve,
/// `1..=4` the boundary lines, `10 +` those for their analytic replacements) and the
/// curve's natural interval (what upstream reads through the curve pointer). Distinct
/// curves have distinct handles, as upstream's pointers.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CurveHandle(u8, Extent1d);

impl SolutionObject for CurveHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.1)
    }
}

/// Whether a plane has a face (`SmSurface::GetFace`), and that face's tolerance as its
/// B-rep cached it on the [`Surface`] ([`Surface::owner_zone_tol_3d`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PlaneOwner {
    /// No face: the boxes expand by `SM_ZONE_TOL_3D`.
    NoFace,
    /// A face, with its cached tolerance (`None`: a bare face handle).
    Face(Option<f64>),
}

impl PlaneOwner {
    /// The owner state of a plane held by `surface`.
    ///
    /// # Errors
    ///
    /// Those of [`Surface::face`].
    pub(crate) fn of(surface: &Surface) -> Result<Self> {
        Ok(if surface.face()?.is_some() {
            Self::Face(surface.owner_zone_tol_3d())
        } else {
            Self::NoFace
        })
    }
}

impl PlaneOwner {
    /// The `expand_tolerance` of a box request on the plane's B-spline part: its face's
    /// tolerance, `None` (the ownerless `SM_ZONE_TOL_3D`) without face.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] (`SmFace::GetTolerance`) for a bare face handle.
    pub(crate) fn expand_tolerance(self) -> Result<Option<f64>> {
        match self {
            Self::NoFace => Ok(None),
            Self::Face(_) => zone_tolerance(self).map(Some),
        }
    }
}

/// The tolerance of a plane's face (`GetFace()->GetTolerance()`, when the plane has a
/// face), else `SM_ZONE_TOL_3D` (the `bExpandPosBoxesByZoneTol3d` expansion of
/// `CalculateBoundingBox`).
fn zone_tolerance(owner: PlaneOwner) -> Result<f64> {
    match owner {
        PlaneOwner::NoFace => Ok(ZONE_TOL_3D),
        PlaneOwner::Face(Some(tol)) => Ok(tol),
        // UNPORTED(topology.face_p2): SmFace::GetTolerance (a bare face handle without
        // the tolerance its B-rep caches on the surface)
        // Proof (unreachable): a B-rep face's surface carries the face tolerance
        // (Brep::sync_face_surface_tolerance invariant, see Surface::face_zone_tol_3d); 0
        // hits in the green difftest sweep.
        PlaneOwner::Face(None) => Err(SolidError::unported(
            "SmFace::GetTolerance",
            "topology.face_p2",
        )),
    }
}

impl Plane {
    // ---- new surfaces -----------------------------------------------------------------

    /// A copy of the plane moved by `signed_offset_distance` along its normal
    /// (`CreateOffsetSurface`; a negative distance moves against the normal).
    ///
    /// # Errors
    ///
    /// Those of [`Plane::transform`].
    pub fn create_offset_surface(&self, k: &Kernel, signed_offset_distance: f64) -> Result<Self> {
        // construct plane copy
        let mut off_plane = self.copy();

        // translate the plane copy
        let mut trans = Axis2Placement::IDENTITY;
        let mv = self.position.z_axis() * signed_offset_distance;
        trans.translate(mv);
        off_plane.transform(k, &trans, None)?;

        // set output
        Ok(off_plane)
    }

    // ---- edits ------------------------------------------------------------------------

    /// Scales about the origin (optional, isotropic only) then rotates and moves the
    /// plane (`Transform`): the NURB is transformed, then the analytic data is rebuilt
    /// from it with [`Plane::is_nurb_surface_plane`] (tolerance scales 1, 10, 100,
    /// 1000). The identity frame with no or unit scale changes nothing.
    ///
    /// # Errors
    ///
    /// Those of [`BSplineSurface::transform`]; `SM_ERR` for a non-isotropic scale (the
    /// NURB is already transformed, as upstream) or when the transformed NURB is not
    /// recognised as a plane; those of [`Plane::is_nurb_surface_plane`].
    pub fn transform(
        &mut self,
        k: &Kernel,
        rotate_n_move: &Axis2Placement,
        scale: Option<Vec3>,
    ) -> Result<()> {
        let identity_scale = Vec3::new(1.0, 1.0, 1.0);

        // no work - identity transform
        if rotate_n_move.is_identity(EFF_ZERO) && scale.is_none_or(|s| s.approx_eq(identity_scale))
        {
            return Ok(());
        }

        // apply m_pNurb (SmBSplineSurface::Transform makes a missing NURB)
        self.ensure_nurb();
        self.base.transform(rotate_n_move, scale)?;

        // when scaling
        if let Some(s) = scale {
            if !(s.x == 1.0 && s.y == 1.0 && s.z == 1.0) {
                // scaling must be isotropic
                if !are_same(s.x, s.y) || !are_same(s.x, s.z) {
                    return Err(SolidError::sm_err(OP_XFORM));
                }
            }
        }

        // check transformed m_pNurb for planarity and gen transformed Plane parameters
        let nurb = self
            .base
            .gw_nurb()
            .ok_or(SolidError::null_pointer(OP_XFORM))?;
        let bss = BSplineSurface::from_nurb(nurb.clone(), true);
        let mut plane = None;
        for tol_scale in [1.0, 10.0, 100.0, 1000.0] {
            plane = Self::is_nurb_surface_plane(k, &bss, tol_scale)?;
            if plane.is_some() {
                break;
            }
        }
        // Something wrong here if scaling a plane does not produce a plane.
        let plane = plane.ok_or(SolidError::sm_err(OP_XFORM))?;

        // save the transformed plane parameters
        self.position = plane.position;
        self.anal_uv_domain = plane.anal_uv_domain;
        self.uv_scale = plane.uv_scale;
        Ok(())
    }

    /// Trims the plane to `trim_domain`, which must lie in the natural domain within
    /// `SM_EFF_ZERO_PARAM` (`TrimWithDomain`): the analytic domain becomes `trim_domain`
    /// and the NURB is rebuilt ([`Plane::adjust_step_uv_domain`]), unless the natural
    /// domain already lies in `trim_domain`.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `trim_domain` is not inside the natural domain; those of
    /// [`Plane::adjust_step_uv_domain`].
    pub fn trim_with_domain(&mut self, trim_domain: &Extent2d) -> Result<()> {
        // (Notify(SM_NO_PRE_EDIT) only drops caches)
        // GetNaturalUVDomain makes a missing NURB in place
        self.ensure_nurb();
        let natural_domain = self.base.natural_uv_domain();

        // Make sure trim domain in inside of natural domain
        if !trim_domain.is_contained_by(&natural_domain, EFF_ZERO_PARAM) {
            return Err(SolidError::sm_err(OP_TRIM));
        }

        // See if no trimming necessary - just return
        if natural_domain.is_contained_by(trim_domain, EFF_ZERO_PARAM) {
            return Ok(());
        }

        self.adjust_step_uv_domain(trim_domain)
    }

    // ---- database ---------------------------------------------------------------------

    /// Reads a plane written by `WriteToDB` (`ReadFromDB`): origin, X axis, Y axis
    /// (set with `SetCanonical`, status ignored), the analytic domain corners and the
    /// scales (one group per line in ASCII files), then the B-spline part
    /// (`SmBSplineSurface::ReadFromDB`). Unreadable ASCII values keep their defaults.
    ///
    /// # Errors
    ///
    /// Those of the binary reads and of [`BSplineSurface::read_from_db`].
    pub fn read_from_db(db: &mut DatabaseReader, version: u64) -> Result<Self> {
        // locals
        let mut orig = Vec3::UNDEF;
        let mut x = Vec3::UNDEF;
        let mut y = Vec3::UNDEF;
        let mut anal_min = Point2::UNDEF;
        let mut anal_max = Point2::UNDEF;
        let mut uv_scale = Vec2::UNDEF;

        if db.file_type() == FileType::Ascii {
            for v in [&mut orig, &mut x, &mut y] {
                if let Some(s) = db.in_stream_mut() {
                    for c in [&mut v.x, &mut v.y, &mut v.z] {
                        if let Some(d) = s.extract_double() {
                            *c = d;
                        }
                    }
                }
                db.go_to_next_line();
            }
            if let Some(s) = db.in_stream_mut() {
                for c in [
                    &mut anal_min.x,
                    &mut anal_min.y,
                    &mut anal_max.x,
                    &mut anal_max.y,
                ] {
                    if let Some(d) = s.extract_double() {
                        *c = d;
                    }
                }
            }
            db.go_to_next_line();
            if let Some(s) = db.in_stream_mut() {
                for c in [&mut uv_scale.x, &mut uv_scale.y] {
                    if let Some(d) = InputStream::extract_double(s) {
                        *c = d;
                    }
                }
            }
            db.go_to_next_line();
        } else {
            orig.x = db.read_double()?;
            orig.y = db.read_double()?;
            orig.z = db.read_double()?;

            x.x = db.read_double()?;
            x.y = db.read_double()?;
            x.z = db.read_double()?;

            y.x = db.read_double()?;
            y.y = db.read_double()?;
            y.z = db.read_double()?;

            anal_min.x = db.read_double()?;
            anal_min.y = db.read_double()?;

            anal_max.x = db.read_double()?;
            anal_max.y = db.read_double()?;

            uv_scale.x = db.read_double()?;
            uv_scale.y = db.read_double()?;
        }

        // load the plane (statuses ignored)
        let mut position = Axis2Placement::IDENTITY;
        let _ = position.set_canonical(orig, x, y);
        let mut anal_uv_domain = Extent2d::new();
        let _ = anal_uv_domain.set_min_max(anal_min, anal_max);

        // read the parent object
        let base = BSplineSurface::read_from_db(db, version)?;

        Ok(Self {
            base,
            position,
            anal_uv_domain,
            uv_scale,
        })
    }

    /// `WriteToDB`: writes the placement origin and axes, the analytic UV domain
    /// corners and the UV scales, then the B-spline surface part with its NURB (made
    /// on a copy when missing, as the virtual `GetOrCreateGwNurbPointer` makes it).
    ///
    /// # Errors
    ///
    /// Those of [`Plane::base_with_nurb`] (upstream ignores the B-spline part's
    /// status, which cannot fail once the NURB exists).
    pub fn write_to_db(&self, db: &mut DatabaseWriter, version: u64) -> Result<()> {
        let o = self.position.origin();
        let x = self.position.x_axis();
        let y = self.position.y_axis();
        let (lo, hi) = (self.anal_uv_domain.min(), self.anal_uv_domain.max());
        let scale = self.uv_scale;
        if db.file_type() == FileType::Ascii {
            db.out_reals(&[o.x, o.y, o.z]);
            db.out_str(" SmPlane Placement Origin \n");
            db.out_reals(&[x.x, x.y, x.z]);
            db.out_str(" SmPlane Placement X Axis \n");
            db.out_reals(&[y.x, y.y, y.z]);
            db.out_str(" SmPlane Placement Y Axis \n");
            db.out_reals(&[lo.x, lo.y, hi.x, hi.y]);
            db.out_str(" SmPlane AnalUVDomain \n");
            db.out_reals(&[scale.x, scale.y]);
            db.out_str(" SmPlane UV Scale \n");
        } else {
            db.write_doubles(&[o.x, o.y, o.z, x.x, x.y, x.z, y.x, y.y, y.z]);
            db.write_doubles(&[lo.x, lo.y, hi.x, hi.y, scale.x, scale.y]);
        }
        self.base_with_nurb()?.write_to_db(db, version)
    }

    // ---- recognition ------------------------------------------------------------------

    /// The plane with the parameterisation of `test` when `test` is one
    /// (`IsNurbSurfacePlane`): the first derivatives at the natural domain corners
    /// `(1, 1)` and `(0, 0)` are perpendicular and their normals parallel to the normal
    /// near the centre (angle tolerance `tolerance_scale * 1e-4` degrees), and ten test
    /// points lie within `SM_EFF_ZERO_SQRT * (1 + |origin|)` of the plane built from
    /// the `(0, 0)` derivatives. The plane is re-parameterised to the natural domain
    /// of `test` (which makes its NURB). `None` when `test` is not a plane.
    ///
    /// # Errors
    ///
    /// Upstream's `SER` statuses (returned as a `TRUE` result without plane): a
    /// failing test-point evaluation, a degenerate Y axis; [`SolidError::Unported`]
    /// from the evaluations.
    pub fn is_nurb_surface_plane(
        k: &Kernel,
        test: &BSplineSurface,
        tolerance_scale: f64,
    ) -> Result<Option<Self>> {
        // (HIGHER_ORDER_USE_ANALYTICS: no degree or control point limit)

        // surface locals
        let uv_domain = test.natural_uv_domain();
        let mut origin = Vec3::UNDEF;
        let mut x_axis = Vec3::UNDEF;
        let mut y_axis = Vec3::UNDEF;

        // get surface throughPoint, tangents, and normal at natural domain upper corner
        // (SE: a failed evaluation keeps the previous values)
        if let Some((p, du, dv)) = ignored(evaluate_1st_derivatives(
            k,
            test,
            uv_domain.evaluate(1.0, 1.0),
            Side::Left,
            Side::Left,
        ))? {
            (origin, x_axis, y_axis) = (p, du, dv);
        }
        let mut normal_at_max = x_axis.cross(y_axis);

        // pick angle tolerance
        let angle_tol = tolerance_scale * 0.0001; // 1/10000 of a degree

        // not a plane - UTangent not perpendicular to VTangent
        if !x_axis.is_perpendicular_to(y_axis, angle_tol) {
            return Ok(None);
        }

        // pick spatial tolerances
        let tol = EFF_ZERO_SQRT * (1.0 + origin.max_dimension());
        let tol_sq = tol * tol;

        // get surface throughPoint, tangents, and normal at natural domain lower corner
        if let Some((p, du, dv)) = ignored(evaluate_1st_derivatives(
            k,
            test,
            uv_domain.evaluate(0.0, 0.0),
            Side::Left,
            Side::Left,
        ))? {
            (origin, x_axis, y_axis) = (p, du, dv);
        }
        let normal_at_origin = x_axis.cross(y_axis);

        // not a plane - UTangent not perpendicular to VTangent
        if !x_axis.is_perpendicular_to(y_axis, angle_tol) {
            return Ok(None);
        }

        // not a plane - origin and max normal vectors not parallel
        if !normal_at_origin.is_parallel_to(normal_at_max, angle_tol) {
            return Ok(None);
        }

        // get normal near domain center (SE)
        if let Some(n) = ignored(base_evaluate_normal(
            k,
            test,
            uv_domain.evaluate(0.45, 0.56),
            Side::Left,
            Side::Left,
        ))? {
            normal_at_max = n;
        }

        // not a plane - center and origin normal vectors not parallel
        if !normal_at_origin.is_parallel_to(normal_at_max, angle_tol) {
            return Ok(None);
        }

        // set up plane origin and X/Y axis (Unitize statuses ignored)
        let uv_scale = Vec2::new(x_axis.length(), y_axis.length());
        let x_axis = x_axis.unitize_or_z();
        let mut y_axis = y_axis.unitize_or_z();

        // Move the origin to correspond to the zero [u=0, v=0] parameter.
        let new_origin = origin
            - uv_domain.min().x * x_axis * uv_scale.x
            - uv_domain.min().y * y_axis * uv_scale.y;

        // Force XAxis and YAxis to be nicely perpendicular
        if x_axis.dot(y_axis).abs() > EFF_ZERO {
            y_axis = x_axis.cross(y_axis).cross(x_axis);
            y_axis = y_axis.unitize().map_err(|_| SolidError::sm_err(OP_NURB))?;
        }

        // construct temporary new plane (m_pNurb == NULL)
        let mut plane = Self::new(new_origin, x_axis, y_axis, uv_scale, &uv_domain);

        // Now test about 10 points along surface just to make sure.
        let uvs = [
            uv_domain.evaluate(0.3, 0.4),
            uv_domain.evaluate(0.2, 0.1),
            uv_domain.evaluate(0.6, 0.4),
            uv_domain.evaluate(0.7, 0.7),
            uv_domain.evaluate(0.8, 0.9),
            uv_domain.evaluate(0.4, 0.6),
            uv_domain.evaluate(0.9, 0.2),
            uv_domain.evaluate(0.6, 0.1),
            uv_domain.evaluate(0.1, 0.8),
            uv_domain.evaluate(1.0, 0.0),
        ];
        for uv in uvs {
            // when any point is not a plane - return not a plane
            let orig_pnt = test.evaluate_point(uv)?;
            let plane_pnt = plane.evaluate_point(uv);
            if orig_pnt.distance_squared(plane_pnt) > tol_sq {
                return Ok(None);
            }
        }

        // reparameterize the NURB so that AnalyticSurface->NurbDomain ==
        // InputSurface->NurbDomain (status ignored)
        ignore_status(plane.reparameterize(&test.natural_uv_domain()))?;

        // (Notify(SM_NO_COPY) copies the test surface's attributes: not modelled)
        Ok(Some(plane))
    }

    // ---- intersections ----------------------------------------------------------------

    /// The axis aligned and pseudo boxes of the plane (`CalculateBoundingBox(domain,
    /// &box, &pbox)`, the inherited `SmBSplineSurface` override with its default
    /// zone-tolerance expansion).
    pub(crate) fn bounding_boxes(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        owner: PlaneOwner,
    ) -> Result<(Extent3d, PseudoBox)> {
        let req = BoundingBoxRequest {
            normal: true,
            pseudo: true,
            expand_by_zone_tol: false,
            ..BoundingBoxRequest::default()
        };
        let b = self
            .base_with_nurb()?
            .calculate_bounding_box(k, uv_domain, &req)?;
        let mut bbox = b.normal.ok_or(SolidError::null_pointer(OP_PP))?;
        let mut pbox = b.pseudo.ok_or(SolidError::null_pointer(OP_PP))?;
        // When asked, expand the normal and pseudo boxes by the zone tolerance
        bbox.expand_absolute(zone_tolerance(owner)?);
        pbox.expand_absolute(zone_tolerance(owner)?);
        Ok((bbox, pbox))
    }

    /// The intersection line of this plane over `uv_domain` with `other` over
    /// `other_uv_domain` (`IntersectWithPlane`), and whether the general intersector
    /// is still needed. Not parallel planes: one infinite line when neither plane is
    /// bounded, else the segment inside both domains (a degenerate curve,
    /// [`TsectCurveType::Touching`], when shorter than the tolerance), crossing, with
    /// no UV curves and deviation `0`. Parallel planes give nothing; coincident ones
    /// (both with NURB, boxes overlapping, within the tolerance) need the general
    /// intersector. The tolerance is `approx_tol_3d`, else `SM_EFF_ZERO_SQRT * (1 +`
    /// the domain mid-point sizes `)`. Both planes are taken as without owner (see the
    /// module docs).
    ///
    /// # Errors
    ///
    /// Those of the boxes and of the linear solve (`SM_ERR_WARNING` when nearly
    /// singular); those of [`BSplineCurve::create_degenerate_curve`].
    pub fn intersect_with_plane(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Self,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<(bool, SurfaceIntersection)> {
        self.intersect_with_plane_owned(
            k,
            uv_domain,
            other,
            other_uv_domain,
            approx_tol_3d,
            [PlaneOwner::NoFace, PlaneOwner::NoFace],
        )
    }

    /// [`Plane::intersect_with_plane`] with the planes' owners (for the zone
    /// tolerance).
    #[allow(clippy::too_many_lines)] // one upstream function
    pub(crate) fn intersect_with_plane_owned(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: &Self,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
        owners: [PlaneOwner; 2],
    ) -> Result<(bool, SurfaceIntersection)> {
        // init output
        let mut out = SurfaceIntersection::default();

        // no work - looking for self-intersections; planes have no self-intersections
        if core::ptr::eq(self, other) {
            return Ok((false, out));
        }

        // locals
        let this_bounded = self.is_bounded();
        let other_bounded = other.is_bounded();

        // get thisPlane midPoint and normal
        let origin_this = self.evaluate_point_fast(uv_domain.evaluate(0.5, 0.5));
        let normal_this = self.position.z_axis();

        // get otherPlane midPoint and normal
        let origin_other = other.evaluate_point_fast(other_uv_domain.evaluate(0.5, 0.5));
        let normal_other = other.position.z_axis();

        // set tol proportional to midPoint size or as given
        let tol = approx_tol_3d.unwrap_or_else(|| {
            EFF_ZERO_SQRT * (1.0 + origin_this.max_dimension() + origin_other.max_dimension())
        });

        // intersection line direction = CrossProduct(PlaneNormal1, PlaneNormal2)
        let line_dir = normal_this.cross(normal_other);

        // when planes are parallel or coincident
        let ang_tol_sq = 1.0e-16; // Cross product length is sine, which is same as angle for small angles.
        let len_sq = line_dir.length_squared();
        if len_sq < ang_tol_sq {
            // Planes are parallel.
            if self.base.gw_nurb().is_none() || other.base.gw_nurb().is_none() {
                return Ok((false, out));
            }

            // get plane1 and plane2 bounding and pseudo boxes
            let (bbox1, pbox1) = self.bounding_boxes(k, uv_domain, owners[0])?;
            let (bbox2, pbox2) = other.bounding_boxes(k, other_uv_domain, owners[1])?;

            // no work - when bounding or pseudo boxes are disjoint
            if bbox1.are_disjoint(&bbox2, 0.0) || pbox1.are_disjoint(&pbox2, None) {
                return Ok((false, out));
            }

            // let sPnt2 = plane2 midPoint projected to plane1
            let uv_found = self.project_point_to_uv_domain(origin_other);
            let pnt2 = self.evaluate_point_fast(uv_found);

            // no work - planes are parallel not coincident
            let dist = origin_other.distance(pnt2);
            if dist > tol {
                return Ok((false, out));
            }

            // more work needed - planes are coincident
            return Ok((true, out));
        }

        // Arrive here when planes are not parallel.

        // unitize lineDir and set OriginCross = avg(midPoint1, midPoint2)
        let line_dir = line_dir.unitize().map_err(|_| SolidError::sm_err(OP_PP))?;
        let origin_cross = if this_bounded && !other_bounded {
            origin_this
        } else if !this_bounded && other_bounded {
            origin_other
        } else {
            (origin_this + origin_other) / 2.0
        };

        // build A of Ax=b matrix problem
        let mut mat = Matrix::new(3, 3);
        for i in 0..3 {
            mat.set_at(0, i, normal_this.component(i))?;
            mat.set_at(1, i, normal_other.component(i))?;
            mat.set_at(2, i, line_dir.component(i))?;
        }

        // build b of Ax=b matrix problem
        let rhs = [
            normal_this.dot(origin_this),
            normal_other.dot(origin_other),
            line_dir.dot(origin_cross),
        ];

        // let sStartPt = solution x in Ax=b
        let sol = mat.solve_linear_system(&rhs)?;
        if sol.near_singular {
            return Err(SolidError::Upstream {
                operation: OP_PP,
                code: SM_ERR_WARNING,
                entity: None,
            });
        }
        let [x0, x1, x2] = <[f64; 3]>::try_from(sol.x.as_slice())
            .map_err(|_| SolidError::invalid_input(OP_PP, "linear solution size"))?;
        let start_pt = Point3::new(x0, x1, x2);

        // when both planes aren't bounded - output infinite line
        if !this_bounded && !other_bounded {
            // Create an 'infinite' line
            let line = Line::infinite(start_pt, line_dir, 3, false);
            out.curves_3d.push(Curve::from(line));
            out.surface1_uv_curves.push(None);
            out.surface2_uv_curves.push(None);
            out.curve_types.push(TsectCurveType::Crossing);
            out.deviations.push(0.0);
            return Ok((false, out));
        }

        // Create a bounded line

        // get bounding and pseudo boxes for both planes
        let mut dist = 0.0;
        let mut bbox1 = Extent3d::new();
        let mut bbox2 = Extent3d::new();
        let mut pbox1 = PseudoBox::new();
        let mut pbox2 = PseudoBox::new();

        // get size of bounded ThisPlane
        if this_bounded {
            (bbox1, pbox1) = self.bounding_boxes(k, uv_domain, owners[0])?;
            let radius1 = bbox1.sphere_bound().radius;
            bbox1.expand_absolute(tol / 2.0);
            pbox1.expand_absolute(tol / 2.0);
            dist += 2.0 * radius1;
        }

        // get size of bounded OtherPlane
        if other_bounded {
            (bbox2, pbox2) = other.bounding_boxes(k, other_uv_domain, owners[1])?;
            let radius2 = bbox2.sphere_bound().radius;
            bbox2.expand_absolute(tol / 2.0);
            pbox2.expand_absolute(tol / 2.0);
            dist += 2.0 * radius2;
        }

        // no work - bounding boxes are disjoint
        if (this_bounded && other_bounded)
            && (bbox1.are_disjoint(&bbox2, 0.0) || pbox1.are_disjoint(&pbox2, None))
        {
            return Ok((false, out));
        }

        // Init the line length to be longer than both bounding boxes
        let mut seg_start = start_pt - dist * line_dir;
        let mut seg_end = start_pt + dist * line_dir;

        // Trim line to Plane boundaries (no trimming infinite planes)
        let mut this_found = true;
        let mut other_found = true;
        if this_bounded {
            match self.trim_plane_line_to_plane_domain(uv_domain, seg_start, seg_end, tol) {
                Some((s, e)) => (seg_start, seg_end) = (s, e),
                None => this_found = false,
            }
        }
        if other_bounded {
            match other.trim_plane_line_to_plane_domain(other_uv_domain, seg_start, seg_end, tol) {
                Some((s, e)) => (seg_start, seg_end) = (s, e),
                None => other_found = false,
            }
        }

        // when line segment doesn't intersect both plane domains
        if !this_found || !other_found {
            return Ok((false, out));
        }

        // when segment is short - return degenerate curve xsect
        let length_of_line = seg_start.distance(seg_end);
        if length_of_line < tol {
            let bsc = BSplineCurve::create_degenerate_curve(3, seg_start)?;
            out.curves_3d.push(Curve::from(bsc));
            out.surface1_uv_curves.push(None);
            out.surface2_uv_curves.push(None);
            out.curve_types.push(TsectCurveType::Touching);
            out.deviations.push(0.0);
            return Ok((false, out));
        }

        // create output line and set outputs
        let line = Line::bounded(
            seg_start,
            line_dir,
            &Extent1d::from_min_max(0.0, 1.0),
            &BoundedLineOptions {
                scale: length_of_line,
                ..BoundedLineOptions::default()
            },
        );
        out.curves_3d.push(Curve::from(line));
        out.surface1_uv_curves.push(None);
        out.surface2_uv_curves.push(None);
        out.curve_types.push(TsectCurveType::Crossing);
        out.deviations.push(0.0);
        Ok((false, out))
    }

    // ---- curves -----------------------------------------------------------------------

    /// The parts of `curve` inside the plane's `uv_domain` (`TrimCurveToPlaneDomain`):
    /// the curve is intersected with the four domain boundary lines within `tol_3d`
    /// (grazing points skipped), the intervals between the intersections are
    /// classified by dropping a point of each onto the domain, and each inside
    /// interval becomes a trimmed copy of the curve (coincident ranges are kept); two
    /// consecutive outside intervals give a degenerate curve at their common end.
    ///
    /// # Errors
    ///
    /// Those of the curve evaluations, copies and degenerate curves (statuses upstream
    /// ignores); [`SolidError::Unported`] from the intersection.
    #[allow(clippy::too_many_lines)] // one upstream function
    pub fn trim_curve_to_plane_domain(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        curve: &Curve,
        tol_3d: f64,
    ) -> Result<Vec<Curve>> {
        // init output
        let mut curves_3d = Vec::new();

        // locals
        let ivl = curve.natural_interval();
        let mut solutions: SolutionArray<CurveHandle> = SolutionArray::new();

        // get Plane domain corners
        let min_x = uv_domain.min().x;
        let min_y = uv_domain.min().y;
        let max_x = uv_domain.max().x;
        let max_y = uv_domain.max().y;
        let p00 = self.evaluate_point(Point2::new(min_x, min_y));
        let p01 = self.evaluate_point(Point2::new(min_x, max_y));
        let p10 = self.evaluate_point(Point2::new(max_x, min_y));
        let p11 = self.evaluate_point(Point2::new(max_x, max_y));

        // boundary line segments - a PlaneSurface domain boundary is a line
        let line_ivl = Extent1d::from_min_max(0.0, 1.0);
        let boundary = |p: Point3, q: Point3| {
            Line::bounded(
                p,
                q - p,
                &line_ivl,
                &BoundedLineOptions {
                    scale: (q - p).length(),
                    ..BoundedLineOptions::default()
                },
            )
        };
        let lines = [
            boundary(p00, p10),
            boundary(p10, p11),
            boundary(p11, p01),
            boundary(p01, p00),
        ];

        // for every boundary line
        for (line_id, line) in (1u8..).zip(&lines) {
            // Intersect Curve with boundary lines (status ignored)
            let line_curve = Curve::from(line.clone());
            let t_solutions = ignored(curve.global_curve_intersect(
                k,
                &ivl,
                &line_curve,
                &line.natural_interval(),
                tol_3d,
                &CurveIntersectOptions {
                    skip_global_coincidence_check: false,
                    handles: [
                        Some(CurveHandle(0, ivl)),
                        Some(CurveHandle(line_id, line.natural_interval())),
                    ],
                    analytic_handles: [
                        Some(CurveHandle(10, ivl)),
                        Some(CurveHandle(10 + line_id, line.natural_interval())),
                    ],
                },
            ))?
            .unwrap_or_default();

            // accumulate the solutions - sorted by crCurve parameter
            let mut gs: GlobalSolver<'_, CurveHandle> = GlobalSolver::new();
            gs.set_solutions(core::mem::take(&mut solutions));
            gs.set_num_variables(1);
            for solution in t_solutions.as_slice() {
                // Skip grazing solutions.  [Boolean Restructure]
                if solution.solution_type == SolutionType::SingleValue
                    && is_grazing_intersection(
                        k,
                        curve,
                        solution.start.param(0)?,
                        line,
                        solution.start.param(1)?,
                    )?
                {
                    continue;
                }

                // (status ignored)
                ignore_status(gs.add_sorted_solution(solution, SortKey::ByFirstParameter, true))?;
            }
            solutions = gs.set_solutions(SolutionArray::new());
        }

        // get all intervals to be turned into curves
        let mut starts: Vec<f64> = Vec::new();
        let mut ends: Vec<f64> = Vec::new();
        let mut keeps: Vec<u8> = Vec::new(); // 0 = no, 1=yes, 2=unknown

        // for every solution
        for solution in solutions.as_slice() {
            // get curve interval from last boundary to start of this solution
            let s_param = ends.last().copied().unwrap_or(ivl.min());
            let e_param = solution.start.param(0)?;

            // skip zero length intervals at the curve start, curve end, and
            // PlaneCorner/curve intersections (statuses ignored)
            let start_point = curve.evaluate(k, s_param, 1, true, true)?;
            let end_point = curve.evaluate_point(k, e_param)?;
            let p0 = start_point.first().copied().unwrap_or(Vec3::UNDEF);
            let d0 = start_point.get(1).copied().unwrap_or(Vec3::UNDEF);
            let dist = p0.distance(end_point);
            let speed = d0.length();
            if is_zero_to_tol(s_param - e_param, 10.0 * tol_3d / max2(1.0, speed)) && dist < tol_3d
            {
                continue;
            }

            // add an interval with an unknown classification
            starts.push(s_param);
            ends.push(e_param);
            keeps.push(2); // unclassified interval

            // when the solution is a coincident length
            if solution.solution_type == SolutionType::RangeOfValues {
                // add another interval marked for keeping
                starts.push(e_param);
                ends.push(solution.end.param(0)?);
                keeps.push(1); // classify interval for keeping
            }
        }

        // get interval from last solution end to curve end
        let s_param = ends.last().copied().unwrap_or(ivl.min());
        let e_param = ivl.max();

        // add non-zero length interval at the end of the curve
        if starts.is_empty() || !is_zero(s_param - e_param) {
            starts.push(s_param);
            ends.push(e_param);
            keeps.push(2); // unclassified interval
        }

        // classify every interval unclassified interval
        for ((&s_param, &e_param), keep) in starts.iter().zip(&ends).zip(keeps.iter_mut()) {
            // skip classified intervals
            if *keep != 2 {
                continue;
            }

            // drop curve interval midPoint to inside of trimmed plane (statuses ignored)
            let t = (s_param + e_param) / 2.0;
            let mid_point = curve.evaluate_point(k, t)?;
            let mut t_solutions = self.drop_point_fast(
                uv_domain,
                SolverOperation::Normalize,
                mid_point,
                tol_3d,
                None,
                None::<CurveHandle>,
            )?;
            if let Some(first) = t_solutions.get(0) {
                // If the point is on the boundary of the domain, use a different curve
                // point.  [B627]
                let pt_uv = Point2::new(first.start.param(0)?, first.start.param(1)?);
                if uv_domain.is_point_on_boundary(pt_uv, 2.0 * tol_3d).0 {
                    let frac = 0.3579; // Make it not quite the midpoint.
                    let param = s_param * frac + e_param * (1.0 - frac);
                    let mid_point = curve.evaluate_point(k, param)?;
                    t_solutions = self.drop_point_fast(
                        uv_domain,
                        SolverOperation::Normalize,
                        mid_point,
                        tol_3d,
                        None,
                        None::<CurveHandle>,
                    )?;
                }
            }

            // classify the interval
            *keep = u8::from(!t_solutions.is_empty());
        }

        // build an output curve for every keeper interval
        let mut prev_keep: Option<u8> = None;
        for ((&s_param, &e_param), &keep) in starts.iter().zip(&ends).zip(&keeps) {
            // degenerate curves for tangency intersections
            if keep == 0 && prev_keep == Some(0) {
                // Create a degenerate curve - at the curve point (statuses ignored)
                let point = curve.evaluate_point(k, s_param)?;
                let bsp = BSplineCurve::create_degenerate_curve(3, point)?;
                curves_3d.push(Curve::from(bsp));
            }
            prev_keep = Some(keep);

            // skip intervals outside the square
            if keep == 0 {
                continue;
            }

            // copy, trim, and add curve to output
            let mut trim_ivl = Extent1d::from_min_max(s_param, e_param);
            let mut new_curve = curve.copy(k)?;
            // may snap sIvl by tol to existing knots (status ignored)
            ignore_status(new_curve.trim(k, &mut trim_ivl, TrimMode::Notify))?;
            curves_3d.push(new_curve);
        }

        Ok(curves_3d)
    }

    /// The segment `start_pt`-`end_pt` (known to lie in the plane) trimmed to
    /// `uv_domain` (`MakeNewPlaneLine`): a bounded [`Line`] over `[0, 1]`, or a
    /// degenerate curve when shorter than `tol`, and whether the segment meets the
    /// domain (`rbFoundInterval`; `true` for the degenerate curve too, as upstream).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] only (a failing degenerate curve gives `None`).
    pub fn make_new_plane_line(
        &self,
        uv_domain: &Extent2d,
        start_pt: Point3,
        end_pt: Point3,
        tol: f64,
    ) -> Result<(Option<Curve>, bool)> {
        // trim line endPoints to plane UVDomain
        let Some((t0, t1)) = self.trim_plane_line_to_plane_domain(uv_domain, start_pt, end_pt, tol)
        else {
            // when no part of the line is on the plane
            return Ok((None, false));
        };

        // get line length
        let line_dist = t0.distance(t1);

        // when line is degenerate
        if line_dist < tol {
            let crv = ignored(BSplineCurve::create_degenerate_curve(3, t0))?;
            return Ok((crv.map(Curve::from), true));
        }

        // when line has a finite length (Unitize status ignored)
        let line_vec = (t1 - t0).unitize_or_z();
        let line = Line::bounded(
            t0,
            line_vec,
            &Extent1d::from_min_max(0.0, 1.0),
            &BoundedLineOptions {
                scale: line_dist,
                ..BoundedLineOptions::default()
            },
        );
        Ok((Some(Curve::from(line)), true))
    }

    /// The ellipse (circle when the radii are the same) centred at `center` with
    /// radii `radius_x` / `radius_y` along `x_axis` / `y_axis` over the angular
    /// interval `anal_ivl` (degrees), known to lie in the plane, trimmed to
    /// `uv_domain` (`MakeNewPlaneEllipse`): zero to five arcs ([`Ellipse`]s) or
    /// degenerate curves. A zero radius gives a degenerate curve at the centre or the
    /// trimmed line of the other diameter; an ellipse meeting the domain boundary in
    /// fewer than two points is kept whole (or as its tangent point) when inside;
    /// otherwise each arc from an entering to the next exiting boundary point, split at
    /// `anal_ivl`'s ends, is kept (a degenerate curve when shorter than `tol`).
    ///
    /// # Errors
    ///
    /// Those of [`Ellipse::new`] and of the degenerate curves.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // one upstream function
    pub fn make_new_plane_ellipse(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        center: Point3,
        radius_x: f64,
        radius_y: f64,
        x_axis: Vec3,
        y_axis: Vec3,
        anal_ivl: &Extent1d,
        tol: f64,
    ) -> Result<Vec<Curve>> {
        // init output
        let mut crvs: Vec<Curve> = Vec::new();

        // locals
        let circle = are_same(radius_x, radius_y);
        let mut trim_points: Vec<Point3> = Vec::new(); // circ/plane boundary xsectPoints
        let mut trim_entering: Vec<bool> = Vec::new(); // TRUE = entering, FALSE = exiting
        let mut trim_side: Vec<usize> = Vec::new(); // isoCurve index (debug checks only)

        // get circle center and XAxis to plane UVPoints
        let circ_z_axis = x_axis.cross(y_axis).unitize_or_z();
        let center_uv = self.project_point_to_uv_domain(center);

        // get circle's yAxis using plane normal as the positive direction
        let plane_normal = self.evaluate_normal();
        let circ_y_axis_for_plane_normal = plane_normal.cross(x_axis);

        // note when circle axis and plane Normal are in opposite directions
        let opposite = circ_z_axis.dot(plane_normal) < 0.0;

        // the curve of an arc
        let new_arc = |ivl: Extent1d| -> Result<Curve> {
            let opts = EllipseOptions::default();
            Ok(Curve::from(if circle {
                Ellipse::new_circle(k, center, x_axis, y_axis, ivl, radius_x, &opts)?
            } else {
                Ellipse::new(k, center, x_axis, y_axis, ivl, radius_x, radius_y, &opts)?
            }))
        };

        // zero radius ellipse
        if radius_x < tol || radius_y < tol {
            // when center is on plane
            if radius_x < tol && radius_y < tol && uv_domain.contains_point(center_uv, 0.0) {
                // create a degenerate curve to mark the point
                crvs.push(Curve::from(BSplineCurve::create_degenerate_curve(
                    3, center,
                )?));
                return Ok(crvs);
            }

            // else make a degenerate line and quit
            let (line_start_pt, line_end_pt) = if radius_x < tol {
                (center - radius_y * y_axis, center + radius_y * y_axis)
            } else {
                (center - radius_x * x_axis, center + radius_x * x_axis)
            };
            let (crv, found) =
                self.make_new_plane_line(uv_domain, line_start_pt, line_end_pt, tol)?;
            if found {
                // (upstream adds the curve even when it is NULL)
                crvs.extend(crv);
            }
            return Ok(crvs);
        }

        // for all 4 plane boundary lines
        let min_x = uv_domain.min().x;
        let max_x = uv_domain.max().x;
        let min_y = uv_domain.min().y;
        let max_y = uv_domain.max().y;
        let mut tangent = false;
        let mut tangent_inside = false;
        let mut tangent_pt = Vec3::UNDEF;
        for ii in 0..4_usize {
            // get Plane isoParamLine running in counter clockwise direction
            // set IsoCurveTangentUV = [dVecX dVecY]; IsoCurveBiNormInside = [-dVecY dVecX]
            let (plane_start_uv, plane_end_uv, vec_x, vec_y) = match ii {
                0 => (
                    Point2::new(min_x, min_y),
                    Point2::new(max_x, min_y),
                    1.0,
                    0.0,
                ),
                1 => (
                    Point2::new(max_x, min_y),
                    Point2::new(max_x, max_y),
                    0.0,
                    1.0,
                ),
                2 => (
                    Point2::new(max_x, max_y),
                    Point2::new(min_x, max_y),
                    -1.0,
                    0.0,
                ),
                _ => (
                    Point2::new(min_x, max_y),
                    Point2::new(min_x, min_y),
                    0.0,
                    -1.0,
                ),
            };

            // set isoLineVec
            let plane_start_pt = self.evaluate_point_fast(plane_start_uv);
            let plane_end_pt = self.evaluate_point_fast(plane_end_uv);
            let plane_vec = plane_end_pt - plane_start_pt;
            let bi_norm_in = plane_normal.cross(plane_vec);

            // intersect the line with the ellipse (status ignored: no intersections)
            let mut params = ignored(line_coplanar_ellipse_intersect(
                plane_start_pt,
                plane_vec,
                &CoplanarEllipse {
                    center,
                    x_axis,
                    x_radius: radius_x,
                    y_axis: circ_y_axis_for_plane_normal,
                    y_radius: radius_y,
                },
                tol,
            ))?
            .unwrap_or_default();

            // when there are solutions
            if params.is_empty() {
                continue;
            }

            // Check for solutions within tol. (The intersection routine doesn't do that.)
            // If so, collapse them to the midpoint.  [B10]
            if let [a0, a1] = params[..] {
                let dist = (a1 - a0).abs() * plane_vec.length();

                // Actually, check for the entire span between the intersections
                // being within tol of the line.
                let rad = min2(radius_x, radius_y);
                if dist < rad * 0.1745 {
                    // About 10 deg: just to avoid a sqrt when not close.
                    // Max dist from ellipse to line between the two solutions:
                    let h = rad - sqrt(rad * rad - dist * dist * 0.25);
                    if h < tol / 2.0 {
                        params = vec![(a0 + a1) / 2.0];
                    }
                }
            }

            // order aParams
            if let [a0, a1] = params[..] {
                if a0 > a1 {
                    params = vec![a1, a0];
                }
            }

            let point_at = |a: f64| (1.0 - a) * plane_start_pt + a * plane_end_pt;
            // (0.0-SM_EFF_ZERO <= a && 1.0+SM_EFF_ZERO >= a)
            let inside_line = |a: f64| (0.0 - EFF_ZERO..=1.0 + EFF_ZERO).contains(&a);

            if let [a0] = params[..] {
                // get the first solution point
                // when inside the line extent - save 1 tangent inside/outside state (only
                // used if no other intersections)
                if inside_line(a0) && !tangent {
                    tangent = true;
                    tangent_pt = point_at(a0);
                    // curve inside when DotProduct(SurfaceBoundaryBinorm,CircleRadiusVec) > 0
                    // Binorm = [-dVecY, dVecX], CircleRadius = sCenterUV - sTangentUV.
                    let tangent_uv = self.project_point_to_uv_domain(tangent_pt);
                    tangent_inside = (-vec_y * (center_uv.x - tangent_uv.x)
                        + vec_x * (center_uv.y - tangent_uv.y))
                        > 0.0;
                }
            } else {
                // save the points inside the line extent
                for &a in &params {
                    if !inside_line(a) {
                        continue;
                    }
                    let pt = point_at(a);
                    trim_points.push(pt);
                    // entering when dot(BiNormInside,EllipseTangent) > 0.0
                    //    Tan  = -Rx/Ry * Dot(P-C,Y) * x + Ry/Rx * Dot(P-C,X) * Y
                    let tangent_vec = -radius_x / radius_y
                        * (pt - center).dot(circ_y_axis_for_plane_normal)
                        * x_axis
                        + radius_y / radius_x
                            * (pt - center).dot(x_axis)
                            * circ_y_axis_for_plane_normal;
                    trim_entering.push(bi_norm_in.dot(tangent_vec) > 0.0);
                    trim_side.push(ii);
                }
            }
        }

        // If there were no trim points, with the whole (untrimmed) ellipse, then it's
        // all either inside or outside the domain. Also do this in the
        // one-intersection case.  [B286]
        if trim_points.len() < 2 {
            // need to classify a circumference point when there are no tangent points
            if !tangent {
                let circle_pt = center + radius_x * x_axis;
                let circle_uv = self.project_point_to_uv_domain(circle_pt);
                tangent_inside = uv_domain.contains_point(circle_uv, 0.0);
            }

            // and the circle is in the plane
            if tangent_inside {
                // build 1 full circle or ellipse
                crvs.push(new_arc(*anal_ivl)?);
                return Ok(crvs);
            }

            // or the circle is outside the plane and there is a tangent point
            if tangent {
                // mark the tangency with a degenerate curve
                crvs.push(Curve::from(BSplineCurve::create_degenerate_curve(
                    3, tangent_pt,
                )?));
                return Ok(crvs);
            }

            // there is no part of the circle in the square
            return Ok(crvs);
        }

        // arrive here when there are trim points, ordered in a CCW traversal of the
        // plane ZAxis. Make a trim curve from every entering xSectPoint to every
        // exiting xSectPoint; any trim curve containing the circle's start/end point is
        // broken up into two intervals.
        let count = trim_points.len();
        for ii in 0..count {
            let entering = trim_entering.get(ii).copied().unwrap_or(false);

            // skip exiting points - when plane and circ normals are opposite, entering
            // and exiting swap
            if (!opposite && !entering) || (opposite && entering) {
                continue;
            }

            // circle arc 3d start/end points
            let next = if opposite {
                ((ii + count) - 1) % count
            } else {
                (ii + 1) % count
            };
            let point1 = trim_points.get(ii).copied().unwrap_or(Vec3::UNDEF);
            let point2 = trim_points.get(next).copied().unwrap_or(Vec3::UNDEF);
            let dist = point2.distance(point1);

            // ellipse start angle Rad
            let vec = point1 - center;
            let dx = vec.dot(x_axis);
            let dy = vec.dot(y_axis);
            let enter_ang_rad = (radius_x / radius_y * dy).atan2(dx);

            // ellipse end angle Rad
            let vec = point2 - center;
            let dx = vec.dot(x_axis);
            let dy = vec.dot(y_axis);
            let exit_ang_rad = (radius_x / radius_y * dy).atan2(dx);

            // ellipse start/stop angles deg with periodicity
            let enter_ang_deg = anal_ivl
                .contains_periodic_value(enter_ang_rad * 180.0 / PI, 360.0, -1.0)
                .periodic_value;
            let exit_ang_deg = anal_ivl
                .contains_periodic_value(exit_ang_rad * 180.0 / PI, 360.0, -1.0)
                .periodic_value;

            // when circNorm is opposite to plane Norm Start/End will be reversed
            let tol_enter_deg = if enter_ang_deg > exit_ang_deg + EFF_ZERO {
                enter_ang_deg - 360.0
            } else if enter_ang_deg > exit_ang_deg {
                exit_ang_deg
            } else {
                enter_ang_deg
            };
            let arc_ivl = Extent1d::from_min_max(tol_enter_deg, exit_ang_deg);

            // trim arc to input circle domain (status ignored)
            let trim_ivls =
                ignored(anal_ivl.intersect_periodic(&arc_ivl, 360.0, true))?.unwrap_or_default();

            // for every trimmed arc interval
            for trim_ivl in trim_ivls {
                // Check for degenerate.
                let arc_len = (trim_ivl.length() * (PI / 180.0)) * radius_x;
                if arc_len < tol {
                    // build a degenerate line at the degenerate point
                    let cos_u = (trim_ivl.min() * PI / 180.0).cos();
                    let sin_u = (trim_ivl.min() * PI / 180.0).sin();
                    let point = center + cos_u * radius_x * x_axis + sin_u * radius_y * y_axis;
                    let Some(crv) = ignored(BSplineCurve::create_degenerate_curve(3, point))?
                    else {
                        return Ok(crvs);
                    };
                    crvs.push(Curve::from(crv));

                    if dist < tol {
                        break;
                    }
                    continue;
                }

                // output a circle or ellipse arc
                crvs.push(new_arc(trim_ivl)?);
            }
        }
        let _ = trim_side; // (only read by upstream's debug assertion)

        Ok(crvs)
    }
}

/// `SmPlane::GlobalSurfaceIntersect` of `this` (whose kind is `plane`) over `uv_domain`
/// with `other` over `other_uv_domain`: the plane/plane special case
/// ([`Plane::intersect_with_plane`]) when `other` is a plane, then the general
/// `SmSurface::GlobalSurfaceIntersect` when that is not complete (or for any other
/// surface kind).
///
/// The result is set in `out` (upstream's output arrays): on an error `out` keeps what
/// the failing special case appended before it; the general intersection resets it
/// first.
///
/// # Errors
///
/// Those of [`Plane::intersect_with_plane`]; [`SolidError::Unported`] from the general
/// intersector.
#[allow(clippy::too_many_arguments)]
pub(crate) fn global_surface_intersect_into<O: SolutionObject + Copy + PartialEq>(
    k: &Kernel,
    this: &Surface,
    plane: &Plane,
    uv_domain: &Extent2d,
    other: &Surface,
    other_uv_domain: &Extent2d,
    use_surface_edges: [bool; 2],
    approx_tol_3d: Option<f64>,
    ang_tol_rad: Option<f64>,
    context: Option<SurfaceIntersectContext<'_, O>>,
    out: &mut SurfaceIntersection,
) -> Result<()> {
    // assume special case intersections will be complete
    let mut need_nurb_intersection = true;
    // (each special case resets the outputs on entry)
    *out = SurfaceIntersection::default();

    // switch on OtherSurface Type looking for cheap special intersection case
    match other.kind_with_nurb(k)? {
        // plane/plane special case intersection
        SurfaceKind::Plane(other_plane) => {
            // (whether each plane has a face: SmSurface::GetFace, and its tolerance)
            let owners = [PlaneOwner::of(this)?, PlaneOwner::of(other)?];
            (need_nurb_intersection, *out) = plane.intersect_with_plane_owned(
                k,
                uv_domain,
                other_plane,
                other_uv_domain,
                approx_tol_3d,
                owners,
            )?;
        }
        // cone/plane special case intersection (SmCone_TYPE and SmCylinder_TYPE):
        // SmCone::IntersectWithPlane, which hides the surface of revolution's; the
        // orders and the UV curves are swapped
        SurfaceKind::Cone(cone) => {
            // the plane's boxes take its face's tolerance
            let (need, mut x) = cone.intersect_with_plane_owned(
                k,
                other_uv_domain,
                plane,
                PlaneOwner::of(this)?,
                uv_domain,
                approx_tol_3d,
            )?;
            core::mem::swap(&mut x.surface1_uv_curves, &mut x.surface2_uv_curves);
            (need_nurb_intersection, *out) = (need, x);
        }
        // SurfOfRevolution/plane special case intersection: the orders and the UV
        // curves are swapped, also in a partial output (the surface of revolution's
        // edge flags and angle tolerance are not used by it)
        SurfaceKind::SurfOfRevolution(rev) => {
            // the plane's boxes take its face's tolerance
            let need = rev.intersect_with_plane_owned_using_into(
                k,
                plane,
                PlaneOwner::of(this)?,
                uv_domain,
                approx_tol_3d,
                &SurfOfRevolution::own_sweep_iso,
                out,
            );
            core::mem::swap(&mut out.surface1_uv_curves, &mut out.surface2_uv_curves);
            need_nurb_intersection = need?;
        }
        // sphere/plane special case intersection: the orders and the UV curves are
        // swapped
        SurfaceKind::Sphere(sphere) => {
            // the plane's boxes take its face's tolerance
            let (need, mut x) = sphere.intersect_with_plane_owned(
                k,
                other_uv_domain,
                plane,
                PlaneOwner::of(this)?,
                uv_domain,
                approx_tol_3d,
            )?;
            core::mem::swap(&mut x.surface1_uv_curves, &mut x.surface2_uv_curves);
            (need_nurb_intersection, *out) = (need, x);
        }
        // SurfOfExtrusion/plane special case intersection: the orders and the UV curves
        // are swapped
        SurfaceKind::SurfOfExtrusion(ext) => {
            let mut x = ext.intersect_with_plane_on(
                k,
                other,
                other_uv_domain,
                plane,
                PlaneOwner::of(this)?,
                uv_domain,
                approx_tol_3d,
            )?;
            core::mem::swap(&mut x.surface1_uv_curves, &mut x.surface2_uv_curves);
            need_nurb_intersection = x.needs_more_intersections;
            *out = SurfaceIntersection::from(x);
        }
        // (an SmTorus has no special case: the default branch)
        SurfaceKind::Torus(_)
        | SurfaceKind::BSpline(_)
        | SurfaceKind::CurveBounded(_)
        | SurfaceKind::Step(_)
        | SurfaceKind::OffsetSurface(_)
        | SurfaceKind::SrfInVolume(_) => {}
    }

    // If we're here, then analytic intersection were not found - do general intersection
    if need_nurb_intersection {
        base_global_surface_intersect_into(
            k,
            this,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d,
            ang_tol_rad,
            context,
            out,
        )?;
    }

    // all done
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Plane {
        let mut p = Plane::new(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(-5.0, -3.0, 5.0, 3.0),
        );
        p.make_nurb().unwrap();
        p
    }

    fn dom() -> Extent2d {
        Extent2d::from_bounds(-5.0, -3.0, 5.0, 3.0)
    }

    #[test]
    fn offset_moves_along_normal() {
        let k = &Kernel::default();
        let p = rect().create_offset_surface(k, 2.0).unwrap();
        assert_eq!(p.position().origin(), Vec3::new(0.0, 0.0, 2.0));
        assert_eq!(p.step_uv_domain(), dom());
    }

    #[test]
    fn transform_rejects_anisotropic_scale() {
        let k = &Kernel::default();
        let mut p = rect();
        let r = p.transform(k, &Axis2Placement::IDENTITY, Some(Vec3::new(2.0, 1.0, 1.0)));
        assert!(r.is_err());
        let mut q = rect();
        q.transform(k, &Axis2Placement::IDENTITY, Some(Vec3::new(2.0, 2.0, 2.0)))
            .unwrap();
        let o = q.evaluate_point(Point2::new(5.0, 3.0));
        assert!((o - Vec3::new(10.0, 6.0, 0.0)).length() < 1e-12);
    }

    #[test]
    fn trim_with_domain_checks_containment() {
        let mut p = rect();
        assert!(p
            .trim_with_domain(&Extent2d::from_bounds(-6.0, 0.0, 1.0, 1.0))
            .is_err());
        p.trim_with_domain(&Extent2d::from_bounds(-1.0, 0.0, 1.0, 1.0))
            .unwrap();
        assert_eq!(
            p.step_uv_domain(),
            Extent2d::from_bounds(-1.0, 0.0, 1.0, 1.0)
        );
    }

    #[test]
    fn recognises_bilinear_plane() {
        let k = &Kernel::default();
        let s = BSplineSurface::create_bilinear_surface(
            Vec3::ZERO,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(2.0, 3.0, 0.0),
        )
        .unwrap();
        let p = Plane::is_nurb_surface_plane(k, &s, 1.0).unwrap().unwrap();
        assert!(p.base().gw_nurb().is_some());
        let twisted = BSplineSurface::create_bilinear_surface(
            Vec3::ZERO,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(2.0, 3.0, 1.0),
        )
        .unwrap();
        assert!(Plane::is_nurb_surface_plane(k, &twisted, 1.0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn plane_plane_intersections() {
        let k = &Kernel::default();
        let a = Plane::from_normal(Vec3::ZERO, Vec3::Z);
        let b = Plane::from_normal(Vec3::new(1.0, 0.0, 0.0), Vec3::X);
        let d = Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0);
        let (more, x) = a.intersect_with_plane(k, &d, &b, &d, None).unwrap();
        assert!(!more);
        assert_eq!(x.curves_3d.len(), 1);
        assert_eq!(x.curve_types, vec![TsectCurveType::Crossing]);
        assert!(x.surface1_uv_curves.iter().all(Option::is_none));

        // bounded crossing
        let mut c = Plane::new(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::Y,
            Vec3::Z,
            Vec2::new(1.0, 1.0),
            &Extent2d::from_bounds(-4.0, -2.0, 4.0, 2.0),
        );
        c.make_nurb().unwrap();
        let (_, x) = rect()
            .intersect_with_plane(k, &dom(), &c, &c.step_uv_domain(), None)
            .unwrap();
        assert_eq!(x.curves_3d.len(), 1);

        // coincident planes need the general intersector, parallel ones give nothing
        let (more, x) = rect()
            .intersect_with_plane(k, &dom(), &rect(), &dom(), None)
            .unwrap();
        assert!(more && x.curves_3d.is_empty());
        let r = rect();
        let (more, _) = r.intersect_with_plane(k, &dom(), &r, &dom(), None).unwrap();
        assert!(!more);
    }

    #[test]
    fn trims_curves_lines_and_ellipses() {
        let k = Kernel::default();
        let p = rect();
        let seg = Curve::from(
            Line::create_line_segment(
                3,
                Vec3::new(-10.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 0.0),
                None,
            )
            .unwrap(),
        );
        let cs = p
            .trim_curve_to_plane_domain(&k, &dom(), &seg, 1e-6)
            .unwrap();
        assert_eq!(cs.len(), 1);

        let (c, found) = p
            .make_new_plane_line(
                &dom(),
                Vec3::new(-10.0, 1.0, 0.0),
                Vec3::new(10.0, 1.0, 0.0),
                1e-6,
            )
            .unwrap();
        assert!(found && c.is_some());
        let (c, found) = p
            .make_new_plane_line(
                &dom(),
                Vec3::new(6.0, 1.0, 0.0),
                Vec3::new(8.0, 1.0, 0.0),
                1e-6,
            )
            .unwrap();
        assert!(!found && c.is_none());

        let full = Extent1d::from_min_max(0.0, 360.0);
        let inside = p
            .make_new_plane_ellipse(
                &k,
                &dom(),
                Vec3::ZERO,
                2.0,
                1.0,
                Vec3::X,
                Vec3::Y,
                &full,
                1e-6,
            )
            .unwrap();
        assert_eq!(inside.len(), 1);
        let crossing = p
            .make_new_plane_ellipse(
                &k,
                &dom(),
                Vec3::new(4.0, 0.0, 0.0),
                2.0,
                2.0,
                Vec3::X,
                Vec3::Y,
                &full,
                1e-6,
            )
            .unwrap();
        assert!(!crossing.is_empty());
        let point = p
            .make_new_plane_ellipse(
                &k,
                &dom(),
                Vec3::ZERO,
                0.0,
                0.0,
                Vec3::X,
                Vec3::Y,
                &full,
                1e-6,
            )
            .unwrap();
        assert_eq!(point.len(), 1);
    }

    #[test]
    fn reads_binary_plane() {
        let mut bytes = Vec::new();
        for x in [
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -5.0, -3.0, 5.0, 3.0, 1.0, 1.0,
        ] {
            bytes.extend_from_slice(&f64::to_ne_bytes(x));
        }
        for n in [1_i64, 1] {
            bytes.extend_from_slice(&n.to_ne_bytes());
        }
        for p in [1_i16, 1, 0] {
            bytes.extend_from_slice(&p.to_ne_bytes());
        }
        for x in [
            -5.0, -3.0, 0.0, -5.0, 3.0, 0.0, 5.0, -3.0, 0.0, 5.0, 3.0, 0.0, -5.0, -5.0, 5.0, 5.0,
            -3.0, -3.0, 3.0, 3.0,
        ] {
            bytes.extend_from_slice(&f64::to_ne_bytes(x));
        }
        let mut db = DatabaseReader::new();
        db.open_for_read(bytes.clone(), FileType::Binary);
        let p = Plane::read_from_db(&mut db, 30).unwrap();
        assert_eq!(p.step_uv_domain(), dom());
        let mut db = DatabaseReader::new();
        db.open_for_read(bytes[..100].to_vec(), FileType::Binary);
        assert!(Plane::read_from_db(&mut db, 30).is_err());
    }
}
