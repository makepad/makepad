//! Face and loop properties of the healer: the cached per-face values `SmHealData` and
//! the healing steps read (`SmFaceProps`), the per-loop values (`SmLoopProps`), the loop
//! containment tree (`SmLooptreeItem`) and the loop classification record
//! (`SmClassifyLoopIO`).
//!
//! Upstream: `source/SMLib/src/SmFaceProps.cpp`, `source/SMLib/inc/SmFaceProps.h`,
//! `source/SMLib/src/SmLoopProps.cpp`, `source/SMLib/inc/SmLoopProps.h`.
//!
//! | Rust                                              | Upstream                                      |
//! |---------------------------------------------------|-----------------------------------------------|
//! | [`FaceProps`], [`FaceProps::new`], [`FaceProps::for_face`] | `SmFaceProps`, `SmFaceProps()`, `SmFaceProps(face, index)` |
//! | `Drop` (implicit)                                 | `~SmFaceProps`, `~SmLooptreeItem`             |
//! | [`FaceProps::reset`]                              | `SmFaceProps::ReSet`                          |
//! | [`FaceProps::set_props`]                          | `SmFaceProps::SetProps`                       |
//! | [`FaceProps::set_props_gaps`]                     | `SmFaceProps::SetProps_Gaps`                  |
//! | [`FaceProps::set_props_stage2`]                   | `SmFaceProps::SetProps_Stage2`                |
//! | [`FaceProps::set_props_stage3`]                   | `SmFaceProps::SetProps_Stage3`                |
//! | [`FaceProps::is_point3d_on_pole`]                 | `SmFaceProps::IsPoint3dOnPole`                |
//! | [`FaceProps::has_problems`]                       | `SmFaceProps::HasProblems`                    |
//! | [`FaceProps::has_props`], [`FaceProps::has_props_stage1`], [`FaceProps::has_props_stage2`], [`FaceProps::has_props_stage3`] | `HasProps`, `HasProps_Stage1`, `HasProps_Stage2`, `HasProps_Stage3` |
//! | [`PropStage`], [`HealNotYetFaceType`]             | `SmPropStageType`, `SmHealNotYetFaceType`     |
//! | [`MixedLinkType`], [`TriedLinkType`], [`HealerOpType`] | `SmMixedLinkType`, `SmTriedLinkType`, `SmHealerOpType` (`SmHealProbArray.h`; CO-PORTED here) |
//! | [`ContainmentType`]                               | `SmContainmentType` (`SmTypes.h`)             |
//! | [`LoopProps`], [`LoopProps::new`]                 | `SmLoopProps`, its constructors               |
//! | [`LoopProps::reset`], [`LoopProps::set_props`]    | `SmLoopProps::ReSet`, `SmLoopProps::SetProps` |
//! | [`ClassifyLoopIo`]                                | `SmClassifyLoopIO`                            |
//! | [`Looptree`], [`LooptreeItem`], [`ItemId`]        | the `SmLooptreeItem` tree and its nodes       |
//! | [`LooptreeItem::new`], [`LooptreeItem::with_loop`], [`LooptreeItem::with_partner`] | `SmLooptreeItem` constructors |
//! | [`LooptreeItem::reset_no_recursion`]              | `SmLooptreeItem::ReSetNoRecursion`            |
//! | [`LooptreeItem::is_no_area_pair`], [`LooptreeItem::add_partner`] | `IsNoArea_Pair`, `AddPartner`  |
//! | [`Looptree::add_child`], [`Looptree::add_sibling`] | `SmLooptreeItem::AddChild`, `AddSibling`     |
//! | [`Looptree::find_item`]                           | `SmLooptreeItem::FindItem`                    |
//!
//! Deviations:
//! - **Object model.** There are no `SmContext` pointers: the context is
//!   `Kernel::context`, so `SetContext` of the face and loop props (and the context
//!   argument of the constructors) have no counterpart; every method that reads
//!   tolerances takes the [`Kernel`]. Pointers to topology are ids; the `SmFace`,
//!   `SmLoop` and surface methods are free functions of the topology units taking the
//!   [`Brep`]. `m_cpSurface` is the face's own surface (a flag, [`FaceProps::has_surface`]).
//!   Where upstream dereferences a null pointer the port returns
//!   [`SolidError::InvalidInput`].
//! - **`SmBoolean` tri-states** (`TRUE`, `FALSE`, `UNSURE`) are [`Tri`]; `SM_UNDEF_ULONG`
//!   counters keep their sentinel ([`UNDEF_ULONG`]) because `HasProblems` and the flag
//!   tests read them before they are set. `SM_OT_UPPERDOMAIN` / `SM_OT_LOWERDOMAIN` are
//!   [`OrientType::Same`] / [`OrientType::Opposite`] (equal values upstream).
//! - **`SmCurveClassification`** of the seam curves is [`CurveClassification`] over a
//!   caller-chosen object handle `O` ([`LoopObject`]: a handle that can name a loop, for
//!   the `GetConnectedTouches` of `SmLoopProps::SetProps`).
//! - **Looptree.** The tree of `SmLooptreeItem` pointers is an arena ([`Looptree`]) of
//!   items addressed by [`ItemId`]; `m_pLoopProps` / `m_pClassifyLoopIO` are indices into
//!   [`FaceProps::loop_props`] and the classification records of `ClassifyLoops`. Items
//!   are dropped with the arena (no recursive destructor). `FindItem` walks sibling
//!   chains iteratively in upstream's visit order and recurses only through children,
//!   with depth [`MAX_LOOPTREE_DEPTH`]; deeper trees are [`SolidError::InvalidInput`].
//! - **Statuses.** `SetProps` keeps upstream's "last status wins" over the three stage
//!   calls; [`SolidError::Unported`] returns at once. `SM_ERR_NOTYET_HEAL_FACE` is
//!   [`SolidError::Upstream`] with code [`SM_ERR_NOTYET_HEAL_FACE`], and the face props
//!   keep the not-yet type it recorded. The optional label argument (debug dumps only)
//!   and the `SM_DEBUG_CODE` graphics and asserts are dropped (asserts are compiled out
//!   of the reference build). The unreachable `SmCurveClassification::ReSet` branch of
//!   `SetProps_Stage2` (the classifications are deleted by `ReSet(SM_PROPSTAGE_2)`
//!   first) is kept and calls [`CurveClassification::reset`].
//! - **Callees.** `SmFace::HasSeamProblem` of [`FaceProps::set_props_stage2`] runs
//!   [`crate::topology::face::p4::has_seam_problem_in_brep`] on the face's own B-rep
//!   objects, so the handle type must name them
//!   ([`crate::topology::face::p4::BrepSeamObjects`]; otherwise
//!   [`SolidError::InvalidInput`]). The `SmLoop` queries (`topology.loopuse`),
//!   `SmCurveClassification::ReSet` (`intersect.curve_class_p2`) and
//!   `SmFace::ClassifyLoops` ([`crate::query::face::p4::classify_loops`], whose status
//!   `SetProps_Stage3` ignores as upstream) are wired. With the opt-in
//!   `Kernel::fixes.heal_stage3_failures_propagate` (WR-268, WR-266, #521)
//!   `SetProps_Stage3` propagates classification failures before reading the
//!   classification records, and an error clears its in-progress stage marker, keeping
//!   partial diagnostics without certifying the properties or skipping a retry.
//! - **Query caches.** The mutable healer explicitly applies the vertex/face and
//!   edge/face gap caches produced by seam queries, even on errors. Read-only props
//!   queries still leave the B-rep unchanged; no geometry or topology is copied back.

use crate::error::{ignored, EntityRef, Result, SolidError};
use crate::geom::global_solver::SolverOperation;
use crate::geom::tol::{calc_face_zone_tol_3d, xsect_tol_3d_from_zones, zone_tol3d};
use crate::geom::vector3d::{Point3, Vec3};
use crate::intersect::curve_class::p1::{
    get_connected_touches, get_winding_count, CrvDirOnFaceType, CurveClassification, TouchData,
};
use crate::intersect::curve_class::p2::ResetOptions;
use crate::intersect::point_class::ClassObject;
use crate::kernel::Kernel;
use crate::math::EFF_ZERO;
use crate::nurbs::b_spline_surface::p1::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::face::p2::{CurveOnClassifyCache, FaceBrep};
use crate::topology::edge as edge_mod;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::face::p2 as face_p2;
use crate::topology::face::p4 as face_p4;
use crate::topology::loopuse;
use crate::topology::vertex as vertex_mod;
use crate::topology::{
    Brep, EdgeId, EdgeuseId, FaceId, LoopId, LoopuseStart, VertexId, VertexuseId,
};

const OP_STAGE2: &str = "heal::face_props::FaceProps::set_props_stage2";
const OP_STAGE3: &str = "heal::face_props::FaceProps::set_props_stage3";
const OP_LOOP: &str = "heal::face_props::LoopProps::set_props";
const OP_TREE: &str = "heal::face_props::Looptree::find_item";

/// `SM_UNDEF_ULONG`: the value of an unset counter or bit mask.
pub const UNDEF_ULONG: u64 = 123_456_789;

/// `SM_ERR_NOTYET_HEAL_FACE` (`SmMessages.h`): a face with a problem the healer does
/// not support yet.
pub const SM_ERR_NOTYET_HEAL_FACE: i32 = 1040;

/// The deepest child nesting [`Looptree::find_item`] follows.
pub const MAX_LOOPTREE_DEPTH: usize = 1_000;

/// `SM_SS_NONE`: no singular side.
pub const SS_NONE: u64 = 0;
/// `SM_SS_UMIN`.
pub const SS_UMIN: u64 = 1;
/// `SM_SS_VMIN`.
pub const SS_VMIN: u64 = 2;
/// `SM_SS_UMAX`.
pub const SS_UMAX: u64 = 4;
/// `SM_SS_VMAX`.
pub const SS_VMAX: u64 = 8;
/// `SM_FC_UMIN_VMIN`.
pub const FC_UMIN_VMIN: u64 = 1;
/// `SM_FC_UMIN_VMAX`.
pub const FC_UMIN_VMAX: u64 = 2;
/// `SM_FC_UMAX_VMIN`.
pub const FC_UMAX_VMIN: u64 = 4;
/// `SM_FC_UMAX_VMAX`.
pub const FC_UMAX_VMAX: u64 = 8;

fn null(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::invalid_input(operation, reason)
}

/// The element `i` of `v`, or [`SolidError::InvalidInput`] (upstream reads past the
/// end of an `SmTArray`).
fn at<T: Copy>(v: &[T], i: usize, operation: &'static str) -> Result<T> {
    v.get(i)
        .copied()
        .ok_or_else(|| null(operation, "index out of range"))
}

// ---- enumerations ---------------------------------------------------------------------

/// An `SmBoolean` that can also be `UNSURE` (`TRUE` 1, `FALSE` 0, `UNSURE` 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tri {
    /// `FALSE`.
    False,
    /// `TRUE`.
    True,
    /// `UNSURE`: not yet known.
    #[default]
    Unsure,
}

impl Tri {
    /// The upstream value (`FALSE` 0, `TRUE` 1, `UNSURE` 2).
    #[must_use]
    pub const fn code(self) -> i64 {
        match self {
            Self::False => 0,
            Self::True => 1,
            Self::Unsure => 2,
        }
    }

    /// `value == TRUE`.
    #[must_use]
    pub const fn is_true(self) -> bool {
        matches!(self, Self::True)
    }

    /// `value == FALSE`.
    #[must_use]
    pub const fn is_false(self) -> bool {
        matches!(self, Self::False)
    }
}

impl From<bool> for Tri {
    fn from(b: bool) -> Self {
        if b {
            Self::True
        } else {
            Self::False
        }
    }
}

/// `SmPropStageType`: how far the face properties have been computed; ordered as
/// upstream (`SM_PROPSTAGE_0` .. `SM_PROPSTAGE_3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PropStage {
    /// `SM_PROPSTAGE_0`: immediately after construction.
    #[default]
    Stage0 = 0,
    /// `SM_PROPSTAGE_GAPSa`: during `SetProps_Gaps`.
    GapsA = 1,
    /// `SM_PROPSTAGE_GAPS`: after `SetProps_Gaps`.
    Gaps = 2,
    /// `SM_PROPSTAGE_2a`: during `SetProps_Stage2`.
    Stage2A = 3,
    /// `SM_PROPSTAGE_2`: after `SetProps_Stage2`.
    Stage2 = 4,
    /// `SM_PROPSTAGE_3a`: during `SetProps_Stage3`.
    Stage3A = 5,
    /// `SM_PROPSTAGE_3`: after `SetProps_Stage3`.
    Stage3 = 6,
}

impl PropStage {
    /// The upstream value.
    #[must_use]
    pub const fn code(self) -> i64 {
        self as i64
    }

    /// The stage with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Stage0,
            1 => Self::GapsA,
            2 => Self::Gaps,
            3 => Self::Stage2A,
            4 => Self::Stage2,
            5 => Self::Stage3A,
            6 => Self::Stage3,
            _ => return None,
        })
    }
}

/// `SmHealNotYetFaceType`: the first `ClassifyLoops` failure of a face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HealNotYetFaceType {
    /// `SM_HNF_NO_PROBS`.
    #[default]
    NoProbs = 0,
    /// `SM_HNF_NO_CLASSIFY_ZERO_CROSSCNT`.
    NoClassifyZeroCrosscnt = 1,
    /// `SM_HNF_NO_CLASSIFY_ODD_CROSSCNT`.
    NoClassifyOddCrosscnt = 2,
    /// `SM_HNF_NO_LOOP_TREE_ROOT`.
    NoLoopTreeRoot = 3,
}

impl HealNotYetFaceType {
    /// The upstream value.
    #[must_use]
    pub const fn code(self) -> i64 {
        self as i64
    }

    /// The type with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::NoProbs,
            1 => Self::NoClassifyZeroCrosscnt,
            2 => Self::NoClassifyOddCrosscnt,
            3 => Self::NoLoopTreeRoot,
            _ => return None,
        })
    }
}

/// `SmMixedLinkType` (`SmHealProbArray.h`, CO-PORTED by `heal.face_props`): whether an
/// object property is a problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MixedLinkType {
    /// `SM_MP_UNDEF`: not yet known.
    #[default]
    Undef = 0,
    /// `SM_MP_NOPROP`: the object does not have the property.
    NoProp = 1,
    /// `SM_MP_HASPROP`: it has the property, not yet known if a problem.
    HasProp = 2,
    /// `SM_MP_OKAY`.
    Okay = 3,
    /// `SM_MP_PROB`: a problem needing a fix.
    Prob = 4,
    /// `SM_MP_FIXED`: a problem that is fixed now.
    Fixed = 5,
}

impl MixedLinkType {
    /// The upstream value.
    #[must_use]
    pub const fn code(self) -> i64 {
        self as i64
    }

    /// The type with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Undef,
            1 => Self::NoProp,
            2 => Self::HasProp,
            3 => Self::Okay,
            4 => Self::Prob,
            5 => Self::Fixed,
            _ => return None,
        })
    }
}

/// `SmTriedLinkType` (`SmHealProbArray.h`, CO-PORTED by `heal.face_props`): whether an
/// object has been through a fix function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TriedLinkType {
    /// `SM_TRY_NONE`.
    #[default]
    None = 0,
    /// `SM_TRY_RAN`.
    Ran = 1,
    /// `SM_TRY_FIXED`.
    Fixed = 2,
    /// `SM_TRY_NOFIX`.
    NoFix = 3,
}

impl TriedLinkType {
    /// The upstream value.
    #[must_use]
    pub const fn code(self) -> i64 {
        self as i64
    }
}

/// `SmHealerOpType` (`SmHealProbArray.h`, CO-PORTED by `heal.face_props`): the last
/// healer step that ran, ordered as the healer runs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum HealerOpType {
    /// `SM_HO_NONE`.
    #[default]
    None = 0,
    /// `SM_HO_FIX_BACKPOINTERS`.
    FixBackpointers = 1,
    /// `SM_HO_CACHE_EDGEPROPS`.
    CacheEdgeprops = 2,
    /// `SM_HO_CACHE_VERTEXPROPS`.
    CacheVertexprops = 3,
    /// `SM_HO_CACHE_FACEPROPS_GAPS`.
    CacheFacepropsGaps = 4,
    /// `SM_HO_FIX_TOLSIZES`.
    FixTolsizes = 5,
    /// `SM_HO_CACHE_COIN_VERTICES`.
    CacheCoinVertices = 6,
    /// `SM_HO_FIX_COIN_VERTICES`.
    FixCoinVertices = 7,
    /// `SM_HO_CACHE_DEGEN_FACES`.
    CacheDegenFaces = 8,
    /// `SM_HO_FIX_DEGEN_FACES`.
    FixDegenFaces = 9,
    /// `SM_HO_FIX_DEGEN_EDGES`.
    FixDegenEdges = 10,
    /// `SM_HO_CACHE_COIN_EDGES`.
    CacheCoinEdges = 11,
    /// `SM_HO_FIX_COIN_EDGES`.
    FixCoinEdges = 12,
    /// `SM_HO_CACHE_MISSED_EDGEXSECTS`.
    CacheMissedEdgexsects = 13,
    /// `SM_HO_FIX_MISSED_EDGEXSECTS`.
    FixMissedEdgexsects = 14,
    /// `SM_HO_FIX_UNCONTAINED_EDGES`.
    FixUncontainedEdges = 15,
    /// `SM_HO_FIX_BADGAPS`.
    FixBadgaps = 16,
    /// `SM_HO_CACHE_FACEPROPS_2`.
    CacheFaceprops2 = 17,
    /// `SM_HO_FIX_MOVESEAM`.
    FixMoveseam = 18,
    /// `SM_HO_FIX_SPLITEDGE_ATSEAM`.
    FixSplitedgeAtseam = 19,
    /// `SM_HO_FIX_BADSHEETS`.
    FixBadsheets = 20,
    /// `SM_HO_CACHE_FACEPROPS_3`.
    CacheFaceprops3 = 21,
    /// `SM_HO_FIX_SPLITFACE_ATSEAMS`.
    FixSplitfaceAtseams = 22,
    /// `SM_HO_FIX_BADLOOPS`.
    FixBadloops = 23,
    /// `SM_HO_MAKE_UVTRIMCURVES`.
    MakeUvtrimcurves = 24,
    /// `SM_HO_FIX_INFINITE_REGIONS`.
    FixInfiniteRegions = 25,
    /// `SM_HO_ALL`.
    All = 26,
}

impl HealerOpType {
    /// The upstream value.
    #[must_use]
    pub const fn code(self) -> i64 {
        self as i64
    }

    /// The step with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::None,
            1 => Self::FixBackpointers,
            2 => Self::CacheEdgeprops,
            3 => Self::CacheVertexprops,
            4 => Self::CacheFacepropsGaps,
            5 => Self::FixTolsizes,
            6 => Self::CacheCoinVertices,
            7 => Self::FixCoinVertices,
            8 => Self::CacheDegenFaces,
            9 => Self::FixDegenFaces,
            10 => Self::FixDegenEdges,
            11 => Self::CacheCoinEdges,
            12 => Self::FixCoinEdges,
            13 => Self::CacheMissedEdgexsects,
            14 => Self::FixMissedEdgexsects,
            15 => Self::FixUncontainedEdges,
            16 => Self::FixBadgaps,
            17 => Self::CacheFaceprops2,
            18 => Self::FixMoveseam,
            19 => Self::FixSplitedgeAtseam,
            20 => Self::FixBadsheets,
            21 => Self::CacheFaceprops3,
            22 => Self::FixSplitfaceAtseams,
            23 => Self::FixBadloops,
            24 => Self::MakeUvtrimcurves,
            25 => Self::FixInfiniteRegions,
            26 => Self::All,
            _ => return None,
        })
    }
}

/// `SmContainmentType` (`SmTypes.h`): how a loop is contained in the other loops of its
/// face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ContainmentType {
    /// `SM_CMT_UNKNOWN`: not set.
    #[default]
    Unknown = 0,
    /// `SM_CMT_OUTERLOOP`.
    OuterLoop = 1,
    /// `SM_CMT_INNERLOOP`.
    InnerLoop = 2,
    /// `SM_CMT_NESTEDLOOP_EVEN`.
    NestedLoopEven = 3,
    /// `SM_CMT_NESTEDLOOP_ODD`.
    NestedLoopOdd = 4,
    /// `SM_CMT_BOTLOOP`.
    BotLoop = 5,
    /// `SM_CMT_TOPLOOP`.
    TopLoop = 6,
    /// `SM_CMT_WIRELOOP`.
    WireLoop = 7,
    /// `SM_CMT_ERROR`.
    Error = 8,
}

impl ContainmentType {
    /// The upstream value.
    #[must_use]
    pub const fn code(self) -> i64 {
        self as i64
    }

    /// The type with upstream value `code`, if any.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Unknown,
            1 => Self::OuterLoop,
            2 => Self::InnerLoop,
            3 => Self::NestedLoopEven,
            4 => Self::NestedLoopOdd,
            5 => Self::BotLoop,
            6 => Self::TopLoop,
            7 => Self::WireLoop,
            8 => Self::Error,
            _ => return None,
        })
    }
}

/// The type of a loop (`m_tLoopuseType`: `SmVertexuse_TYPE`, `SmEdgeuse_TYPE` or
/// `SmUnknown_TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LoopuseType {
    /// `SmUnknown_TYPE` (16999).
    #[default]
    Unknown,
    /// `SmVertexuse_TYPE` (16006): a vertex loop.
    Vertexuse,
    /// `SmEdgeuse_TYPE` (16005): an edge loop.
    Edgeuse,
    /// `0`: the zero bytes of a record `SmTArray::SetSize` adds without running the
    /// constructor (an unclassified record of `SmFace::ClassifyLoops`).
    Zero,
}

impl LoopuseType {
    /// The upstream `SM_TYPE` value.
    #[must_use]
    pub const fn code(self) -> i64 {
        match self {
            Self::Unknown => 16999,
            Self::Vertexuse => 16006,
            Self::Edgeuse => 16005,
            Self::Zero => 0,
        }
    }
}

/// The orientation value of an `SmOrientType` as an integer (`SM_OT_UNKNOWN` 0,
/// `SM_OT_SAME` 1, `SM_OT_OPPOSITE` 2).
#[must_use]
pub const fn orient_code(o: OrientType) -> i64 {
    match o {
        OrientType::Unknown => 0,
        OrientType::Same => 1,
        OrientType::Opposite => 2,
    }
}

/// An object handle that can name a loop (what `SmLoopProps::SetProps` hands
/// `GetConnectedTouches` as `(SmTopology*) cpLoop`).
pub trait LoopObject: ClassObject {
    /// The handle of loop `l`.
    fn from_loop(l: LoopId) -> Self;

    /// The loop this handle names, if it is a loop (`SmFace::ClassifyLoops` casts a
    /// touch's `m_pConnectedTo` to `SmLoop*`); `None` (the default) for a handle type
    /// that names no loops.
    fn as_loop(&self) -> Option<LoopId> {
        None
    }

    /// The vertex this handle names, if it is a vertex (`GetVertexObject` of a vertex
    /// point classification); `None` (the default) for a handle type that names no
    /// vertices.
    fn as_vertex(&self) -> Option<VertexId> {
        None
    }

    /// The handle of face `f` (the face a curve classification classifies against),
    /// `None` (the default) when this handle type cannot name faces. Used by
    /// [`crate::query::face::p3::LoopObjectCurves`].
    fn from_face(_f: FaceId) -> Option<Self> {
        None
    }

    /// The handle of face `f`'s surface (a classification's surface 1), `None` (the
    /// default) when this handle type cannot name it.
    fn from_face_surface(_f: FaceId) -> Option<Self> {
        None
    }

    /// The handle of a curve a classification makes and classifies: its 3D curve
    /// (`uv` false) or its UV curve (`uv` true); `None` (the default) when this handle
    /// type cannot name them.
    fn from_classify_curve(_uv: bool) -> Option<Self> {
        None
    }

    /// The handle of B-rep entity `t` (a vertex, edge, vertex use or edge use the
    /// boundary intersections of a classification name), `None` (the default) when
    /// this handle type cannot name it. A handle type names every entity of a kind or
    /// none.
    fn from_topology(_t: EntityRef) -> Option<Self> {
        None
    }

    /// The B-rep entity this handle names (the inverse of [`Self::from_topology`]),
    /// `None` (the default) for any other handle.
    fn as_topology(&self) -> Option<EntityRef> {
        None
    }

    /// The handle of edge `e`'s curve, `None` (the default) when this handle type
    /// cannot name it.
    fn from_edge_curve(_e: EdgeId) -> Option<Self> {
        None
    }

    /// The edge whose curve this handle names (the inverse of
    /// [`Self::from_edge_curve`]).
    fn as_edge_curve(&self) -> Option<EdgeId> {
        None
    }

    /// The handle of the UV trim curve of edge use `eu`, `None` (the default) when
    /// this handle type cannot name it.
    fn from_trim_curve(_eu: EdgeuseId) -> Option<Self> {
        None
    }

    /// The edge use whose UV trim curve this handle names (the inverse of
    /// [`Self::from_trim_curve`]).
    fn as_trim_curve(&self) -> Option<EdgeuseId> {
        None
    }
}

// ---- SmClassifyLoopIO -----------------------------------------------------------------

/// The `SmFace::ClassifyLoops` result of one loop (`SmClassifyLoopIO`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClassifyLoopIo {
    /// `m_pLoop`: the classified loop.
    pub loop_: Option<LoopId>,
    /// `m_tLoopuseType`.
    pub loopuse_type: LoopuseType,
    /// `m_eOrient`: [`OrientType::Same`] is counter-clockwise (the outer loop
    /// direction; upper domain for no-area loops), [`OrientType::Opposite`] clockwise.
    pub orient: OrientType,
    /// `m_bDegen_Loop`.
    pub degen_loop: Tri,
    /// `m_bArea_Loop`: a loop with area (not a no-area loop of a missing seam).
    pub area_loop: Tri,
    /// `m_eContainmentType`.
    pub containment: ContainmentType,
}

// ---- SmLoopProps ----------------------------------------------------------------------

/// The properties of one face loop (`SmLoopProps`).
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean members, as tri-states
pub struct LoopProps {
    /// `m_cpLoop`.
    pub loop_: Option<LoopId>,
    /// `m_tLoopuseType`.
    pub loopuse_type: LoopuseType,
    /// `m_bVertexLoopOnPole`: for a vertex loop, whether its vertex is on a pole.
    pub vertex_loop_on_pole: Tri,
    /// `m_lEdgeCnt`.
    pub edge_cnt: u64,
    /// `m_lEdgeuseCnt`.
    pub edgeuse_cnt: u64,
    /// `m_bClosed3d`.
    pub closed_3d: Tri,
    /// `m_bClosedPtrs`.
    pub closed_ptrs: Tri,
    /// `m_bGoodClosed`: `closed_3d == closed_ptrs`.
    pub good_closed: Tri,
    /// `m_lSeamCrossingCntU`.
    pub seam_crossing_cnt_u: u64,
    /// `m_lSeamWindingCntU`.
    pub seam_winding_cnt_u: u64,
    /// `m_eWindingOrientU`.
    pub winding_orient_u: OrientType,
    /// `m_lSeamCrossingCntV`.
    pub seam_crossing_cnt_v: u64,
    /// `m_lSeamWindingCntV`.
    pub seam_winding_cnt_v: u64,
    /// `m_eWindingOrientV`.
    pub winding_orient_v: OrientType,
    /// `m_bDegen_Loop` (not set yet upstream).
    pub degen_loop: Tri,
    /// `m_bArea_Loop`.
    pub area_loop: Tri,
    /// `m_eContainmentType`.
    pub containment: ContainmentType,
    /// `m_eLoopOrient`.
    pub loop_orient: OrientType,
    /// `m_eDesiredLoopOrient`.
    pub desired_loop_orient: OrientType,
    /// `m_bGoodOrient`.
    pub good_orient: Tri,
}

impl Default for LoopProps {
    /// `SmLoopProps(ctx)`: [`LoopProps::reset`].
    fn default() -> Self {
        Self::new()
    }
}

impl LoopProps {
    /// `SmLoopProps(cpContext)`: every value unset.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            loop_: None,
            loopuse_type: LoopuseType::Unknown,
            vertex_loop_on_pole: Tri::Unsure,
            edge_cnt: UNDEF_ULONG,
            edgeuse_cnt: UNDEF_ULONG,
            closed_3d: Tri::Unsure,
            closed_ptrs: Tri::Unsure,
            good_closed: Tri::Unsure,
            seam_crossing_cnt_u: UNDEF_ULONG,
            seam_winding_cnt_u: UNDEF_ULONG,
            winding_orient_u: OrientType::Unknown,
            seam_crossing_cnt_v: UNDEF_ULONG,
            seam_winding_cnt_v: UNDEF_ULONG,
            winding_orient_v: OrientType::Unknown,
            degen_loop: Tri::Unsure,
            area_loop: Tri::Unsure,
            containment: ContainmentType::Unknown,
            loop_orient: OrientType::Unknown,
            desired_loop_orient: OrientType::Unknown,
            good_orient: Tri::Unsure,
        }
    }

    /// `SmLoopProps(cpLoop, cpContext)`: unset values for `loop_`.
    #[must_use]
    pub const fn for_loop(loop_: LoopId) -> Self {
        let mut p = Self::new();
        p.loop_ = Some(loop_);
        p
    }

    /// `ReSet`: every value unset.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// `SmLoopProps::SetProps`: sets the values of loop `loop_` marked `*` upstream (the
    /// type, edge and edge use counts, the closed flags, the seam crossing and winding
    /// counts and the area flag); the containment and orientation values are set later
    /// by `SetProps_Stage3`. `face_props` is the owner face's props after
    /// `SetProps_Stage2`.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a
    /// loop without loop use, a vertex loop without vertex, or an odd touch list;
    /// [`SolidError::Unported`] for `SmLoop::GetEdges`, `GetEdgeuses`, `IsClosed3d` and
    /// `IsClosedPtrs` (`topology.loopuse`).
    pub fn set_props<O: LoopObject>(
        &mut self,
        k: &Kernel,
        b: &Brep,
        loop_: LoopId,
        face_props: &FaceProps<O>,
    ) -> Result<()> {
        // init all values
        self.reset();

        // gather data (statuses ignored upstream: the callees return void)
        let edges = loop_edges(k, b, loop_)?;
        let edgeuses = loop_edgeuses(k, b, loop_)?;

        // set member values
        self.loop_ = Some(loop_);
        self.loopuse_type = loop_loopuse_type(b, loop_)?;

        // Get Loop Properties
        if self.loopuse_type == LoopuseType::Vertexuse {
            let point = loop_vertex_point(b, loop_)?;
            self.vertex_loop_on_pole = Tri::from(face_props.is_point3d_on_pole(point));
        }
        self.edge_cnt = edges.len() as u64;
        self.edgeuse_cnt = edgeuses.len() as u64;
        self.closed_3d = Tri::from(loop_is_closed_3d(k, b, loop_)?);
        self.closed_ptrs = Tri::from(loop_is_closed_ptrs(k, b, loop_)?);
        self.good_closed = Tri::from(self.closed_3d == self.closed_ptrs);

        // valid loops don't cross seams - they use them as part of the naturally
        // trimmed outer-loop boundary; an invalid loop crossing a surface seam is
        // missing a seam edge. How many times does the loop cross a U seam?
        let this = O::from_loop(loop_);
        if face_props.closed_u.is_true() {
            // get this loop's touch list for the U Seam classify crv
            let this_touches = get_connected_touches(Some(this), &face_props.done_touch_list_u);
            self.seam_crossing_cnt_u = (this_touches.len() as u64) / 2;
            let (cnt, orient) = get_winding_count(&this_touches)?;
            self.seam_winding_cnt_u = cnt;
            self.winding_orient_u = orient;
        } else {
            self.seam_crossing_cnt_u = 0;
            self.seam_winding_cnt_u = 0;
            self.winding_orient_u = OrientType::Unknown;
        }

        // How many times does the loop cross a V seam?
        if face_props.closed_v.is_true() {
            let this_touches = get_connected_touches(Some(this), &face_props.done_touch_list_v);
            self.seam_crossing_cnt_v = (this_touches.len() as u64) / 2;
            let (cnt, orient) = get_winding_count(&this_touches)?;
            self.seam_winding_cnt_v = cnt;
            self.winding_orient_v = orient;
        } else {
            self.seam_crossing_cnt_v = 0;
            self.seam_winding_cnt_v = 0;
            self.winding_orient_v = OrientType::Unknown;
        }

        // Is the loop an area or a no-area loop? (zero-area loops cross a seam an odd
        // number of times)
        self.area_loop =
            Tri::from(self.seam_crossing_cnt_u % 2 == 0 && self.seam_crossing_cnt_v % 2 == 0);
        Ok(())
    }

    /// `SmLoopProps::HasProblems`: whether the loop has any problem.
    #[must_use]
    pub fn has_problems(&self) -> bool {
        let mut b = false;
        b |= self.good_closed == Tri::False;
        b |= self.degen_loop == Tri::True;
        b |= self.area_loop == Tri::False;
        b |= self.seam_crossing_cnt_u > 0 || self.seam_crossing_cnt_v > 0;
        b |= self.containment == ContainmentType::NestedLoopEven
            || self.containment == ContainmentType::NestedLoopOdd;
        b |= self.good_orient == Tri::False;
        b
    }
}

// ---- SmLooptreeItem -------------------------------------------------------------------

/// Identifies a [`LooptreeItem`] within its [`Looptree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ItemId(pub usize);

/// One node of the loop containment tree (`SmLooptreeItem`): an area loop, or a pair of
/// no-area loops (a loop and its partner), with the first child and next sibling.
/// `loop_props` index [`FaceProps::loop_props`]; `classify_loop_io` index the records of
/// `SmFace::ClassifyLoops`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LooptreeItem {
    /// `m_bPoleVertex`: the item is a pole vertex (no loop).
    pub pole_vertex: bool,
    /// `m_pLoop`.
    pub loop_: Option<LoopId>,
    /// `m_pLoopProps`.
    pub loop_props: Option<usize>,
    /// `m_pClassifyLoopIO`.
    pub classify_loop_io: Option<usize>,
    /// `m_bPartnerPoleVertex`.
    pub partner_pole_vertex: bool,
    /// `m_pPartnerLoop`.
    pub partner_loop: Option<LoopId>,
    /// `m_pPartnerLoopProps`.
    pub partner_loop_props: Option<usize>,
    /// `m_pPartnerClassifyLoopIO`.
    pub partner_classify_loop_io: Option<usize>,
    /// `m_pSibling`.
    pub sibling: Option<ItemId>,
    /// `m_pChild`.
    pub child: Option<ItemId>,
}

impl LooptreeItem {
    /// `SmLooptreeItem()` (`ReSetNoRecursion`): every value unset.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pole_vertex: false,
            loop_: None,
            loop_props: None,
            classify_loop_io: None,
            partner_pole_vertex: false,
            partner_loop: None,
            partner_loop_props: None,
            partner_classify_loop_io: None,
            sibling: None,
            child: None,
        }
    }

    /// `SmLooptreeItem(pLoop, pLoopProps, pClassifyLoopIO)`: an area loop or the first
    /// of a no-area pair (`None` loop: a pole vertex); no child and no sibling.
    #[must_use]
    pub const fn with_loop(
        loop_: Option<LoopId>,
        loop_props: Option<usize>,
        classify_loop_io: Option<usize>,
    ) -> Self {
        let mut i = Self::new();
        i.pole_vertex = loop_.is_none();
        i.loop_ = loop_;
        i.loop_props = loop_props;
        i.classify_loop_io = classify_loop_io;
        i
    }

    /// The no-area pair constructor of `SmLooptreeItem`: the lower parameter loop and
    /// its upper parameter partner (`None`: a pole vertex).
    #[must_use]
    pub const fn with_partner(
        loop_: Option<LoopId>,
        loop_props: Option<usize>,
        classify_loop_io: Option<usize>,
        partner_loop: Option<LoopId>,
        partner_loop_props: Option<usize>,
        partner_classify_loop_io: Option<usize>,
    ) -> Self {
        let mut i = Self::with_loop(loop_, loop_props, classify_loop_io);
        i.partner_pole_vertex = partner_loop.is_none();
        i.partner_loop = partner_loop;
        i.partner_loop_props = partner_loop_props;
        i.partner_classify_loop_io = partner_classify_loop_io;
        i
    }

    /// `ReSetNoRecursion`: every value of this item unset.
    pub fn reset_no_recursion(&mut self) {
        *self = Self::new();
    }

    /// `IsNoArea_Pair`: the item holds a partner loop (or partner pole vertex).
    #[must_use]
    pub const fn is_no_area_pair(&self) -> bool {
        self.partner_loop.is_some() || self.partner_pole_vertex
    }

    /// `AddPartner`: adds the partner data (`None` loop: a pole vertex).
    pub fn add_partner(
        &mut self,
        partner_loop: Option<LoopId>,
        partner_props: Option<usize>,
        partner_io: Option<usize>,
    ) {
        self.partner_pole_vertex = partner_loop.is_none();
        self.partner_loop = partner_loop;
        self.partner_loop_props = partner_props;
        self.partner_classify_loop_io = partner_io;
    }

    /// The match test of `FindItem`: a pole vertex for a `None` loop, or the loop (or
    /// its partner).
    fn matches(&self, loop_: Option<LoopId>) -> bool {
        (self.pole_vertex && loop_.is_none())
            || self.loop_ == loop_
            || (self.partner_pole_vertex && loop_.is_none())
            || self.partner_loop == loop_
    }
}

/// What [`Looptree::find_item`] found: the item and its parent (`None`: the item is in
/// the top sibling list of the tree, parent is the root level).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoundItem {
    /// The item found.
    pub item: ItemId,
    /// `rpParent`: the parent of the item, `None` when the search started at its
    /// level.
    pub parent: Option<ItemId>,
}

/// The loop containment tree (`m_pLooptreeRoot` and the items it reaches): an arena of
/// [`LooptreeItem`] linked by child and sibling ids.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Looptree {
    items: Vec<LooptreeItem>,
    root: Option<ItemId>,
}

impl Looptree {
    /// An empty tree.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            items: Vec::new(),
            root: None,
        }
    }

    /// Adds `item` to the arena (the first one becomes the root) and returns its id.
    pub fn push(&mut self, item: LooptreeItem) -> ItemId {
        let id = ItemId(self.items.len());
        self.items.push(item);
        if self.root.is_none() {
            self.root = Some(id);
        }
        id
    }

    /// `m_pLooptreeRoot`.
    #[must_use]
    pub const fn root(&self) -> Option<ItemId> {
        self.root
    }

    /// The item `id`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an id outside the tree.
    pub fn item(&self, id: ItemId) -> Result<&LooptreeItem> {
        self.items
            .get(id.0)
            .ok_or_else(|| null(OP_TREE, "item out of range"))
    }

    /// The item `id`, mutable.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an id outside the tree.
    pub fn item_mut(&mut self, id: ItemId) -> Result<&mut LooptreeItem> {
        self.items
            .get_mut(id.0)
            .ok_or_else(|| null(OP_TREE, "item out of range"))
    }

    /// The number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the tree has no item.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// `AddChild`: makes `new_child` the first child of `parent` (its sibling the old
    /// first child).
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an id outside the tree.
    pub fn add_child(&mut self, parent: ItemId, new_child: ItemId) -> Result<()> {
        let old_child = self.item(parent)?.child;
        self.item_mut(new_child)?.sibling = old_child;
        self.item_mut(parent)?.child = Some(new_child);
        Ok(())
    }

    /// `AddSibling`: inserts `new_sibling` after `this` in its sibling list.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an id outside the tree.
    pub fn add_sibling(&mut self, this: ItemId, new_sibling: ItemId) -> Result<()> {
        let old_sibling = self.item(this)?.sibling;
        self.item_mut(new_sibling)?.sibling = old_sibling;
        self.item_mut(this)?.sibling = Some(new_sibling);
        Ok(())
    }

    /// `FindItem`: the item of `start`'s sibling list and descendants that holds `loop_`
    /// (or its partner; `None` finds a pole vertex), and its parent. Visits `start`,
    /// then its siblings in order, then the children of the last sibling back to
    /// `start`'s, as the recursion upstream does.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for an id outside the tree, a sibling list that
    /// loops, or children nested deeper than [`MAX_LOOPTREE_DEPTH`].
    pub fn find_item(&self, start: ItemId, loop_: Option<LoopId>) -> Result<Option<FoundItem>> {
        self.find_item_at(start, loop_, 0)
    }

    fn find_item_at(
        &self,
        start: ItemId,
        loop_: Option<LoopId>,
        depth: usize,
    ) -> Result<Option<FoundItem>> {
        if depth > MAX_LOOPTREE_DEPTH {
            return Err(null(OP_TREE, "loop tree nested too deeply"));
        }

        // the sibling list from `start`: each item is tested before its siblings
        let mut chain: Vec<ItemId> = Vec::new();
        let mut at = Some(start);
        while let Some(id) = at {
            if chain.len() > self.items.len() {
                return Err(null(OP_TREE, "sibling list loops"));
            }
            let item = self.item(id)?;
            chain.push(id);
            if item.matches(loop_) {
                // found the item: parent unset (NULL) at this level
                return Ok(Some(FoundItem {
                    item: id,
                    parent: None,
                }));
            }
            at = item.sibling;
        }

        // then the children, from the last sibling back to `start`
        for &id in chain.iter().rev() {
            if let Some(child) = self.item(id)?.child {
                if let Some(found) = self.find_item_at(child, loop_, depth + 1)? {
                    // found the item in the child subtree
                    return Ok(Some(FoundItem {
                        item: found.item,
                        parent: found.parent.or(Some(id)),
                    }));
                }
            }
        }

        // not found here or in the child and sibling subtrees
        Ok(None)
    }
}

// ---- unported callees -----------------------------------------------------------------

/// `SmLoop::GetEdges(sEdges)`.
fn loop_edges(_k: &Kernel, b: &Brep, l: LoopId) -> Result<Vec<Option<EdgeId>>> {
    loopuse::loop_edges(b, l)
}

/// `SmLoop::GetEdgeuses(sEdgeuses)`.
fn loop_edgeuses(_k: &Kernel, b: &Brep, l: LoopId) -> Result<Vec<EdgeuseId>> {
    loopuse::loop_edgeuses(b, l)
}

/// `SmLoop::IsClosed3d()` (upstream's default arguments: `bCheckAllGaps` `TRUE`, no
/// outputs); a loop without loop use is closed.
fn loop_is_closed_3d(k: &Kernel, b: &Brep, l: LoopId) -> Result<bool> {
    Ok(loopuse::loop_is_closed_3d(k, b, l, true)?.is_none_or(|c| c.closed))
}

/// `SmLoop::IsClosedPtrs()`.
fn loop_is_closed_ptrs(_k: &Kernel, b: &Brep, l: LoopId) -> Result<bool> {
    loopuse::loop_is_closed_ptrs(b, l)
}

/// `m_pFace->GetVertices(sVertices)` (default arguments: no attribute filter, all
/// loops; co-ported in `query.face_p2`).
fn face_vertices(b: &Brep, f: FaceId) -> Result<Vec<VertexId>> {
    crate::query::face::p2::face_vertices(b, f, false, None)
}

/// The outputs of `SmFace::HasSeamProblem` that `SetProps_Stage2` stores.
#[derive(Debug, Clone, PartialEq)]
pub struct SeamProblem {
    /// `reCrossedSeam`.
    pub crossed: SurfParamType,
    /// `reMissingSeam`.
    pub missing: SurfParamType,
    /// `reNearMissSeam`.
    pub near_miss: SurfParamType,
}

/// `SmFace::HasSeamProblem` as `SetProps_Stage2` calls it (see
/// [`FaceProps::set_props_stage2_with`]): the face's seam classifications into `cu`,
/// `cv`, its raw and done touch lists into `props`, and the seam problem.
///
/// `C` is the caller's view of the face's B-rep ([`FaceBrep`]): `&Brep`, the B-rep
/// itself (`&mut Brep`) or classification objects over it.
pub type SeamProblemFn<'f, C, O> = dyn FnMut(
        &Kernel,
        &mut C,
        FaceId,
        &mut CurveClassification<O>,
        &mut CurveClassification<O>,
        &mut FaceProps<O>,
    ) -> Result<SeamProblem>
    + 'f;

/// The B-rep [`FaceProps::set_props_with_seam`] runs on: a private copy of it
/// (`Copy`, the temporary deviation of `query::face::p2`) or the B-rep itself.
enum PropsBrep<'a> {
    Copy(&'a Brep),
    InPlace(&'a mut Brep),
}

impl PropsBrep<'_> {
    fn brep(&self) -> &Brep {
        match self {
            Self::Copy(b) => b,
            Self::InPlace(b) => b,
        }
    }
}

/// `m_pFace->HasSeamProblem(*m_pCrvClassU, *m_pCrvClassV, crossed, missing, nearMiss,
/// FALSE, &m_bClosedU, &m_bClosedV, this)` for the callers of
/// [`FaceProps::set_props_stage2`], which hold no classification objects: the face's own
/// B-rep objects ([`crate::topology::face::p4::has_seam_problem_in_brep`]; callers that
/// have their own objects use [`FaceProps::set_props_stage2_with`]). An `UNSURE` closed
/// flag reads as `TRUE`; the returned flag is dropped, as upstream.
fn face_has_seam_problem<O: LoopObject>(
    k: &Kernel,
    b: &Brep,
    f: FaceId,
    class_u: &mut CurveClassification<O>,
    class_v: &mut CurveClassification<O>,
    props: &mut FaceProps<O>,
    cache: &mut CurveOnClassifyCache,
) -> Result<SeamProblem> {
    let (closed_u, closed_v) = (props.closed_u != Tri::False, props.closed_v != Tri::False);
    let check = crate::topology::face::p4::has_seam_problem_in_brep_cached(
        k,
        b,
        f,
        crate::topology::face::p4::HasSeamProblemOptions {
            outer_loop_only: false,
            periodic_u: Some(closed_u),
            periodic_v: Some(closed_v),
            face_props: Some(props),
        },
        cache,
    )?;
    *class_u = check.class_u;
    *class_v = check.class_v;
    Ok(SeamProblem {
        crossed: check.problem.crossed,
        missing: check.problem.missing,
        near_miss: check.problem.near_miss,
    })
}

/// [`face_has_seam_problem`] on the face's B-rep in place
/// ([`crate::topology::face::p4::has_seam_problem_in_brep_mut`]).
fn face_has_seam_problem_mut<O: LoopObject>(
    k: &Kernel,
    b: &mut Brep,
    f: FaceId,
    class_u: &mut CurveClassification<O>,
    class_v: &mut CurveClassification<O>,
    props: &mut FaceProps<O>,
) -> Result<SeamProblem> {
    let (closed_u, closed_v) = (props.closed_u != Tri::False, props.closed_v != Tri::False);
    let check = crate::topology::face::p4::has_seam_problem_in_brep_mut(
        k,
        b,
        f,
        crate::topology::face::p4::HasSeamProblemOptions {
            outer_loop_only: false,
            periodic_u: Some(closed_u),
            periodic_v: Some(closed_v),
            face_props: Some(props),
        },
    )?;
    *class_u = check.class_u;
    *class_v = check.class_v;
    Ok(SeamProblem {
        crossed: check.problem.crossed,
        missing: check.problem.missing,
        near_miss: check.problem.near_miss,
    })
}

/// `SmCurveClassification::ReSet()` of an existing seam classification.
fn curve_classification_reset<O: LoopObject>(
    k: &Kernel,
    c: &mut CurveClassification<O>,
) -> Result<()> {
    c.reset(k, &ResetOptions::default())
}

/// The out-params of `SmFace::ClassifyLoops` ([`crate::query::face::p4::classify_loops`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifyLoopsOut {
    /// `reClassifiable`.
    pub classifiable: HealNotYetFaceType,
    /// `rClassifyLoopIOs`: one record per target loop.
    pub classify_loop_ios: Vec<ClassifyLoopIo>,
    /// `reClassifyCrv_StartType`.
    pub start_type: CrvDirOnFaceType,
    /// `reClassifyCrv_StopType`.
    pub stop_type: CrvDirOnFaceType,
    /// `rOrderedNoAreaIndices`: the no-area loops, lowest domain to highest.
    pub ordered_no_area_indices: Vec<usize>,
    /// `rpLooptreeRoot`.
    pub looptree: Option<Looptree>,
}

impl Default for ClassifyLoopsOut {
    /// The values `SetProps_Stage3` initialises them with: no problem, no records,
    /// `SM_CD_UNINIT` types, no tree.
    fn default() -> Self {
        Self {
            classifiable: HealNotYetFaceType::NoProbs,
            classify_loop_ios: Vec::new(),
            start_type: CrvDirOnFaceType::Uninit,
            stop_type: CrvDirOnFaceType::Uninit,
            ordered_no_area_indices: Vec::new(),
            looptree: None,
        }
    }
}

/// `pLoop->GetLoopuse()->GetLoopuseType()`.
fn loop_loopuse_type(b: &Brep, l: LoopId) -> Result<LoopuseType> {
    let Some(lu) = b.loop_(l)?.loopuse else {
        return Err(null(OP_LOOP, "loop without loop use"));
    };
    Ok(match b.loopuse(lu)?.start {
        LoopuseStart::Unknown => LoopuseType::Unknown,
        LoopuseStart::Edgeuse(_) => LoopuseType::Edgeuse,
        LoopuseStart::Vertexuse(_) => LoopuseType::Vertexuse,
    })
}

/// `pLoop->GetLoopuse()->GetVertexuse()->GetVertex()->GetPoint()`.
fn loop_vertex_point(b: &Brep, l: LoopId) -> Result<Point3> {
    let Some(lu) = b.loop_(l)?.loopuse else {
        return Err(null(OP_LOOP, "loop without loop use"));
    };
    let LoopuseStart::Vertexuse(Some(vu)) = b.loopuse(lu)?.start else {
        return Err(null(OP_LOOP, "loop use without vertex use"));
    };
    let vu: VertexuseId = vu;
    let Some(v) = b.vertexuse(vu)?.vertex else {
        return Err(null(OP_LOOP, "vertex use without vertex"));
    };
    vertex_mod::point(b, v)
}

// ---- SmFaceProps ----------------------------------------------------------------------

/// The cached properties of one face (`SmFaceProps`), built in three stages
/// ([`PropStage`]) as the healer runs: the gap dependent values
/// ([`FaceProps::set_props_gaps`]), the values independent of edge use data
/// ([`FaceProps::set_props_stage2`]) and the loop values
/// ([`FaceProps::set_props_stage3`]).
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // upstream's SmBoolean members, as tri-states
pub struct FaceProps<O: LoopObject> {
    /// `m_ePropStage`.
    pub prop_stage: PropStage,
    /// `m_bBadHasNotYetProb`.
    pub bad_has_not_yet_prob: Tri,
    /// `m_eNotYetFaceType`: the first unsupported problem found, if any.
    pub not_yet_face_type: HealNotYetFaceType,
    /// `m_bBadDegenFace`: the face is degenerate and will be squeezed.
    pub bad_degen_face: Tri,

    // SetProps(SM_PROPSTAGE_GAPS)
    /// `m_pFace`: the target face.
    pub face: Option<FaceId>,
    /// `m_lFaceIndx`: the face's index in the managing healer's face list.
    pub face_index: u64,
    /// `m_sZoneTol3d`: the tolerance as changed by the healer.
    pub zone_tol_3d: f64,
    /// `m_sOrigZoneTol3d`: the tolerance as given to the healer.
    pub orig_zone_tol_3d: f64,
    /// `m_bBadSmallZoneTol3d`: the original tolerance is below the desired one.
    pub bad_small_zone_tol_3d: Tri,
    /// `m_bBadLargeZoneTol3d`: the original tolerance is above the desired one.
    pub bad_large_zone_tol_3d: Tri,

    // SetProps(SM_PROPSTAGE_2)
    /// `m_cpSurface != NULL`: the face's surface has been read.
    pub has_surface: bool,
    /// `m_sNaturalUVDomain`: the natural domain of the face's surface.
    pub natural_uv_domain: crate::geom::extent2d::Extent2d,
    /// `m_eSheet`.
    pub sheet: MixedLinkType,
    /// `m_bClosedU`.
    pub closed_u: Tri,
    /// `m_bClosedV`.
    pub closed_v: Tri,
    /// `m_bClosedSurf`: a torus or a sphere.
    pub closed_surf: Tri,
    /// `m_eClosedValU`.
    pub closed_val_u: crate::geom::geom_utility::p2::ContinuityType,
    /// `m_eClosedValV`.
    pub closed_val_v: crate::geom::geom_utility::p2::ContinuityType,
    /// `m_ePoles`.
    pub poles_link: MixedLinkType,
    /// `m_lPoles`: `SM_SS_NONE` or an or-of the singular sides.
    pub poles: u64,
    /// `m_lApproxPoles`.
    pub approx_poles: u64,
    /// `m_lNoVertexPoles`: the poles no vertex marks.
    pub no_vertex_poles: u64,
    /// `m_sPolePoints`: `[UMinPole, VMinPole, UMaxPole, VMaxPole]`.
    pub pole_points: Vec<Point3>,
    /// `m_sPoleNormals`: the pole normals in the same order.
    pub pole_normals: Vec<Vec3>,
    /// `m_lBadFlatCorners`: `SM_FC_NONE` or an or-of the flat corners.
    pub bad_flat_corners: u64,
    /// `m_sFlatCornerPoints`: `[UMinVMin, UMinVMax, UMaxVMin, UMaxVMax]`.
    pub flat_corner_points: Vec<Point3>,
    /// `m_pCrvClassU`: the U seam classification (set when the surface is closed in U).
    pub crv_class_u: Option<CurveClassification<O>>,
    /// `m_sRawTouchListU`.
    pub raw_touch_list_u: Vec<TouchData<O>>,
    /// `m_sDoneTouchListU`.
    pub done_touch_list_u: Vec<TouchData<O>>,
    /// `m_pCrvClassV`.
    pub crv_class_v: Option<CurveClassification<O>>,
    /// `m_sRawTouchListV`.
    pub raw_touch_list_v: Vec<TouchData<O>>,
    /// `m_sDoneTouchListV`.
    pub done_touch_list_v: Vec<TouchData<O>>,
    /// `m_bBadCrossedPole`: one interior pole crossing is cached (after splitting,
    /// Stage 2 finds the next crossing).
    pub bad_crossed_pole: Tri,
    /// `m_pCrossedPoleEdge`: the boundary edge of the cached crossing.
    pub crossed_pole_edge: Option<EdgeId>,
    /// `m_dCrossedPoleParameter`: the split parameter on [`FaceProps::crossed_pole_edge`].
    pub crossed_pole_parameter: f64,
    /// `m_bBadCrossedSeam`.
    pub bad_crossed_seam: Tri,
    /// `m_eBadCrossedSeam`.
    pub bad_crossed_seam_dir: SurfParamType,
    /// `m_bBadMissingSeam`.
    pub bad_missing_seam: Tri,
    /// `m_eBadMissingSeam`.
    pub bad_missing_seam_dir: SurfParamType,
    /// `m_bBadNearMissSeam`.
    pub bad_near_miss_seam: Tri,
    /// `m_eBadNearMissSeam`.
    pub bad_near_miss_seam_dir: SurfParamType,

    /// `m_eBeenThroughMoveSeam`.
    pub been_through_move_seam: TriedLinkType,
    /// `m_eBeenThroughSplitEdgesAtSeam`.
    pub been_through_split_edges_at_seam: TriedLinkType,
    /// `m_eBeenThroughSplitFacesAtSeam`.
    pub been_through_split_faces_at_seam: TriedLinkType,

    // SetProps(SM_PROPSTAGE_3)
    /// `m_pLooptreeRoot`: the loop containment tree.
    pub looptree: Option<Looptree>,
    /// `m_sLoopProps`: one per face loop.
    pub loop_props: Vec<LoopProps>,
    /// `m_lClosedLoopCnt`.
    pub closed_loop_cnt: u64,
    /// `m_lOuterLoopCnt`: one is required, more are bad.
    pub outer_loop_cnt: u64,
    /// `m_bBadLoopProblems`: the face has at least one loop problem.
    pub bad_loop_problems: Tri,
    /// `m_bBadOuterLoopOrder`: the first loop is not an outer loop.
    pub bad_outer_loop_order: Tri,
    /// `m_lBadOrient_LoopCnt`.
    pub bad_orient_loop_cnt: u64,
    /// `m_lBadNoArea_LoopCnt`.
    pub bad_no_area_loop_cnt: u64,
    /// `m_lBadNested_LoopCnt`.
    pub bad_nested_loop_cnt: u64,
    /// `m_bBadUnpairedNoArea_Loop`.
    pub bad_unpaired_no_area_loop: Tri,
    /// `m_lBadClosed3d_LoopCnt`.
    pub bad_closed_3d_loop_cnt: u64,
    /// `m_lBadClosedPtr_LoopCnt`.
    pub bad_closed_ptr_loop_cnt: u64,
    /// `m_lBadMissingPoles`.
    pub bad_missing_poles: u64,
}

impl<O: LoopObject> Default for FaceProps<O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: LoopObject> FaceProps<O> {
    /// `SmFaceProps(cpContext)`: `ReSet(SM_PROPSTAGE_0)` of a props object with no
    /// face.
    #[must_use]
    pub fn new() -> Self {
        let mut p = Self {
            prop_stage: PropStage::Stage0,
            bad_has_not_yet_prob: Tri::False,
            not_yet_face_type: HealNotYetFaceType::NoProbs,
            bad_degen_face: Tri::Unsure,
            face: None,
            face_index: UNDEF_ULONG,
            zone_tol_3d: crate::math::UNDEF_DOUBLE,
            orig_zone_tol_3d: crate::math::UNDEF_DOUBLE,
            bad_small_zone_tol_3d: Tri::Unsure,
            bad_large_zone_tol_3d: Tri::Unsure,
            has_surface: false,
            natural_uv_domain: crate::geom::extent2d::Extent2d::new(),
            sheet: MixedLinkType::Undef,
            closed_u: Tri::Unsure,
            closed_v: Tri::Unsure,
            closed_surf: Tri::Unsure,
            closed_val_u: crate::geom::geom_utility::p2::ContinuityType::Undefined,
            closed_val_v: crate::geom::geom_utility::p2::ContinuityType::Undefined,
            poles_link: MixedLinkType::Undef,
            poles: UNDEF_ULONG,
            approx_poles: UNDEF_ULONG,
            no_vertex_poles: UNDEF_ULONG,
            pole_points: Vec::new(),
            pole_normals: Vec::new(),
            bad_flat_corners: UNDEF_ULONG,
            flat_corner_points: Vec::new(),
            crv_class_u: None,
            raw_touch_list_u: Vec::new(),
            done_touch_list_u: Vec::new(),
            crv_class_v: None,
            raw_touch_list_v: Vec::new(),
            done_touch_list_v: Vec::new(),
            bad_crossed_pole: Tri::False,
            crossed_pole_edge: None,
            crossed_pole_parameter: 0.0,
            bad_crossed_seam: Tri::False,
            bad_crossed_seam_dir: SurfParamType::Unknown,
            bad_missing_seam: Tri::False,
            bad_missing_seam_dir: SurfParamType::Unknown,
            bad_near_miss_seam: Tri::False,
            bad_near_miss_seam_dir: SurfParamType::Unknown,
            been_through_move_seam: TriedLinkType::None,
            been_through_split_edges_at_seam: TriedLinkType::None,
            been_through_split_faces_at_seam: TriedLinkType::None,
            looptree: None,
            loop_props: Vec::new(),
            closed_loop_cnt: UNDEF_ULONG,
            outer_loop_cnt: UNDEF_ULONG,
            bad_loop_problems: Tri::Unsure,
            bad_outer_loop_order: Tri::Unsure,
            bad_orient_loop_cnt: UNDEF_ULONG,
            bad_no_area_loop_cnt: UNDEF_ULONG,
            bad_nested_loop_cnt: UNDEF_ULONG,
            bad_unpaired_no_area_loop: Tri::Unsure,
            bad_closed_3d_loop_cnt: UNDEF_ULONG,
            bad_closed_ptr_loop_cnt: UNDEF_ULONG,
            bad_missing_poles: UNDEF_ULONG,
        };
        // init all member values to uninit values
        p.reset(PropStage::Stage0);
        p
    }

    /// `SmFaceProps(pFace, lFaceIndx, cpContext)`: [`FaceProps::new`] with the face and
    /// its index set.
    #[must_use]
    pub fn for_face(face: FaceId, face_index: u64) -> Self {
        let mut p = Self::new();
        p.face = Some(face);
        p.face_index = face_index;
        p
    }

    /// `IsPeriodicU`: closed in U with at least C0 continuity across the seam.
    #[must_use]
    pub fn is_periodic_u(&self) -> bool {
        self.closed_u.is_true() && (self.closed_val_u as i32) >= 2
    }

    /// `IsPeriodicV`.
    #[must_use]
    pub fn is_periodic_v(&self) -> bool {
        self.closed_v.is_true() && (self.closed_val_v as i32) >= 2
    }

    /// `HasNotYetProblems`.
    #[must_use]
    pub fn has_not_yet_problems(&self) -> bool {
        self.not_yet_face_type != HealNotYetFaceType::NoProbs
    }

    /// `ReSet`: clears all values from stage `clear` on (`SM_PROPSTAGE_0` everything,
    /// `SM_PROPSTAGE_GAPS` the gap values and later, `SM_PROPSTAGE_2` the stage 2
    /// values and later, `SM_PROPSTAGE_3` the stage 3 values).
    pub fn reset(&mut self, clear: PropStage) {
        use crate::geom::geom_utility::p2::ContinuityType;

        // Clear General Properties
        self.prop_stage = if clear <= PropStage::Stage0 || clear <= PropStage::Gaps {
            PropStage::Stage0
        } else if clear <= PropStage::Stage2 {
            PropStage::Gaps
        } else if clear <= PropStage::Stage3 {
            PropStage::Stage2
        } else {
            self.prop_stage
        };
        self.bad_has_not_yet_prob = Tri::False;
        self.not_yet_face_type = HealNotYetFaceType::NoProbs;
        self.bad_degen_face = Tri::Unsure;

        // when asked - clear SetState1 _Props values
        if clear <= PropStage::Gaps {
            self.face = None;
            self.face_index = UNDEF_ULONG;
            self.zone_tol_3d = crate::math::UNDEF_DOUBLE;
            self.orig_zone_tol_3d = crate::math::UNDEF_DOUBLE;
            self.bad_small_zone_tol_3d = Tri::Unsure;
            self.bad_large_zone_tol_3d = Tri::Unsure;
        }

        // when asked - clear SetProps_BeforeSplitEdge values
        if clear <= PropStage::Stage2 {
            self.has_surface = false;
            self.natural_uv_domain.init();
            self.sheet = MixedLinkType::Undef;
            self.closed_u = Tri::Unsure;
            self.closed_v = Tri::Unsure;
            self.closed_surf = Tri::Unsure;
            self.closed_val_u = ContinuityType::Undefined;
            self.closed_val_v = ContinuityType::Undefined;

            // Singular pFace->Surface classification
            self.poles_link = MixedLinkType::Undef;
            self.poles = UNDEF_ULONG;
            self.approx_poles = UNDEF_ULONG;
            self.no_vertex_poles = UNDEF_ULONG;
            self.pole_points = vec![Vec3::UNDEF; 4];
            self.pole_normals = vec![Vec3::UNDEF; 4];
            self.bad_flat_corners = UNDEF_ULONG;
            self.flat_corner_points = vec![Vec3::UNDEF; 4];

            // when pFace->Surface is closed the seam classifications are set by
            // SetProps_Stage2
            if self.crv_class_u.take().is_some() {
                self.raw_touch_list_u.clear();
                self.done_touch_list_u.clear();
            }
            if self.crv_class_v.take().is_some() {
                self.raw_touch_list_v.clear();
                self.done_touch_list_v.clear();
            }
            self.bad_crossed_pole = Tri::False;
            self.crossed_pole_edge = None;
            self.crossed_pole_parameter = 0.0;
            self.bad_crossed_seam = Tri::False;
            self.bad_crossed_seam_dir = SurfParamType::Unknown;
            self.bad_missing_seam = Tri::False;
            self.bad_missing_seam_dir = SurfParamType::Unknown;
            self.bad_near_miss_seam = Tri::False;
            self.bad_near_miss_seam_dir = SurfParamType::Unknown;

            // Stage 3 loop classification must retain the earlier crossing-repair result.
            self.been_through_split_edges_at_seam = TriedLinkType::None;
        }

        self.been_through_move_seam = TriedLinkType::None;
        self.been_through_split_faces_at_seam = TriedLinkType::None;

        // when asked - clear SetProps_AfterSplitEdge values
        if clear <= PropStage::Stage3 {
            self.looptree = None;
            self.loop_props.clear();
            self.closed_loop_cnt = UNDEF_ULONG;
            self.outer_loop_cnt = UNDEF_ULONG;
            self.bad_outer_loop_order = Tri::Unsure;
            self.bad_orient_loop_cnt = UNDEF_ULONG;
            self.bad_no_area_loop_cnt = UNDEF_ULONG;
            self.bad_nested_loop_cnt = UNDEF_ULONG;
            self.bad_unpaired_no_area_loop = Tri::Unsure;
            self.bad_closed_3d_loop_cnt = UNDEF_ULONG;
            self.bad_closed_ptr_loop_cnt = UNDEF_ULONG;
            self.bad_missing_poles = UNDEF_ULONG;
        }
    }

    /// `HasProps_Stage1` (the asserts are compiled out): the gap values have been set.
    #[must_use]
    pub fn has_props_stage1(&self, _face: Option<FaceId>) -> bool {
        self.prop_stage >= PropStage::Gaps
    }

    /// `HasProps_Stage2`: the stage 2 values have been set for `face`.
    #[must_use]
    pub fn has_props_stage2(&self, face: Option<FaceId>) -> bool {
        self.prop_stage >= PropStage::Stage2 && self.face == face
    }

    /// `HasProps_Stage3`: the stage 3 values have been set for `face`.
    #[must_use]
    pub fn has_props_stage3(&self, face: Option<FaceId>) -> bool {
        self.prop_stage >= PropStage::Stage3 && self.face == face
    }

    /// `HasProps(ePropStage, cpFace)`.
    #[must_use]
    pub fn has_props(&self, stage: PropStage, face: Option<FaceId>) -> bool {
        match stage {
            PropStage::Gaps => self.has_props_stage1(face),
            PropStage::Stage2 => self.has_props_stage2(face),
            PropStage::Stage3 => self.has_props_stage3(face),
            _ => self.face == face,
        }
    }

    /// `SetProps`: sets the properties through `stage`, calling [`FaceProps::set_props_gaps`],
    /// [`FaceProps::set_props_stage2`] and [`FaceProps::set_props_stage3`] as needed. The
    /// result is the status of the last stage call made.
    ///
    /// # Errors
    ///
    /// The status of the last stage call; [`SolidError::Unported`] from any of them at
    /// once.
    pub fn set_props(
        &mut self,
        k: &Kernel,
        b: &Brep,
        stage: PropStage,
        face: FaceId,
        face_index: u64,
        exe_heal_op: HealerOpType,
    ) -> Result<()> {
        let mut cache = CurveOnClassifyCache::default();
        self.set_props_with_seam(
            k,
            &mut PropsBrep::Copy(b),
            stage,
            face,
            face_index,
            exe_heal_op,
            &mut cache,
        )
    }

    /// The healer's `SetProps` call on its B-rep: what the lazy face queries leave on the
    /// B-rep (the UV trim curves `HasSeamProblem` and `ClassifyLoops`' bisectors make,
    /// the tolerance growth and the gap caches) stays on `b`, as upstream's `const` face
    /// queries leave it. `SetProps_Stage2`'s seam check runs on `b` in place; stage 3
    /// still runs on a copy whose edits are copied back (the temporary deviation of
    /// `query::face::p2`), even if a query fails after invalidating a sibling's cache.
    pub fn set_props_with_query_gaps(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        stage: PropStage,
        face: FaceId,
        face_index: u64,
        exe_heal_op: HealerOpType,
    ) -> Result<()> {
        let mut cache = CurveOnClassifyCache::default();
        let result = self.set_props_with_seam(
            k,
            &mut PropsBrep::InPlace(&mut *b),
            stage,
            face,
            face_index,
            exe_heal_op,
            &mut cache,
        );
        cache.apply_uv_trim_updates(b)?;
        result
    }

    fn set_props_with_seam(
        &mut self,
        k: &Kernel,
        b: &mut PropsBrep<'_>,
        stage: PropStage,
        face: FaceId,
        face_index: u64,
        exe_heal_op: HealerOpType,
        cache: &mut CurveOnClassifyCache,
    ) -> Result<()> {
        // init rtn
        let mut rtn: Result<()> = Ok(());
        let some_face = Some(face);

        // SetProps_Gaps
        if stage == PropStage::Gaps
            || (stage > PropStage::Gaps && !self.has_props(PropStage::Gaps, some_face))
        {
            rtn = self.set_props_gaps(k, b.brep(), face, face_index, exe_heal_op);
            keep_unported(&rtn)?;
        }

        // SetProps_Stage2
        if stage == PropStage::Stage2
            || (stage > PropStage::Stage2 && !self.has_props(PropStage::Stage2, some_face))
        {
            rtn = match b {
                PropsBrep::Copy(b) => self.set_props_stage2_with(
                    k,
                    b,
                    face,
                    face_index,
                    exe_heal_op,
                    &mut |k, b, f, cu, cv, p| {
                        face_has_seam_problem(k, b, f, cu, cv, p, &mut *cache)
                    },
                ),
                PropsBrep::InPlace(b) => self.set_props_stage2_with(
                    k,
                    b,
                    face,
                    face_index,
                    exe_heal_op,
                    &mut |k, b, f, cu, cv, p| face_has_seam_problem_mut(k, b, f, cu, cv, p),
                ),
            };
            keep_unported(&rtn)?;
        }

        // SetProps_Stage3
        if stage == PropStage::Stage3
            || (stage > PropStage::Stage3 && !self.has_props(PropStage::Stage3, some_face))
        {
            rtn = self.set_props_stage3_cached(k, b.brep(), face, face_index, exe_heal_op, cache);
            keep_unported(&rtn)?;
        }

        // all done
        rtn
    }

    /// `SetProps_Gaps`: sets the face, its index, the zone tolerances and the bad
    /// tolerance flags (after `Fix_CoinEdges`).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling face id.
    pub fn set_props_gaps(
        &mut self,
        k: &Kernel,
        b: &Brep,
        face: FaceId,
        face_index: u64,
        _exe_heal_op: HealerOpType,
    ) -> Result<()> {
        // no work - already done or being done
        if self.prop_stage >= PropStage::GapsA {
            return Ok(());
        }

        // Init props about to be computed - also resets SM_PROPSTAGE_2 and 3 values
        self.reset(PropStage::Gaps);

        // increment PropState here - prevent possible infinite loops
        self.prop_stage = PropStage::GapsA;

        // locals and queries
        self.face = Some(face); // forward ptr: FaceProps->Face
        self.face_index = face_index;

        // SmTol::GetZoneTol3d(m_pFace): the face's tolerance (SM_USE_OLDTOL)
        self.zone_tol_3d = face_p2::tolerance(b, face)?;
        if self.orig_zone_tol_3d == crate::math::UNDEF_DOUBLE {
            self.orig_zone_tol_3d = self.zone_tol_3d;
        }

        // CalcZone is the desired tolerance for an object with the current gaps
        let calc_zone_tol_3d = calc_face_zone_tol_3d(k, Some(&k.context));

        // ZoneTol3d for current Gap3d sizes
        self.bad_small_zone_tol_3d = Tri::from(self.zone_tol_3d <= calc_zone_tol_3d - EFF_ZERO);
        self.bad_large_zone_tol_3d = Tri::from(self.zone_tol_3d >= calc_zone_tol_3d + EFF_ZERO);

        // all done
        self.prop_stage = PropStage::Gaps;
        Ok(())
    }

    /// `SetProps_Stage2`: sets the surface values that do not depend on edge use data
    /// (after `Fix_TolSizes`): the natural domain, the sheet flag, the closed and pole
    /// classification, the flat corners and the seam problems.
    ///
    /// # Errors
    ///
    /// `SM_ERR` ([`SolidError::Upstream`]) when `exe_heal_op` is before
    /// [`HealerOpType::FixTolsizes`]; [`SolidError::InvalidInput`] for a face without
    /// surface or face use, and for a handle type that cannot name the face's B-rep
    /// objects (`SmFace::HasSeamProblem`, see the module docs); the errors of the
    /// surface queries and the seam check.
    pub fn set_props_stage2(
        &mut self,
        k: &Kernel,
        b: &Brep,
        face: FaceId,
        face_index: u64,
        exe_heal_op: HealerOpType,
    ) -> Result<()> {
        let mut cache = CurveOnClassifyCache::default();
        self.set_props_stage2_with(
            k,
            &mut &*b,
            face,
            face_index,
            exe_heal_op,
            &mut |k, b, f, cu, cv, p| face_has_seam_problem(k, b, f, cu, cv, p, &mut cache),
        )
    }

    /// [`FaceProps::set_props_stage2`] on the face's B-rep in place: what its seam check
    /// leaves on `b` (UV trim curves, the trimmed cache's tolerance growth, gap caches)
    /// stays there, as upstream's `SetProps_Stage2` leaves it.
    ///
    /// # Errors
    ///
    /// As [`FaceProps::set_props_stage2`].
    pub fn set_props_stage2_mut(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        face: FaceId,
        face_index: u64,
        exe_heal_op: HealerOpType,
    ) -> Result<()> {
        self.set_props_stage2_with(
            k,
            &mut &mut *b,
            face,
            face_index,
            exe_heal_op,
            &mut |k, b, f, cu, cv, p| face_has_seam_problem_mut(k, b, f, cu, cv, p),
        )
    }

    /// [`FaceProps::set_props_stage2`] with `seam` as its `SmFace::HasSeamProblem` call
    /// (`m_pFace->HasSeamProblem(*m_pCrvClassU, *m_pCrvClassV, .., FALSE, &m_bClosedU,
    /// &m_bClosedV, this)`, made at [`PropStage::Stage2A`]): a caller holding the
    /// classification objects passes [`crate::topology::face::p4::has_seam_problem`].
    ///
    /// # Errors
    ///
    /// As [`FaceProps::set_props_stage2`], with `seam`'s errors for its seam call.
    pub fn set_props_stage2_with<C: FaceBrep>(
        &mut self,
        k: &Kernel,
        b: &mut C,
        face: FaceId,
        face_index: u64,
        exe_heal_op: HealerOpType,
        seam: &mut SeamProblemFn<'_, C, O>,
    ) -> Result<()> {
        // no work - already done or being done
        if self.prop_stage >= PropStage::Stage2A {
            return Ok(());
        }

        // no work - called before SmHealData::Fix_TolSizes() has run on this Brep
        if exe_heal_op < HealerOpType::FixTolsizes {
            return Err(SolidError::sm_err(OP_STAGE2));
        }

        // extra work - get props that need to be built before these props
        // (UB-131: for a stage at or past GapsA the call returns at once, so the re-run
        // that `self.face != Some(face)` asks for never happens; upstream then reads
        // `m_pFace` unchecked below. Here a missing face is `InvalidInput`, and the
        // props keep the other face's values if the face differs.)
        if self.prop_stage < PropStage::Gaps || self.face != Some(face) {
            self.set_props_gaps(k, b.brep(), face, face_index, exe_heal_op)?;
        }

        // Init props about to be computed - also resets SM_PROPSTAGE_3 values
        self.reset(PropStage::Stage2);

        // increment PropState here - prevent infinite loops
        self.prop_stage = PropStage::Stage2A;

        // manage contained memory - CrvClass U and V
        // (the classifications were deleted by the ReSet above; the else branches are
        // upstream's dead code)
        let mut class_u = match self.crv_class_u.take() {
            None => CurveClassification::new(k),
            Some(mut c) => {
                curve_classification_reset(k, &mut c)?;
                c
            }
        };
        class_u.set_ok_to_set_tolerance(false);
        let mut class_v = match self.crv_class_v.take() {
            None => CurveClassification::new(k),
            Some(mut c) => {
                curve_classification_reset(k, &mut c)?;
                c
            }
        };
        class_v.set_ok_to_set_tolerance(false);

        // locals and queries
        let xsect_tol_3d = xsect_tol_3d_from_zones(k, self.zone_tol_3d, self.zone_tol_3d);
        let Some(m_face) = self.face else {
            return Err(null(OP_STAGE2, "no face"));
        };
        let Some(surface) = face_p2::surface(b.brep(), m_face)? else {
            return Err(null(OP_STAGE2, "face without surface"));
        };
        self.has_surface = true;
        self.natural_uv_domain = surface.natural_uv_domain_with_nurb(k)?;

        let (up_faceuse, down_faceuse) = face_p2::faceuses(b.brep(), m_face)?;
        let (Some(up_faceuse), Some(down_faceuse)) = (up_faceuse, down_faceuse) else {
            return Err(null(OP_STAGE2, "face without face uses"));
        };

        // Major Face->Surface properties that force the face to be checked for
        // problems and healing
        self.sheet = if eu1::faceuse_shell(b.brep(), up_faceuse)?
            == eu1::faceuse_shell(b.brep(), down_faceuse)?
        {
            MixedLinkType::HasProp
        } else {
            MixedLinkType::NoProp
        };

        // Closed pFace->Surface classification: m_bClosedU, m_bClosedV, m_eClosedValU,
        // m_eClosedValV
        let (closed_u, val_u) = surface.is_closed(
            k,
            &self.natural_uv_domain,
            SurfParamType::U,
            Some(xsect_tol_3d),
            true,
        )?;
        self.closed_u = Tri::from(closed_u);
        if let Some(c) = val_u {
            self.closed_val_u = c;
        }
        let (closed_v, val_v) = surface.is_closed(
            k,
            &self.natural_uv_domain,
            SurfParamType::V,
            Some(xsect_tol_3d),
            true,
        )?;
        self.closed_v = Tri::from(closed_v);
        if let Some(c) = val_v {
            self.closed_val_v = c;
        }

        // Singularity classification: m_lPoles, m_lApproxPoles, m_sPolePoints,
        // m_sPoleNormals
        // (GetSingularities has no status; its evaluation errors other than Unported are
        // ignored inside, as upstream does)
        // (the approximation tolerance is half the face's zone tolerance,
        // `SmTol::GetApproxTol3d(this)` of the face's surface: passed with the face)
        let sing = surface.get_singularities_of_face(
            k,
            face_p2::tolerance(b.brep(), m_face)?,
            true,
            true,
        )?;
        self.poles = u64::from(sing.singular);
        self.pole_points = sing.pole_points.map(|p| p.to_vec()).unwrap_or_default();
        self.pole_normals = sing.pole_normals.map(|p| p.to_vec()).unwrap_or_default();
        self.approx_poles = u64::from(sing.approx_singular);

        // Closed pFace->Surface = tori or spheres
        let closed_surf = (closed_u && closed_v)
            || (closed_v && (self.poles & SS_UMIN) != 0 && (self.poles & SS_UMAX) != 0)
            || (closed_u && (self.poles & SS_VMIN) != 0 && (self.poles & SS_VMAX) != 0);
        self.closed_surf = Tri::from(closed_surf);

        // find any poles not marked by a vertex (this can be okay if the pole is not
        // 'in' or 'on the boundary' of the face): m_lNoVertexPoles
        if self.poles == SS_NONE {
            self.poles_link = MixedLinkType::NoProp;
        } else {
            let vertices = face_vertices(b.brep(), m_face)?;
            let mut found_vertices: u64 = 0;
            let zone_tol_3d_sq = self.zone_tol_3d * self.zone_tol_3d;
            let sides = [SS_UMIN, SS_VMIN, SS_UMAX, SS_VMAX];

            // for every pole
            for (ii, &side) in sides.iter().enumerate() {
                // no work - no pole
                if (self.poles & side) == 0 {
                    continue;
                }
                let pole = at(&self.pole_points, ii, OP_STAGE2)?;

                // for every vertex
                for &v in &vertices {
                    let dist_sq = vertex_mod::point(b.brep(), v)?.distance_squared(pole);

                    // note any poles that we find marked with a vertex
                    if dist_sq < zone_tol_3d_sq {
                        found_vertices |= side;
                        break;
                    }
                }
            }

            // set the missing pole value
            self.no_vertex_poles = !found_vertices & self.poles;
            self.poles_link = if self.no_vertex_poles == SS_NONE {
                MixedLinkType::Okay
            } else {
                MixedLinkType::Prob
            };
        }

        // Find a boundary edge passing through a pole, using defining 3D geometry.
        // Endpoints already on the pole do not need splitting. Cache one crossing:
        // the repair refreshes Stage 2 after each split before consuming another.
        self.bad_crossed_pole = Tri::False;
        self.crossed_pole_edge = None;
        self.crossed_pole_parameter = 0.0;
        if self.poles_link == MixedLinkType::Prob {
            self.find_crossed_pole(k, b.brep(), m_face)?;
        }

        // Flat Corner Surface Classification: m_lBadFlatCorners (GetFlatCorners with
        // the points array and the default angle tolerance SM_EFF_ZERO_DEG)
        let flat = surface.flat_corners(k, crate::math::EFF_ZERO_DEG, true)?;
        self.bad_flat_corners = u64::from(flat.flags);
        self.flat_corner_points = flat.points.map_or_else(Vec::new, |p| p.to_vec());

        // when pFace->Surface is closed the seam classifications, touch lists and the
        // bad seam directions are set by this SmFace::HasSeamProblem call
        if self.closed_u.is_true() || self.closed_v.is_true() {
            let seam = seam(k, b, m_face, &mut class_u, &mut class_v, self);
            self.crv_class_u = Some(class_u);
            self.crv_class_v = Some(class_v);
            let seam = seam?;
            self.bad_crossed_seam_dir = seam.crossed;
            self.bad_missing_seam_dir = seam.missing;
            self.bad_near_miss_seam_dir = seam.near_miss;

            // set the boolean error flags
            self.bad_missing_seam = Tri::from(self.bad_missing_seam_dir != SurfParamType::Neither);
            self.bad_crossed_seam = Tri::from(self.bad_crossed_seam_dir != SurfParamType::Neither);
            self.bad_near_miss_seam =
                Tri::from(self.bad_near_miss_seam_dir != SurfParamType::Neither);
        } else {
            self.crv_class_u = Some(class_u);
            self.crv_class_v = Some(class_v);
        }

        // all done
        self.prop_stage = PropStage::Stage2;
        Ok(())
    }

    /// `SetProps_Stage2`'s pole-crossing search: the first boundary edge of `face` whose
    /// interior passes through a pole no vertex marks, cached with its split parameter.
    fn find_crossed_pole(&mut self, k: &Kernel, b: &Brep, face: FaceId) -> Result<()> {
        let edges = face_p4::edges(b, face, None, false, None)?;
        let face_tol = zone_tol3d(k, face_p2::tolerance(b, face)?);
        let sides = [SS_UMIN, SS_VMIN, SS_UMAX, SS_VMAX];

        // for every pole
        for (pole_index, &pole) in self.pole_points.iter().enumerate() {
            if self.bad_crossed_pole.is_true() {
                break;
            }

            // no work unless pole is not marked with a vertex
            if sides
                .get(pole_index)
                .is_some_and(|&side| (self.no_vertex_poles & side) == 0)
            {
                continue;
            }

            // for every edge on this face
            for &edge in &edges {
                // loop locals
                let tol = xsect_tol_3d_from_zones(
                    k,
                    zone_tol3d(k, edge_mod::tolerance(b, edge)?),
                    face_tol,
                );
                let interval = edge_mod::interval(b, edge)?;

                let (start_vertex, end_vertex) = edge_mod::start_end_vertices(b, edge)?;
                let start_vertex =
                    start_vertex.ok_or_else(|| null(OP_STAGE2, "edge without start vertex"))?;
                let end_vertex =
                    end_vertex.ok_or_else(|| null(OP_STAGE2, "edge without end vertex"))?;
                let start_tol = xsect_tol_3d_from_zones(
                    k,
                    zone_tol3d(k, vertex_mod::tolerance(b, start_vertex)?),
                    face_tol,
                );
                let end_tol = xsect_tol_3d_from_zones(
                    k,
                    zone_tol3d(k, vertex_mod::tolerance(b, end_vertex)?),
                    face_tol,
                );

                if vertex_mod::point(b, start_vertex)?.distance(pole) <= start_tol
                    || vertex_mod::point(b, end_vertex)?.distance(pole) <= end_tol
                {
                    continue;
                }

                // Drop the pole to the edge curve. INTERSECT will be faster than MINIMIZE
                // by culling out points that are too far away. (`SE`: a failing status
                // is ignored, read as no drop point.)
                let curve = edge_mod::curve(b, edge)?
                    .ok_or_else(|| null(OP_STAGE2, "edge without curve"))?;
                let drop = ignored(curve.drop_point(
                    k,
                    &interval,
                    pole,
                    None,
                    tol,
                    None,
                    SolverOperation::Intersect,
                ))?
                .unwrap_or_default();

                if drop.success
                    && drop.distance <= tol
                    && drop.parameter > interval.min()
                    && drop.parameter < interval.max()
                {
                    self.bad_crossed_pole = Tri::True;
                    self.crossed_pole_edge = Some(edge);
                    self.crossed_pole_parameter = drop.parameter;
                    break;
                }
            }
        }
        Ok(())
    }

    /// `SetProps_Stage3`: sets the loop values (after `Fix_SplitEdgesAtSeam`): the
    /// per-loop props, the closed loop count, and from `SmFace::ClassifyLoops` the
    /// containment, orientation and the loop problem counts.
    /// By default a failed stage keeps its in-progress marker (`SM_PROPSTAGE_3a`), as
    /// upstream; with the opt-in `Kernel::fixes.heal_stage3_failures_propagate`
    /// (WR-266) it remains incomplete and can be retried.
    ///
    /// # Errors
    ///
    /// `SM_ERR` when `exe_heal_op` is before [`HealerOpType::FixSplitedgeAtseam`];
    /// `SM_ERR_NOTYET_HEAL_FACE` ([`SM_ERR_NOTYET_HEAL_FACE`]) for a face
    /// `ClassifyLoops` cannot classify; the errors of [`FaceProps::set_props_stage2`]
    /// and [`LoopProps::set_props`]; [`SolidError::InvalidInput`] where upstream reads
    /// outside an array; [`SolidError::Unported`] from `SmFace::ClassifyLoops` (with
    /// the opt-in `Kernel::fixes.heal_stage3_failures_propagate`, all its errors).
    pub fn set_props_stage3(
        &mut self,
        k: &Kernel,
        b: &Brep,
        face: FaceId,
        face_index: u64,
        exe_heal_op: HealerOpType,
    ) -> Result<()> {
        let mut cache = CurveOnClassifyCache::default();
        self.set_props_stage3_cached(k, b, face, face_index, exe_heal_op, &mut cache)
    }

    /// [`FaceProps::set_props_stage3`] on the face's B-rep: what `ClassifyLoops`' lazy
    /// queries leave (the loop bisectors' UV trim curves, the tolerance growth and the gap
    /// caches) is copied back to `b`, as upstream's `const` face queries leave it on the
    /// B-rep (the Stage 2 props it builds first still run on a copy, as in
    /// [`FaceProps::set_props_with_query_gaps`]).
    ///
    /// # Errors
    ///
    /// As [`FaceProps::set_props_stage3`]; the copy-back's lookup errors.
    pub fn set_props_stage3_mut(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        face: FaceId,
        face_index: u64,
        exe_heal_op: HealerOpType,
    ) -> Result<()> {
        let mut cache = CurveOnClassifyCache::default();
        let result = self.set_props_stage3_cached(k, b, face, face_index, exe_heal_op, &mut cache);
        cache.apply_uv_trim_updates(b)?;
        result
    }

    /// [`FaceProps::set_props_stage3`] on the query state `cache`: what `ClassifyLoops`'
    /// loop bisectors leave on its snapshot stays there for the caller to keep
    /// ([`CurveOnClassifyCache::apply_uv_trim_updates`]).
    pub(crate) fn set_props_stage3_cached(
        &mut self,
        k: &Kernel,
        b: &Brep,
        face: FaceId,
        face_index: u64,
        exe_heal_op: HealerOpType,
        cache: &mut CurveOnClassifyCache,
    ) -> Result<()> {
        let result = self.set_props_stage3_inner(k, b, face, face_index, exe_heal_op, cache);
        if k.fixes.heal_stage3_failures_propagate
            && result.is_err()
            && self.prop_stage == PropStage::Stage3A
        {
            // Keep diagnostics, but an unfinished stage is not a cached success.
            self.prop_stage = PropStage::Stage2;
        }
        result
    }

    fn set_props_stage3_inner(
        &mut self,
        k: &Kernel,
        b: &Brep,
        face: FaceId,
        face_index: u64,
        exe_heal_op: HealerOpType,
        cache: &mut CurveOnClassifyCache,
    ) -> Result<()> {
        // no work - already done or being done
        if self.prop_stage >= PropStage::Stage3A {
            return Ok(());
        }

        // no work - called before SmHealData::Fix_SplitEdgesAtSeam() has run
        if exe_heal_op < HealerOpType::FixSplitedgeAtseam {
            return Err(SolidError::sm_err(OP_STAGE3));
        }

        // extra work - get props that need to be built before these props
        if self.prop_stage < PropStage::Stage2 || self.face != Some(face) {
            self.set_props_stage2(k, b, face, face_index, exe_heal_op)?;
        }

        // Init props about to be computed - also resets any future higher stage values
        self.reset(PropStage::Stage3);

        // increment PropState here - prevent infinite loops
        self.prop_stage = PropStage::Stage3A;

        // Get Face->Loops
        let Some(m_face) = self.face else {
            return Err(null(OP_STAGE3, "no face"));
        };
        let loops = face_p2::loops(b, m_face, false, None)?;

        // for every Loop - set the loop props marked '*' upstream: SmLoopProps::SetProps
        // (the props '##' are set later from the ClassifyLoops results)
        let mut loop_props = vec![LoopProps::new(); loops.len()];
        let mut closed_loops: Vec<LoopId> = Vec::new();
        let built = self.build_loop_props(k, b, &loops, &mut loop_props, &mut closed_loops);
        self.loop_props = loop_props;
        built?;

        // closed Loop count
        self.closed_loop_cnt = closed_loops.len() as u64;

        // Call SmFace::ClassifyLoops to get loop orientations and containment for all
        // non-wire loops
        let mut scratch = CurveClassification::new(k);
        let mut classified = ClassifyLoopsOut {
            looptree: self.looptree.take(),
            ..ClassifyLoopsOut::default()
        };
        let mut target_loops = closed_loops;
        let classify_result = crate::query::face::p4::classify_loops_cached(
            k,
            b,
            m_face,
            &mut target_loops,
            &mut classified,
            &mut scratch,
            self,
            cache,
        );
        let fix = k.fixes.heal_stage3_failures_propagate;
        let classify_status = if fix {
            classify_result.map(Some)
        } else {
            // (the status is only reported upstream; a handle type that cannot name the
            // loop bisector's objects is a limitation of the caller, not a status)
            match classify_result {
                Err(
                    e @ (SolidError::InvalidInput { reason, .. }
                    | SolidError::PortInvariant { reason, .. }),
                ) if reason == crate::query::face::p3::NO_CURVE_HANDLES => {
                    return Err(e);
                }
                r => Ok(ignored(r)?),
            }
        };
        let ClassifyLoopsOut {
            classifiable,
            classify_loop_ios,
            start_type: classify_crv_start_type,
            stop_type: classify_crv_stop_type,
            ordered_no_area_indices,
            looptree,
        } = classified;
        self.looptree = looptree;
        if fix {
            // WR-268: preserve the owned tree and problem classification on error, but
            // do not consume output arrays that the failed classification did not fill.
            self.not_yet_face_type = classifiable;
            classify_status?;
        }

        // watch out for problem faces that defy ClassifyLoops
        if classifiable != HealNotYetFaceType::NoProbs {
            // remember this problem - return an error
            self.not_yet_face_type = classifiable;
            return Err(not_yet_heal_face(OP_STAGE3));
        }

        // for every Loop - set the containment, orientation and desired orientation
        // from the ClassifyLoops results
        let n = loops.len();
        let mut ii = 0usize;
        let mut jj = 0usize;
        while ii < n {
            // WireLoops were skipped when building the sClassifyLoopIO array
            while ii < n && self.loop_skipped_in_classify(ii)? {
                ii += 1;
            }
            if ii >= n {
                break;
            }

            // match SmClassifyLoopIO and m_sLoopProps indices
            let p_loop = at(&loops, ii, OP_STAGE3)?;
            let io = at(&classify_loop_ios, jj, OP_STAGE3)?;

            // guard against NULL m_pLooptreeRoot - can happen when ClassifyLoops cannot
            // process loops (a failed status is checked upstream by SE_MSG only)
            let Some(root) = self.looptree.as_ref().and_then(Looptree::root) else {
                let _ = classify_status;
                self.not_yet_face_type = HealNotYetFaceType::NoLoopTreeRoot;
                return Err(not_yet_heal_face(OP_STAGE3));
            };
            let found = match self.looptree.as_ref() {
                Some(tree) => tree.find_item(root, Some(p_loop))?,
                None => None,
            };

            // update the TreeLoop objects - store LoopProps, remove old pClassifyLoopIO
            // refs (owned by the classify records)
            if let (Some(found), Some(tree)) = (found, self.looptree.as_mut()) {
                let item = tree.item_mut(found.item)?;
                if item.loop_ == Some(p_loop) {
                    item.loop_props = Some(ii);
                    item.classify_loop_io = None;
                } else if item.partner_loop == Some(p_loop) {
                    item.partner_loop_props = Some(ii);
                    item.partner_classify_loop_io = None;
                }
            }

            // move ClassifyLoops results to m_sLoopProps (yuk!)
            let lp = self
                .loop_props
                .get_mut(ii)
                .ok_or_else(|| null(OP_STAGE3, "loop props out of range"))?;
            lp.containment = io.containment;
            lp.loop_orient = io.orient;
            lp.area_loop = io.area_loop;

            // Set DesiredLoop Orientations
            lp.desired_loop_orient = if lp.loopuse_type == LoopuseType::Unknown {
                OrientType::Unknown
            } else {
                match lp.containment {
                    ContainmentType::OuterLoop | ContainmentType::NestedLoopEven => {
                        OrientType::Same
                    }
                    ContainmentType::InnerLoop | ContainmentType::NestedLoopOdd => {
                        OrientType::Opposite
                    }
                    // SM_OT_UPPERDOMAIN
                    ContainmentType::BotLoop => OrientType::Same,
                    // SM_OT_LOWERDOMAIN
                    ContainmentType::TopLoop => OrientType::Opposite,
                    _ => OrientType::Unknown,
                }
            };

            // remember okay/problem orientations
            lp.good_orient = Tri::from(lp.loop_orient == lp.desired_loop_orient);

            ii += 1;
            jj += 1;
        }

        // iter all the loops again (tree items are initialized) counting loop
        // properties and problems
        let mut ii = 0usize;
        let mut cnt = 0usize;
        while ii < n {
            // Skip WireLoops - they have no associated sClassifyLoopIO entry
            while ii < n && self.loop_wire(ii)? {
                ii += 1;
            }
            if ii >= n {
                if cnt == 0 {
                    // All ProblemLoop counts are zero - no non-wire loops
                    self.outer_loop_cnt = 0;
                    self.bad_orient_loop_cnt = 0;
                    self.bad_no_area_loop_cnt = 0;
                    self.bad_nested_loop_cnt = 0;
                    self.bad_closed_3d_loop_cnt = 0;
                    self.bad_closed_ptr_loop_cnt = 0;
                }
                break;
            }

            // find the Loop's LooptreeItem
            let lp = at(&self.loop_props, ii, OP_STAGE3)?;
            let p_loop = lp.loop_;
            let Some(tree) = self.looptree.as_ref() else {
                return Err(null(OP_STAGE3, "no loop tree"));
            };
            let Some(root) = tree.root() else {
                return Err(null(OP_STAGE3, "no loop tree"));
            };
            let tree_item = match tree.find_item(root, p_loop)? {
                Some(f) => Some(*tree.item(f.item)?),
                None => None,
            };

            // 1. count the OuterLoops - NoArea_Loop pairs count as well
            if cnt == 0 {
                self.outer_loop_cnt = 0;
            }
            if lp.containment == ContainmentType::OuterLoop
                || tree_item.is_some_and(|item| item.loop_ == p_loop && item.is_no_area_pair())
            {
                self.outer_loop_cnt = self.outer_loop_cnt.wrapping_add(1);
            }

            // 2. count the problem orientations
            if cnt == 0 {
                self.bad_orient_loop_cnt = 0;
            }
            if lp.good_orient == Tri::False {
                self.bad_orient_loop_cnt = self.bad_orient_loop_cnt.wrapping_add(1);
            }

            // 3. count the NoArea_Loops
            if cnt == 0 {
                self.bad_no_area_loop_cnt = 0;
            }
            if lp.containment == ContainmentType::BotLoop
                || lp.containment == ContainmentType::TopLoop
            {
                self.bad_no_area_loop_cnt = self.bad_no_area_loop_cnt.wrapping_add(1);
            }

            // 4. count the problem nested Loops
            if cnt == 0 {
                self.bad_nested_loop_cnt = 0;
            }
            if lp.containment == ContainmentType::NestedLoopEven
                || lp.containment == ContainmentType::NestedLoopOdd
            {
                self.bad_nested_loop_cnt = self.bad_nested_loop_cnt.wrapping_add(1);
            }

            // 5. count the problem closed loops
            if cnt == 0 {
                self.bad_closed_3d_loop_cnt = 0;
                self.bad_closed_ptr_loop_cnt = 0;
            }
            if lp.good_closed == Tri::False {
                if lp.closed_3d == Tri::False {
                    self.bad_closed_3d_loop_cnt = self.bad_closed_3d_loop_cnt.wrapping_add(1);
                } else {
                    self.bad_closed_ptr_loop_cnt = self.bad_closed_ptr_loop_cnt.wrapping_add(1);
                }
            }

            ii += 1;
            cnt += 1;
        }

        // 5. m_bBadOuterLoopOrder from m_lOuterLoopCnt and Loop orientations
        let first_containment = self.loop_props.first().map(|p| p.containment);
        self.bad_outer_loop_order = Tri::from(
            self.outer_loop_cnt == 1
                && first_containment != Some(ContainmentType::OuterLoop)
                && first_containment != Some(ContainmentType::BotLoop),
        );

        // 6. m_lBadMissingPoles: the pole of a side that has no vertex, is on a closed
        // surface and is contained in the lowest no-area loop
        // (upstream indexes `sClassifyLoopIOs[sOrderedNoAreaIndices[0]]` only after the
        // short-circuiting tests of each side, so the lookup is lazy here as well)
        let pole_in = |side: u64,
                       type_: CrvDirOnFaceType,
                       closed: Tri,
                       want: ContainmentType|
         -> Result<u64> {
            if type_ == CrvDirOnFaceType::PoleNoVertex
                && (self.no_vertex_poles & side) != 0
                && closed.is_true()
            {
                let lowest = match ordered_no_area_indices.first() {
                    Some(&i) => Some(at(&classify_loop_ios, i, OP_STAGE3)?.containment),
                    None => None,
                };
                if lowest == Some(want) {
                    return Ok(side);
                }
            }
            Ok(SS_NONE)
        };
        let mut missing = SS_NONE;
        // UMIN missing pole
        missing |= pole_in(
            SS_UMIN,
            classify_crv_start_type,
            self.closed_u,
            ContainmentType::TopLoop,
        )?;
        // VMIN missing pole
        missing |= pole_in(
            SS_VMIN,
            classify_crv_stop_type,
            self.closed_v,
            ContainmentType::BotLoop,
        )?;
        // UMAX missing pole
        missing |= pole_in(
            SS_UMAX,
            classify_crv_stop_type,
            self.closed_u,
            ContainmentType::TopLoop,
        )?;
        // VMAX missing pole
        missing |= pole_in(
            SS_VMAX,
            classify_crv_stop_type,
            self.closed_v,
            ContainmentType::BotLoop,
        )?;
        self.bad_missing_poles = missing;

        // 7. m_bBadUnpairedNoArea_Loop from m_lBadMissingPoles bit array
        let start_extra = u64::from(
            (self.bad_missing_poles & (SS_UMIN | SS_VMIN)) != 0
                && classify_crv_start_type == CrvDirOnFaceType::PoleNoVertex,
        );
        let stop_extra = u64::from(
            (self.bad_missing_poles & (SS_UMAX | SS_VMAX)) != 0
                && classify_crv_stop_type == CrvDirOnFaceType::PoleNoVertex,
        );
        let total = start_extra
            .wrapping_add(ordered_no_area_indices.len() as u64)
            .wrapping_add(stop_extra);
        self.bad_unpaired_no_area_loop = Tri::from(total % 2 == 1);

        // set m_bBadLoopProblems = TRUE: has at least one loop problem
        // (4: post SetProps_Stage3 problems)
        self.bad_loop_problems = Tri::from(self.has_problems(4));

        // all done
        self.prop_stage = PropStage::Stage3;
        Ok(())
    }

    /// The loop-props loop of `SetProps_Stage3`: sets every loop's props and collects
    /// the closed loops; marks open loops as wire loops.
    fn build_loop_props(
        &self,
        k: &Kernel,
        b: &Brep,
        loops: &[LoopId],
        loop_props: &mut [LoopProps],
        closed_loops: &mut Vec<LoopId>,
    ) -> Result<()> {
        for (ii, &p_loop) in loops.iter().enumerate() {
            let lp = loop_props
                .get_mut(ii)
                .ok_or_else(|| null(OP_STAGE3, "loop props out of range"))?;

            // Set Loop props (the status is ignored upstream: it always succeeds)
            lp.set_props(k, b, p_loop, self)?;

            // accumulate closed loops and mark open loops as wires
            if lp.loopuse_type == LoopuseType::Vertexuse {
                // include vertex loops when the vertex is on a pole and the pole is on
                // a missing seam edge
                if (lp.seam_crossing_cnt_u > 0 || lp.seam_crossing_cnt_v > 0)
                    && lp.vertex_loop_on_pole.is_true()
                {
                    closed_loops.push(p_loop);
                }
            } else if lp.closed_3d.is_true() || lp.closed_ptrs.is_true() {
                closed_loops.push(p_loop);
            } else {
                lp.containment = ContainmentType::WireLoop;
            }
        }
        Ok(())
    }

    /// `m_sLoopProps[ii]` is a wire loop or a vertex loop (skipped when matching the
    /// classification records).
    fn loop_skipped_in_classify(&self, ii: usize) -> Result<bool> {
        let lp = at(&self.loop_props, ii, OP_STAGE3)?;
        Ok(
            lp.containment == ContainmentType::WireLoop
                || lp.loopuse_type == LoopuseType::Vertexuse,
        )
    }

    /// `m_sLoopProps[ii]` is a wire loop.
    fn loop_wire(&self, ii: usize) -> Result<bool> {
        Ok(at(&self.loop_props, ii, OP_STAGE3)?.containment == ContainmentType::WireLoop)
    }

    /// `IsPoint3dOnPole`: whether `point` is within the face's zone tolerance of a
    /// surface pole. Uses `m_sZoneTol3d`, `m_lPoles` and `m_sPolePoints`.
    #[must_use]
    pub fn is_point3d_on_pole(&self, point: Point3) -> bool {
        // init output
        let mut rtn = false;

        // Points within ZoneTol3d of a pole location are on the pole
        if self.poles != 0 {
            let zone_tol_3d_sq = self.zone_tol_3d * self.zone_tol_3d;
            let sides = [SS_UMIN, SS_VMIN, SS_UMAX, SS_VMAX];
            for (i, &side) in sides.iter().enumerate() {
                let hit = (self.poles & side) != 0
                    && self
                        .pole_points
                        .get(i)
                        .is_some_and(|p| (*p - point).length_squared() < zone_tol_3d_sq);
                rtn |= hit;
            }
        }
        rtn
    }

    /// `HasProblems(lBitArray)`: whether the props show a problem. `bits` is an or-of
    /// `1` post `SetProps_Gaps`, `2` post `SetProps_Stage2` and `4` post
    /// `SetProps_Stage3` problems (upstream default `7`).
    #[must_use]
    pub fn has_problems(&self, bits: u64) -> bool {
        // return val
        let mut prob = false;

        // NotYet supported Face problem
        prob |= self.not_yet_face_type != HealNotYetFaceType::NoProbs;

        // SetProps(SM_PROPSTAGE_GAPS) problem list
        if self.prop_stage >= PropStage::Gaps && (bits & 1) != 0 {
            prob |= self.bad_small_zone_tol_3d.is_true();
            prob |= self.bad_large_zone_tol_3d.is_true();
        }

        // SetProps(SM_PROPSTAGE_2) problem list
        if self.prop_stage >= PropStage::Stage2 && (bits & 2) != 0 {
            prob |= self.sheet == MixedLinkType::Prob;
            prob |= self.poles_link == MixedLinkType::Prob;
            prob |= (self.bad_flat_corners & FC_UMIN_VMIN) != 0
                || (self.bad_flat_corners & FC_UMIN_VMAX) != 0
                || (self.bad_flat_corners & FC_UMAX_VMIN) != 0
                || (self.bad_flat_corners & FC_UMAX_VMAX) != 0;
            prob |= self.bad_crossed_pole.is_true();
            prob |= self.bad_crossed_seam.is_true();
            prob |= self.bad_missing_seam.is_true();
            // NOT yet - needed later when Fix_BadGaps() gets built: bad_near_miss_seam
        }

        // SetProps(SM_PROPSTAGE_3) problem list
        if self.prop_stage >= PropStage::Stage3 && (bits & 4) != 0 {
            prob |= self.outer_loop_cnt != 1;

            // don't check m_bBadLoopProblems - it's just the union of these problems
            prob |= self.bad_outer_loop_order.is_true();
            prob |= self.bad_orient_loop_cnt > 0;
            prob |= self.bad_no_area_loop_cnt > 0;
            prob |= self.bad_nested_loop_cnt > 0;
            prob |= self.bad_unpaired_no_area_loop.is_true();
            prob |= self.bad_closed_3d_loop_cnt > 0;
            prob |= self.bad_closed_ptr_loop_cnt > 0;
            prob |= self.bad_missing_poles > 0;
        }

        // all done
        prob
    }
}

/// `SM_ERR_NOTYET_HEAL_FACE` as an error.
fn not_yet_heal_face(operation: &'static str) -> SolidError {
    SolidError::Upstream {
        operation,
        code: SM_ERR_NOTYET_HEAL_FACE,
        entity: None,
    }
}

/// Returns `r`'s error when it is [`SolidError::Unported`] (a status upstream
/// overwrites with the next stage's otherwise).
fn keep_unported(r: &Result<()>) -> Result<()> {
    match r {
        Err(e) if e.is_unported() => Err(e.clone()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intersect::point_class::ClassObjectKind;
    use crate::topology::Brep;

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum H {
        Loop(LoopId),
    }

    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            ClassObjectKind::Loop
        }
    }

    impl LoopObject for H {
        fn from_loop(l: LoopId) -> Self {
            Self::Loop(l)
        }

        fn as_loop(&self) -> Option<LoopId> {
            let Self::Loop(l) = self;
            Some(*l)
        }

        fn as_vertex(&self) -> Option<VertexId> {
            None
        }
    }

    fn lid(i: u32) -> LoopId {
        LoopId::from_index(i)
    }

    #[test]
    fn reset_stages() {
        let mut p: FaceProps<H> = FaceProps::for_face(FaceId::from_index(3), 7);
        assert_eq!(p.prop_stage, PropStage::Stage0);
        assert_eq!(p.face_index, 7);
        assert_eq!(p.pole_points.len(), 4);
        p.prop_stage = PropStage::Stage3;
        p.face = Some(FaceId::from_index(1));
        p.outer_loop_cnt = 1;
        p.reset(PropStage::Stage3);
        assert_eq!(p.prop_stage, PropStage::Stage2);
        assert_eq!(p.face, Some(FaceId::from_index(1)));
        assert_eq!(p.outer_loop_cnt, UNDEF_ULONG);
        p.reset(PropStage::Stage2);
        assert_eq!(p.prop_stage, PropStage::Gaps);
        p.reset(PropStage::Gaps);
        assert_eq!(p.prop_stage, PropStage::Stage0);
        assert_eq!(p.face, None);
    }

    #[test]
    fn stage3_reset_keeps_the_crossing_repair_result_and_stage2_clears_it() {
        let mut p: FaceProps<H> = FaceProps::new();
        p.prop_stage = PropStage::Stage3;
        p.been_through_split_edges_at_seam = TriedLinkType::NoFix;
        p.been_through_split_faces_at_seam = TriedLinkType::NoFix;
        p.bad_crossed_pole = Tri::True;
        p.crossed_pole_edge = Some(EdgeId::from_index(2));
        p.crossed_pole_parameter = 0.5;
        p.reset(PropStage::Stage3);
        assert_eq!(p.been_through_split_edges_at_seam, TriedLinkType::NoFix);
        assert_eq!(p.been_through_split_faces_at_seam, TriedLinkType::None);
        assert_eq!(p.bad_crossed_pole, Tri::True);
        p.reset(PropStage::Stage2);
        assert_eq!(p.been_through_split_edges_at_seam, TriedLinkType::None);
        assert_eq!(
            (
                p.bad_crossed_pole,
                p.crossed_pole_edge,
                p.crossed_pole_parameter
            ),
            (Tri::False, None, 0.0)
        );
    }

    #[test]
    fn a_crossed_pole_is_a_stage2_problem() {
        let mut p: FaceProps<H> = FaceProps::new();
        p.prop_stage = PropStage::Stage2;
        p.sheet = MixedLinkType::NoProp;
        p.poles_link = MixedLinkType::Okay;
        p.bad_flat_corners = 0;
        assert!(!p.has_problems(2));
        p.bad_crossed_pole = Tri::True;
        assert!(p.has_problems(2));
        assert!(!p.has_problems(1));
    }

    #[test]
    fn has_problems_by_stage() {
        let mut p: FaceProps<H> = FaceProps::new();
        assert!(!p.has_problems(7));
        p.prop_stage = PropStage::Gaps;
        p.bad_small_zone_tol_3d = Tri::True;
        assert!(p.has_problems(1));
        assert!(!p.has_problems(6));
        p.prop_stage = PropStage::Stage3;
        p.bad_small_zone_tol_3d = Tri::False;
        // the unset outer loop count is a problem
        assert!(p.has_problems(4));
        p.outer_loop_cnt = 1;
        p.bad_orient_loop_cnt = 0;
        p.bad_no_area_loop_cnt = 0;
        p.bad_nested_loop_cnt = 0;
        p.bad_closed_3d_loop_cnt = 0;
        p.bad_closed_ptr_loop_cnt = 0;
        p.bad_missing_poles = 0;
        p.bad_outer_loop_order = Tri::False;
        p.bad_unpaired_no_area_loop = Tri::False;
        // the flat corner mask is unset (SM_UNDEF_ULONG) but only read from stage 2
        assert!(!p.has_problems(4));
        p.not_yet_face_type = HealNotYetFaceType::NoClassifyOddCrosscnt;
        assert!(p.has_problems(4));
    }

    #[test]
    fn pole_points() {
        let mut p: FaceProps<H> = FaceProps::new();
        p.zone_tol_3d = 1e-3;
        p.poles = SS_VMAX;
        p.pole_points = vec![
            Vec3::UNDEF,
            Vec3::UNDEF,
            Vec3::UNDEF,
            Vec3::new(1.0, 2.0, 3.0),
        ];
        assert!(p.is_point3d_on_pole(Vec3::new(1.0, 2.0, 3.0005)));
        assert!(!p.is_point3d_on_pole(Vec3::new(1.0, 2.0, 3.01)));
        p.poles = SS_NONE;
        assert!(!p.is_point3d_on_pole(Vec3::new(1.0, 2.0, 3.0)));
    }

    #[test]
    fn has_props_stages() {
        let f = Some(FaceId::from_index(0));
        let mut p: FaceProps<H> = FaceProps::new();
        p.face = f;
        p.prop_stage = PropStage::Gaps;
        assert!(p.has_props(PropStage::Gaps, None));
        assert!(!p.has_props(PropStage::Stage2, f));
        p.prop_stage = PropStage::Stage2;
        assert!(p.has_props(PropStage::Stage2, f));
        assert!(!p.has_props(PropStage::Stage2, None));
        assert!(!p.has_props(PropStage::Stage3, f));
        assert!(p.has_props(PropStage::Stage0, f));
    }

    #[test]
    fn issue_521_stage3_propagates_classification_failure() {
        let mut k = Kernel::default();
        k.fixes.heal_stage3_failures_propagate = true;
        let (b, f) = plane_without_loops();
        let mut p: FaceProps<H> = FaceProps::for_face(f, 0);
        p.set_props_stage2(&k, &b, f, 0, HealerOpType::All).unwrap();
        let expected = crate::query::face::p4::classify_loops(
            &k,
            &b,
            f,
            &mut Vec::new(),
            &mut ClassifyLoopsOut::default(),
            &mut CurveClassification::new(&k),
            &mut p,
        )
        .unwrap_err();
        assert_eq!(
            p.set_props_stage3(&k, &b, f, 0, HealerOpType::All),
            Err(expected)
        );
        assert!(!p.has_props_stage3(Some(f)));
    }

    #[test]
    fn issue_521_stage3_failure_does_not_certify_cached_properties() {
        for cached in [false, true] {
            let mut k = Kernel::default();
            k.fixes.heal_stage3_failures_propagate = true;
            let (mut b, f) = plane_without_loops();
            let mut p: FaceProps<H> = FaceProps::for_face(f, 0);
            p.set_props_stage2(&k, &b, f, 0, HealerOpType::All).unwrap();
            b.faceuse_mut(crate::topology::FaceuseId::from_index(0))
                .unwrap()
                .loopuses
                .push(crate::topology::LoopuseId::from_index(99));
            let mut cache = CurveOnClassifyCache::default();
            let mut run = |props: &mut FaceProps<H>| {
                if cached {
                    props.set_props_stage3_cached(&k, &b, f, 0, HealerOpType::All, &mut cache)
                } else {
                    props.set_props_stage3(&k, &b, f, 0, HealerOpType::All)
                }
            };
            let first = run(&mut p).unwrap_err();
            assert_eq!(run(&mut p), Err(first));
            assert_eq!(p.prop_stage, PropStage::Stage2);
        }
    }

    #[test]
    fn stage3_failure_keeps_in_progress_marker_by_default() {
        // WR-266: upstream leaves SM_PROPSTAGE_3a set, so a retry returns at once.
        let k = Kernel::default();
        let (mut b, f) = plane_without_loops();
        let mut p: FaceProps<H> = FaceProps::for_face(f, 0);
        p.set_props_stage2(&k, &b, f, 0, HealerOpType::All).unwrap();
        b.faceuse_mut(crate::topology::FaceuseId::from_index(0))
            .unwrap()
            .loopuses
            .push(crate::topology::LoopuseId::from_index(99));
        assert!(p.set_props_stage3(&k, &b, f, 0, HealerOpType::All).is_err());
        assert_eq!(p.prop_stage, PropStage::Stage3A);
        assert_eq!(p.set_props_stage3(&k, &b, f, 0, HealerOpType::All), Ok(()));
    }

    fn plane_without_loops() -> (Brep, FaceId) {
        use crate::topology::{Face, Faceuse, FaceuseId, Shell};
        let mut b = Brep::new();
        let s = b.add_shell_record(Shell::default()).unwrap();
        let f = b
            .add_face_record(Face {
                surface: Some(crate::surface::surface::Surface::from(
                    crate::surface::plane::Plane::from_normal(Vec3::ZERO, Vec3::Z),
                )),
                faceuse: Some(FaceuseId::from_index(0)),
                ..Face::new(s)
            })
            .unwrap();
        b.shell_mut(s).unwrap().faces.push(f);
        for (i, orientation) in [OrientType::Same, OrientType::Opposite]
            .into_iter()
            .enumerate()
        {
            b.add_faceuse_record(Faceuse {
                shell: Some(s),
                face: Some(f),
                mate: Some(FaceuseId::from_index(1 - i as u32)),
                orientation,
                ..Faceuse::default()
            })
            .unwrap();
        }
        (b, f)
    }

    #[test]
    fn early_stage_statuses() {
        let k = Kernel::default();
        let b = Brep::new();
        let f = FaceId::from_index(0);
        let mut p: FaceProps<H> = FaceProps::new();
        // before Fix_TolSizes: SM_ERR, nothing is touched
        let e = p.set_props_stage2(&k, &b, f, 0, HealerOpType::None);
        assert!(matches!(e, Err(SolidError::Upstream { code: 1001, .. })));
        assert_eq!(p.prop_stage, PropStage::Stage0);
        let e = p.set_props_stage3(&k, &b, f, 0, HealerOpType::FixTolsizes);
        assert!(matches!(e, Err(SolidError::Upstream { code: 1001, .. })));
    }

    #[test]
    fn loop_props_has_problems() {
        let mut lp = LoopProps::for_loop(lid(0));
        // the unset seam crossing counts (SM_UNDEF_ULONG) are problems
        assert!(lp.has_problems());
        lp.seam_crossing_cnt_u = 0;
        lp.seam_crossing_cnt_v = 0;
        assert!(!lp.has_problems());
        lp.seam_crossing_cnt_u = 1;
        assert!(lp.has_problems());
        lp.seam_crossing_cnt_u = 0;
        lp.containment = ContainmentType::NestedLoopOdd;
        assert!(lp.has_problems());
    }

    fn sample_tree() -> (Looptree, [ItemId; 5]) {
        // root(L0) -> sibling L1; root child L2 (sibling L3); L3 child L4
        let mut t = Looptree::new();
        let r = t.push(LooptreeItem::with_loop(Some(lid(0)), None, None));
        let s = t.push(LooptreeItem::with_loop(Some(lid(1)), None, None));
        let c = t.push(LooptreeItem::with_loop(Some(lid(2)), None, None));
        let c2 = t.push(LooptreeItem::with_partner(
            Some(lid(3)),
            None,
            None,
            Some(lid(5)),
            None,
            None,
        ));
        let g = t.push(LooptreeItem::with_loop(Some(lid(4)), None, None));
        t.add_sibling(r, s).unwrap();
        t.add_child(r, c2).unwrap();
        t.add_child(r, c).unwrap(); // c is now first child, c2 its sibling
        t.add_child(c2, g).unwrap();
        (t, [r, s, c, c2, g])
    }

    #[test]
    fn looptree_find_item() {
        let (t, [r, s, c, c2, g]) = sample_tree();
        let root = t.root().unwrap();
        assert_eq!(root, r);
        let f = |l: u32| t.find_item(root, Some(lid(l))).unwrap();
        assert_eq!(
            f(0),
            Some(FoundItem {
                item: r,
                parent: None
            })
        );
        assert_eq!(
            f(1),
            Some(FoundItem {
                item: s,
                parent: None
            })
        );
        assert_eq!(
            f(2),
            Some(FoundItem {
                item: c,
                parent: Some(r)
            })
        );
        assert_eq!(
            f(3),
            Some(FoundItem {
                item: c2,
                parent: Some(r)
            })
        );
        // a partner loop finds its pair item
        assert_eq!(f(5).map(|x| x.item), Some(c2));
        assert_eq!(
            f(4),
            Some(FoundItem {
                item: g,
                parent: Some(c2)
            })
        );
        assert_eq!(f(9), None);
        assert!(t.item(c2).unwrap().is_no_area_pair());
        assert!(!t.item(c).unwrap().is_no_area_pair());
    }

    #[test]
    fn looptree_pole_vertex() {
        let mut t = Looptree::new();
        // (an item without partner loop also matches a pole search: NULL == NULL upstream)
        let mut root = LooptreeItem::with_loop(Some(lid(0)), None, None);
        root.partner_loop = Some(lid(5));
        let r = t.push(root);
        let mut pole = LooptreeItem::with_loop(None, None, None);
        assert!(pole.pole_vertex);
        let p = t.push(pole);
        t.add_child(r, p).unwrap();
        assert_eq!(t.find_item(r, None).unwrap().map(|x| x.item), Some(p));
        pole.reset_no_recursion();
        assert!(!pole.pole_vertex);
        let mut item = LooptreeItem::with_loop(Some(lid(0)), None, None);
        item.add_partner(None, None, None);
        assert!(item.partner_pole_vertex && item.is_no_area_pair());
    }

    #[test]
    fn looptree_cycle_is_an_error() {
        let mut t = Looptree::new();
        let a = t.push(LooptreeItem::with_loop(Some(lid(0)), None, None));
        t.item_mut(a).unwrap().sibling = Some(a);
        assert!(t.find_item(a, Some(lid(9))).is_err());
    }
}
