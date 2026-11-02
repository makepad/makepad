//! Tolerance policy (`SmTol`): typed tolerance values, model-size based default
//! tolerances, zone / intersection / approximation tolerances, scaled zeros, simple
//! tolerance predicates, and the pointer-pair container `SmRelation`.
//!
//! Upstream: `source/SMLib/inc/SmTol.h`, `source/SMLib/src/SmTol.cpp`,
//! `source/SMLib/inc/SmRelation.h`.
//!
//! | Rust                                   | Upstream                                       |
//! |----------------------------------------|------------------------------------------------|
//! | [`tol3d`]                              | `SmTol3d(double)`, `SmScaledZero(double)`, `SmTol2d(double)`, `SmTol1d(double)`, every `operator=(double)` of `SM_TOL_CALLS`, `SmTol3d::operator=(const SmTol3d)` |
//! | [`zone_tol3d`]                         | `SmZoneTol3d(double)`                          |
//! | [`xsect_tol3d`]                        | `SmXSectTol3d(double)`                         |
//! | [`approx_tol3d`]                       | `SmApproxTol3d(double)`                        |
//! | [`tol_mul`]                            | `operator*=(SmTol3d&, double)`                 |
//! | (plain `f64`)                          | `operator double&`, `operator const double&` of every tolerance class |
//! | [`SmallTopoType`]                      | `SmSmallTopoType`                              |
//! | [`size_estimate_to_zone_tol_3d`]       | `SmTol::SizeEstimateToZoneTol3d`               |
//! | [`in_tol`]                             | `SmTol::InTol(double, const SmTol3d&)`         |
//! | [`is_zero`], [`is_zero_squared`], [`is_zero_to_tol`] | `SM_IS_ZERO`, `SM_IS_ZERO_SQUARED`, `SM_IS_ZERO_TO_TOL` (`SmMath.h`) |
//! | [`are_same`], [`are_same_to_tol`]      | `SM_ARE_SAME`, `SM_ARE_SAME_TO_TOL` (`SmMath.h`) |
//! | [`calc_face_zone_tol_3d`]              | `SmTol::CalcFaceZoneTol3d(const SmContext*)`   |
//! | [`calc_edge_zone_tol_3d`]              | `SmTol::CalcEdgeZoneTol3d(double, double, SmZoneTol3d*, const SmContext*)` |
//! | [`calc_vertex_zone_tol_3d`]            | `SmTol::CalcVertexZoneTol3d(double, double, SmZoneTol3d*, const SmContext*)` |
//! | [`eff_zero`], [`eff_zero_sq`], [`eff_zero_param`], [`eff_zero_ang_rad`] | `SmTol::GetEffZero`, `GetEffZeroSq`, `GetEffZeroParam`, `GetEffZeroAngRad` |
//! | [`xsect_tol_3d_from_zones`]            | `SmTol::GetXSectTol3d(const SmZoneTol3d&, const SmZoneTol3d&)` |
//! | [`ang_tol_rad`]                        | `SmTol::GetAngTolRad()`                        |
//! | [`model_size_estimate`]                | `SmTol::GetModelSizeEstimate()`                |
//! | [`context_model_size_estimate`]        | `SmTol::GetModelSizeEstimate(const SmContext&)` |
//! | [`set_context_model_size_estimate`]    | `SmTol::SetModelSizeEstimate(SmContext&, double)` |
//! | [`are_parallel`]                       | `SmTol::AreParallel`                           |
//! | [`are_perpendicular`]                  | `SmTol::ArePerpendicular`                      |
//! | [`is_zero_speed`]                      | `SmTol::IsZeroSpeed`                           |
//! | [`is_degenerate_vec3`]                 | `SmTol::IsDegenerate(const SmVector3d, SmXSectTol3d*)` |
//! | [`gap_gain`]                           | `SmTol::GetGapGain`                            |
//! | [`zone_tol_3d`]                        | `SmTol::GetZoneTol3d(SmSmallTopoType)` (system) |
//! | [`context_zone_tol_3d`]                | `SmTol::GetZoneTol3d(const SmContext*, SmSmallTopoType)` |
//! | [`approx_tol_3d`]                      | `SmTol::GetApproxTol3d(SmSmallTopoType)`       |
//! | [`context_approx_tol_3d`]              | `SmTol::GetApproxTol3d(const SmContext*, SmSmallTopoType)` |
//! | [`xsect_tol_3d`]                       | `SmTol::GetXSectTol3d(SmSmallTopoType)`        |
//! | [`context_xsect_tol_3d`]               | `SmTol::GetXSectTol3d(const SmContext*, SmSmallTopoType)` |
//! | [`scaled_zero`], [`scaled_zero_sq`], [`scaled_zero_sqrt`] | `SmTol::GetScaledZero()`, `GetScaledZeroSq()`, `GetScaledZeroSqrt()` |
//! | [`ScaledZero`] (`f64`, [`Vec2`], [`Vec3`], [`Extent1d`], `Extent2d`, [`Extent3d`], `PseudoBox`) | `SmTol::GetScaledZero(const double)`, `(const SmVector2d&)`, `(const SmVector3d&)`, `(const SmExtent1d&)`, `(const SmExtent2d&)` (impl in `geom::extent2d`), `(const SmExtent3d&)`, `(const SmPseudoBox&)` (impl in `geom::pseudo_box`) |
//! | [`scaled_zero_of_equations`]           | `SmTol::GetScaledZero(const SmEvalNFunctionsObject&)` |
//! | [`scaled_zero_of_volume`]              | `SmTol::GetScaledZero(const SmVolume&)`        |
//! | [`scaled_zero_of_curve`]               | `SmTol::GetScaledZero(const SmCurve&)` (CO-PORTED by `intersect.surface_drop_lib_p3`) |
//! | [`map_to_1d`]                          | `SmTol::MapTo1d(SmTol3d&, SmVector3d&, const SmExtent1d&)` |
//! | [`map_to_1d_on_curve`]                 | `SmTol::MapTo1d(SmTol3d&, double, const SmCurve&)` (PR #136 round 2) |
//! | [`is_degenerate_curve`], [`zone_tol_3d_of_curve`] | `SmTol::IsDegenerate(const SmCurve*, SmExtent1d, SmXSectTol3d*)`, the `SmCurve` branch of `SmTol::GetZoneTol3d(const SmObject*)` |
//! | [`scaled_zero_sq_of`], [`scaled_zero_sqrt_of`] | unary `GetScaledZeroSq<cA>`, `GetScaledZeroSqrt<cA>` |
//! | [`scaled_zero2`], [`scaled_zero_sq2`], [`scaled_zero_sqrt2`] | binary `GetScaledZero<cA,cB>`, `...Sq`, `...Sqrt` |
//! | [`scaled_zero3`], [`scaled_zero_sq3`], [`scaled_zero_sqrt3`] | tertiary templates     |
//! | [`scaled_zero4`], [`scaled_zero_sq4`], [`scaled_zero_sqrt4`] | quaternary templates   |
//! | [`Relation`]                           | `SmRelation<TYPE1, TYPE2>`                     |
//! | [`Relation::new`]                      | `SmRelation(const SmContext&, SmRelationCardinalityType)` |
//! | [`Relation::cardinality`]              | `m_eCardinality` (`GetType`/`GetTypeString`/`IsKindOf` are RTTI, see below) |
//! | [`Relation::relate_pair`]              | `RelatePair`                                   |
//! | [`Relation::relate_to_many`]           | `RelateToMany`                                 |
//! | [`Relation::remove_all`]               | `RemoveAll`                                    |
//! | [`Relation::remove_pair`]              | `RemovePair`                                   |
//! | [`Relation::second`]                   | `GetSecond`                                    |
//! | [`Relation::seconds`]                  | `GetSeconds`                                   |
//! | [`Relation::firsts`]                   | `GetFirsts`                                    |
//! | [`Relation::all_firsts`]               | `GetAllFirsts`                                 |
//! | [`Relation::all_seconds`]              | `GetAllSeconds`                                |
//! | [`Cardinality`]                        | `SmRelationCardinalityType`                    |
//!
//! System state: upstream keeps the system tolerances in thread-local statics
//! (`s_ZoneTol3d`, `s_TolGapGain`, `s_ModelSizeEstimate`, `s_LargeSmallSizeRatio`);
//! here they are the [`Tolerances`] of the [`Kernel`] (`zone_tol_3d`, `tol_gap_gain`,
//! `model_size_estimate`, `large_small_size_ratio`). The `const SmContext*` overloads
//! take an `Option<&`[`Context`]`>` (`None` = null pointer: the system value; usually
//! `Some(&k.context)`). Note that the context overloads ignore the small-topology
//! flag, as upstream does.
//!
//! Not here yet (they need types of later units; see the `UNPORTED` markers below):
//! every overload taking an `SmObject`, `SmCurve`, `SmSurface`, `SmVolume`,
//! `SmPseudoBox`, `SmBrep`, `SmBrepData`, a topology
//! entity, a classification / interval object, or `SmEvalNFunctionsObject`.
//!
//! Deviations:
//! - The tolerance classes (`SmTol3d`, `SmZoneTol3d`, ...) are plain `f64` values; their
//!   converting constructors are the functions [`tol3d`], [`zone_tol3d`],
//!   [`xsect_tol3d`] and [`approx_tol3d`], applied exactly where upstream converts a
//!   `double` into a tolerance class (a return statement, a cast, an initialisation),
//!   so that `fabs` and the `SM_USE_DEFAULT` substitution happen at the same places.
//! - Optional pointer arguments become `Option<f64>` holding the pointee's value.
//! - `SmRelation`: upstream stores pairs in `std::unordered_map` / `std::unordered_set`
//!   keyed by pointer, so `GetSeconds`, `GetAllFirsts` and `GetAllSeconds` come out in
//!   an unspecified, address-dependent order. [`Relation`] keeps keys in `BTreeMap`s /
//!   `BTreeSet`s and returns them sorted (see `plan/findings.md`). `RelatePair`'s null
//!   checks have no Rust counterpart; its `SM_ERR` status is
//!   [`SolidError::Upstream`] with code 1001. `GetSecond`'s `SE(SM_ERR)` for an
//!   ambiguous key is a no-op in the release build; [`Relation::second`] returns
//!   `None` there as upstream returns `NULL`. `GetType`, `GetTypeString`, `IsKindOf`
//!   (the `SmObject` run-time type system) are replaced by Rust's static types.
//! - `SM_ASSERT*`, `WARN`, `ERR_MSG` and `SM_DEBUG_CODE` blocks are not ported (they do
//!   not change results in the release build).

use std::collections::{BTreeMap, BTreeSet};

use crate::curve::curve::Curve;
use crate::error::{Result, SolidError, SM_ERR_INVALID_INPUT};
use crate::geom::context::Context;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::local_solve_nd::EvalNFunctions;
use crate::geom::vector2d::Vec2;
use crate::geom::vector3d::sqrt as sm_sqrt;
use crate::kernel::{Kernel, Tolerances};
use crate::math::{
    max2, max3, max4, rad_to_deg, Vec3, EFF_ZERO, EFF_ZERO_PARAM, EFF_ZERO_RAD, EFF_ZERO_SQ,
    EFF_ZERO_SQRT, PI,
};
use crate::surface::volume::Volume;
use crate::topology::{Brep, EdgeId, FaceId, VertexId};

// UNPORTED(geom.tol): SmTol::GetModelSizeEstimate(const SmBrep&),
// SmTol::SetModelSizeEstimate(SmBrep&, double), SmTol::GetZoneTol3d(const SmBrep*),
// SmTol::GetApproxTol3d(const SmBrep*), SmTol::CalcFaceZoneTol3d(SmBrep*),
// SmTol::CalcEdgeZoneTol3d(.., SmBrep*, ..), SmTol::CalcVertexZoneTol3d(.., SmBrep*, ..)
// can now be written over `topology::Brep` (`Brep::tolerance`,
// `Brep::this_model_size_estimate`, `Brep::set_this_model_size_estimate`, unit
// topology.brep_p1); SmTol::GetScaledZero(const SmBrep&) needs SmBrep::CalculateBoundingBox
// (query.brep).
// UNPORTED(geom.tol) (the owner in harness/callgraph/units.json; every call site of
// these overloads carries this tag): SmTol::GetZoneTol3d(const
// SmObject*), SmTol::GetZoneTol3d(const SmBrepData*), SmTol::GetApproxTol3d(const
// SmObject*), SmTol::GetXSectTol3d(const SmObject*, const SmObject*, ..),
// SmTol::GetZoneTol3d_ForXSectResult(const SmObject*, const SmObject*),
// SmTol::IsDegenerate(const SmObject*), SmTol::IsClosed(const SmObject*),
// SmTol::GetScaledZero(const SmObject&) dispatch on IsKindOf over SmTopology (the
// virtual GetTolerance: the base is topology::topology::Topology::tolerance, the
// overrides belong to SmVertex, SmEdge, SmFace, SmEdgeuse), SmSurface/SmCurve
// (GetFace/GetEdge), SmBrep, SmBrepData and SmCurveClassification; those classes have
// no Rust object type yet (the SmBrep arm itself, SmBrep::GetTolerance, is
// `Brep::tolerance`).
// SmTol::IsDegenerate(const SmCurve*, SmExtent1d, ..) is is_degenerate_curve,
// SmTol::MapTo1d(SmTol3d&, double, const SmCurve&) is map_to_1d_on_curve and
// SmTol::GetScaledZero(const SmCurve&) is scaled_zero_of_curve (PR #136). The edge branch
// of GetZoneTol3d(const SmObject*) for a curve owned by an edge is
// UNPORTED(topology.edge): SmEdge::GetTolerance; justified: findings.md intersect.surface_drop_lib_p3 (a bare curve holds only its owner's handle).
// CO-PORTED in intersect.curve_class_p1: SmTol::MapTo2d(const SmTol3d&, double, const
// SmCurve&, const SmSurface&) is surface::surface::p1::map_to_2d_on_curve (its UV-curve
// and 3D branches; the SmCrvOnSurf branch is Unported there).
// CO-PORTED in intersect.curve_class_p3 / _p7: SmTol::IsDegenerate(const SmCurve*,
// SmExtent1d, SmXSectTol3d*) and SmTol::IsClosed(const SmObject*, SmXSectTol3d*) for a
// curve, with the tolerance given, are intersect::curve_class::p3::curve_is_degenerate and
// intersect::curve_class::p7's tol_is_closed_curve (private).
// CO-PORTED in surface.surface_p1: SmTol::MapTo2d(const SmTol3d&, const SmVector2d&,
// [const SmVector2d&,] const SmSurface&), SmTol::GetScaledZero(const SmSurface&) and
// the SmSurface branch of SmTol::GetZoneTol3d(const SmObject*) are
// surface::surface::p1::{map_to_2d, map_to_2d_dir, scaled_zero_of_surface, zone_tol_3d}.
// SmTol::GetSrcZoneTol3d / GetObjZoneTol3d of SmCurveClassification / SmCurveInterval
// and SmTol::GetScaledZero(const SmCurveInterval&) are
// intersect::curve_class::src_zone_tol_3d_of / interval_obj_zone_tol_3d and the
// ScaledZero impl of intersect::curve_class::CurveInterval; SmTol::SetZoneTol3d of a
// classification is CurveClassification::set_src_zone_tol_3d.
// SmTol::GetSrcZoneTol3d / GetObjZoneTol3d / GetXSectTol3d of SmPointClassification
// are intersect::point_class::PointClassification::src_zone_tol_3d / obj_zone_tol_3d /
// xsect_tol_3d.
// SmTol::GetSrcZoneTol3d / GetObjZoneTol3d of SmLineSegClassification /
// SmPolyPointClassification / SmLineSegInterval: those classes have no Rust type (their
// constructors are outside the in-scope call graph).
// SmTol::GetAngTolRad(const SmFilletSolver*) is blend::fillet_solver::p1::tol_ang_tol_rad.
// The SmFilletCorner branch of GetApproxTol3d(SmObject*) is
// blend::fillet_corner::p3::FilletCorner::this_approx_tol_3d (GetThisApproxTol3d);
// the other GetApproxTol3d / GetAngTolRad overloads of algorithm objects (SmHCR,
// SmPolyIntersector, SmSurfaceIntersector, SmSurfaceTracer, SmTopologyIntersector,
// SmOffsetGeometryCreation) belong with those objects' units.
// UNPORTED(topology.vertex): SmTol::GetScaledZero(const SmVertex&) (SmVertex::GetPoint).
// UNPORTED(topology.edge): SmTol::GetScaledZero(const SmEdge&) (SmEdge::GetCurve).
// UNPORTED(topology.face_p2): SmTol::GetScaledZero(const SmFace&) (SmFace::GetSurface).
// SmTol::GetScaledZero(const SmRegion&) and (const SmShell&) (SmTol.cpp:1394-1395) are not
// ported: nothing calls them yet. Each is the scaled zero of one box, now
// query::face::p1::calculate_region_bounding_box / calculate_shell_bounding_box (query.face_p1);
// geom cannot call query, so port them beside those when a caller needs them.
// SmTol::UpdateObjectTolerance(SmTopology*, double, double, SmBoolean) is
// update_object_tolerance below (CO-PORTED in boolean.topology_intersector_p1).

/// `SM_USE_DEFAULT`: sentinel meaning "use the current system value".
pub const USE_DEFAULT: f64 = 9876543.21;
/// `SM_UNINIT_TOL`: sentinel of an uninitialised tolerance.
pub const UNINIT_TOL: f64 = 1.23456e-61;
/// `SM_MODEL_SIZE_ESTIMATE`: default model size estimate.
pub const MODEL_SIZE_ESTIMATE: f64 = 50.0;
/// `SM_LARGE_SMALL_SIZE_RATIO`: default large/small topology size ratio.
pub const LARGE_SMALL_SIZE_RATIO: f64 = 1000.0;
/// `SM_ZONE_TOL_3D`: default zone tolerance.
pub const ZONE_TOL_3D: f64 = 1.0e-5;
/// `SM_TOL_GAP_GAIN`: over-size factor above an object's largest gap.
pub const TOL_GAP_GAIN: f64 = 1.1;
/// `SM_ANG_TOL_DEG`: angle tolerance in degrees.
pub const ANG_TOL_DEG: f64 = 0.5;
/// `SM_ANG_TOL_RAD` = `SM_ANG_TOL_DEG * SM_PI / 180.0`.
pub const ANG_TOL_RAD: f64 = ANG_TOL_DEG * PI / 180.0;

/// Upstream status `SM_ERR`.
const SM_ERR: i32 = 1001;

// ---- tolerance values ------------------------------------------------------------

/// `SmTol3d(double)` (and the `SmScaledZero`, `SmTol2d`, `SmTol1d` constructors and
/// every `operator=(double)`): a tolerance is stored as `fabs(d)`.
#[must_use]
pub fn tol3d(d: f64) -> f64 {
    d.abs()
}

/// `SmZoneTol3d(double)`: [`USE_DEFAULT`] becomes the system zone tolerance, then
/// `fabs`.
#[must_use]
pub fn zone_tol3d(k: &Kernel, d: f64) -> f64 {
    tol3d(if d == USE_DEFAULT {
        k.tolerances.zone_tol_3d
    } else {
        d
    })
}

/// `SmXSectTol3d(double)`: [`USE_DEFAULT`] becomes twice the system zone tolerance,
/// then `fabs`.
#[must_use]
pub fn xsect_tol3d(k: &Kernel, d: f64) -> f64 {
    tol3d(if d == USE_DEFAULT {
        k.tolerances.zone_tol_3d * 2.0
    } else {
        d
    })
}

/// `SmApproxTol3d(double)`: [`USE_DEFAULT`] becomes half the system zone tolerance,
/// then `fabs`.
#[must_use]
pub fn approx_tol3d(k: &Kernel, d: f64) -> f64 {
    tol3d(if d == USE_DEFAULT {
        k.tolerances.zone_tol_3d / 2.0
    } else {
        d
    })
}

/// `operator*=(SmTol3d&, double)`: `tol * d` (no `fabs`, as upstream).
#[must_use]
pub fn tol_mul(tol: f64, d: f64) -> f64 {
    tol * d
}

/// Control flags for small-topology tolerances (`SmSmallTopoType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SmallTopoType {
    /// `SM_ST_NORMAL_SIZE`: normal (large) scale unless the object says otherwise.
    #[default]
    NormalSize,
    /// `SM_ST_SMALL_SIZE`: small ("pinhole in a battleship") scale unless the object
    /// says otherwise.
    SmallSize,
    /// `SM_ST_FORCE_NORMAL_SIZE`: always normal scale.
    ForceNormalSize,
    /// `SM_ST_FORCE_SMALL_SIZE`: always small scale.
    ForceSmallSize,
    /// `SM_ST_NO_CHANGE`: no-action value for `SetZoneTol3d`.
    NoChange,
}

impl SmallTopoType {
    /// Upstream enum value (`SM_ST_NORMAL_SIZE` = 0 .. `SM_ST_NO_CHANGE` = 4).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::NormalSize => 0,
            Self::SmallSize => 1,
            Self::ForceNormalSize => 2,
            Self::ForceSmallSize => 3,
            Self::NoChange => 4,
        }
    }

    /// The flag with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::NormalSize),
            1 => Some(Self::SmallSize),
            2 => Some(Self::ForceNormalSize),
            3 => Some(Self::ForceSmallSize),
            4 => Some(Self::NoChange),
            _ => None,
        }
    }

    /// `eIsSmall == SM_ST_SMALL_SIZE || eIsSmall == SM_ST_FORCE_SMALL_SIZE`.
    const fn is_small(self) -> bool {
        matches!(self, Self::SmallSize | Self::ForceSmallSize)
    }
}

// ---- model size and system tolerances ----------------------------------------------

/// Maps a model size estimate to a default zone tolerance
/// (`SmTol::SizeEstimateToZoneTol3d`):
///
/// | size estimate      | zone tolerance |
/// |--------------------|----------------|
/// | `0.00005 - 0.005`  | `1e-7`         |
/// | `0.005 - 0.5`      | `1e-6`         |
/// | `0.5 - 500`        | `1e-5`         |
/// | `500 - 50000`      | `1e-4`         |
/// | `50000 - 5e6`      | `1e-3`         |
///
/// and `10^floor(log10(|size|) / 2) * 1e-5` outside those ranges; [`USE_DEFAULT`]
/// means [`MODEL_SIZE_ESTIMATE`]. (The `SmZoneTol3d` conversions of the result never
/// see `SM_USE_DEFAULT` and only apply `fabs`.)
#[must_use]
pub fn size_estimate_to_zone_tol_3d(size: f64) -> f64 {
    if size == USE_DEFAULT {
        size_estimate_to_zone_tol_3d(MODEL_SIZE_ESTIMATE)
    } else if (0.5..=500.0).contains(&size) {
        1.0e-5
    } else if (500.0..=50000.0).contains(&size) {
        1.0e-4
    } else if (0.005..=0.5).contains(&size) {
        1.0e-6
    } else if (50000.0..=5e6).contains(&size) {
        1.0e-3
    } else if (0.00005..=0.005).contains(&size) {
        1.0e-7
    } else {
        tol3d(10f64.powf((size.abs().log10() / 2.0).floor()) * 1.0e-5)
    }
}

/// The system model size estimate (`SmTol::GetModelSizeEstimate()`).
#[must_use]
pub fn model_size_estimate(k: &Kernel) -> f64 {
    k.tolerances.model_size_estimate
}

/// A context's model size estimate, the system one if it has none
/// (`SmTol::GetModelSizeEstimate(const SmContext&)`).
#[must_use]
pub fn context_model_size_estimate(k: &Kernel, ctx: &Context) -> f64 {
    ctx.this_model_size_estimate(k)
}

/// Sets a context's model size estimate and returns its new zone tolerance
/// (`SmTol::SetModelSizeEstimate(SmContext&, double)`).
pub fn set_context_model_size_estimate(ctx: &mut Context, model_size_estimate: f64) -> f64 {
    ctx.set_this_model_size_estimate(model_size_estimate)
}

/// The gap gain: the over-size factor that keeps an object's zone tolerance a little
/// larger than half its largest gap (`SmTol::GetGapGain`).
#[must_use]
pub fn gap_gain(k: &Kernel) -> f64 {
    k.tolerances.tol_gap_gain
}

/// The system zone tolerance (`SmTol::GetZoneTol3d(SmSmallTopoType)`): multiplied by
/// the large/small size ratio for small topology.
#[must_use]
pub fn zone_tol_3d(k: &Kernel, small: SmallTopoType) -> f64 {
    let t: &Tolerances = &k.tolerances;
    if t.zone_tol_3d == UNINIT_TOL {
        // upstream: smos_AssertErrorMessage(.., "Uninit-Tol", ..)
        zone_tol3d(k, UNINIT_TOL)
    } else if small.is_small() {
        zone_tol3d(k, zone_tol3d(k, t.large_small_size_ratio) * t.zone_tol_3d)
    } else {
        t.zone_tol_3d
    }
}

/// The context zone tolerance (`SmTol::GetZoneTol3d(const SmContext*,
/// SmSmallTopoType)`): the context's own zone tolerance, or the system one for `None`
/// (a null context); `small` is ignored, as upstream.
#[must_use]
pub fn context_zone_tol_3d(k: &Kernel, ctx: Option<&Context>, _small: SmallTopoType) -> f64 {
    let z = k.tolerances.zone_tol_3d;
    if z == UNINIT_TOL {
        zone_tol3d(k, UNINIT_TOL)
    } else {
        ctx.map_or(z, Context::this_zone_tol_3d)
    }
}

/// The system approximation tolerance, half the zone tolerance
/// (`SmTol::GetApproxTol3d(SmSmallTopoType)`).
#[must_use]
pub fn approx_tol_3d(k: &Kernel, small: SmallTopoType) -> f64 {
    approx_tol3d(k, 0.5 * zone_tol_3d(k, small))
}

/// The context approximation tolerance (`SmTol::GetApproxTol3d(const SmContext*,
/// SmSmallTopoType)`; `None` = null context).
#[must_use]
pub fn context_approx_tol_3d(k: &Kernel, ctx: Option<&Context>, small: SmallTopoType) -> f64 {
    approx_tol3d(k, 0.5 * context_zone_tol_3d(k, ctx, small))
}

/// The system intersection tolerance, twice the zone tolerance
/// (`SmTol::GetXSectTol3d(SmSmallTopoType)`).
#[must_use]
pub fn xsect_tol_3d(k: &Kernel, small: SmallTopoType) -> f64 {
    xsect_tol3d(k, 2.0 * zone_tol_3d(k, small))
}

/// The context intersection tolerance (`SmTol::GetXSectTol3d(const SmContext*,
/// SmSmallTopoType)`; `None` = null context).
#[must_use]
pub fn context_xsect_tol_3d(k: &Kernel, ctx: Option<&Context>, small: SmallTopoType) -> f64 {
    xsect_tol3d(k, 2.0 * context_zone_tol_3d(k, ctx, small))
}

/// The intersection tolerance of two objects with zone tolerances `zone1`, `zone2`:
/// their sum (`SmTol::GetXSectTol3d(const SmZoneTol3d&, const SmZoneTol3d&)`).
#[must_use]
pub fn xsect_tol_3d_from_zones(k: &Kernel, zone1: f64, zone2: f64) -> f64 {
    xsect_tol3d(k, zone1 + zone2)
}

/// The system angle tolerance in radians (`SmTol::GetAngTolRad()`, `SM_ANG_TOL_RAD`).
#[must_use]
pub fn ang_tol_rad() -> f64 {
    ANG_TOL_RAD
}

/// `SM_EFF_ZERO` as a scaled zero (`SmTol::GetEffZero`).
#[must_use]
pub fn eff_zero() -> f64 {
    tol3d(EFF_ZERO)
}

/// `SM_EFF_ZERO_SQ` as a scaled zero (`SmTol::GetEffZeroSq`).
#[must_use]
pub fn eff_zero_sq() -> f64 {
    tol3d(EFF_ZERO_SQ)
}

/// `SM_EFF_ZERO_PARAM` as a scaled zero (`SmTol::GetEffZeroParam`): the smallest
/// distance between distinct parameter values.
#[must_use]
pub fn eff_zero_param() -> f64 {
    tol3d(EFF_ZERO_PARAM)
}

/// `SM_EFF_ZERO_RAD` as a scaled zero (`SmTol::GetEffZeroAngRad`).
#[must_use]
pub fn eff_zero_ang_rad() -> f64 {
    tol3d(EFF_ZERO_RAD)
}

// ---- topology zone tolerances from gaps --------------------------------------------

/// A face's zone tolerance: the context zone tolerance
/// (`SmTol::CalcFaceZoneTol3d(const SmContext*)`; `None` = null context).
#[must_use]
pub fn calc_face_zone_tol_3d(k: &Kernel, ctx: Option<&Context>) -> f64 {
    context_zone_tol_3d(k, ctx, SmallTopoType::NormalSize)
}

/// An edge's zone tolerance from its largest edge/edge and edge/face gaps
/// (`SmTol::CalcEdgeZoneTol3d(double, double, SmZoneTol3d*, const SmContext*)`;
/// `None` = null context):
/// `max(def, max_face_zone_tol?, gain * edge_edge_gap / 2, gain * edge_face_gap - def)`
/// where `def` is the context zone tolerance and `gain` the [`gap_gain`].
#[must_use]
pub fn calc_edge_zone_tol_3d(
    k: &Kernel,
    edge_edge_max_gap: f64,
    edge_face_max_gap: f64,
    max_face_zone_tol: Option<f64>,
    ctx: Option<&Context>,
) -> f64 {
    calc_gap_zone_tol(
        k,
        edge_edge_max_gap,
        edge_face_max_gap,
        max_face_zone_tol,
        ctx,
    )
}

/// A vertex's zone tolerance from its largest vertex/edge and vertex/face gaps
/// (`SmTol::CalcVertexZoneTol3d(double, double, SmZoneTol3d*, const SmContext*)`;
/// `None` = null context); same formula as
/// [`calc_edge_zone_tol_3d`].
#[must_use]
pub fn calc_vertex_zone_tol_3d(
    k: &Kernel,
    vertex_edge_max_gap: f64,
    vertex_face_max_gap: f64,
    max_edge_zone_tol: Option<f64>,
    ctx: Option<&Context>,
) -> f64 {
    calc_gap_zone_tol(
        k,
        vertex_edge_max_gap,
        vertex_face_max_gap,
        max_edge_zone_tol,
        ctx,
    )
}

/// Shared body of `CalcEdgeZoneTol3d` / `CalcVertexZoneTol3d`.
fn calc_gap_zone_tol(
    k: &Kernel,
    gap_1d: f64,
    gap_2d: f64,
    max_up: Option<f64>,
    ctx: Option<&Context>,
) -> f64 {
    let def = context_zone_tol_3d(k, ctx, SmallTopoType::NormalSize);
    let gain = gap_gain(k);
    let r = match max_up {
        Some(up) => max4(def, up, gain * gap_1d / 2.0, gain * gap_2d - def),
        None => max3(def, gain * gap_1d / 2.0, gain * gap_2d - def),
    };
    zone_tol3d(k, r)
}

// ---- predicates --------------------------------------------------------------------

/// `|val| <= tol` (`SmTol::InTol(double, const SmTol3d&)`).
#[must_use]
pub fn in_tol(val: f64, tol: f64) -> bool {
    val.abs() <= tol
}

/// `SM_IS_ZERO(a)`: `|a| <=` [`scaled_zero`] (`SmMath.h`).
#[must_use]
pub fn is_zero(a: f64) -> bool {
    in_tol(a, scaled_zero())
}

/// `SM_IS_ZERO_SQUARED(a)`: `|a| <=` [`scaled_zero_sq`] (`SmMath.h`).
#[must_use]
pub fn is_zero_squared(a: f64) -> bool {
    in_tol(a, scaled_zero_sq())
}

/// `SM_IS_ZERO_TO_TOL(a, tol)`: `|a| <= |tol|` (`SmMath.h`).
#[must_use]
pub fn is_zero_to_tol(a: f64, tol: f64) -> bool {
    in_tol(a, tol3d(tol))
}

/// `SM_ARE_SAME(a, b)`: `|a - b| <=` the larger scaled zero of `a` and `b`
/// (`SmMath.h`).
#[must_use]
pub fn are_same(a: f64, b: f64) -> bool {
    in_tol(a - b, scaled_zero2(&a, &b))
}

/// `SM_ARE_SAME_TO_TOL(a, b, tol)`: `|a - b| <= |tol|` (`SmMath.h`).
#[must_use]
pub fn are_same_to_tol(a: f64, b: f64, tol: f64) -> bool {
    in_tol(a - b, tol3d(tol))
}

/// True when `speed` is zero relative to the size `max_pos_dimension`:
/// `|speed| < override` or, without override, `|speed| < SM_EFF_ZERO * (1 + |max|)`
/// (`SmTol::IsZeroSpeed`).
#[must_use]
pub fn is_zero_speed(
    speed: f64,
    max_pos_dimension: f64,
    override_scaled_zero: Option<f64>,
) -> bool {
    speed.abs()
        < match override_scaled_zero {
            Some(z) => z,
            None => EFF_ZERO * (1.0 + max_pos_dimension.abs()),
        }
}

/// True when `v` is shorter than the intersection tolerance (the override, else the
/// system [`xsect_tol_3d`]) (`SmTol::IsDegenerate(const SmVector3d, SmXSectTol3d*)`).
#[must_use]
pub fn is_degenerate_vec3(k: &Kernel, v: Vec3, override_xsect_tol: Option<f64>) -> bool {
    let tol = match override_xsect_tol {
        Some(t) => t,
        None => xsect_tol_3d(k, SmallTopoType::NormalSize),
    };
    v.is_zero(tol)
}

/// True when `v1` and `v2` are parallel within the angle tolerance (the override in
/// radians, else [`ang_tol_rad`]) (`SmTol::AreParallel`).
#[must_use]
pub fn are_parallel(v1: Vec3, v2: Vec3, override_ang_tol_rad: Option<f64>) -> bool {
    let ang = override_ang_tol_rad.unwrap_or_else(ang_tol_rad);
    v1.is_parallel_to(v2, rad_to_deg(ang))
}

/// True when `v1` and `v2` are perpendicular within the angle tolerance (the override
/// in radians, else [`ang_tol_rad`]) (`SmTol::ArePerpendicular`).
#[must_use]
pub fn are_perpendicular(v1: Vec3, v2: Vec3, override_ang_tol_rad: Option<f64>) -> bool {
    let ang = override_ang_tol_rad.unwrap_or_else(ang_tol_rad);
    v1.is_perpendicular_to(v2, rad_to_deg(ang))
}

// ---- parameter-space mapping ------------------------------------------------------

/// Maps the 3D tolerance `tol` to a curve's parameter space at a point with first
/// derivative `deriv`: `tol / |deriv|`, or `natural_interval.length() / 100` where the
/// derivative is (near) zero (`SmTol::MapTo1d(SmTol3d&, SmVector3d&, const
/// SmExtent1d&)`). `tol` is the value of an `SmTol3d` (already `fabs`).
#[must_use]
pub fn map_to_1d(tol: f64, deriv: Vec3, natural_interval: &Extent1d) -> f64 {
    let scaled_zero = scaled_zero();
    let deriv_size = deriv.length();
    tol3d(if deriv_size < scaled_zero {
        natural_interval.length() / 100.0
    } else {
        tol / deriv_size
    })
}

/// `SmTol::MapTo1d(SmTol3d&, double, const SmCurve&)`: [`map_to_1d`] at the first
/// derivative of `curve` at `param` (`Evaluate(param, 1, TRUE, ..)`, its status ignored
/// upstream) and the curve's natural interval. `tol` is the value of an `SmTol3d`.
///
/// # Errors
///
/// Those of the curve evaluation (upstream ignores its status and then reads the unset
/// derivative; [`SolidError::Unported`] always propagates).
pub fn map_to_1d_on_curve(k: &Kernel, tol: f64, param: f64, curve: &Curve) -> Result<f64> {
    // evaluate CurvePt
    let pd = curve.evaluate(k, param, 1, true, true)?;
    let deriv = pd.get(1).copied().ok_or(SolidError::InvalidInput {
        operation: "geom::tol::map_to_1d_on_curve",
        reason: "no first derivative",
    })?;

    // pass the call along
    Ok(map_to_1d(tol, deriv, &curve.natural_interval()))
}

/// The `SmCurve` branch of `SmTol::GetZoneTol3d(const SmObject*)`: the tolerance of the
/// curve's edge, else the system zone tolerance.
///
/// # Errors
///
/// [`SolidError::Unported`]: `SmEdge::GetTolerance` (`topology.edge`) for a curve that
/// defines an edge's shape, and the owner chain of a generated curve or UV trim curve
/// (whose owners are not reachable from a bare curve).
pub fn zone_tol_3d_of_curve(k: &Kernel, curve: &Curve) -> Result<f64> {
    let mut chain = false;
    let edge = curve.edge(|_| {
        chain = true;
        None
    })?;
    if edge.is_some() {
        // UNPORTED(topology.edge): SmEdge::GetTolerance; justified: findings.md intersect.surface_drop_lib_p3
        // Proof (unreachable): callers are geom::tol::is_degenerate_curve (tests only) and
        // offset_geometry_creation (an unowned arc copy, the system branch); 0 hits in the
        // green difftest sweep.
        return Err(SolidError::unported(
            "SmEdge::GetTolerance",
            "topology.edge",
        ));
    }
    if chain {
        // UNPORTED(topology.edge): SmCurve::GetEdge through the owning curves and edge uses (the owner-chain resolver needs the B-rep arena); justified: findings.md intersect.surface_drop_lib_p3
        // Proof (unreachable): callers are geom::tol::is_degenerate_curve (tests only) and
        // offset_geometry_creation (an unowned arc copy, the system branch); 0 hits in the
        // green difftest sweep.
        return Err(SolidError::unported("SmCurve::GetEdge", "topology.edge"));
    }
    Ok(zone_tol_3d(k, SmallTopoType::NormalSize))
}

/// `SmTol::IsDegenerate(const SmCurve*, SmExtent1d, SmXSectTol3d*)`: the curve over
/// `interval` is shorter than the intersection tolerance (`override_xsect_tol`, else
/// twice-zone from [`zone_tol_3d_of_curve`]). A missing curve is degenerate.
///
/// # Errors
///
/// Those of [`zone_tol_3d_of_curve`] and [`Curve::is_degenerate`].
pub fn is_degenerate_curve(
    k: &Kernel,
    curve: Option<&Curve>,
    interval: &Extent1d,
    override_xsect_tol: Option<f64>,
) -> Result<bool> {
    let Some(curve) = curve else {
        return Ok(true);
    };
    let zone = zone_tol_3d_of_curve(k, curve)?;
    let xsect = override_xsect_tol.unwrap_or_else(|| xsect_tol_3d_from_zones(k, zone, zone));
    curve.is_degenerate(k, xsect, Some(interval))
}

// ---- scaled zeros ------------------------------------------------------------------

/// The default scaled zero, `SM_EFF_ZERO` (`SmTol::GetScaledZero()`).
#[must_use]
pub fn scaled_zero() -> f64 {
    tol3d(EFF_ZERO)
}

/// `SM_EFF_ZERO_SQ` (`SmTol::GetScaledZeroSq()`).
#[must_use]
pub fn scaled_zero_sq() -> f64 {
    tol3d(EFF_ZERO_SQ)
}

/// `SM_EFF_ZERO_SQRT` (`SmTol::GetScaledZeroSqrt()`).
#[must_use]
pub fn scaled_zero_sqrt() -> f64 {
    tol3d(EFF_ZERO_SQRT)
}

/// Values with a size-relative numerical zero (the unary `SmTol::GetScaledZero`
/// overloads): `SM_EFF_ZERO * (1 + size)`.
pub trait ScaledZero {
    /// The scaled zero of `self`.
    fn scaled_zero(&self) -> f64;
}

impl ScaledZero for f64 {
    /// `SmTol::GetScaledZero(const double)`: `SM_EFF_ZERO * (1 + |d|)`.
    fn scaled_zero(&self) -> f64 {
        tol3d(EFF_ZERO * (1.0 + self.abs()))
    }
}

impl ScaledZero for Vec2 {
    /// `SmTol::GetScaledZero(const SmVector2d&)`: `SM_EFF_ZERO * (1 + max |coord|)`.
    fn scaled_zero(&self) -> f64 {
        tol3d(EFF_ZERO * (1.0 + self.max_dimension()))
    }
}

impl ScaledZero for Vec3 {
    /// `SmTol::GetScaledZero(const SmVector3d&)`: `SM_EFF_ZERO * (1 + max |coord|)`.
    fn scaled_zero(&self) -> f64 {
        tol3d(EFF_ZERO * (1.0 + self.max_dimension()))
    }
}

impl ScaledZero for Extent1d {
    /// `SmTol::GetScaledZero(const SmExtent1d&)`: `SM_EFF_ZERO * (1 + max |end|)`.
    fn scaled_zero(&self) -> f64 {
        tol3d(EFF_ZERO * (1.0 + self.max_dimension()))
    }
}

impl ScaledZero for Extent3d {
    /// `SmTol::GetScaledZero(const SmExtent3d&)`: `SM_EFF_ZERO * (1 + max |coord|)`
    /// over both corners.
    fn scaled_zero(&self) -> f64 {
        tol3d(EFF_ZERO * (1.0 + max2(self.min().max_dimension(), self.max().max_dimension())))
    }
}

/// `SmTol::GetScaledZero(const SmEvalNFunctionsObject&)`: the equation set's own
/// scaled zero ([`EvalNFunctions::scaled_zero`]) as a tolerance (`fabs`).
#[must_use]
pub fn scaled_zero_of_equations<E: EvalNFunctions + ?Sized>(equations: &E) -> f64 {
    tol3d(equations.scaled_zero())
}

/// `SmTol::GetScaledZero(const SmVolume&)`: `100 * SM_EFF_ZERO` for an unbounded
/// parameter domain; for a bounded one the scaled zero of the out-space bounding box
/// of the domain (`EvaluateBoundingBox`, status ignored: on a failure the box holds
/// whatever was written before it, possibly nothing, as upstream).
///
/// The bounded branch is compared on unit-weight trilinear B-spline volumes in
/// the `geom.tol` difftest unit. Those fixtures do not cover higher-degree or
/// rational volumes.
/// # Errors
///
/// [`SolidError::Unported`] from the volume; the undefined cases of
/// `EvaluateBoundingBox` ([`SolidError::Unsupported`]).
pub fn scaled_zero_of_volume(k: &Kernel, volume: &Volume) -> Result<f64> {
    let param_domain = volume.natural_param_domain()?;
    if param_domain.is_bounded() {
        let mut out_box = Extent3d::new();
        let r = volume.evaluate_bounding_box(
            k,
            &param_domain,
            None,
            Some(&mut out_box),
            None,
            None,
            None,
            true,
        );
        crate::surface::volume::p2::ignore_read(r)?;
        return Ok(out_box.scaled_zero());
    }
    Ok(tol3d(100.0 * EFF_ZERO))
}

/// `SmTol::GetScaledZero(const SmCurve&)`: the binary scaled zero of the curve's mid
/// point and its approximate length (3 samples) over its natural interval.
///
/// # Errors
///
/// Those of `ApproximateLength` and `EvaluatePoint` (upstream ignores the point's
/// status and then reads an unset point; [`SolidError::Unported`] always propagates).
pub fn scaled_zero_of_curve(k: &Kernel, curve: &Curve) -> Result<f64> {
    let ivl = curve.natural_interval();
    let length = curve.approximate_length(k, &ivl, 3, false)?.0;
    let pnt = curve.evaluate_point(k, ivl.evaluate(0.5))?;
    Ok(scaled_zero2(&pnt, &length))
}

/// Square of the scaled zero of `a` (unary `SmTol::GetScaledZeroSq<cA>`).
#[must_use]
pub fn scaled_zero_sq_of<A: ScaledZero + ?Sized>(a: &A) -> f64 {
    let r = a.scaled_zero();
    tol3d(r * r)
}

/// Square root of the scaled zero of `a` (unary `SmTol::GetScaledZeroSqrt<cA>`).
#[must_use]
pub fn scaled_zero_sqrt_of<A: ScaledZero + ?Sized>(a: &A) -> f64 {
    tol3d(sm_sqrt(a.scaled_zero()))
}

/// Largest scaled zero of two values (binary `SmTol::GetScaledZero<cA, cB>`).
#[must_use]
pub fn scaled_zero2<A: ScaledZero + ?Sized, B: ScaledZero + ?Sized>(a: &A, b: &B) -> f64 {
    max2(a.scaled_zero(), b.scaled_zero())
}

/// Square of [`scaled_zero2`] (binary `SmTol::GetScaledZeroSq<cA, cB>`).
#[must_use]
pub fn scaled_zero_sq2<A: ScaledZero + ?Sized, B: ScaledZero + ?Sized>(a: &A, b: &B) -> f64 {
    let r = scaled_zero2(a, b);
    tol3d(r * r)
}

/// Square root of [`scaled_zero2`] (binary `SmTol::GetScaledZeroSqrt<cA, cB>`).
#[must_use]
pub fn scaled_zero_sqrt2<A: ScaledZero + ?Sized, B: ScaledZero + ?Sized>(a: &A, b: &B) -> f64 {
    tol3d(sm_sqrt(scaled_zero2(a, b)))
}

/// Largest scaled zero of three values (tertiary `SmTol::GetScaledZero<cA, cB, cC>`).
#[must_use]
pub fn scaled_zero3<A, B, C>(a: &A, b: &B, c: &C) -> f64
where
    A: ScaledZero + ?Sized,
    B: ScaledZero + ?Sized,
    C: ScaledZero + ?Sized,
{
    max3(a.scaled_zero(), b.scaled_zero(), c.scaled_zero())
}

/// Square of [`scaled_zero3`] (tertiary `SmTol::GetScaledZeroSq`).
#[must_use]
pub fn scaled_zero_sq3<A, B, C>(a: &A, b: &B, c: &C) -> f64
where
    A: ScaledZero + ?Sized,
    B: ScaledZero + ?Sized,
    C: ScaledZero + ?Sized,
{
    let r = scaled_zero3(a, b, c);
    tol3d(r * r)
}

/// Square root of [`scaled_zero3`] (tertiary `SmTol::GetScaledZeroSqrt`).
#[must_use]
pub fn scaled_zero_sqrt3<A, B, C>(a: &A, b: &B, c: &C) -> f64
where
    A: ScaledZero + ?Sized,
    B: ScaledZero + ?Sized,
    C: ScaledZero + ?Sized,
{
    tol3d(sm_sqrt(scaled_zero3(a, b, c)))
}

/// Largest scaled zero of four values (quaternary `SmTol::GetScaledZero`).
#[must_use]
pub fn scaled_zero4<A, B, C, D>(a: &A, b: &B, c: &C, d: &D) -> f64
where
    A: ScaledZero + ?Sized,
    B: ScaledZero + ?Sized,
    C: ScaledZero + ?Sized,
    D: ScaledZero + ?Sized,
{
    max4(
        a.scaled_zero(),
        b.scaled_zero(),
        c.scaled_zero(),
        d.scaled_zero(),
    )
}

/// Square of [`scaled_zero4`] (quaternary `SmTol::GetScaledZeroSq`).
#[must_use]
pub fn scaled_zero_sq4<A, B, C, D>(a: &A, b: &B, c: &C, d: &D) -> f64
where
    A: ScaledZero + ?Sized,
    B: ScaledZero + ?Sized,
    C: ScaledZero + ?Sized,
    D: ScaledZero + ?Sized,
{
    let r = scaled_zero4(a, b, c, d);
    tol3d(r * r)
}

/// Square root of [`scaled_zero4`] (quaternary `SmTol::GetScaledZeroSqrt`).
#[must_use]
pub fn scaled_zero_sqrt4<A, B, C, D>(a: &A, b: &B, c: &C, d: &D) -> f64
where
    A: ScaledZero + ?Sized,
    B: ScaledZero + ?Sized,
    C: ScaledZero + ?Sized,
    D: ScaledZero + ?Sized,
{
    tol3d(sm_sqrt(scaled_zero4(a, b, c, d)))
}

// ---- SmRelation ----------------------------------------------------------------------

/// Cardinality of a [`Relation`] (`SmRelationCardinalityType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Cardinality {
    /// `SM_RC_ONE_TO_ONE`: each first and each second in at most one pair.
    OneToOne,
    /// `SM_RC_ONE_TO_MANY`: each second has at most one first.
    OneToMany,
    /// `SM_RC_MANY_TO_ONE`: each first has at most one second.
    ManyToOne,
    /// `SM_RC_MANY_TO_MANY`: unrestricted (upstream default).
    #[default]
    ManyToMany,
}

impl Cardinality {
    /// Upstream enum value (`SM_RC_ONE_TO_ONE` = 0 .. `SM_RC_MANY_TO_MANY` = 3).
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::OneToOne => 0,
            Self::OneToMany => 1,
            Self::ManyToOne => 2,
            Self::ManyToMany => 3,
        }
    }

    /// The cardinality with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::OneToOne),
            1 => Some(Self::OneToMany),
            2 => Some(Self::ManyToOne),
            3 => Some(Self::ManyToMany),
            _ => None,
        }
    }
}

/// A many-to-many (or restricted) relation between values of `A` and `B`, typically
/// entity ids (`SmRelation<TYPE1, TYPE2>`).
///
/// Both directions are indexed. Query results are sorted by key (upstream: hash
/// order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation<A, B> {
    first_to_second: BTreeMap<A, BTreeSet<B>>,
    second_to_first: BTreeMap<B, BTreeSet<A>>,
    cardinality: Cardinality,
}

impl<A: Ord + Copy, B: Ord + Copy> Default for Relation<A, B> {
    fn default() -> Self {
        Self::new(Cardinality::default())
    }
}

impl<A: Ord + Copy, B: Ord + Copy> Relation<A, B> {
    /// An empty relation with the given cardinality (`SmRelation(crContext,
    /// eCardinality)`).
    #[must_use]
    pub const fn new(cardinality: Cardinality) -> Self {
        Self {
            first_to_second: BTreeMap::new(),
            second_to_first: BTreeMap::new(),
            cardinality,
        }
    }

    /// The cardinality restriction (`m_eCardinality`).
    #[must_use]
    pub const fn cardinality(&self) -> Cardinality {
        self.cardinality
    }

    /// Relates `first` with `second` (`RelatePair`). Relating an existing pair again
    /// is a no-op when the cardinality allows it.
    ///
    /// # Errors
    ///
    /// [`SolidError::Upstream`] with code 1001 (`SM_ERR`) when the pair would violate
    /// the cardinality; nothing is changed then.
    pub fn relate_pair(&mut self, first: A, second: B) -> Result<()> {
        let has_second = self.first_to_second.contains_key(&first);
        let has_first = self.second_to_first.contains_key(&second);
        let violates = match self.cardinality {
            Cardinality::OneToOne => has_first || has_second,
            Cardinality::OneToMany => has_first,
            Cardinality::ManyToOne => has_second,
            Cardinality::ManyToMany => false,
        };
        if violates {
            return Err(SolidError::Upstream {
                operation: "geom::tol::Relation::relate_pair",
                code: SM_ERR,
                entity: None,
            });
        }
        self.first_to_second
            .entry(first)
            .or_default()
            .insert(second);
        self.second_to_first
            .entry(second)
            .or_default()
            .insert(first);
        Ok(())
    }

    /// Relates `first` with each of `seconds` in order (`RelateToMany`).
    ///
    /// # Errors
    ///
    /// The first [`Relation::relate_pair`] error; the pairs before it stay related.
    pub fn relate_to_many(&mut self, first: A, seconds: &[B]) -> Result<()> {
        for &second in seconds {
            self.relate_pair(first, second)?;
        }
        Ok(())
    }

    /// Removes every pair (`RemoveAll`).
    pub fn remove_all(&mut self) {
        self.first_to_second.clear();
        self.second_to_first.clear();
    }

    /// Removes a pair from both directions (`RemovePair`). Empty keys are removed.
    /// As upstream, succeeds without changing anything when both keys exist but
    /// belong to different pairs.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when either key is absent; nothing is changed then.
    pub fn remove_pair(&mut self, first: A, second: B) -> Result<()> {
        if !self.first_to_second.contains_key(&first) || !self.second_to_first.contains_key(&second)
        {
            return Err(SolidError::sm_err("geom::tol::Relation::remove_pair"));
        }
        if let Some(seconds) = self.first_to_second.get_mut(&first) {
            seconds.remove(&second);
            if seconds.is_empty() {
                self.first_to_second.remove(&first);
            }
        }
        if let Some(firsts) = self.second_to_first.get_mut(&second) {
            firsts.remove(&first);
            if firsts.is_empty() {
                self.second_to_first.remove(&second);
            }
        }
        Ok(())
    }

    /// The only second related to `first`, or `None` when there is none or more than
    /// one (`GetSecond`).
    #[must_use]
    pub fn second(&self, first: A) -> Option<B> {
        let set = self.first_to_second.get(&first)?;
        if set.len() == 1 {
            set.iter().next().copied()
        } else {
            None
        }
    }

    /// Every second related to `first`, sorted (`GetSeconds`).
    #[must_use]
    pub fn seconds(&self, first: A) -> Vec<B> {
        self.first_to_second
            .get(&first)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Every first related to `second`, sorted (`GetFirsts`).
    #[must_use]
    pub fn firsts(&self, second: B) -> Vec<A> {
        self.second_to_first
            .get(&second)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Every first that is in a pair, sorted (`GetAllFirsts`).
    #[must_use]
    pub fn all_firsts(&self) -> Vec<A> {
        self.first_to_second.keys().copied().collect()
    }

    /// Every second that is in a pair, sorted (`GetAllSeconds`).
    #[must_use]
    pub fn all_seconds(&self) -> Vec<B> {
        self.second_to_first.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relation_removal_preserves_other_pairs_and_releases_empty_keys() {
        let mut relation = Relation::default();
        for (first, second) in [(0, 10), (0, 11), (1, 10), (2, 12)] {
            relation.relate_pair(first, second).unwrap();
        }
        assert_eq!(relation.firsts(10), vec![0, 1]);
        relation.remove_pair(0, 10).unwrap();
        assert_eq!(relation.seconds(0), vec![11]);
        assert_eq!(relation.firsts(10), vec![1]);
        // Both keys exist, but are not paired: native RemovePair succeeds.
        relation.remove_pair(0, 12).unwrap();
        assert_eq!(relation.seconds(0), vec![11]);
        assert_eq!(relation.firsts(12), vec![2]);
        assert!(relation.remove_pair(9, 10).is_err());
        assert!(relation.remove_pair(0, 19).is_err());
        relation.remove_pair(1, 10).unwrap();
        assert!(!relation.all_firsts().contains(&1));
        assert!(!relation.all_seconds().contains(&10));
        assert!(relation.firsts(10).is_empty());
        relation.relate_pair(1, 10).unwrap();
        assert_eq!(relation.second(1), Some(10));
    }

    fn test_line(len: f64) -> Curve {
        use crate::curve::line::{BoundedLineOptions, Line};
        Curve::from(Line::bounded(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(len, 0.0, 0.0),
            &Extent1d::from_min_max(0.0, 1.0),
            &BoundedLineOptions::default(),
        ))
    }

    #[test]
    fn curve_tolerances() {
        let k = Kernel::default();
        let c = test_line(4.0);
        // the tolerance over the speed of the curve at the parameter
        let speed = c.evaluate(&k, 0.5, 1, true, true).unwrap()[1].length();
        assert!(speed > scaled_zero());
        let t = map_to_1d_on_curve(&k, 1.0e-3, 0.5, &c).unwrap();
        assert_eq!(t, 1.0e-3 / speed);
        let ivl = Extent1d::from_min_max(0.0, 1.0);
        assert_eq!(is_degenerate_curve(&k, None, &ivl, None), Ok(true));
        assert_eq!(is_degenerate_curve(&k, Some(&c), &ivl, None), Ok(false));
        assert_eq!(
            is_degenerate_curve(&k, Some(&c), &ivl, Some(10.0)),
            Ok(true)
        );
    }

    #[test]
    fn scaled_zero_of_unbounded_volume() {
        use crate::geom::transform::Transform;
        use crate::surface::volume::VolumeKind;
        let v = Volume::new(VolumeKind::Transform(Transform::IDENTITY));
        assert_eq!(
            scaled_zero_of_volume(&Kernel::default(), &v),
            Ok(100.0 * EFF_ZERO)
        );
        // the base class's zero natural domain is bounded: the scaled zero of its
        // (sampled) bounding box
        let t = Volume::new(VolumeKind::Base);
        let z = scaled_zero_of_volume(&Kernel::default(), &t).unwrap();
        assert!(z.is_finite() && z > 0.0, "{z}");
    }
    use crate::kernel::KernelOptions;

    fn kernel(model: f64, ratio: f64) -> Kernel {
        let model = if model == USE_DEFAULT {
            MODEL_SIZE_ESTIMATE
        } else {
            model
        };
        let zone = size_estimate_to_zone_tol_3d(model);
        let mut o = KernelOptions::default();
        o.tolerances.model_size_estimate = model;
        o.tolerances.zone_tol_3d = zone;
        o.tolerances.xsect_tol_3d = zone * 2.0;
        o.tolerances.approx_tol_3d = zone / 2.0;
        o.tolerances.large_small_size_ratio = ratio;
        Kernel::new(&o)
    }

    #[test]
    fn size_estimate_table() {
        // SmTol.cpp sm_TemplateTrial.
        assert_eq!(size_estimate_to_zone_tol_3d(50.0), 1.0e-5);
        assert_eq!(size_estimate_to_zone_tol_3d(50000.0), 1.0e-4);
        assert_eq!(size_estimate_to_zone_tol_3d(0.005), 1.0e-6);
        assert_eq!(size_estimate_to_zone_tol_3d(USE_DEFAULT), 1.0e-5);
        assert_eq!(size_estimate_to_zone_tol_3d(1.0e8), 1.0e-1);
        assert_eq!(size_estimate_to_zone_tol_3d(0.0), 0.0);
    }

    #[test]
    fn system_tolerances() {
        let k = Kernel::default();
        assert_eq!(zone_tol_3d(&k, SmallTopoType::NormalSize), 1.0e-5);
        assert_eq!(xsect_tol_3d(&k, SmallTopoType::NormalSize), 2.0e-5);
        assert_eq!(approx_tol_3d(&k, SmallTopoType::NormalSize), 5.0e-6);
        assert_eq!(zone_tol_3d(&k, SmallTopoType::SmallSize), 1000.0 * 1.0e-5);
        assert_eq!(
            context_zone_tol_3d(&k, None, SmallTopoType::ForceSmallSize),
            1.0e-5
        );
        assert_eq!(gap_gain(&k), 1.1);
        assert_eq!(model_size_estimate(&k), 50.0);
        let k = kernel(50000.0, -2.0);
        assert_eq!(zone_tol_3d(&k, SmallTopoType::ForceSmallSize), 2.0e-4);
    }

    #[test]
    fn tol_constructors() {
        let k = Kernel::default();
        assert_eq!(tol3d(-0.5), 0.5);
        assert_eq!(zone_tol3d(&k, USE_DEFAULT), 1.0e-5);
        assert_eq!(xsect_tol3d(&k, USE_DEFAULT), 2.0e-5);
        assert_eq!(approx_tol3d(&k, USE_DEFAULT), 5.0e-6);
        assert_eq!(zone_tol3d(&k, -3.0), 3.0);
        assert_eq!(tol_mul(tol3d(-2.0), -3.0), -6.0);
    }

    #[test]
    fn gap_zone_tolerances() {
        let k = Kernel::default();
        assert_eq!(calc_edge_zone_tol_3d(&k, 0.0, 0.0, None, None), 1.0e-5);
        assert_eq!(calc_edge_zone_tol_3d(&k, 2.0, 0.0, None, None), 1.1);
        assert_eq!(
            calc_vertex_zone_tol_3d(&k, 0.0, 0.0, Some(0.25), None),
            0.25
        );
        assert_eq!(calc_face_zone_tol_3d(&k, None), 1.0e-5);
    }

    #[test]
    fn in_tol_and_predicates() {
        // SmTol.cpp sm_TemplateTrial: InTol(.001, .004) etc.
        assert!(in_tol(0.001, 0.004));
        assert!(in_tol(-0.001, 0.004));
        assert!(!in_tol(0.008, 0.004));
        assert!(is_zero_speed(1e-13, 0.0, None));
        assert!(!is_zero_speed(1e-11, 0.0, None));
        assert!(is_zero_speed(0.5, 0.0, Some(1.0)));
        let k = Kernel::default();
        assert!(is_degenerate_vec3(&k, Vec3::new(1e-6, 0.0, 0.0), None));
        assert!(!is_degenerate_vec3(&k, Vec3::new(1e-4, 0.0, 0.0), None));
        assert!(are_parallel(Vec3::X, -Vec3::X, None));
        assert!(!are_parallel(Vec3::X, Vec3::Y, None));
        assert!(are_perpendicular(Vec3::X, Vec3::Y, Some(0.1)));
        assert!(!are_perpendicular(Vec3::X, Vec3::ZERO, Some(0.1)));
    }

    #[test]
    fn scaled_zeros() {
        // SmTol.cpp sm_TemplateTrial values.
        let v2 = Vec2::new(1.0, -80.0);
        let v3 = Vec3::new(16.0, 100.0, 1000.0);
        let e3 = Extent3d::from_bounds(0.0, -30.0, -600.0, 2000.0, 100.0, 10000.0);
        assert_eq!(v2.scaled_zero(), EFF_ZERO * 81.0);
        assert_eq!(v3.scaled_zero(), EFF_ZERO * 1001.0);
        assert_eq!(e3.scaled_zero(), EFF_ZERO * 10001.0);
        assert_eq!(scaled_zero2(&v2, &v3), EFF_ZERO * 1001.0);
        assert_eq!(scaled_zero3(&v2, &v3, &e3), EFF_ZERO * 10001.0);
        assert_eq!(scaled_zero4(&1.0, &v2, &v3, &e3), EFF_ZERO * 10001.0);
        let z = 7.0f64.scaled_zero();
        assert_eq!(scaled_zero_sq_of(&7.0), z * z);
        assert_eq!(scaled_zero_sqrt_of(&7.0), z.sqrt());
        assert_eq!(scaled_zero(), EFF_ZERO);
        assert_eq!(scaled_zero_sq(), EFF_ZERO_SQ);
        assert_eq!(scaled_zero_sqrt(), EFF_ZERO_SQRT);
        assert_eq!(eff_zero_param(), EFF_ZERO_PARAM);
        assert_eq!(eff_zero_ang_rad(), EFF_ZERO_RAD);
        assert_eq!(eff_zero(), EFF_ZERO);
        assert_eq!(eff_zero_sq(), EFF_ZERO_SQ);
    }

    #[test]
    fn relation_cardinalities() {
        let mut r: Relation<u32, u32> = Relation::new(Cardinality::ManyToMany);
        assert!(r.relate_to_many(1, &[5, 3, 4]).is_ok());
        assert!(r.relate_pair(2, 3).is_ok());
        assert!(r.relate_pair(2, 3).is_ok());
        assert_eq!(r.seconds(1), vec![3, 4, 5]);
        assert_eq!(r.second(1), None);
        assert_eq!(r.second(2), Some(3));
        assert_eq!(r.second(9), None);
        assert_eq!(r.all_firsts(), vec![1, 2]);
        assert_eq!(r.all_seconds(), vec![3, 4, 5]);
        r.remove_all();
        assert!(r.all_firsts().is_empty());

        let mut r: Relation<u32, u32> = Relation::new(Cardinality::OneToOne);
        assert!(r.relate_pair(1, 1).is_ok());
        assert!(r.relate_pair(1, 2).is_err());
        assert!(r.relate_pair(2, 1).is_err());
        assert!(r.relate_pair(1, 1).is_err());

        let mut r: Relation<u32, u32> = Relation::new(Cardinality::OneToMany);
        assert!(r.relate_to_many(1, &[1, 2]).is_ok());
        assert!(r.relate_pair(2, 2).is_err());
        assert!(r.relate_to_many(3, &[3, 1, 4]).is_err());
        assert_eq!(r.seconds(3), vec![3]);

        let mut r: Relation<u32, u32> = Relation::new(Cardinality::ManyToOne);
        assert!(r.relate_pair(1, 1).is_ok());
        assert!(r.relate_pair(2, 1).is_ok());
        assert!(r.relate_pair(1, 2).is_err());
        assert_eq!(r.cardinality(), Cardinality::ManyToOne);
        assert_eq!(Cardinality::from_code(2), Some(Cardinality::ManyToOne));
        assert_eq!(
            SmallTopoType::from_code(3).map(SmallTopoType::code),
            Some(3)
        );
    }
}

// ---- object tolerance updates ------------------------------------------------------------------

const OP_UPDATE: &str = "geom::tol::update_object_tolerance";

/// A status upstream ignores, except the errors that cannot be ignored here: unported
/// callees and undefined behaviour upstream.
fn ignore_soft<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (SolidError::Unported { .. } | SolidError::InvalidInput { .. })) => Err(e),
        Err(_) => Ok(None),
    }
}

/// `SmFace::SetTolerance(tol)`.
fn face_set_tolerance(
    b: &mut Brep,
    f: FaceId,
    tol: f64,
    update_only_if_larger: bool,
    cascade_to_boundaries: bool,
) -> Result<()> {
    crate::topology::face::p3::set_tolerance(
        b,
        f,
        tol,
        update_only_if_larger,
        cascade_to_boundaries,
    )
}

/// `SmFace::GetEdges(rEdges)`.
fn face_edges(b: &Brep, f: FaceId) -> Result<Vec<EdgeId>> {
    crate::topology::face::p4::edges(b, f, None, false, None)
}

/// The topology object `SmTol::UpdateObjectTolerance` is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyToUpdate {
    /// An `SmVertex`.
    Vertex(VertexId),
    /// An `SmEdge`.
    Edge(EdgeId),
    /// An `SmFace`.
    Face(FaceId),
}

/// `SmTol::UpdateObjectTolerance(pTopo, dThisGap, dThisTol, bCheckGaps)` (CO-PORTED
/// in unit `boolean.topology_intersector_p1`): raises the object's tolerance to the
/// larger of `1.001 * this_gap` and `this_tol` (`SetTolerance` with
/// `bUpdateOnlyIfLarger`; a face only when larger). An edge updates its two vertices
/// the same way, a face its edges. With `check_gaps`, a vertex's (edge's) cached gaps
/// to its edges and faces (faces) raise those objects' tolerances to `2.001` times the
/// gap (`SmBrep::UpdateAndValidateTolerances` checks against half the tolerance): an
/// edge with `bUpdateOnlyIfLarger`, a face only when its tolerance is below the gap
/// (less `SM_EFF_ZERO`). `None` (a null object) does nothing.
///
/// # Errors
///
/// `SM_ERR_INVALID_INPUT` (as [`SolidError::Upstream`]) when the tolerance to use is
/// below `SM_EFF_ZERO`; [`SolidError::Unported`] for `SmFace::GetEdges`
/// (`topology.face_p4`); those of `SmFace::SetTolerance`, the
/// setters and gap queries.
pub fn update_object_tolerance(
    k: &Kernel,
    b: &mut Brep,
    topo: Option<TopologyToUpdate>,
    this_gap: f64,
    this_tol: f64,
    check_gaps: bool,
) -> Result<()> {
    // SM_ASSERT_TOL(dThisTol) only reports

    // no work - no target object
    let Some(topo) = topo else {
        return Ok(());
    };

    let tol_to_use = max2(1.001 * this_gap, this_tol); // cbiTolProj: some places do 2*.

    // If both input gap and tol are zero, there's nothing to do.
    if tol_to_use < EFF_ZERO {
        return Err(SolidError::Upstream {
            operation: OP_UPDATE,
            code: SM_ERR_INVALID_INPUT,
            entity: None,
        });
    }

    match topo {
        TopologyToUpdate::Vertex(v) => {
            // (SmZoneTol3d(dTolToUse): SM_USE_DEFAULT becomes the system tolerance)
            crate::topology::vertex::set_tolerance(b, v, zone_tol3d(k, tol_to_use), true, true)?; // TRUE: update only if larger.
                                                                                                  // when asked - update tolerance from current gap sizes
            if check_gaps {
                // Get gaps to connected edges and faces
                let mut gaps = Vec::new();
                // MaxGap3d = Max( Vertex/Edge Vertex/Face Gap ); the status is ignored
                ignore_soft(crate::topology::vertex::max_gap_3d(
                    k,
                    b,
                    v,
                    Some(&mut gaps),
                    None,
                ))?;

                // for every vertex/ConnectedNeighbor gap
                for held in gaps {
                    let gap: crate::heal::gap::Gap = match held.gap {
                        crate::topology::vertex::VertexGap::Edge(g) => g.into(),
                        crate::topology::vertex::VertexGap::Face(g) => g.into(),
                    };
                    let gap_length = gap.length();

                    // Note, SmBrep::UpdateAndValidateTolerances() checks against tol/2,
                    // so have to double the gap (plus a little).
                    let new_tol = 2.001 * gap_length;
                    if let Some(e) = gap.edge(b, 0)? {
                        crate::topology::edge::set_tolerance(
                            b,
                            e,
                            zone_tol3d(k, new_tol),
                            true,
                            true,
                        )?;
                        // TRUE: update only if larger.
                    }

                    if let Some(f) = gap.face(b)? {
                        if crate::topology::face::p2::tolerance(b, f)? < gap_length - EFF_ZERO {
                            face_set_tolerance(b, f, zone_tol3d(k, new_tol), true, true)?;
                        }
                    }
                } // end iter every gap
            }
        }

        TopologyToUpdate::Edge(e) => {
            crate::topology::edge::set_tolerance(b, e, zone_tol3d(k, tol_to_use), true, true)?; // TRUE: update only if larger.

            // We have to update lower-dimensional topology.  [cbiTolProj]
            // (the nested statuses are ignored upstream)
            let vtx = crate::topology::edge::start_vertex(b, e)?;
            ignore_soft(update_object_tolerance(
                k,
                b,
                vtx.map(TopologyToUpdate::Vertex),
                this_gap,
                this_tol,
                false,
            ))?;
            let vtx = crate::topology::edge::other_vertex(b, e, vtx)?;
            ignore_soft(update_object_tolerance(
                k,
                b,
                vtx.map(TopologyToUpdate::Vertex),
                this_gap,
                this_tol,
                false,
            ))?;

            if check_gaps {
                // (the status is ignored: no gaps on failure)
                let gaps = ignore_soft(crate::topology::edge::max_gap_3d(k, b, e, true, None))?
                    .and_then(|g| g.gaps)
                    .unwrap_or_default();
                for edge_gap in gaps {
                    let gap: crate::heal::gap::Gap = match edge_gap {
                        crate::topology::edge::EdgeGap::EdgeFace(g) => g.into(),
                        crate::topology::edge::EdgeGap::VertexEdge(g) => g.into(),
                    };
                    let gap_length = gap.length();

                    if let Some(f) = gap.face(b)? {
                        if crate::topology::face::p2::tolerance(b, f)? < gap_length - EFF_ZERO {
                            face_set_tolerance(
                                b,
                                f,
                                zone_tol3d(k, 2.001 * gap_length),
                                true,
                                true,
                            )?;
                        }
                    }
                }
            }
        }

        TopologyToUpdate::Face(f) => {
            let face_tol = crate::topology::face::p2::tolerance(b, f)?;
            if tol_to_use > face_tol {
                face_set_tolerance(b, f, zone_tol3d(k, tol_to_use), true, true)?;
            }

            // We have to update lower-dimensional topology.  [cbiTolProj]
            // Edges are sufficient, as they will take care of their vertices (just above).
            let edges = face_edges(b, f)?;
            for e in edges {
                // (the status is ignored upstream)
                ignore_soft(update_object_tolerance(
                    k,
                    b,
                    Some(TopologyToUpdate::Edge(e)),
                    this_gap,
                    this_tol,
                    false,
                ))?;
            }
        }
    } // end switch

    Ok(())
}
