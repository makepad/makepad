//! `SmSurfOfExtrusion`, part 2: the special-case surface/surface intersections of a
//! surface of extrusion (with a plane, a cylinder and another surface of extrusion,
//! and their dispatcher `GlobalSurfaceIntersect`), the recognition of a B-spline
//! surface as an extrusion (`IsNurbSurfaceSurfOfExtrusion`), trimming to a NURB
//! domain and database input.
//!
//! Upstream: `source/SMLib/src/SmSurfOfExtrusion.cpp`,
//! `source/SMLib/inc/SmSurfOfExtrusion.h`.
//!
//! | Rust                                              | Upstream                                         |
//! |---------------------------------------------------|--------------------------------------------------|
//! | [`SurfOfExtrusion::global_surface_intersect`], [`IntersectTarget`] | `GlobalSurfaceIntersect`        |
//! | [`SurfOfExtrusion::intersect_with_plane`], [`ExtrusionIntersection`] | `IntersectWithPlane`          |
//! | [`SurfOfExtrusion::intersect_with_cylinder`]      | `IntersectWithCylinder`                          |
//! | [`SurfOfExtrusion::intersect_with_surf_of_extrusion`] | `IntersectWithSurfOfExtrusion`               |
//! | [`SurfOfExtrusion::is_nurb_surface_surf_of_extrusion`] | `IsNurbSurfaceSurfOfExtrusion` (static)     |
//! | [`SurfOfExtrusion::trim_with_domain`]             | `TrimWithDomain`                                 |
//! | [`SurfOfExtrusion::read_from_db`]                 | `ReadFromDB` (static)                            |
//!
//! Deviations:
//! - **Free surfaces.** `GlobalSurfaceIntersect` takes its other surface as an
//!   [`IntersectTarget`] (a plane, a cone, a surface of extrusion, or a [`Surface`] of
//!   any kind); `IntersectWithPlane`'s classification and the general intersection
//!   run on ownerless [`Surface`] stand-ins of the surfaces (no face reaches them).
//!   Through
//!   [`crate::surface::surface::SurfaceKind::SurfOfExtrusion`] the `SmSurface` dispatch
//!   passes the real surfaces instead (`global_surface_intersect_on`,
//!   `intersect_with_plane_on`, with the plane's face tolerance for its boxes and the
//!   plane/plane intersection).
//! - **Outputs.** Out-params become return values ([`ExtrusionIntersection`]). Upstream
//!   lets the caller pass `NULL` for each output array, but `IntersectWithPlane`,
//!   `IntersectWithCylinder` and `IntersectWithSurfOfExtrusion` dereference the 3D
//!   curve array unconditionally; here every output is always produced (with
//!   `pOpt3DCurves` and `pOptCurveTypes` present upstream's choices do not depend on
//!   the others). The UV curves upstream adds are `NULL` (`None`). `SmContext`
//!   arguments and the arguments upstream marks `NotUsed` are dropped; the
//!   approximation tolerance is the caller's `double`, converted as upstream's
//!   `SmApproxTol3d` parameter (its magnitude; `SM_USE_DEFAULT` is half the zone
//!   tolerance) by each special case and before the general intersection.
//! - **Statuses.** Calls whose status upstream ignores keep upstream's values, except
//!   that [`SolidError::Unported`] always propagates and a failure whose output
//!   upstream would read uninitialised (the generator's evaluation at a plane
//!   intersection, the projection of the generator, the surface evaluations of
//!   `IsNurbSurfaceSurfOfExtrusion`) is returned. A failed `CreateDirectrixFromPoint`
//!   (`pCrv` stays `NULL`) or `CreateIsoCircleFromPoint` (the circle stays `NULL`)
//!   is skipped as upstream; the null circle is then an `SM_ERR_NULL_POINTER` where
//!   upstream would dereference it (parallel extrusion and cylinder axes).
//!   `IsNurbSurfaceSurfOfExtrusion` returns `SmBoolean`; its `SER`s return the
//!   failing status as the boolean: here they are errors.
//! - **`IntersectWithPlane` tangent heuristic.** The first plane intersection point
//!   is only recorded when the first solution makes a curve (upstream then compares
//!   later points with an uninitialised point): here such a comparison is false.
//! - **Out-of-bounds evaluation.** As in part 1, `SmTemporaryChangeValue` gives the
//!   generator the surface's out-of-bounds flag (only a B-spline generator reads it,
//!   and the surface's setter already keeps both equal).
//! - **`ReadFromDB`.** The generator's owner is not set (as part 1). An orientation
//!   code outside `SmCurveOrientType` is `InvalidInput` (upstream stores it). A
//!   generator not derived from `SmBSplineCurve` is `InvalidInput` (upstream only
//!   asserts, then casts). An ASCII stream whose curve type cannot be read is
//!   `InvalidInput` (upstream reads its uninitialised type variable).
//! - **`IsNurbSurfaceSurfOfExtrusion`** does not copy the attributes of the test
//!   surface (`Notify(SM_NO_COPY, ..)`): a free [`BSplineSurface`] has none here.
//!   `TrimWithDomain`'s edit notifications are not modelled either.
//! - **General intersection.** When a special case needs more, the general
//!   intersection replaces its curves (`SmSurfaceIntersector::DoIntersection` resets
//!   the output arrays upstream).
//! - **`sm_IntersectGenerators` status.** `IntersectWithCylinder` and
//!   `IntersectWithSurfOfExtrusion` ignore it as upstream: a failure keeps the curves
//!   and types already added, with no more intersections needed (upstream clears the
//!   flag before its first failure). [`SolidError::Unported`] and
//!   [`SolidError::InvalidInput`] (an input upstream would not survive) propagate.
//! - **Debug code.** The `SM_DEBUG_CODE` drawing and dumps are omitted.

use crate::curve::curve::p7::PropertyAnalysisOptions;
use crate::curve::curve::{Curve, CurvePropertyType};
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::solution_array::{ObjectKind, SolutionObject, SolutionType};
use crate::geom::tol::{approx_tol3d, is_zero};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::curve_class::CurveClassification;
use crate::intersect::point_class::{ClassGeometry, ClassObject, ClassObjectKind, PointClassType};
use crate::kernel::Kernel;
use crate::math::{EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::BoundingBoxRequest as CurveBoxRequest;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::{
    BSplineSurface, BoundingBoxRequest as SurfaceBoxRequest, IsoCurveOptions, SurfParamType,
};
use crate::nurbs::math::{CPoint, NOW, NOZ};
use crate::surface::cone::Cone;
use crate::surface::plane::p2::PlaneOwner;
use crate::surface::plane::Plane;
use crate::surface::surface::p1::SurfaceIntersectContext;
use crate::surface::surface::p2::{
    base_global_surface_intersect_in, SurfaceIntersection, TsectCurveType, CONE_TYPE,
};
use crate::surface::surface::p4::NoObject;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::surface::volume::Volume;
use crate::topology::database_io::{DatabaseReader, FileType, InputStream};

use super::p1::{
    gen_base, gen_with_out_of_bounds, intersect_generators, CurveOrientType, SurfOfExtrusion,
};

const OP_PLANE: &str = "surface::surf_of_extrusion::p2::SurfOfExtrusion::intersect_with_plane";
const OP_CYL: &str = "surface::surf_of_extrusion::p2::SurfOfExtrusion::intersect_with_cylinder";
const OP_EXT: &str =
    "surface::surf_of_extrusion::p2::SurfOfExtrusion::intersect_with_surf_of_extrusion";
const OP_IS_NURB: &str =
    "surface::surf_of_extrusion::p2::SurfOfExtrusion::is_nurb_surface_surf_of_extrusion";
const OP_TRIM: &str = "surface::surf_of_extrusion::p2::SurfOfExtrusion::trim_with_domain";
const OP_READ: &str = "surface::surf_of_extrusion::p2::SurfOfExtrusion::read_from_db";

/// `ANALYTIC_TOL_SCALE` (`SmConfig.h`).
const ANALYTIC_TOL_SCALE: f64 = 100.0;

// ---- outputs ---------------------------------------------------------------------------

/// The outputs of the special-case intersections (`IntersectWithPlane`,
/// `IntersectWithCylinder`, `IntersectWithSurfOfExtrusion`, and
/// `GlobalSurfaceIntersect` when a special case answers): one entry per curve in each
/// array.
#[derive(Debug, Clone, Default)]
pub struct ExtrusionIntersection {
    /// `rbNeedsMoreIntersections`: the special case could not answer, use the general
    /// surface/surface intersection (always `false` from `GlobalSurfaceIntersect`).
    pub needs_more_intersections: bool,
    /// `pOpt3DCurves`: the intersection curves.
    pub curves_3d: Vec<Curve>,
    /// `pOptSurface1UVCurves`: their UV curves on this surface (`NULL` upstream).
    pub surface1_uv_curves: Vec<Option<Curve>>,
    /// `pOptSurface2UVCurves`: their UV curves on the other surface (`NULL` upstream).
    pub surface2_uv_curves: Vec<Option<Curve>>,
    /// `pOptCurveTypes`: the kind of each intersection.
    pub curve_types: Vec<TsectCurveType>,
    /// `pOptDeviations`: the distance of each curve from the surfaces (`0`).
    pub deviations: Vec<f64>,
}

impl ExtrusionIntersection {
    /// The "needs the general intersection" answer with empty outputs.
    fn needs_more() -> Self {
        Self {
            needs_more_intersections: true,
            ..Self::default()
        }
    }

    /// Adds the `NULL` UV curves and zero deviations of every curve (the closing
    /// loop of `IntersectWithCylinder` and `IntersectWithSurfOfExtrusion`).
    fn finish_null_outputs(&mut self) {
        for _ in 0..self.curves_3d.len() {
            self.surface1_uv_curves.push(None);
            self.surface2_uv_curves.push(None);
            self.deviations.push(0.0);
        }
    }
}

/// The other surface of [`SurfOfExtrusion::global_surface_intersect`] (upstream's
/// `const SmSurface&`, dispatched on its type).
#[derive(Debug, Clone, Copy)]
pub enum IntersectTarget<'a> {
    /// An `SmPlane`.
    Plane(&'a Plane),
    /// An `SmCone` or `SmCylinder` (only the `SmCone_TYPE` is a special case).
    Cone(&'a Cone),
    /// An `SmSurfOfExtrusion`.
    SurfOfExtrusion(&'a SurfOfExtrusion),
    /// A surface of a ported [`SurfaceKind`].
    Surface(&'a Surface),
}

/// The generator's handle in its plane intersection solutions (upstream's curve
/// pointer): the solver's duplicate test reads its natural interval.
#[derive(Debug, Clone, Copy, PartialEq)]
struct GenHandle {
    ivl: Extent1d,
}

impl SolutionObject for GenHandle {
    fn kind(&self) -> ObjectKind {
        ObjectKind::Curve
    }

    fn natural_interval(&self) -> Option<Extent1d> {
        Some(self.ivl)
    }
}

/// `TO_EUCLID(from, to)` (`SmNurbsCrv.h`): divides by the weight unless non-rational;
/// a missing `z` becomes `0`.
fn to_euclid(p: CPoint) -> Point3 {
    if p.w == NOW {
        let z = if p.z == NOZ { 0.0 } else { p.z };
        Point3::new(p.x, p.y, z)
    } else {
        let z = if p.z == NOZ { 0.0 } else { p.z / p.w };
        Point3::new(p.x / p.w, p.y / p.w, z)
    }
}

/// `COPY_XYZ(from, to)` (`SmNurbsCrv.h`): overwrites the stored `x`, `y`, `z`.
fn copy_xyz(from: Point3, to: &mut CPoint) {
    to.x = from.x;
    to.y = from.y;
    to.z = from.z;
}

/// `SM_DEG2RAD`.
fn deg2rad(adeg: f64) -> f64 {
    adeg * (PI / 180.0)
}

/// The handles of the classification `IntersectWithPlane` runs on this surface: the
/// plane/plane line and this surface (upstream's pointers), plus the boundary
/// temporaries the classification makes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ClassifyHandle {
    Curve,
    Surface,
    Temporary(usize, ClassObjectKind),
}

impl ClassObject for ClassifyHandle {
    fn class_kind(&self) -> ClassObjectKind {
        match self {
            Self::Curve => ClassObjectKind::Curve,
            Self::Surface => ClassObjectKind::Surface,
            Self::Temporary(_, kind) => *kind,
        }
    }

    fn temporary(index: usize, kind: ClassObjectKind) -> Option<Self> {
        Some(Self::Temporary(index, kind))
    }
}

/// The geometry behind [`ClassifyHandle`]s.
struct ClassifyGeometry<'a> {
    curve: &'a Curve,
    surface: &'a Surface,
}

impl ClassGeometry<ClassifyHandle> for ClassifyGeometry<'_> {
    fn curve(&self, o: ClassifyHandle) -> Option<&Curve> {
        (o == ClassifyHandle::Curve).then_some(self.curve)
    }

    fn surface(&self, o: ClassifyHandle) -> Option<&Surface> {
        (o == ClassifyHandle::Surface).then_some(self.surface)
    }

    fn volume(&self, _: ClassifyHandle) -> Option<&Volume> {
        None
    }

    fn point(&self, _: ClassifyHandle) -> Option<Point3> {
        None
    }
}

impl From<ExtrusionIntersection> for SurfaceIntersection {
    fn from(x: ExtrusionIntersection) -> Self {
        Self {
            curves_3d: x.curves_3d,
            surface1_uv_curves: x.surface1_uv_curves,
            surface2_uv_curves: x.surface2_uv_curves,
            curve_types: x.curve_types,
            deviations: x.deviations,
        }
    }
}

impl SurfOfExtrusion {
    /// The generator (`m_pGenCurve`).
    fn gen_or_null(&self, op: &'static str) -> Result<&Curve> {
        self.gen_curve.as_ref().ok_or(SolidError::null_pointer(op))
    }

    // ---- GlobalSurfaceIntersect -------------------------------------------------------

    /// The intersection curves of this surface over `uv_domain` with `other` over
    /// `other_uv_domain` (`GlobalSurfaceIntersect`): a plane, an `SmCone_TYPE` cone
    /// and a surface of extrusion first try their special case
    /// ([`Self::intersect_with_plane`], [`Self::intersect_with_cylinder`],
    /// [`Self::intersect_with_surf_of_extrusion`]); when that needs more, and for any
    /// other surface, the general intersection (`SmSurface::GlobalSurfaceIntersect`)
    /// runs.
    ///
    /// # Errors
    ///
    /// Those of the special cases and of the general intersection
    /// (`SmAdvSurfaceIntersector`, on ownerless [`Surface`] stand-ins of both
    /// surfaces).
    #[allow(clippy::too_many_arguments)] // upstream's signature
    pub fn global_surface_intersect(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        other: IntersectTarget<'_>,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
    ) -> Result<ExtrusionIntersection> {
        // (the general intersection takes the edge flags and angle tolerance)

        // switch on the other surface's type: a wrapped surface by its kind, as
        // `global_surface_intersect_on_in`
        let target = match other {
            IntersectTarget::Surface(s) => match s.kind_with_nurb(k)? {
                SurfaceKind::Plane(p) => IntersectTarget::Plane(p),
                SurfaceKind::Cone(c) => IntersectTarget::Cone(c.as_ref()),
                SurfaceKind::SurfOfExtrusion(e) => IntersectTarget::SurfOfExtrusion(e.as_ref()),
                _ => other,
            },
            _ => other,
        };
        let special = match target {
            IntersectTarget::Plane(p) => {
                Some(self.intersect_with_plane(k, uv_domain, p, other_uv_domain, approx_tol_3d)?)
            }
            // (an SmCylinder is not SmCone_TYPE: the default branch)
            IntersectTarget::Cone(c) if c.type_code() == CONE_TYPE => {
                Some(self.intersect_with_cylinder(k, c, approx_tol_3d)?)
            }
            IntersectTarget::SurfOfExtrusion(s) => {
                Some(self.intersect_with_surf_of_extrusion(k, s, approx_tol_3d)?)
            }
            _ => None,
        };

        // when bNeedNurbIntersection == TRUE, intersection was too complicated for
        // analytic intersectors
        if let Some(r) = special {
            if !r.needs_more_intersections {
                return Ok(r);
            }
        }
        // SmSurface::GlobalSurfaceIntersect of a free extrusion: the general intersection
        // of ownerless stand-ins of both surfaces (no face reaches them, as upstream for
        // free surfaces); it replaces the special case's curves
        let this = Surface::from(self.clone());
        let stand_in;
        let other = match other {
            IntersectTarget::Surface(s) => s,
            IntersectTarget::Plane(p) => {
                stand_in = Surface::from(p.clone());
                &stand_in
            }
            IntersectTarget::Cone(c) => {
                stand_in = Surface::from(c.clone());
                &stand_in
            }
            IntersectTarget::SurfOfExtrusion(s) => {
                stand_in = Surface::from(s.clone());
                &stand_in
            }
        };
        let r = base_global_surface_intersect_in::<NoObject>(
            k,
            &this,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d.map(|t| approx_tol3d(k, t)),
            ang_tol_rad,
            None,
        )?;
        Ok(ExtrusionIntersection {
            needs_more_intersections: false,
            curves_3d: r.curves_3d,
            surface1_uv_curves: r.surface1_uv_curves,
            surface2_uv_curves: r.surface2_uv_curves,
            curve_types: r.curve_types,
            deviations: r.deviations,
        })
    }

    /// [`Self::global_surface_intersect`] of the surface `this` holding this extrusion
    /// with the surface `other` (the `SmSurface` dispatch): the special cases of a
    /// plane (with its face's tolerance), an `SmCone_TYPE` cone and a surface of
    /// extrusion, then, where they need more, the general intersection of `this` and
    /// `other` (which replaces the special case's curves).
    ///
    /// # Errors
    ///
    /// Those of [`Self::global_surface_intersect`].
    #[allow(clippy::too_many_arguments)] // upstream's signature
    pub(crate) fn global_surface_intersect_on_in<O: SolutionObject + Copy + PartialEq>(
        &self,
        k: &Kernel,
        this: &Surface,
        uv_domain: &Extent2d,
        other: &Surface,
        other_uv_domain: &Extent2d,
        use_surface_edges: [bool; 2],
        approx_tol_3d: Option<f64>,
        ang_tol_rad: Option<f64>,
        context: Option<SurfaceIntersectContext<'_, O>>,
    ) -> Result<SurfaceIntersection> {
        let special = match other.kind_with_nurb(k)? {
            SurfaceKind::Plane(p) => Some(self.intersect_with_plane_on(
                k,
                this,
                uv_domain,
                p,
                PlaneOwner::of(other)?,
                other_uv_domain,
                approx_tol_3d,
            )?),
            // (an SmCylinder is not SmCone_TYPE: the default branch)
            SurfaceKind::Cone(c) if c.type_code() == CONE_TYPE => {
                Some(self.intersect_with_cylinder(k, c, approx_tol_3d)?)
            }
            SurfaceKind::SurfOfExtrusion(s) => {
                Some(self.intersect_with_surf_of_extrusion(k, s, approx_tol_3d)?)
            }
            _ => None,
        };
        if let Some(r) = special {
            if !r.needs_more_intersections {
                return Ok(SurfaceIntersection::from(r));
            }
        }
        // (SmSurfaceIntersector::DoIntersection resets the output arrays: the special
        // case's curves are dropped; the tolerance is the SmApproxTol3d upstream passes
        // on, already converted)
        base_global_surface_intersect_in(
            k,
            this,
            uv_domain,
            other,
            other_uv_domain,
            use_surface_edges,
            approx_tol_3d.map(|t| approx_tol3d(k, t)),
            ang_tol_rad,
            context,
        )
    }

    // ---- IntersectWithPlane -----------------------------------------------------------

    /// The special cases of the intersection with `plane` over `other_uv_domain`
    /// (`IntersectWithPlane`); `uv_domain` (this surface's NURB domain) gives the
    /// generator's interval for the closed test. The tolerance is the smaller of
    /// `approx_tol_3d` and `SM_EFF_ZERO_SQRT`.
    ///
    /// * A linear or coplanar generator whose swept plane is not parallel to the
    ///   plane: the line common to both planes, classified on this surface and
    ///   trimmed to the pieces inside it (crossing).
    /// * A perpendicular or independent generator whose plane is parallel to the
    ///   plane: nothing when the plane cuts the extrusion axis outside the extrusion
    ///   interval (ends snapped within the tolerance), else the generator projected
    ///   onto the plane along the extrusion, trimmed to the plane domain.
    /// * An extrusion parallel to the plane: a directrix through every generator /
    ///   plane intersection point, trimmed to the plane domain (a degenerate one when
    ///   shorter than the tolerance), touching, tangent (generator tangent within 2
    ///   degrees of the plane) or crossing; of close tangent points, and of close
    ///   points at the two ends of a closed generator, only the better solution is
    ///   kept. A range solution (generator in the plane) needs more.
    /// * Otherwise the general intersection is needed.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` without generator; those of the generator's plane
    /// intersection, evaluations, closed and degenerate tests;
    /// [`SolidError::Unported`]: the first case when the planes meet in a line
    /// (`SmSurface::CurveOnClassify` up to `IntersectNaturalBounds`, unit
    /// `intersect.curve_class_p8`); the status of
    /// the plane/plane intersection is ignored as upstream (a failure gives no
    /// curves).
    pub fn intersect_with_plane(
        &self,
        k: &Kernel,
        uv_domain: &Extent2d,
        plane: &Plane,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<ExtrusionIntersection> {
        // (a free surface: this surface as an ownerless stand-in, the plane faceless)
        let this = Surface::from(self.clone_exact());
        self.intersect_with_plane_on(
            k,
            &this,
            uv_domain,
            plane,
            PlaneOwner::NoFace,
            other_uv_domain,
            approx_tol_3d,
        )
    }

    /// [`Self::intersect_with_plane`] of the surface `this` holding this extrusion (the
    /// classification's surface), with the owner of `plane` (its face's zone tolerance
    /// expands the plane's boxes, as `GetFace()->GetTolerance()` upstream).
    ///
    /// # Errors
    ///
    /// Those of [`Self::intersect_with_plane`].
    #[allow(clippy::too_many_lines)] // one upstream function
    #[allow(clippy::too_many_arguments)] // upstream's arguments and the owners
    pub(crate) fn intersect_with_plane_on(
        &self,
        k: &Kernel,
        this: &Surface,
        uv_domain: &Extent2d,
        plane: &Plane,
        plane_owner: PlaneOwner,
        other_uv_domain: &Extent2d,
        approx_tol_3d: Option<f64>,
    ) -> Result<ExtrusionIntersection> {
        // init output
        let mut out = ExtrusionIntersection::default();

        // plane and ExtrusionSurface locals
        let plane_normal = plane.position().z_axis();
        let plane_origin = plane.position().origin();
        let ang_tol = EFF_ZERO_SQRT;
        let tol = match approx_tol_3d {
            Some(t) => approx_tol3d(k, t).min(EFF_ZERO_SQRT),
            None => EFF_ZERO_SQRT,
        };

        let gen_curve_nurb_ivl = if self.swap_uv {
            uv_domain.v_interval()
        } else {
            uv_domain.u_interval()
        };
        let swept_normal = match self.orientation {
            CurveOrientType::Linear => self.curve_vec.cross(self.extrusion_vec),
            CurveOrientType::Coplanar => self.curve_vec,
            _ => Vec3::UNDEF,
        };

        // If we are allowing out-of-bounds eval, then the gen curve needs to too.
        let gen_own = self.gen_or_null(OP_PLANE)?;
        let tmp = gen_with_out_of_bounds(gen_own, self.base.is_out_of_bounds_enabled());
        let gen = tmp.as_ref().unwrap_or(gen_own);

        // When GenCurve is linear or Planar and swept in its own plane
        // and SweptPlanarSurface is not parallel to input Surface
        // Then,
        //   XSect = Line
        if matches!(
            self.orientation,
            CurveOrientType::Linear | CurveOrientType::Coplanar
        ) && !swept_normal.is_parallel_to(plane_normal, ang_tol)
        {
            // build a plane bigger than the Surface
            let this_plane = Plane::from_normal(self.curve_point, swept_normal);

            // get the Plane/Plane intersection line trimmed to input Plane
            //  builds an SmLine or an SmBSplineCurve made by CreateDegenerateCurve().
            //  (status ignored upstream: every failure of SmPlane::IntersectWithPlane
            //  comes before its first output, which it resets with the needs-more
            //  flag, so a failure is "no curves, no more intersections")
            let plane_tol = approx_tol_3d.map(|t| approx_tol3d(k, t));
            let (needs_more, plane_x) = match plane.intersect_with_plane_owned(
                k,
                other_uv_domain,
                &this_plane,
                &this_plane.step_uv_domain(),
                plane_tol,
                [plane_owner, PlaneOwner::NoFace],
            ) {
                Ok(r) => r,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => (false, SurfaceIntersection::default()),
            };
            out.needs_more_intersections = needs_more;

            // no work - no solutions
            if plane_x.curves_3d.is_empty() {
                return Ok(out);
            }

            // arrive here when two planes intersect in one line
            let mut lines = plane_x.curves_3d;
            if lines.is_empty() {
                return Err(SolidError::sm_err(OP_PLANE));
            }
            let line = lines.swap_remove(0);
            let line_ivl = line.natural_interval();

            // classify the 3dCurves against the natural boundaries of this surface (for
            // trimming)
            let mut classification =
                CurveClassification::with_curve_interval(k, ClassifyHandle::Curve, line_ivl, tol);
            this.curve_on_classify(
                k,
                ClassifyHandle::Surface,
                &ClassifyGeometry {
                    curve: &line,
                    surface: this,
                },
                &mut classification,
            )?;

            // for every classification interval in this surface (the line moves to the
            // output with the first whole-curve interval; later copies copy it there)
            let mut line = Some(line);
            let mut moved_to = 0;
            for interval in classification.intervals() {
                // skip intervals outside the surface
                if interval.mid.point_class() == PointClassType::Unknown {
                    continue;
                }

                // add interval to output
                let mut ivl = *interval.interval();

                // quick case - the whole curve
                if ivl.are_equal(&line_ivl, EFF_ZERO) {
                    let whole = match line.take() {
                        Some(l) => {
                            moved_to = out.curves_3d.len();
                            l
                        }
                        None => out
                            .curves_3d
                            .get(moved_to)
                            .ok_or(SolidError::sm_err(OP_PLANE))?
                            .clone(),
                    };
                    out.curves_3d.push(whole);
                    out.curve_types.push(TsectCurveType::Crossing);
                    continue;
                }

                // else - add a trimmed curve to the output (Trim may snap the
                // interval to existing knots; statuses ignored upstream)
                let source = match line.as_ref() {
                    Some(l) => l,
                    None => out
                        .curves_3d
                        .get(moved_to)
                        .ok_or(SolidError::sm_err(OP_PLANE))?,
                };
                let mut curve = source.copy(k)?;
                ignore_status(curve.trim(k, &mut ivl, TrimMode::Notify))?;
                out.curves_3d.push(curve);
                out.curve_types.push(TsectCurveType::Crossing);
            }
        }
        // When GenCurve is planar with an independent sweep direction
        // and crPlane and plane containing GenCurve are parallel
        // Then,
        //   XSect = translated copy of sweptCurve
        else if matches!(
            self.orientation,
            CurveOrientType::Perpendicular | CurveOrientType::Independent
        ) && self.curve_vec.is_parallel_to(plane_normal, ang_tol)
        {
            // get param along Extrusion Vector where InputPlane intersects
            // ExtrusionVector (AngleBetween's status ignored: 0 on failure)
            let dist_center_base_to_plane = (plane_origin - self.origin).dot(self.curve_vec);
            let angle_rad = self
                .curve_vec
                .angle_between(self.extrusion_vec)
                .unwrap_or(0.0);
            let dist_along_extrusion = dist_center_base_to_plane * angle_rad.cos();
            let param_along_extrusion = dist_along_extrusion / self.extrusion_vec.length();

            // get extrusionSurface's sweep interval
            let orig_ivl = self.anal_uv_domain.v_interval();

            // snap nearby distanceValues to extrusion begin / end position (the
            // snapped value is not read again), skip planes outside of the
            // extrusion domain
            let snapped = (param_along_extrusion - orig_ivl.min()).abs() < tol
                || (param_along_extrusion - orig_ivl.max()).abs() < tol;
            if !snapped && !orig_ivl.contains_value(param_along_extrusion, 0.0) {
                return Ok(out); // No intersections
            }

            // intersectionCurve = sweptCurveCopy projected to the plane along the
            // extrusion vector (status ignored upstream, which then reads the curve)
            let proj_curve = gen_base(gen)?.create_plane_projection(
                crate::nurbs::b_spline_curve::ProjectionType::Parallel,
                plane_origin,
                plane_normal,
                self.extrusion_vec,
            )?;

            // get plane and intersection curve bounding boxes (statuses ignored: a
            // failed box stays initialised)
            // (bExpandPosBoxesByZoneTol3d defaults to TRUE)
            let plane_req = SurfaceBoxRequest {
                pseudo: true,
                expand_by_zone_tol: true,
                // (the zone tolerance of the plane's face)
                expand_tolerance: plane_owner.expand_tolerance()?,
                ..SurfaceBoxRequest::default()
            };
            let plane_nurb = plane.base_with_nurb()?;
            let mut plane_pbox = ignored(plane_nurb.calculate_bounding_box(
                k,
                &plane.natural_uv_domain(),
                &plane_req,
            ))?
            .and_then(|b| b.pseudo)
            .unwrap_or_default();
            let curve_req = CurveBoxRequest {
                pseudo: true,
                ..CurveBoxRequest::default()
            };
            let curve_pbox = ignored(
                proj_curve.calculate_bounding_box(&proj_curve.natural_interval(), curve_req),
            )?
            .and_then(|b| b.pseudo)
            .unwrap_or_default();

            // only use curves intersecting plane domain
            plane_pbox.expand_absolute(tol);
            if !plane_pbox.are_disjoint(&curve_pbox, None) {
                // trim curves to plane domain (status ignored upstream)
                let proj = Curve::from(proj_curve);
                let curves_3d =
                    ignored(plane.trim_curve_to_plane_domain(k, other_uv_domain, &proj, tol))?
                        .unwrap_or_default();

                // add trim curves to output
                out.curves_3d.extend(curves_3d);
            }
        }
        // when extrusion vec is perpendicular to plane normal
        // - xsect is one or more lines each line generated by sweeping
        //   a plane/GenCurve intersection point.
        else if self
            .extrusion_vec
            .is_perpendicular_to(plane_normal, ang_tol)
        {
            // get sweptCurve/plane intersection point
            let d = -plane_origin.dot(plane_normal);
            let vecs = [plane_normal];
            let opts = PropertyAnalysisOptions {
                property_value: Some(d),
                opt_vectors: &vecs,
                want_comb: false,
                handle: Some(GenHandle {
                    ivl: gen.natural_interval(),
                }),
            };
            let solutions = gen
                .global_property_analysis(
                    k,
                    &gen.natural_interval(),
                    CurvePropertyType::PlaneIntersection,
                    tol,
                    &opts,
                )?
                .solutions;
            let sols = solutions.as_slice();

            // for every SweptCurve/Plane intersection
            if sols
                .iter()
                .any(|s| s.solution_type == SolutionType::RangeOfValues)
            {
                // Some Surf/Surf coincidence - handle it at a higher level
                out.needs_more_intersections = true;
                return Ok(out);
            }

            // arrive here when all solutions are PointSolutions
            let is_closed = gen.is_closed(k, &gen_curve_nurb_ivl, 0.0)?;

            // for every SweptCurve/Plane point intersection - build, trim and if
            // nonNULL, output a line
            let mut last_tangent = false;
            let mut last_crv_pt = Vec3::UNDEF;
            let mut first_crv_pt: Option<Point3> = None;
            for (ii, sol) in sols.iter().enumerate() {
                // create line segment intersection curve (Evaluate's status ignored
                // upstream, which then reads the points)
                let crv_pt = gen.evaluate(k, sol.start.parameters[0], 1, true, true)?;
                let (Some(&pt0), Some(&pt1)) = (crv_pt.first(), crv_pt.get(1)) else {
                    return Err(SolidError::sm_err(OP_PLANE));
                };
                let crv = match self.create_directrix_from_point(k, pt0, EFF_ZERO_SQRT) {
                    Ok(c) => c,
                    Err(e) if e.is_unported() => return Err(e),
                    Err(_) => continue, // pCrv stays NULL
                };
                let mut crv = crv;

                // trim line control Points to plane
                let nurb = crv.get_or_create_gw_nurb()?;
                let (Some(&pw0), Some(&pw1)) = (nurb.cpts().first(), nurb.cpts().get(1)) else {
                    return Err(SolidError::sm_err(OP_PLANE));
                };
                let start_pt = to_euclid(pw0);
                let end_pt = to_euclid(pw1);
                let Some((mut start_pt, mut end_pt)) = plane.trim_plane_line_to_plane_domain(
                    &plane.natural_uv_domain(),
                    start_pt,
                    end_pt,
                    tol,
                ) else {
                    // only use curves that intersect plane domain
                    continue;
                };

                // get trimmed xsectCurve length
                let length = start_pt.distance(end_pt);

                // turn short curves into degenerate ones by snapping endPoints
                if length < tol {
                    // make a degenerate line
                    start_pt = (start_pt + end_pt) / 2.0;
                    end_pt = start_pt;
                }

                // Trim the curve definition - move control points
                let nurb = crv.get_or_create_gw_nurb()?;
                let cpts = nurb.cpts_mut();
                if let Some(p) = cpts.get_mut(0) {
                    copy_xyz(start_pt, p);
                }
                if let Some(p) = cpts.get_mut(1) {
                    copy_xyz(end_pt, p);
                }

                // classify the solution - crossing or tangent
                let crv_speed = pt1.length();
                let scaled_zero = EFF_ZERO * (1.0 + crv_speed.max(pt0.max_dimension()));
                let angle_tol = deg2rad(2.0);
                let tangent = crv_speed > scaled_zero
                    && (pt1 / crv_speed).is_perpendicular_to(plane_normal, angle_tol);

                // cheesy heuristic - GlobalPropertyAnalysis returns multiple close hits
                // for a single tangent intersection and intersections on closed seam
                // boundaries; the solutions are sorted by param value - so just save
                // the best one.
                if ii > 0 {
                    let value = sol.start.solution_value;
                    // remove multiple solutions due to tangent points
                    if tangent && last_tangent && last_crv_pt.distance(pt0) < tol {
                        let prev = sols.get(ii - 1).map_or(0.0, |s| s.start.solution_value);
                        if value < prev {
                            // replace the last saved solution with this solution
                            out.curves_3d.pop();
                            out.curve_types.pop();
                        } else {
                            // don't save this solution
                            continue;
                        }
                    }
                    // remove multiple solutions due to close curve start/end hits
                    // (the other end of the seam)
                    else if is_closed && first_crv_pt.is_some_and(|f| f.distance(pt0) < tol) {
                        let first = sols.first().map_or(0.0, |s| s.start.solution_value);
                        if value < first {
                            // replace the first saved solution with this solution
                            if !out.curves_3d.is_empty() {
                                out.curves_3d.remove(0);
                            }
                            if !out.curve_types.is_empty() {
                                out.curve_types.remove(0);
                            }
                        } else {
                            // don't save this solution
                            continue;
                        }
                    }
                }

                // prepare for next iteration
                last_tangent = tangent;
                last_crv_pt = pt0;
                if ii == 0 {
                    first_crv_pt = Some(pt0);
                }

                // add curve to ouput
                let curve_type = if crv.is_degenerate(k, EFF_ZERO, None)? {
                    TsectCurveType::Touching
                } else if tangent {
                    TsectCurveType::Tangent
                } else {
                    TsectCurveType::Crossing
                };
                out.curves_3d.push(Curve::from(crv));
                out.curve_types.push(curve_type);
            }
        } else {
            // this is not a special case intersection:
            //  plane cuts through SurfOfExtrusion at an angle,
            out.needs_more_intersections = true;
            return Ok(out);
        }

        // set output
        let cnt = out.curves_3d.len();
        for ii in 0..cnt {
            out.surface1_uv_curves.push(None);
            out.surface2_uv_curves.push(None);
            // Line solutions have already added the curve types
            if cnt != out.curve_types.len() {
                let crv = out.curves_3d.get(ii).ok_or(SolidError::sm_err(OP_PLANE))?;
                let t = if crv.is_degenerate(k, EFF_ZERO, None)? {
                    TsectCurveType::Touching
                } else {
                    TsectCurveType::Crossing
                };
                out.curve_types.push(t);
            }
            out.deviations.push(0.0);
        }

        // all done
        Ok(out)
    }

    // ---- IntersectWithCylinder --------------------------------------------------------

    /// The intersection with the cone `cylinder` when its radii agree within
    /// `SM_EFF_ZERO * (1 + max radius)` (`IntersectWithCylinder`): the generators'
    /// intersection of [`intersect_generators`] with the cylinder's bottom iso circle
    /// swept along its axis over its heights (lines along a cylinder axis parallel to
    /// the extrusion). Otherwise, and for an axis not parallel to the extrusion, the
    /// general intersection is needed. The tolerance is `approx_tol_3d`
    /// (`SM_EFF_ZERO_SQRT` without).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when the iso circle could not be made for a parallel
    /// axis (upstream dereferences `NULL`), and when this surface has none;
    /// [`SolidError::Unported`] and [`SolidError::InvalidInput`] of
    /// [`intersect_generators`] (its other failures are ignored as upstream).
    pub fn intersect_with_cylinder(
        &self,
        k: &Kernel,
        cylinder: &Cone,
        approx_tol_3d: Option<f64>,
    ) -> Result<ExtrusionIntersection> {
        // locals
        let other_bot_radius = cylinder.bot_radius();
        let other_top_radius = cylinder.top_radius();

        // no work - other surface not a rectilinear cylinder:
        let radius_diff_tolerance = EFF_ZERO * (1.0 + other_bot_radius.max(other_top_radius));
        if (other_bot_radius - other_top_radius).abs() > radius_diff_tolerance {
            return Ok(ExtrusionIntersection::needs_more());
        }

        // pick tolerance
        let tol = approx_tol_3d.map_or(EFF_ZERO_SQRT, |t| approx_tol3d(k, t));

        // otherCylinderSurface locals
        let other_position = cylinder.rev().position();
        let other_origin = other_position.origin();
        let other_z_axis = other_position.z_axis();
        let other_ivl = Extent1d::from_min_max(cylinder.bot_height(), cylinder.top_height());
        let other_axis_segment_start_point = other_origin + other_ivl.min() * other_z_axis;
        // (CreateIsoCircleFromPoint's status ignored: the circle stays NULL)
        let other_gen_curve =
            match cylinder.create_iso_circle_from_point(k, other_axis_segment_start_point, tol) {
                Ok(c) => c,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => None,
            };

        // compute intersections based on overlap of extrusion lines that coalign from
        // the GenCurves
        let mut out = ExtrusionIntersection::default();
        out.needs_more_intersections = self.intersect_generators_with(
            k,
            tol,
            other_gen_curve.as_ref(),
            other_z_axis,
            &other_ivl,
            &mut out,
            OP_CYL,
        )?;

        // for every intersection found - finish the outputs
        out.finish_null_outputs();
        Ok(out)
    }

    // ---- IntersectWithSurfOfExtrusion -------------------------------------------------

    /// The intersection with the surface of extrusion `other`
    /// (`IntersectWithSurfOfExtrusion`): the generators' intersection of
    /// [`intersect_generators`] with the other generator swept along its extrusion
    /// over its STEP V interval (lines along parallel extrusions); not parallel
    /// extrusions need the general intersection. The tolerance is `approx_tol_3d`
    /// (`SM_EFF_ZERO_SQRT` without).
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` for parallel extrusions when `other` has no generator
    /// (upstream dereferences `NULL`), and when this surface has none;
    /// [`SolidError::Unported`] and [`SolidError::InvalidInput`] of
    /// [`intersect_generators`] (its other failures are ignored as upstream).
    pub fn intersect_with_surf_of_extrusion(
        &self,
        k: &Kernel,
        other: &Self,
        approx_tol_3d: Option<f64>,
    ) -> Result<ExtrusionIntersection> {
        // pick tolerance
        let tol = approx_tol_3d.map_or(EFF_ZERO_SQRT, |t| approx_tol3d(k, t));

        // OtherExtrusionSurface Extrusion Segment
        let other_extrusion_vec = other.extrusion_vector();
        let other_ivl = other.step_uv_domain().v_interval();
        let other_gen_curve = other.gen_curve();

        // compute intersections based on overlap of extrusion lines that coalign from
        // the GenCurves
        let mut out = ExtrusionIntersection::default();
        out.needs_more_intersections = self.intersect_generators_with(
            k,
            tol,
            other_gen_curve,
            other_extrusion_vec,
            &other_ivl,
            &mut out,
            OP_EXT,
        )?;

        // for every intersection found - finish the outputs
        out.finish_null_outputs();
        Ok(out)
    }

    /// `sm_IntersectGenerators(*this, tol, *pOtherGenCurve, ...)` with a generator
    /// that may be `NULL`: dereferenced only past the parallel test.
    #[allow(clippy::too_many_arguments)]
    fn intersect_generators_with(
        &self,
        k: &Kernel,
        tol: f64,
        other_gen: Option<&Curve>,
        other_vec: Vec3,
        other_ivl: &Extent1d,
        out: &mut ExtrusionIntersection,
        op: &'static str,
    ) -> Result<bool> {
        // (sm_IntersectGenerators reads this surface's generator before anything
        // else: upstream dereferences NULL)
        self.gen_or_null(op)?;
        let Some(other_gen) = other_gen else {
            // (sm_IntersectGenerators returns at its parallel test before touching
            // the other generator)
            if !other_vec.is_parallel_to(self.extrusion_vec, crate::math::EFF_ZERO_RAD) {
                return Ok(true);
            }
            return Err(SolidError::null_pointer(op));
        };
        // (status ignored upstream: every failure comes after sm_IntersectGenerators
        // cleared the needs-more flag, and keeps the curves and types it already
        // added; an Unported callee, or an input upstream would not survive, still
        // propagates)
        match intersect_generators(
            k,
            self,
            tol,
            other_gen,
            other_vec,
            other_ivl,
            Some(&mut out.curves_3d),
            Some(&mut out.curve_types),
        ) {
            Ok(r) => Ok(r.needs_more_intersections),
            Err(e)
                if e.is_unported()
                    || matches!(
                        e,
                        SolidError::InvalidInput { .. } | SolidError::PortInvariant { .. }
                    ) =>
            {
                Err(e)
            }
            Err(_) => Ok(false),
        }
    }

    // ---- IsNurbSurfaceSurfOfExtrusion -------------------------------------------------

    /// The surface of extrusion whose B-spline surface equals `test_surface`, if it is
    /// one (`IsNurbSurfaceSurfOfExtrusion`): degree 1 with 2 control points in exactly
    /// one direction (the sweep), and the iso curve at the end of the sweep a
    /// translation of the one at its start (control points within `tol =
    /// tolerance_scale * ANALYTIC_TOL_SCALE * SM_EFF_ZERO * (1 + |min corner| + |max
    /// corner|)`, weight ratios too for a rational surface). The generator is the
    /// start iso curve (translated to sweep parameter 0); the extrusion vector spans
    /// one unit of the sweep parameter; the NURB is reparameterised to the test
    /// surface's domain. `None` when the surface is not an extrusion.
    ///
    /// # Errors
    ///
    /// Those of the iso curves, their control polygons and the surface evaluations
    /// (upstream returns the failing status as its boolean), and of the new surface's
    /// construction.
    pub fn is_nurb_surface_surf_of_extrusion(
        k: &Kernel,
        test_surface: &BSplineSurface,
        tolerance_scale: f64,
    ) -> Result<Option<Self>> {
        // surface locals
        let deg_u = test_surface.degree(SurfParamType::U);
        let deg_v = test_surface.degree(SurfParamType::V);

        // check parameterization
        // - must be deg 1 in just one direction and
        //   have more than 2 control points in the other direction
        // (both linear: a bilinear or plane; neither linear; or a linear direction with
        // more than 2 control points: only 2-CPnt lines)
        let (lin_u, lin_v) = (deg_u == 1, deg_v == 1);
        if lin_u == lin_v
            || (lin_u && test_surface.number_control_points(SurfParamType::U) > 2)
            || (lin_v && test_surface.number_control_points(SurfParamType::V) > 2)
        {
            return Ok(None);
        }

        // surface locals
        let nurb_uv_domain = test_surface.natural_uv_domain();

        // get a domain corner surface point to act as the sweep curve's end point
        let (surf_param, end_param, start_knot, end_knot, swap_uv) = if deg_u == 1 {
            (
                SurfParamType::U,
                nurb_uv_domain.evaluate(1.0, 0.0),
                nurb_uv_domain.min().x, // begin sweep direction Domain Value
                nurb_uv_domain.max().x, // end   sweep direction Domain Value
                true,                   // so that the STEP standard of linear in V is maintained.
            )
        } else {
            (
                SurfParamType::V,
                nurb_uv_domain.evaluate(0.0, 1.0),
                nurb_uv_domain.min().y,
                nurb_uv_domain.max().y,
                false,
            )
        };

        // evaluate domain upper and lower corner surface points and sweep end point
        // (statuses ignored upstream, which then reads the points)
        let mut origin = test_surface.evaluate_point(nurb_uv_domain.min())?;
        let max_pnt = test_surface.evaluate_point(nurb_uv_domain.max())?;
        let line_end = test_surface.evaluate_point(end_param)?;

        // compute a tolerance
        let pt_tol = tolerance_scale
            * ANALYTIC_TOL_SCALE
            * EFF_ZERO
            * (1.0 + origin.max_dimension() + max_pnt.max_dimension());

        // let extrusion vector = sweep line endPoint - begPoint
        let mut extrusion_vec = line_end - origin;
        let extrusion_length = extrusion_vec.length();

        // not an extrusion surface - sweep vector is zero length
        if extrusion_length < EFF_ZERO {
            return Ok(None);
        }

        // build GenCurve = Start of sweep IsoParameter Curve
        let iso_opts = IsoCurveOptions {
            domain: None,
            uv_iso_curve: false,
        };
        let mut gen_curve = test_surface
            .create_iso_parametric_curve(k, surf_param, start_knot, iso_opts)?
            .curve;

        // Get GenCurve locals
        let crv_step_ivl = gen_curve.step_interval();

        // convert GenCurve rational to euclidean
        let (start_ctrl_pts, start_weights) = gen_curve.control_polygon()?;
        let rational = gen_curve.is_rational()?;

        // build EndCurve = End of Sweep IsoParameter Curve
        let end_crv = test_surface
            .create_iso_parametric_curve(k, surf_param, end_knot, iso_opts)?
            .curve;

        // convert EndCurve rational to euclidean
        let (end_ctrl_pts, end_weights) = end_crv.control_polygon()?;

        // test every end curve controlPoint for constant sweep vectors
        let weight = |w: &[f64], i: usize| -> Result<f64> {
            w.get(i).copied().ok_or(SolidError::sm_err(OP_IS_NURB))
        };
        for (i, end_pt) in end_ctrl_pts.iter().enumerate() {
            let start_pt = start_ctrl_pts
                .get(i)
                .ok_or(SolidError::sm_err(OP_IS_NURB))?;

            // get sVec = this controlPoint sweep - sweepVector
            let vec = (*end_pt - *start_pt) - extrusion_vec;

            // when any of the EndControlPoints is not a constant sweep of the
            // StartControlPoints this is not a SurfOfExtrusion
            if vec.length() > pt_tol {
                return Ok(None);
            }

            // for rational surfaces also test the weights: the ratio between the
            // weights must be constant
            if rational
                && (weight(&start_weights, i)? * weight(&end_weights, 0)?
                    - weight(&end_weights, i)? * weight(&start_weights, 0)?)
                .abs()
                    > pt_tol
            {
                return Ok(None);
            }
        }

        // surface is an extrusion - build the output surface

        // scale extrusion by the sweep range to match ExtrusionLength and domain ranges
        extrusion_vec /= end_knot - start_knot;

        // UVDomain is not required to start at 0.0 so GenCurve has to be translated
        // to keep AnalUVDomain == NurbUVDomain and to have equivalent evaluations
        if !is_zero(start_knot) {
            let mut shift_rf = Axis2Placement::IDENTITY;
            shift_rf.translate(-start_knot * extrusion_vec);

            // translate (status ignored)
            ignore_status(gen_curve.transform(&shift_rf, None))?;
            origin -= start_knot * extrusion_vec;
        }

        // build new SmSurfOfExtrusion - constructor sets all internal values
        let anal_uv_domain = Extent2d::from_points(
            Point2::new(crv_step_ivl.min(), start_knot),
            Point2::new(crv_step_ivl.max(), end_knot),
        );
        let mut surf = Self::new(
            k,
            Curve::from(gen_curve),
            origin,
            extrusion_vec,
            anal_uv_domain,
            swap_uv,
        )?;

        // reparameterize the NURB so that AnalyticSurface->NurbDomain ==
        // InputSurface->NurbDomain (status ignored)
        ignore_status(surf.reparameterize(k, &test_surface.natural_uv_domain()))?;

        // all done
        Ok(Some(surf))
    }

    // ---- TrimWithDomain ---------------------------------------------------------------

    /// Trims the surface to the NURB domain `new_nurb_uv_domain` (`TrimWithDomain`):
    /// the analytic domain and the generator follow
    /// ([`Self::update_analytical_domain`], status ignored), then the NURB is rebuilt
    /// and reparameterised to the new domain. Nothing changes when the domain is the
    /// current one (within `SM_EFF_ZERO_PARAM`). A missing NURB is made first
    /// (`GetNaturalUVDomain`, status ignored).
    ///
    /// # Errors
    ///
    /// `SM_ERR` when the domain is not inside the natural domain (within
    /// `SM_EFF_ZERO_PARAM`); those of [`Self::make_nurb`] and
    /// [`Self::reparameterize`].
    pub fn trim_with_domain(&mut self, k: &Kernel, new_nurb_uv_domain: &Extent2d) -> Result<()> {
        // locals (GetNaturalUVDomain makes a missing NURB; its status is ignored)
        if self.base.gw_nurb().is_none() {
            ignore_status(self.make_nurb(k))?;
        }
        let current_nurb_uv_domain = self.base.natural_uv_domain();

        // check input - new trim UV domain must be inside current natural UV domain
        if !new_nurb_uv_domain.is_contained_by(&current_nurb_uv_domain, EFF_ZERO_PARAM) {
            return Err(SolidError::sm_err(OP_TRIM));
        }

        // no work - new and current domains are same size
        if current_nurb_uv_domain.is_contained_by(new_nurb_uv_domain, EFF_ZERO_PARAM) {
            return Ok(());
        }

        // Update just the analytic domain and the GenCurve (both analytic and NURB)
        // (status ignored)
        ignore_status(self.update_analytical_domain(k, new_nurb_uv_domain))?;

        // Now rebuild the Nurbs surface using the updated analytic domain.
        self.make_nurb(k)?;
        self.reparameterize(k, new_nurb_uv_domain)?;

        // all done
        Ok(())
    }

    // ---- ReadFromDB -------------------------------------------------------------------

    /// Reads a surface of extrusion (`ReadFromDB`): the origin, extrusion vector,
    /// analytic domain corners, orientation, swap flag, generator plane point and
    /// vector, then the generator curve (its type header and record), then the
    /// B-spline surface record. An analytic domain with `min > max` is not set (the
    /// empty surface's domain stays).
    ///
    /// # Errors
    ///
    /// Those of the stream reads, of `SmCurve::ReadFromDB` and of
    /// `SmBSplineSurface::ReadFromDB`; `InvalidInput` for an orientation code outside
    /// `SmCurveOrientType`, a generator not derived from `SmBSplineCurve`, or an ASCII
    /// curve type header that could not be read.
    pub fn read_from_db(k: &Kernel, db: &mut DatabaseReader, version: u64) -> Result<Self> {
        // init output object
        let mut surf = Self::empty();

        // locals
        let mut orig = Vec3::ZERO;
        let mut extrusion_vec = Vec3::ZERO;
        let mut anal_min = Point2::ZERO;
        let mut anal_max = Point2::ZERO;
        let mut orientation: u64 = 0;
        let mut swap_uv = false;
        let mut curve_point = Vec3::ZERO;
        let mut curve_vec = Vec3::ZERO;

        // file type
        let ascii = db.file_type() == FileType::Ascii;
        if ascii {
            let dbl = |db: &mut DatabaseReader, slot: &mut f64| {
                if let Some(v) = db.in_stream_mut().and_then(InputStream::extract_double) {
                    *slot = v;
                }
            };
            dbl(db, &mut orig.x);
            dbl(db, &mut orig.y);
            dbl(db, &mut orig.z);
            db.go_to_next_line();
            dbl(db, &mut extrusion_vec.x);
            dbl(db, &mut extrusion_vec.y);
            dbl(db, &mut extrusion_vec.z);
            db.go_to_next_line();

            dbl(db, &mut anal_min.x);
            dbl(db, &mut anal_min.y);
            dbl(db, &mut anal_max.x);
            dbl(db, &mut anal_max.y);
            db.go_to_next_line();

            if let Some(v) = db.in_stream_mut().and_then(InputStream::extract_ulong) {
                orientation = v;
            }
            db.go_to_next_line();
            if let Some(v) = db.in_stream_mut().and_then(InputStream::extract_int) {
                swap_uv = v != 0;
            }
            db.go_to_next_line();

            dbl(db, &mut curve_point.x);
            dbl(db, &mut curve_point.y);
            dbl(db, &mut curve_point.z);
            db.go_to_next_line();
            dbl(db, &mut curve_vec.x);
            dbl(db, &mut curve_vec.y);
            dbl(db, &mut curve_vec.z);
            db.go_to_next_line();
        } else {
            orig.x = db.read_double()?;
            orig.y = db.read_double()?;
            orig.z = db.read_double()?;

            extrusion_vec.x = db.read_double()?;
            extrusion_vec.y = db.read_double()?;
            extrusion_vec.z = db.read_double()?;

            anal_min.x = db.read_double()?;
            anal_min.y = db.read_double()?;
            anal_max.x = db.read_double()?;
            anal_max.y = db.read_double()?;

            orientation = db.read_ulong()?;

            swap_uv = db.read_boolean()? != 0;

            curve_point.x = db.read_double()?;
            curve_point.y = db.read_double()?;
            curve_point.z = db.read_double()?;

            curve_vec.x = db.read_double()?;
            curve_vec.y = db.read_double()?;
            curve_vec.z = db.read_double()?;
        }

        // base curve
        if ascii {
            db.go_to_next_line();
        }
        let header = db.read_type(true)?;
        let gen_type = header
            .type_code
            .ok_or(SolidError::invalid_input(OP_READ, "curve type not read"))?;
        let gen_dim = header.dim.ok_or(SolidError::invalid_input(
            OP_READ,
            "curve dimension not read",
        ))?;
        let gen_curve = Curve::read_from_db(k, gen_type, db, gen_dim, version)?;
        if gen_base(&gen_curve).is_err() {
            return Err(SolidError::invalid_input(
                OP_READ,
                "generator is not a kind of SmBSplineCurve",
            ));
        }

        // load the analytic
        surf.gen_curve = Some(gen_curve);
        surf.origin = orig;
        surf.extrusion_vec = extrusion_vec;
        // (SetMinMax's status ignored: an inverted domain is not set)
        let _ = surf.anal_uv_domain.set_min_max(anal_min, anal_max);
        surf.orientation = orientation_from_code(orientation)?;
        surf.swap_uv = swap_uv;
        surf.set_curve_point(curve_point);
        surf.set_curve_vec(curve_vec);

        // read the parent object
        surf.base = BSplineSurface::read_from_db(db, version)?;

        // all done
        Ok(surf)
    }
}

/// The `SmCurveOrientType` of a stored code.
fn orientation_from_code(code: u64) -> Result<CurveOrientType> {
    Ok(match code {
        0 => CurveOrientType::NotPlanar,
        1 => CurveOrientType::Perpendicular,
        2 => CurveOrientType::Independent,
        3 => CurveOrientType::Coplanar,
        4 => CurveOrientType::Linear,
        5 => CurveOrientType::Undefined,
        _ => {
            return Err(SolidError::invalid_input(
                OP_READ,
                "orientation code outside SmCurveOrientType",
            ))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::topology::database_io::DatabaseReader;

    fn k() -> Kernel {
        Kernel::default()
    }

    /// A quarter-circle-like quadratic B-spline in the XY plane.
    fn arc_gen() -> Curve {
        let pw = vec![
            CPoint::new(1.0, 0.0, 0.0, NOW),
            CPoint::new(1.0, 1.0, 0.0, NOW),
            CPoint::new(0.0, 1.0, 0.0, NOW),
        ];
        let cur = NurbCurve::new(pw, 2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).unwrap();
        Curve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    fn extrusion() -> SurfOfExtrusion {
        let k = k();
        SurfOfExtrusion::create_canonical(&k, arc_gen(), Vec3::new(0.0, 0.0, 2.0)).unwrap()
    }

    #[test]
    fn surface_wrapped_cone_takes_the_cone_special_case() {
        let k = k();
        // a line along X swept along Z, against a vertical SmCone_TYPE cylinder shape
        let pw = vec![
            CPoint::new(-2.0, 0.0, 0.0, NOW),
            CPoint::new(2.0, 0.0, 0.0, NOW),
        ];
        let line = NurbCurve::new(pw, 1, vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        let s = SurfOfExtrusion::create_canonical(
            &k,
            Curve::from(BSplineCurve::from_nurb(line, 3, false)),
            Vec3::new(0.0, 0.0, 2.0),
        )
        .unwrap();
        let cone =
            Cone::create_canonical(&k, &Axis2Placement::IDENTITY, 1.0, 1.0, 3.0, false, false)
                .unwrap();
        assert_eq!(cone.type_code(), CONE_TYPE);
        let wrapped = Surface::from(cone.clone());
        let d = s.base().natural_uv_domain();
        let od = cone.rev().base().natural_uv_domain();
        let run = |target| {
            s.global_surface_intersect(&k, &d, target, &od, [true, true], None, None)
                .unwrap()
        };
        let direct = run(IntersectTarget::Cone(&cone));
        let via_surface = run(IntersectTarget::Surface(&wrapped));
        // the direct cone takes the special case: curves, no UV curves (NULL upstream)
        assert!(!direct.curves_3d.is_empty());
        assert!(direct.surface1_uv_curves.iter().all(Option::is_none));
        // upstream switches on the other surface's type (SmSurfOfExtrusion.cpp:1677-1695
        // at c9c5979: SmPlane_TYPE, SmCone_TYPE, SmSurfOfExtrusion_TYPE), so the wrapped
        // cone takes the same special case
        assert_eq!(direct.curves_3d.len(), via_surface.curves_3d.len());
        assert_eq!(direct.curve_types, via_surface.curve_types);
        let none = |r: &ExtrusionIntersection| {
            r.surface1_uv_curves
                .iter()
                .map(Option::is_none)
                .collect::<Vec<_>>()
        };
        assert_eq!(none(&direct), none(&via_surface));
    }

    #[test]
    fn nurb_of_an_extrusion_is_recognised() {
        let k = k();
        let s = extrusion();
        let mut found = SurfOfExtrusion::is_nurb_surface_surf_of_extrusion(&k, s.base(), 1.0)
            .unwrap()
            .unwrap();
        // (the domains already agree: Reparameterize leaves the NURB to be made)
        assert!(found.base().gw_nurb().is_none());
        found.make_nurb(&k).unwrap();
        let p = found.base().evaluate_point(Point2::new(0.5, 0.25)).unwrap();
        let q = s.base().evaluate_point(Point2::new(0.5, 0.25)).unwrap();
        assert!(p.distance(q) < 1e-12);
    }

    #[test]
    fn plane_parallel_to_the_extrusion_gives_directrices() {
        let k = k();
        let s = extrusion();
        // the plane x = 0.5 (normal along X) cuts the arc once
        let plane = Plane::from_normal(Point3::new(0.5, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        let mut plane = plane;
        plane.make_nurb().unwrap();
        let r = s
            .intersect_with_plane(
                &k,
                &s.base().natural_uv_domain(),
                &plane,
                &plane.natural_uv_domain(),
                None,
            )
            .unwrap();
        assert!(!r.needs_more_intersections);
        assert_eq!(r.curves_3d.len(), 1);
        assert_eq!(r.curve_types, vec![TsectCurveType::Crossing]);
        assert_eq!(r.deviations, vec![0.0]);
    }

    #[test]
    fn oblique_plane_needs_more() {
        let k = k();
        let s = extrusion();
        let plane = Plane::from_normal(Point3::ZERO, Vec3::new(1.0, 0.0, 1.0));
        let r = s
            .intersect_with_plane(
                &k,
                &s.base().natural_uv_domain(),
                &plane,
                &plane.step_uv_domain(),
                None,
            )
            .unwrap();
        assert!(r.needs_more_intersections);
    }

    /// A line along X extruded along Z: its swept plane is `y = 0`.
    fn line_extrusion(k: &Kernel) -> SurfOfExtrusion {
        let mut line = Line::from_points(Point3::ZERO, Point3::new(1.0, 0.0, 0.0), 3);
        line.make_nurb().unwrap();
        SurfOfExtrusion::create_canonical(k, Curve::from(line), Vec3::new(0.0, 0.0, 1.0)).unwrap()
    }

    #[test]
    fn linear_generator_and_missed_bounded_plane_give_nothing() {
        let k = k();
        let s = line_extrusion(&k);
        // a bounded plane x = 0 over y in [49, 51]: the swept plane misses it
        let dom = Extent2d::from_bounds(-1.0, -1.0, 1.0, 1.0);
        let mut plane = Plane::new(
            Point3::new(0.0, 50.0, 0.0),
            Vec3::Y,
            Vec3::Z,
            crate::geom::vector2d::Vec2::new(1.0, 1.0),
            &dom,
        );
        plane.make_nurb().unwrap();
        let r = s
            .intersect_with_plane(
                &k,
                &s.base().natural_uv_domain(),
                &plane,
                &plane.step_uv_domain(),
                None,
            )
            .unwrap();
        assert!(!r.needs_more_intersections);
        assert!(r.curves_3d.is_empty());
    }

    #[test]
    fn linear_generator_line_is_classified_on_the_surface() {
        let k = k();
        let s = line_extrusion(&k);
        let plane = Plane::from_normal(Point3::new(0.5, 0.0, 0.0), Vec3::X);
        let r = s
            .intersect_with_plane(
                &k,
                &s.base().natural_uv_domain(),
                &plane,
                &plane.step_uv_domain(),
                None,
            )
            .unwrap();
        assert!(!r.needs_more_intersections);
        assert!(!r.curves_3d.is_empty());
        assert_eq!(r.curves_3d.len(), r.curve_types.len());
        assert!(r.curve_types.iter().all(|t| *t == TsectCurveType::Crossing));
        assert_eq!(r.surface1_uv_curves.len(), r.curves_3d.len());
    }

    #[test]
    fn non_parallel_extrusions_need_more() {
        let k = k();
        let a = extrusion();
        let mut line = Line::from_points(Point3::ZERO, Point3::new(1.0, 1.0, 0.0), 3);
        line.make_nurb().unwrap();
        let b = SurfOfExtrusion::create_canonical(&k, Curve::from(line), Vec3::new(1.0, 0.0, 1.0))
            .unwrap();
        let r = a.intersect_with_surf_of_extrusion(&k, &b, None).unwrap();
        assert!(r.needs_more_intersections);
        assert!(r.curves_3d.is_empty());
    }

    #[test]
    fn trim_rejects_a_larger_domain() {
        let k = k();
        let mut s = extrusion();
        let d = s.base().natural_uv_domain();
        let mut big = d;
        big.expand_absolute(1.0);
        assert!(s.trim_with_domain(&k, &big).is_err());
        assert!(s.trim_with_domain(&k, &d).is_ok());
    }

    #[test]
    fn truncated_binary_record_fails() {
        let k = k();
        let mut db = DatabaseReader::new();
        db.open_for_read(vec![0u8; 20], FileType::Binary);
        assert!(SurfOfExtrusion::read_from_db(&k, &mut db, 30).is_err());
    }

    #[test]
    fn orientation_codes() {
        assert_eq!(orientation_from_code(4).unwrap(), CurveOrientType::Linear);
        assert!(orientation_from_code(6).is_err());
    }
}
