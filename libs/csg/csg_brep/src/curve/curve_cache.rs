//! Curve caches: the Bezier decomposition of a curve into spans that each pass a
//! set of tessellation tolerances, arranged as a binary tree of boxes for the
//! curve global solvers.
//!
//! Upstream: `source/SMLib/src/SmCurveCache.cpp`, `source/SMLib/inc/SmCurveCache.h`.
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`BezierAux1d`]                              | `SmBezierAux1d`                                  |
//! | [`BezierAux1d::interval`]                    | `SmBezierAux1d::GetInterval`                     |
//! | [`BezierAux1d::ends_on`]                     | `SmBezierAux1d::GetEnds`                         |
//! | [`BezierSpan`], [`BezierSpan::pseudo_box`]   | `SmBezierSpan`, `SmBezierSpan::GetPseudoBox`     |
//! | [`CurveCache`], [`CurveCacheOptions`]        | `SmCurveCache`, its constructor arguments        |
//! | [`CurveCache::new`]                          | `SmCurveCache::SmCurveCache`                     |
//! | `Drop` (implicit)                            | `SmCurveCache::~SmCurveCache`                    |
//! | [`CurveCache::tessellate`]                   | `SmCurveCache::Tessellate`                       |
//! | [`CurveCache::build_tree`]                   | `SmCurveCache::BuildTree`                        |
//! | [`CurveCache::output_segment`]               | `SmCurveCache::OutputSegment`                    |
//! | [`CurveCache::analytic_curve`]               | `SmCurveCache::GetAnalyticCurve`                 |
//! | [`CurveCache::span_count`]                   | `SmCurveCache::GetSpanCount`                     |
//! | [`CurveCache::span`]                         | `SmCurveCache::GetAt`                            |
//! | [`CurveCache::parent`]                       | `SmCurveCache::GetParentAt`                      |
//! | [`CurveCache::tree`]                         | `SmCurveCache::GetTree`                          |
//! | [`CurveCache::curve`]                        | `SmCurveCache::GetCurve`                         |
//! | [`CurveCache::continuities`]                 | `SmCurveCache::GetContinuitiesArray`             |
//! | [`TYPE`], [`TYPE_STRING`], [`CurveCache::type_code`], [`CurveCache::type_string`] | `GetType`, `GetTypeString` (`SM_COMMON`) |
//! | [`is_kind_of`], [`CurveCache::is_kind_of`]   | `SmCurveCache::IsKindOf`                         |
//! | [`bezier_polygon`]                           | `sm_GetBezierPolygon` (static)                   |
//! | `compute_parent_data` (private)              | `sm_ComputeParentData`                           |
//! | [`compute_constants`], [`SpanConstants`]     | `sm_ComputeConstants` (static)                   |
//! | [`build_bezier_spans`]                       | `sm_BuildBezierSpans` (static)                   |
//! | [`create_bezier_segment_block`]              | `sm_CreateBezierSegmentBlock` (static)           |
//! | [`split_segment`]                            | `sm_SplitSegment` (static)                       |
//! | `impl EvalFunction for FindCurveDistanceExtrema` | `SmFindCurveDistanceExtremaEFO::Evaluate` (co-ported from unit `curve.curve_p4`) |
//!
//! Deviations:
//! - **Ownership.** Upstream holds a reference to its curve; the cache stores its
//!   own copy ([`Curve`] clone), since a cache lives apart from the curve in the
//!   cache store and is dropped whenever the curve changes. The curve's handle in
//!   the caller's object space (`O`, see [`crate::geom::tree`]) is optional and is
//!   what the tree reports as its owner object.
//! - **Handle space.** `CurveCache::retag` (not upstream) copies a cache into
//!   another handle space; the memo of `curve.curve_p5` (`make_curve_cache`) keeps one
//!   handle-free cache per curve value and hands each caller such a copy.
//! - **Memory managers.** `m_sBezMgr` / `m_sParentMgr` are `Vec`s; the NLib Bezier
//!   blocks (`sSegmentBeziersMgr`, `sBezStack`, the working Bezier) are owned
//!   [`Nurb`] values and a `Vec` used as the split stack (same push/pop order). The
//!   byte-size bookkeeping (`m_lBezierSize`, `m_lTotalBezSize`,
//!   `sm_ComputeNurbCurveSize`) has no counterpart. Parent data are pre-allocated
//!   zero-filled, as upstream's block allocation, and so is a span's pseudo box
//!   ([`PseudoBox::zero_filled`]): `GetNewElement` runs no constructor, and
//!   `CalculateBoundingBox` keeps that zero basis when it can set none (a span whose
//!   squared extent overflows).
//! - **Tree data.** Tree nodes point at spans through
//!   [`crate::geom::tree::CurveSpanRef`]; [`CurveCache::build_tree`] hands the span
//!   and parent arrays to the tree ([`Tree::set_curve_data`]) so the global solvers
//!   can read them from the tree, as upstream reaches the cache through the tree's
//!   owner. Because the tree cannot reach the curve, `build_tree` also stores the
//!   end points `GetEnds` evaluates for every span and parent
//!   ([`BezierAux1d::ends`]); the values are those `GetEnds` returns.
//! - **Ignored statuses.** Where upstream ignores a status (the first evaluation of
//!   `sm_BuildBezierSpans`, `sm_SplitSegment`'s evaluation, `GetEnds`,
//!   `CalculateBoundingBox` in `BuildTree`, `sm_ComputeParentData`, the evaluations
//!   and the local solve of `sm_ComputeConstants`' distance sampling) the port keeps
//!   upstream's values (the previous ones or [`Vec3::UNDEF`] for a never-written
//!   `SmVector3d`), except [`SolidError::Unported`], which is always propagated.
//! - **Tolerance.** `SmTol::GetApproxTol3d(const SmObject*)` of the curve is half
//!   the zone tolerance of the edge whose shape the curve defines, else half the
//!   system value. The edge's tolerance needs the owning `Brep`, which the cache does
//!   not hold: the builder resolves it ([`CurveCache::set_owner_zone_tol_3d`],
//!   `make_curve_cache_with_zone_tol`); an owned curve without it returns
//!   [`SolidError::Unported`].
//! - **Co-port.** `SmFindCurveDistanceExtremaEFO::Evaluate` (unit `curve.curve_p4`,
//!   called by every tessellation) is ported here as the [`EvalFunction`]
//!   implementation of [`FindCurveDistanceExtrema`].
//! - **Curve kinds.** `SM_CAST_NONNULL_PTR(SmBSplineCurve, ...)` is non-null for
//!   [`CurveKind::BSpline`] and the analytic B-spline kinds ([`CurveKind::Hyperbola`],
//!   [`CurveKind::Ellipse`], [`CurveKind::Parabola`], [`CurveKind::Line`]),
//!   whose NURB is decomposed exactly; the other kinds (Hermite, conic, projected) are
//!   approximated by cubic spans over their knots.
//! - **Undefined behaviour.** Where upstream would index past an array (a
//!   continuity array shorter than the span list, a non-cubic span in
//!   `sm_SplitSegment`) the port returns [`SolidError::InvalidInput`].
//! - **Split bound.** `Tessellate` halves a span until it passes the tests, is
//!   shorter than `SM_EFF_ZERO_SQRT` in parameter or (when set, as by default) below
//!   the minimum parametric ratio of the interval. Without that ratio, a long
//!   parameter range of geometry that never passes splits into up to
//!   `length / SM_EFF_ZERO_SQRT` leaves. The port stops after [`MAX_TESS_SPLITS`]
//!   halves in one tessellation with [`SolidError::InvalidInput`] (upstream fixtures
//!   and the committed difftest cases examine at most 4,024).
//! - **Not ported.** `GetTessellation`, `GetStart`, `GetEnd`, `GetDegree`,
//!   `GetBrep`, `GetBSplineCurve`, `GetMemoryUsed`, `SetComputeAuxTreeData`, `Dump`
//!   and `Draw` belong to other units or are debug output.

use crate::curve::curve::knots_ignoring_status;
use crate::curve::curve::p2::{at, Curve, CurveKind, FindCurveDistanceExtrema};
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::geom_utility::p1::line_closest_point;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::local_solve1d::{EvalFunction, FunctionValue, LocalSolve1d};
use crate::geom::object;
use crate::geom::pseudo_box::PseudoBox;
use crate::geom::tol::{approx_tol_3d, context_approx_tol_3d, is_zero, SmallTopoType};
use crate::geom::tree::{
    AuxDataType, CurveSpanRef, NodeData, NodeGeomType, NodeId, Tree, TreeNode, TreeOwner,
};
use crate::geom::vector3d::{acos, sqrt, Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{max2, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::{BSplineCurve, BoundingBoxRequest};
use crate::nurbs::crv_basic::Curve as Nurb;
use crate::nurbs::math::{CPoint, NOW, NOZ};
use crate::nurbs::nurbs_crv::{
    copy_nurb_curve, create_bez_segments, crv_split, crv_trim, to_euclid,
};

/// `SmCurveCache_TYPE` (`CURV_BASE_TYPE + 100`).
pub const TYPE: i64 = 14100;

/// `SmCurveCache::GetTypeString`.
pub const TYPE_STRING: &str = "SmCurveCache_TYPE";

/// `SM_CONTINUITY_ANGLE` (`SM_ANG_TOL_DEG`, degrees).
const CONTINUITY_ANGLE: f64 = 0.5;

const OP_TESS: &str = "curve::curve_cache::CurveCache::tessellate";
const OP_TREE: &str = "curve::curve_cache::CurveCache::build_tree";
const OP_CONST: &str = "curve::curve_cache::compute_constants";
const OP_BLOCK: &str = "curve::curve_cache::create_bezier_segment_block";
const OP_BUILD: &str = "curve::curve_cache::build_bezier_spans";
const OP_SPLIT: &str = "curve::curve_cache::split_segment";
const OP_SPAN: &str = "curve::curve_cache::CurveCache::span";

/// Most split halves one [`CurveCache::tessellate`] examines (not upstream; see the
/// module deviations): `2^16`, 16 times the most any upstream fixture or committed
/// difftest case examines (4,024).
pub const MAX_TESS_SPLITS: u64 = 1 << 16;
const OP_PARENT: &str = "curve::curve_cache::CurveCache::parent";

/// True for `SmCurveCache_TYPE` and its base `SmObject_TYPE`
/// (`SmCurveCache::IsKindOf`).
#[must_use]
pub const fn is_kind_of(t: i64) -> bool {
    t == TYPE || object::is_kind_of(t)
}

fn invalid(operation: &'static str, reason: &'static str) -> SolidError {
    SolidError::InvalidInput { operation, reason }
}

/// Evaluates `curve` at `t` into `pv` (position and first derivative), leaving
/// `pv` unchanged when the (ignored) status is a failure.
fn evaluate_ignored(
    k: &Kernel,
    curve: &Curve,
    t: f64,
    from_left: bool,
    pv: &mut [Vec3; 2],
) -> Result<()> {
    if let Some(v) = ignored(curve.evaluate(k, t, 1, from_left, true))? {
        *pv = [at(&v, 0), at(&v, 1)];
    }
    Ok(())
}

// ---- span data ------------------------------------------------------------------

/// The data of one node of a curve decomposition (`SmBezierAux1d`): for a leaf
/// the properties of its Bezier span, for a parent those propagated up from its
/// children.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BezierAux1d {
    /// `m_lLeft`: index of the leftmost leaf span below (a leaf's own index).
    pub left: u64,
    /// `m_lRight`: index of the rightmost leaf span below (a leaf's own index).
    pub right: u64,
    /// `m_aeConts`: continuity to the left and right decomposition neighbours.
    pub conts: [ContinuityType; 2],
    /// `m_dMaxLength`: control polygon base line length (parents: the sum).
    pub max_length: f64,
    /// `m_dMaxChordHeightSquared`: largest squared polygon vertex to base line
    /// distance.
    pub max_chord_height_squared: f64,
    /// `m_dMaxTurningAngleDeg`: accumulated polygon vertex angle, degrees.
    pub max_turning_angle_deg: f64,
    /// `m_sIvl`: parameter interval of the span.
    pub interval: Extent1d,
    /// Start and end point of the span (`GetEnds`), stored when the tree is built
    /// (see the module deviations); `UNDEF` before.
    pub ends: [Point3; 2],
}

impl Default for BezierAux1d {
    /// Zero-filled block memory (`SmMemBlockMgr`).
    fn default() -> Self {
        Self {
            left: 0,
            right: 0,
            conts: [ContinuityType::Undefined; 2],
            max_length: 0.0,
            max_chord_height_squared: 0.0,
            max_turning_angle_deg: 0.0,
            interval: Extent1d::from_raw(0.0, 0.0),
            ends: [Vec3::UNDEF; 2],
        }
    }
}

impl BezierAux1d {
    /// The span's parameter interval (`GetInterval`).
    #[must_use]
    pub const fn interval(&self) -> &Extent1d {
        &self.interval
    }

    /// Start point, end point and interval of the span on `curve` (`GetEnds`; the
    /// evaluation statuses are ignored, a failed one leaves `UNDEF`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluation.
    pub fn ends_on(&self, k: &Kernel, curve: &Curve) -> Result<(Point3, Point3, Extent1d)> {
        let ivl = self.interval;
        let start = ignored(curve.evaluate_point(k, ivl.min()))?.unwrap_or(Vec3::UNDEF);
        let end = ignored(curve.evaluate_point(k, ivl.max()))?.unwrap_or(Vec3::UNDEF);
        Ok((start, end, ivl))
    }
}

/// A leaf of a curve decomposition (`SmBezierSpan`): its [`BezierAux1d`] data and
/// a tight box.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BezierSpan {
    /// The `SmBezierAux1d` base.
    pub aux: BezierAux1d,
    /// `m_sPseudoBox`: tight (parallelepiped) box of the span, set by
    /// [`CurveCache::build_tree`].
    pub pseudo_box: PseudoBox,
}

impl BezierSpan {
    /// The span's pseudo box (`GetPseudoBox`).
    #[must_use]
    pub const fn pseudo_box(&self) -> &PseudoBox {
        &self.pseudo_box
    }

    /// The span's parameter interval (`GetInterval`).
    #[must_use]
    pub const fn interval(&self) -> &Extent1d {
        &self.aux.interval
    }
}

// ---- the cache ------------------------------------------------------------------

/// The tessellation controls of a [`CurveCache`] (the constructor arguments of
/// `SmCurveCache`); a zero tolerance is ignored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveCacheOptions {
    /// `bMakeFullTree`: keep every node; else parents passing all tests become
    /// leaves (only with `compute_aux_tree_data`).
    pub make_full_tree: bool,
    /// `dChordHeightTolerance`: largest leaf polygon vertex to base line distance.
    pub chord_height_tolerance: f64,
    /// `dAngleTolDeg`: largest leaf polygon vertex angle sum, degrees.
    pub angle_tol_deg: f64,
    /// `dOffAxisTolerance`: largest second dimension of a leaf polygon box.
    pub off_axis_tolerance: f64,
    /// `lMinimumNumberOfSegments`: limits the largest leaf interval.
    pub min_segments: u64,
    /// `bComputeAuxTreeData`: propagate child properties up to parent nodes.
    pub compute_aux_tree_data: bool,
    /// `dMax3DDistance`: largest leaf polygon base line length.
    pub max_3d_distance: f64,
    /// `dMinimumParametricRatio`: smallest leaf interval / curve interval.
    pub min_parametric_ratio: f64,
}

impl Default for CurveCacheOptions {
    /// Upstream's default arguments (only `dMinimumParametricRatio` is non-zero,
    /// `0.0001`).
    fn default() -> Self {
        Self {
            make_full_tree: false,
            chord_height_tolerance: 0.0,
            angle_tol_deg: 0.0,
            off_axis_tolerance: 0.0,
            min_segments: 0,
            compute_aux_tree_data: false,
            max_3d_distance: 0.0,
            min_parametric_ratio: 0.0001,
        }
    }
}

/// The Bezier decomposition of a curve (`SmCurveCache`). `O` is the caller's
/// object handle type of the decomposition [`Tree`].
#[derive(Debug, Clone)]
pub struct CurveCache<O> {
    /// `m_crCurve` (a copy, see the module deviations).
    curve: Curve,
    /// The curve's handle, reported as the tree's owner object.
    curve_handle: Option<O>,
    /// `m_sInterval`: the interval covered by the tree.
    interval: Extent1d,
    /// `m_pTree`.
    tree: Option<Tree<O>>,
    /// `m_pContinuities`: continuity at every knot of the curve (full trees only).
    continuities: Vec<ContinuityType>,
    /// `m_bTriedToCreateAnalytical`.
    tried_analytic: bool,
    /// `m_pAnalyticalCurve`.
    analytic: Option<Curve>,
    /// `m_sBezMgr` until the tree is built (then in the tree).
    spans: Vec<BezierSpan>,
    /// `m_sParentMgr` until the tree is built (then in the tree).
    parents: Vec<BezierAux1d>,
    /// The tessellation controls.
    opts: CurveCacheOptions,
    /// `SmTol::GetZoneTol3d(&m_crCurve)` resolved by the caller: the tolerance of the
    /// edge whose shape the curve defines (see the module deviations). `None` for a
    /// curve that defines no edge's shape.
    owner_zone_tol_3d: Option<f64>,
}

/// Outputs of [`compute_constants`] (`sm_ComputeConstants`). When a tolerance is
/// exceeded the computation stops and only the value tested against it is set.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SpanConstants {
    /// `rdMaxLength`: control polygon base line length.
    pub max_length: f64,
    /// `rdMaxChordHeightSquared`: largest squared vertex to base line distance.
    pub max_chord_height_squared: f64,
    /// `rdMaxTurningAngleDeg`: sum of the polygon vertex angles, degrees.
    pub max_turning_angle_deg: f64,
    /// `rdOffAxisDistance`: second largest polygon box size when the off-axis test
    /// failed.
    pub off_axis_distance: f64,
    /// `rdCurveDistance`: Bezier / curve distance when above the approximation
    /// tolerance.
    pub curve_distance: f64,
    /// `rbNullCurve`: every polygon vertex coincides with the first.
    pub null_curve: bool,
}

impl<O: Copy + PartialEq> CurveCache<O> {
    /// A cache of `curve` over `interval` with the tessellation controls `opts`,
    /// not tessellated yet (`SmCurveCache::SmCurveCache`).
    #[must_use]
    pub fn new(curve: Curve, interval: Extent1d, opts: CurveCacheOptions) -> Self {
        Self {
            curve,
            curve_handle: None,
            interval,
            tree: None,
            continuities: Vec::new(),
            tried_analytic: false,
            analytic: None,
            spans: Vec::new(),
            parents: Vec::new(),
            opts,
            owner_zone_tol_3d: None,
        }
    }

    /// This cache in another handle space, naming the curve `handle` (not upstream; see
    /// [`crate::curve::curve::p5::make_curve_cache`]): what a build with `handle` gives,
    /// since the handle is only stored. `None` when the tree names other objects.
    pub(crate) fn retag<P: Copy + PartialEq>(&self, handle: Option<P>) -> Option<CurveCache<P>> {
        let tree = match &self.tree {
            Some(t) => Some(t.retag_curve_tree(handle)?),
            None => None,
        };
        let analytic = match &self.analytic {
            Some(a) => Some(a.clone_detached().ok()?),
            None => None,
        };
        Some(CurveCache {
            curve: self.curve.clone_detached().ok()?,
            curve_handle: handle,
            interval: self.interval,
            tree,
            continuities: self.continuities.clone(),
            tried_analytic: self.tried_analytic,
            analytic,
            spans: self.spans.clone(),
            parents: self.parents.clone(),
            opts: self.opts,
            owner_zone_tol_3d: self.owner_zone_tol_3d,
        })
    }

    /// Sets the handle of the cached curve, reported as the owner object of the
    /// tree built afterwards.
    pub fn set_curve_handle(&mut self, handle: Option<O>) {
        self.curve_handle = handle;
    }

    /// Sets the zone tolerance of the edge whose shape the cached curve defines
    /// (`SmTol::GetZoneTol3d(&m_crCurve)`, resolved by the caller, which holds the
    /// B-rep): the tessellation's approximation tolerance is half of it. `None` (the
    /// default) for a curve that defines no edge's shape.
    pub fn set_owner_zone_tol_3d(&mut self, zone_tol_3d: Option<f64>) {
        self.owner_zone_tol_3d = zone_tol_3d;
    }

    /// `SmCurveCache_TYPE` (`GetType`).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        TYPE
    }

    /// `"SmCurveCache_TYPE"` (`GetTypeString`).
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        TYPE_STRING
    }

    /// [`is_kind_of`] (`IsKindOf`).
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        is_kind_of(t)
    }

    /// The cached curve (`GetCurve`).
    #[must_use]
    pub const fn curve(&self) -> &Curve {
        &self.curve
    }

    /// The interval covered by the decomposition (`m_sInterval`).
    #[must_use]
    pub const fn interval(&self) -> &Extent1d {
        &self.interval
    }

    /// The tessellation controls.
    #[must_use]
    pub const fn options(&self) -> &CurveCacheOptions {
        &self.opts
    }

    /// The decomposition tree, once built (`GetTree`).
    #[must_use]
    pub const fn tree(&self) -> Option<&Tree<O>> {
        self.tree.as_ref()
    }

    /// The continuity at every knot of the curve, computed by full-tree
    /// tessellations (`GetContinuitiesArray`).
    #[must_use]
    pub fn continuities(&self) -> &[ContinuityType] {
        &self.continuities
    }

    /// The leaf spans in curve order (`m_sBezMgr`).
    #[must_use]
    pub fn spans(&self) -> &[BezierSpan] {
        match &self.tree {
            Some(t) if self.spans.is_empty() => t.curve_spans(),
            _ => &self.spans,
        }
    }

    /// The parent data (`m_sParentMgr`).
    #[must_use]
    pub fn parents(&self) -> &[BezierAux1d] {
        match &self.tree {
            Some(t) if self.parents.is_empty() => t.curve_parents(),
            _ => &self.parents,
        }
    }

    /// Number of leaf spans (`GetSpanCount`); zero before tessellation.
    #[must_use]
    pub fn span_count(&self) -> usize {
        self.spans().len()
    }

    /// Leaf span `index` (`GetAt`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an index out of range (upstream `SM_ASSERT`).
    pub fn span(&self, index: usize) -> Result<&BezierSpan> {
        self.spans()
            .get(index)
            .ok_or_else(|| invalid(OP_SPAN, "span index out of range"))
    }

    /// Parent data `index` (`GetParentAt`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an index out of range (upstream `SM_ASSERT`).
    pub fn parent(&self, index: usize) -> Result<&BezierAux1d> {
        self.parents()
            .get(index)
            .ok_or_else(|| invalid(OP_PARENT, "parent index out of range"))
    }

    /// The curve itself when analytic, else a line or circle equal to it over the
    /// cache interval, made on the first call (`GetAnalyticCurve`); `None` when the
    /// curve has no analytic form.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the analytic conversion.
    pub fn analytic_curve(&mut self, k: &Kernel) -> Result<Option<&Curve>> {
        if self.curve.is_analytic() {
            return Ok(Some(&self.curve));
        }
        if self.analytic.is_none() && !self.tried_analytic {
            // See if the curve can be represented by a line or a circle
            self.analytic = ignored(self.curve.create_analytic_curve(k, &self.interval))?.flatten();
            self.tried_analytic = true;
        }
        Ok(self.analytic.as_ref())
    }

    /// The continuity of the knot `index` (`(*m_pContinuities)[index]`).
    fn continuity(&self, index: usize) -> Result<ContinuityType> {
        self.continuities
            .get(index)
            .copied()
            .ok_or_else(|| invalid(OP_TESS, "continuity index out of range"))
    }

    /// Appends the next leaf span made from `bez` and its properties, returning the
    /// Bezier for the leaf list (`OutputSegment`; `copy` is upstream's
    /// `pSegmentBeziersMgr`: copy the Bezier out of scratch memory).
    pub fn output_segment(
        &mut self,
        bez: &Nurb,
        copy: bool,
        max_length: f64,
        max_chord_height_squared: f64,
        max_turning_angle_deg: f64,
        left_continuity: ContinuityType,
        right_continuity: ContinuityType,
    ) -> Nurb {
        let num_span = self.spans.len() as u64;
        // (a zero-filled GetNewElement slot: no constructor runs, so the pseudo box's
        // basis stays zero until build_tree sets it)
        let mut new = BezierSpan {
            pseudo_box: PseudoBox::zero_filled(),
            ..BezierSpan::default()
        };

        // a leaf node is its own left and right most bezier component
        new.aux.left = num_span;
        new.aux.right = num_span;
        new.aux.conts = [left_continuity, right_continuity];
        new.aux.max_length = max_length;
        new.aux.max_chord_height_squared = max_chord_height_squared;
        new.aux.max_turning_angle_deg = max_turning_angle_deg;

        // note: the pseudo box is set later in build_tree
        let u = bez.knots();
        let u0 = u.first().copied().unwrap_or(0.0);
        let um = u.get(bez.m()).copied().unwrap_or(0.0);
        // (SetMinMax status ignored upstream)
        let _ = new.aux.interval.set_min_max(u0, um);
        self.spans.push(new);

        // when asked copy the Bezier to working memory
        if copy {
            copy_nurb_curve(bez)
        } else {
            bez.clone()
        }
    }

    /// Splits the curve into leaf spans that pass the tessellation tests and builds
    /// the decomposition tree over them (`Tessellate`). Call once, right after
    /// [`CurveCache::new`].
    ///
    /// # Errors
    ///
    /// `InvalidInput` when neither the chord height nor the angle tolerance is set
    /// (`SM_ERR_INVALID_INPUT`) and past [`MAX_TESS_SPLITS`] split halves; `SM_ERR`
    /// for a B-spline without NURB, a curve without spans, failing NLib splits and
    /// trims or an empty decomposition; those of the control polygon, continuity and
    /// evaluation queries;
    /// [`SolidError::Unported`] for unported callees.
    pub fn tessellate(&mut self, k: &Kernel) -> Result<()> {
        // check state: need at least one stored tessellation parameter
        if is_zero(self.opts.chord_height_tolerance) && is_zero(self.opts.angle_tol_deg) {
            return Err(invalid(OP_TESS, "no chord height or angle tolerance"));
        }

        // local tessellation params
        let ch_tol_sq = self.opts.chord_height_tolerance * self.opts.chord_height_tolerance;
        let angle_tol_deg = self.opts.angle_tol_deg;

        let mut approx_tol = curve_approx_tol_3d(k, &self.curve, self.owner_zone_tol_3d)?;
        if approx_tol < EFF_ZERO {
            approx_tol = context_approx_tol_3d(k, Some(&k.context), SmallTopoType::NormalSize);
        }

        // force curve to have a NURB representation
        let owner = self.curve.owner();
        let is_bspline = match self.curve.kind_mut() {
            CurveKind::BSpline(b) => {
                if b.gw_nurb().is_none() {
                    b.make_nurb()?;
                }
                true
            }
            CurveKind::Hyperbola(h) => {
                if h.base().gw_nurb().is_none() {
                    h.make_nurb(owner)?;
                }
                true
            }
            CurveKind::Ellipse(e) => {
                if e.base().gw_nurb().is_none() {
                    // m_crCurve's GetEdge(): the edge whose tolerance the builder gave
                    e.make_nurb_owned_with_edge_tol(k, owner, self.owner_zone_tol_3d)?;
                }
                true
            }
            CurveKind::Parabola(p) => {
                if p.base().gw_nurb().is_none() {
                    p.make_nurb(owner)?;
                }
                true
            }
            CurveKind::Line(l) => {
                if l.base().gw_nurb().is_none() {
                    l.make_nurb_owned(owner)?;
                }
                true
            }
            CurveKind::Hermite(_)
            | CurveKind::Conic(_)
            | CurveKind::Projected(_)
            | CurveKind::Iso(_)
            | CurveKind::Offset(_)
            | CurveKind::CrvInVolume(_)
            | CurveKind::CrvOnSurf(_)
            | CurveKind::TangentField(_)
            | CurveKind::Composite(_) => false,
        };

        // ordered leaf Beziers each passing the tessellation tests
        let mut leaf_beziers: Vec<Nurb> = Vec::new();

        // When not simplifying the decomposition-tree
        if self.opts.make_full_tree {
            let (_, conts) = self.curve.calculate_continuities(k, CONTINUITY_ANGLE)?;
            self.continuities.extend(conts);
        }

        // 1. build a Bezier span for each unique-knot span
        let beziers = {
            let nurb = match self.curve.kind() {
                CurveKind::BSpline(b) => b.gw_nurb(),
                CurveKind::Hyperbola(h) => h.base().gw_nurb(),
                CurveKind::Ellipse(e) => e.base().gw_nurb(),
                CurveKind::Parabola(p) => p.base().gw_nurb(),
                CurveKind::Line(l) => l.base().gw_nurb(),
                CurveKind::Hermite(_)
                | CurveKind::Conic(_)
                | CurveKind::Projected(_)
                | CurveKind::Iso(_)
                | CurveKind::Offset(_)
                | CurveKind::CrvInVolume(_)
                | CurveKind::CrvOnSurf(_)
                | CurveKind::TangentField(_)
                | CurveKind::Composite(_) => None,
            };
            create_bezier_segment_block(k, nurb, &self.curve)?
        };

        // max step size
        let max_step = self.max_step();

        let dim = self.curve.dim();
        let num_beziers = beziers.len();

        // split halves examined (bounded: see MAX_TESS_SPLITS)
        let mut splits: u64 = 0;
        // 2. for every unique curve parameter span
        for (i, seg) in beziers.iter().enumerate() {
            let mut bez = seg.clone();
            let mut span_ivl = Extent1d::from_min_max(
                bez.knots().first().copied().unwrap_or(0.0),
                bez.knots().get(bez.m()).copied().unwrap_or(0.0),
            );

            // skip intervals outside the tessellation range
            if self.interval.are_disjoint(&span_ivl, 0.0) {
                continue;
            }

            // build new bezier span trimmed to the tessellation interval when needed
            if !span_ivl.is_contained_by(&self.interval, EFF_ZERO) {
                span_ivl = self.interval.intersect(&span_ivl)?;

                // skip intervals too small to process
                let scaled_tol = EFF_ZERO * (1.0 + span_ivl.max().abs() + span_ivl.min().abs());
                if span_ivl.max() - span_ivl.min() < scaled_tol {
                    continue;
                }

                bez = crv_trim(&bez, span_ivl.min(), span_ivl.max())
                    .map_err(SolidError::nl_ser(OP_TESS))?;
            }

            // control-polygon properties while checking all tolerances
            let tmp = Curve::from(BSplineCurve::from_nurb(bez.clone(), dim, true));
            let c = compute_constants(
                k,
                &tmp,
                &self.curve,
                span_ivl,
                ch_tol_sq,
                angle_tol_deg,
                self.opts.off_axis_tolerance,
                self.opts.max_3d_distance,
                approx_tol,
                is_bspline, // B-spline spans are exact pieces of the curve
            )?;

            // skip short spans - unless its the last span and no other spans have
            // yet to be output
            if c.null_curve && (!self.spans.is_empty() || i + 1 < num_beziers) {
                continue;
            }

            let mut pass1 = self.passes(
                &c,
                &span_ivl,
                ch_tol_sq,
                angle_tol_deg,
                max_step,
                approx_tol,
            );

            // don't split - small bezier spans when asked, NULL curves
            if (self.opts.min_parametric_ratio != 0.0
                && span_ivl.length() / 2.0
                    < self.interval.length() * self.opts.min_parametric_ratio)
                || c.null_curve
            {
                pass1 = true;
            }

            if pass1 {
                let (left, right) = if self.opts.make_full_tree {
                    (self.continuity(i)?, self.continuity(i + 1)?)
                } else {
                    (ContinuityType::C1C2, ContinuityType::C1C2)
                };
                let leaf = self.output_segment(
                    &bez,
                    false,
                    c.max_length,
                    c.max_chord_height_squared,
                    c.max_turning_angle_deg,
                    left,
                    right,
                );
                leaf_beziers.push(leaf);
                continue;
            }

            // split pBez into two equal halves: the stack holds [max, min]
            let mut stack: Vec<Nurb> = Vec::new();
            let (bez_min, bez_max) = if is_bspline {
                crv_split(&bez, span_ivl.mid()).map_err(SolidError::nl_ser(OP_TESS))?
            } else {
                split_segment(k, &bez, &self.curve)?
            };
            stack.push(bez_max);
            stack.push(bez_min);

            let mut first_in_seg = true;
            while let Some(top) = stack.pop() {
                splits += 1;
                if splits > MAX_TESS_SPLITS {
                    return Err(invalid(
                        OP_TESS,
                        "tessellation needs more than MAX_TESS_SPLITS splits",
                    ));
                }
                let bsp = Curve::from(BSplineCurve::from_nurb(top.clone(), dim, true));
                let ivl = bsp.natural_interval();

                let c = compute_constants(
                    k,
                    &bsp,
                    &self.curve,
                    ivl,
                    ch_tol_sq,
                    angle_tol_deg,
                    self.opts.off_axis_tolerance,
                    self.opts.max_3d_distance,
                    approx_tol,
                    is_bspline, // B-spline spans are exact pieces of the curve
                )?;

                // skip very small curves
                if c.null_curve {
                    continue;
                }

                let mut pass2 =
                    self.passes(&c, &ivl, ch_tol_sq, angle_tol_deg, max_step, approx_tol);

                // don't split bezier spans below minimum size when asked
                if self.opts.min_parametric_ratio != 0.0
                    && ivl.length() / 2.0 < self.interval.length() * self.opts.min_parametric_ratio
                {
                    pass2 = true;
                }

                // don't split bezier spans below zero size
                if ivl.length() < EFF_ZERO_SQRT {
                    pass2 = true;
                }

                if pass2 {
                    let left = if self.opts.make_full_tree && first_in_seg {
                        first_in_seg = false;
                        self.continuity(i)?
                    } else {
                        ContinuityType::C1C2
                    };
                    let right = if self.opts.make_full_tree && stack.is_empty() {
                        self.continuity(i + 1)?
                    } else {
                        ContinuityType::C1C2
                    };
                    let leaf = self.output_segment(
                        &top,
                        true,
                        c.max_length,
                        c.max_chord_height_squared,
                        c.max_turning_angle_deg,
                        left,
                        right,
                    );
                    leaf_beziers.push(leaf);
                    continue;
                }

                // split the current span in half, children onto the stack
                let (bez_min, bez_max) = if is_bspline {
                    crv_split(&top, ivl.mid()).map_err(SolidError::nl_ser(OP_TESS))?
                } else {
                    split_segment(k, &top, &self.curve)?
                };
                stack.push(bez_max);
                stack.push(bez_min);
            }
        }

        // combine the ordered leaf spans into a binary tree
        self.build_tree(k, &leaf_beziers)
    }

    /// The largest leaf interval: the cache interval split into
    /// `min_segments` (when above one).
    fn max_step(&self) -> f64 {
        if self.opts.min_segments > 1 {
            (self.interval.max() - self.interval.min()) / self.opts.min_segments as f64
        } else {
            self.interval.max() - self.interval.min()
        }
    }

    /// The tessellation tests of `Tessellate` (`bPassTests1` / `bPassTests2`).
    fn passes(
        &self,
        c: &SpanConstants,
        ivl: &Extent1d,
        ch_tol_sq: f64,
        angle_tol_deg: f64,
        max_step: f64,
        approx_tol: f64,
    ) -> bool {
        let o = &self.opts;
        !((c.max_turning_angle_deg > 90.0)
            || (ivl.length() > max_step)
            || (o.chord_height_tolerance != 0.0 && c.max_chord_height_squared > ch_tol_sq)
            || (o.angle_tol_deg != 0.0 && c.max_turning_angle_deg > angle_tol_deg)
            || (o.off_axis_tolerance != 0.0 && c.off_axis_distance > o.off_axis_tolerance)
            || (o.max_3d_distance != 0.0 && c.max_length > o.max_3d_distance)
            || (c.curve_distance > approx_tol))
    }

    /// The data of node data `r` during the build (copied out).
    fn aux_of(&self, r: CurveSpanRef) -> Result<BezierAux1d> {
        match r {
            CurveSpanRef::Span(i) => self.spans.get(i).map(|s| s.aux),
            CurveSpanRef::Parent(i) => self.parents.get(i).copied(),
        }
        .ok_or_else(|| invalid(OP_TREE, "span data index out of range"))
    }

    /// Builds the decomposition tree over the leaf spans (`BuildTree`): one leaf
    /// node per span, then parents over pairs of nodes, generation by generation;
    /// with aux data the parents get their children's propagated properties and,
    /// unless making a full tree, parents passing every test lose their children.
    /// `leaf_beziers` are the spans' Beziers in span order.
    ///
    /// # Errors
    ///
    /// `SM_ERR` without spans (the empty tree is kept);
    /// `InvalidInput` for fewer Beziers than spans; [`SolidError::Unported`] from
    /// the box and evaluation callees.
    pub fn build_tree(&mut self, k: &Kernel, leaf_beziers: &[Nurb]) -> Result<()> {
        let mut tree: Tree<O> = Tree::new(Some(TreeOwner::CurveCache));
        let dim = self.curve.dim();

        let span_count = self.spans.len();
        if span_count == 0 {
            self.tree = Some(tree);
            return Err(SolidError::sm_err(OP_TREE));
        }

        // decomposition control parameters
        let ch_tol_sq = self.opts.chord_height_tolerance * self.opts.chord_height_tolerance;
        let angle_tol_deg = self.opts.angle_tol_deg;

        // the maximum allowable step size
        let max_step = self.max_step();

        // leaf generation: one node per span
        let mut curr: Vec<NodeId> = Vec::with_capacity(span_count);
        for i in 0..span_count {
            let bez = leaf_beziers
                .get(i)
                .ok_or_else(|| invalid(OP_TREE, "fewer leaf Beziers than spans"))?;
            let bsp = BSplineCurve::from_nurb(bez.clone(), dim, true);
            let req = BoundingBoxRequest {
                normal: true,
                pseudo: true,
                polar: false,
            };
            // (status ignored upstream: a failure leaves the zero-filled box; the span's
            // own pseudo box goes in, keeping its basis when none can be set)
            let initial = self
                .spans
                .get(i)
                .map_or_else(PseudoBox::zero_filled, |s| s.pseudo_box);
            let boxes =
                ignored(bsp.calculate_bounding_box_onto(&bsp.natural_interval(), req, initial))?;
            let (bbox, pseudo) = boxes.map_or((None, None), |b| (b.normal, b.pseudo));
            if let (Some(p), Some(span)) = (pseudo, self.spans.get_mut(i)) {
                span.pseudo_box = p;
            }
            let id = tree.add_node(TreeNode {
                parent: None,
                child1: None,
                child2: None,
                bbox: bbox.unwrap_or_else(|| Extent3d::from_bounds(0.0, 0.0, 0.0, 0.0, 0.0, 0.0)),
                aux_type: AuxDataType::BezierCurve,
                data: NodeData::Curve(CurveSpanRef::Span(i)),
                geom_type: NodeGeomType::Default,
            });
            curr.push(id);
        }

        // pre-allocate the parent data when used
        if self.opts.compute_aux_tree_data {
            self.parents = vec![BezierAux1d::default(); span_count];
        }

        // build parents generation by generation
        let mut aux_index = 0usize;
        while curr.len() > 1 {
            let mut next: Vec<NodeId> = Vec::with_capacity(curr.len() / 2 + 1);

            let mut j = 0usize;
            while j + 1 < curr.len() {
                let (Some(&c1), Some(&c2)) = (curr.get(j), curr.get(j + 1)) else {
                    break;
                };
                let bbox = tree.node(c1)?.bbox.union(&tree.node(c2)?.bbox);
                let id = tree.add_node(TreeNode {
                    parent: None,
                    child1: Some(c1),
                    child2: Some(c2),
                    bbox,
                    aux_type: AuxDataType::None,
                    data: NodeData::None,
                    geom_type: NodeGeomType::None,
                });
                tree.node_mut(c1)?.parent = Some(id);
                tree.node_mut(c2)?.parent = Some(id);
                next.push(id);

                // When asked, build up parent node auxiliary data
                if self.opts.compute_aux_tree_data {
                    let parent_index = aux_index;
                    aux_index += 1;
                    {
                        let node = tree.node_mut(id)?;
                        node.aux_type = AuxDataType::CurveData;
                        node.data = NodeData::Curve(CurveSpanRef::Parent(parent_index));
                    }
                    let left_ref = curve_ref(&tree, c1)?;
                    let right_ref = curve_ref(&tree, c2)?;
                    let left = self.aux_of(left_ref)?;
                    let right = self.aux_of(right_ref)?;
                    let mut parent = self
                        .parents
                        .get(parent_index)
                        .copied()
                        .ok_or_else(|| invalid(OP_TREE, "parent index out of range"))?;

                    // (status ignored upstream)
                    ignored(self.compute_parent_data(
                        k,
                        &mut parent,
                        &left,
                        &right,
                        leaf_beziers,
                        self.opts.make_full_tree,
                    ))?;
                    if let Some(slot) = self.parents.get_mut(parent_index) {
                        *slot = parent;
                    }

                    // when the parent passes every test, it becomes a leaf
                    if !self.opts.make_full_tree {
                        let ivl = parent.interval;
                        let o = &self.opts;
                        let mut pass = !((parent.max_turning_angle_deg > 90.0)
                            || (o.chord_height_tolerance != 0.0
                                && parent.max_chord_height_squared > ch_tol_sq)
                            || (o.angle_tol_deg != 0.0
                                && parent.max_turning_angle_deg > angle_tol_deg)
                            || (o.max_3d_distance != 0.0 && parent.max_length > o.max_3d_distance)
                            || (ivl.length() > max_step));

                        // Limit subdivision by the minimum parametric ratio
                        if o.min_parametric_ratio != 0.0
                            && ivl.length() < self.interval.length() * o.min_parametric_ratio
                        {
                            pass = true;
                        }

                        if pass {
                            let node = tree.node_mut(id)?;
                            node.child1 = None;
                            node.child2 = None;
                        }
                    }
                }
                j += 2;
            }

            // pick up any left over node
            if curr.len() % 2 == 1 {
                if let Some(&last) = curr.last() {
                    next.push(last);
                }
            }
            curr = next;
        }

        // store the root node
        tree.set_top_node(curr.first().copied());

        // the end points GetEnds evaluates, for the tree's readers
        for s in &mut self.spans {
            let (a, b, _) = s.aux.ends_on(k, &self.curve)?;
            s.aux.ends = [a, b];
        }
        for p in &mut self.parents {
            let (a, b, _) = p.ends_on(k, &self.curve)?;
            p.ends = [a, b];
        }

        let spans = core::mem::take(&mut self.spans);
        let parents = core::mem::take(&mut self.parents);
        tree.set_curve_data(spans, parents, self.curve_handle);
        self.tree = Some(tree);
        Ok(())
    }

    /// Sets a parent's data from its children (`sm_ComputeParentData`): every
    /// property but the pseudo box; the chord height only when not making a full
    /// tree; the turning angle grows by the tangent angle at a joint below `G1`.
    fn compute_parent_data(
        &self,
        k: &Kernel,
        parent: &mut BezierAux1d,
        left: &BezierAux1d,
        right: &BezierAux1d,
        leaf_beziers: &[Nurb],
        make_full_tree: bool,
    ) -> Result<()> {
        // propagate simple values from children to parents
        parent.left = left.left;
        parent.right = right.right;
        parent.conts = [left.conts[0], right.conts[1]];
        parent.max_length = left.max_length + right.max_length;
        parent.max_turning_angle_deg = left.max_turning_angle_deg + right.max_turning_angle_deg;
        parent.max_chord_height_squared = 0.0;
        // (SetMinMax status ignored upstream)
        let _ = parent
            .interval
            .set_min_max(left.interval.min(), right.interval.max());

        // When NOT making a full tree - compute parent MaxChordHeightSquared
        if !make_full_tree {
            let polygon =
                bezier_polygon(leaf_beziers, parent.left as usize, parent.right as usize)?;

            let (start, end, _) = parent.ends_on(k, &self.curve)?;

            // define span chord for chord height test
            let mut line_vec = end - start;
            if line_vec.length_squared() < EFF_ZERO_SQRT {
                line_vec = Vec3::X; // Chose any old vector
            } else {
                line_vec = line_vec.unitize_or_z();
            }

            // find max chord height to polygon vertices
            let mut max_chord_sq = 0.0;
            for &pnt in &polygon {
                let param = line_closest_point(start, line_vec, pnt);
                let found = start + param * line_vec;
                let dist_sq = found.distance_squared(pnt);
                if dist_sq > max_chord_sq {
                    max_chord_sq = dist_sq;
                }
            }
            parent.max_chord_height_squared = max_chord_sq;
        }

        // when child-child continuity is not G1 or better add the angle at the joint
        if (left.conts[1] as i32) < (ContinuityType::G1 as i32) {
            let ext1 = *self.span(left.right as usize)?.interval();
            let ext2 = *self.span(right.left as usize)?.interval();

            // evaluate tangents at same positions; the left/right flags matter
            let pv_left = self.curve.evaluate(k, ext1.evaluate(1.0), 1, false, true)?;
            let pv_right = self.curve.evaluate(k, ext2.evaluate(0.0), 1, true, true)?;
            let (t_left, t_right) = (at(&pv_left, 1), at(&pv_right, 1));
            if t_left.length_squared() > EFF_ZERO_SQ && t_right.length_squared() > EFF_ZERO_SQ {
                if let Ok(angle) = t_left.angle_between(t_right) {
                    parent.max_turning_angle_deg += angle * 180.0 / PI;
                }
            }
        }
        Ok(())
    }
}

/// The curve data a node points at.
fn curve_ref<O: Copy + PartialEq>(tree: &Tree<O>, id: NodeId) -> Result<CurveSpanRef> {
    match tree.node(id)?.data {
        NodeData::Curve(r) => Ok(r),
        NodeData::None | NodeData::ObjectList(_) | NodeData::Surface(_) => {
            Err(invalid(OP_TREE, "child node has no curve data"))
        }
    }
}

/// `SmTol::GetApproxTol3d(&m_crCurve)`: half the curve's zone tolerance, the
/// `SmCurve` branch of `SmTol::GetZoneTol3d(const SmObject*)` (the tolerance of the
/// edge whose shape the curve defines, else the system value). The edge's tolerance
/// is `owner_zone_tol_3d`, resolved by the cache's builder, which holds the B-rep.
///
/// # Errors
///
/// [`SolidError::Unported`] for a curve with an owner and no resolved tolerance: the
/// owner may be (or lead to) an edge whose tolerance only the B-rep holds.
fn curve_approx_tol_3d(k: &Kernel, curve: &Curve, owner_zone_tol_3d: Option<f64>) -> Result<f64> {
    if let Some(zone_tol_3d) = owner_zone_tol_3d {
        return Ok(0.5 * zone_tol_3d);
    }
    if curve.owner().is_some() {
        // DISPATCH GAP: SmTol::GetZoneTol3d(const SmObject*) (geom.tol, green) on a curve
        // whose owner the builder did not resolve (the edge's tolerance needs the
        // `&Brep`). make_curve_cache_with_zone_tol resolves it; a direct build over an
        // owned curve (Curve::length_base) stops here
        return Err(SolidError::Unported {
            upstream: "SmTol::GetZoneTol3d(const SmObject*)",
            unit: "geom.tol",
        });
    }
    Ok(approx_tol_3d(k, SmallTopoType::NormalSize))
}

// ---- file-static helpers ----------------------------------------------------------

/// The control polygon of the leaf Beziers `left..=right` without the shared end
/// vertices (`sm_GetBezierPolygon`).
///
/// # Errors
///
/// `InvalidInput` for spans or control points out of range.
pub fn bezier_polygon(leaf_beziers: &[Nurb], left: usize, right: usize) -> Result<Vec<Point3>> {
    const OP: &str = "curve::curve_cache::bezier_polygon";
    let first = leaf_beziers
        .get(left)
        .ok_or_else(|| invalid(OP, "span index out of range"))?;
    let mut polygon = Vec::new();

    // init with the 1st control point of the 1st span
    let cp0 = first
        .cpts()
        .first()
        .ok_or_else(|| invalid(OP, "empty control polygon"))?;
    polygon.push(to_euclid(*cp0));

    // add non-overlapping control-polygon vertices for the rest of the spans
    for i in left..=right {
        let bez = leaf_beziers
            .get(i)
            .ok_or_else(|| invalid(OP, "span index out of range"))?;
        for j in 1..=bez.degree() {
            let cp = bez
                .cpts()
                .get(j)
                .ok_or_else(|| invalid(OP, "control point index out of range"))?;
            polygon.push(to_euclid(*cp));
        }
    }
    Ok(polygon)
}

/// The control polygon properties of the Bezier span `bez` (a B-spline curve) of
/// `original` over `interval`, stopping at the first tolerance (zero = unused)
/// the polygon exceeds (`sm_ComputeConstants`): base line length (`d3d_tol`),
/// chord height (`ch_tol_sq`), turning angle (`angle_tol_deg`; note that upstream
/// also stops at the first non-zero angle when this tolerance is zero), off-axis
/// size (`off_axis_tol`); then, unless `exact_span` (`bez` is an exact piece of
/// `original`, a B-spline, so the check would only measure rounding differences), the
/// sampled distance between `bez` and `original` against `approx_tol`.
///
/// # Errors
///
/// `SM_ERR` for a polygon of fewer than two vertices or a `bez` that is not a
/// B-spline; those of the control polygon; [`SolidError::Unported`] from the
/// evaluations.
pub fn compute_constants(
    k: &Kernel,
    bez: &Curve,
    original: &Curve,
    interval: Extent1d,
    ch_tol_sq: f64,
    angle_tol_deg: f64,
    off_axis_tol: f64,
    d3d_tol: f64,
    approx_tol: f64,
    exact_span: bool,
) -> Result<SpanConstants> {
    let mut out = SpanConstants::default();

    // tolerance
    let angle_tol_rad = angle_tol_deg * PI / 180.0;

    // the Euclidean control polygon
    let CurveKind::BSpline(bsp) = bez.kind() else {
        return Err(SolidError::sm_err(OP_CONST));
    };
    let polygon = bsp.polygon(&bsp.natural_interval())?;
    let poly_size = polygon.len();
    let (Some(&line_pnt), Some(&last_point)) = (polygon.first(), polygon.last()) else {
        return Err(SolidError::sm_err(OP_CONST));
    };
    if poly_size < 2 {
        return Err(SolidError::sm_err(OP_CONST));
    }

    let mut last_vec = Vec3::ZERO;
    let mut total_angle = 0.0;
    let mut max_segment_ch_sq = 0.0;
    let mut line_vec = last_point - line_pnt;

    // set base line length
    out.max_length = line_vec.length();

    // when asked - quit when interval is too big
    if d3d_tol != 0.0 && out.max_length > d3d_tol {
        return Ok(out);
    }

    // for closed curves - find a vertex not coincident with the 1st vertex
    let mut try_index = poly_size - 2;
    while line_vec.length_squared() < EFF_ZERO * EFF_ZERO {
        if try_index == 0 {
            out.null_curve = true;
            return Ok(out);
        }
        let try_point = polygon
            .get(try_index)
            .copied()
            .ok_or_else(|| invalid(OP_CONST, "polygon index out of range"))?;
        try_index -= 1;
        line_vec = try_point - line_pnt;
    }

    let mut bbox = Extent3d::new();
    bbox.add_point(line_pnt);

    for pair in polygon.windows(2) {
        let &[curr, next] = pair else {
            continue;
        };

        // build control-polygon bounding box
        bbox.add_point(next);

        // return when the polygon box becomes non linear and not axis aligned
        if off_axis_tol != 0.0 {
            let diff = bbox.max() - bbox.min();
            let mut count_over = 0u32;
            if diff.x > off_axis_tol {
                count_over += 1;
            }
            if diff.y > off_axis_tol {
                count_over += 1;
            }
            if diff.z > off_axis_tol {
                count_over += 1;
            }
            if count_over > 1 {
                out.off_axis_distance = if diff.x >= diff.y && diff.x >= diff.z {
                    max2(diff.y, diff.z)
                } else if diff.y >= diff.x && diff.y >= diff.z {
                    max2(diff.x, diff.z)
                } else {
                    max2(diff.x, diff.y)
                };
                return Ok(out);
            }
        }

        // current vertex distance to the 1st/last line segment
        let line_param = line_closest_point(line_pnt, line_vec, next);
        let pnt_on_line = line_pnt + (line_vec * line_param);
        let ch_vec = next - pnt_on_line;
        let ch_sq = ch_vec.length_squared();
        if ch_sq > max_segment_ch_sq {
            max_segment_ch_sq = ch_sq;
        }
        if ch_tol_sq != 0.0 && max_segment_ch_sq > ch_tol_sq {
            // Quit as soon as we know we don't qualify
            out.max_chord_height_squared = max_segment_ch_sq;
            return Ok(out);
        }

        // next/current vector
        let diff_vec = next - curr;
        let dist = diff_vec.length();
        let scale = 1.0 + next.max_dimension();

        // when possible normalize the vector (operator/, not operator/=)
        if dist < EFF_ZERO * scale {
            continue;
        }
        let curr_vec = diff_vec / dist;

        // accumulate control-polygon angles (convex polygon: the sum)
        if last_vec.length_squared() > EFF_ZERO_SQ {
            let cos_angle = curr_vec.dot(last_vec);
            let this_angle = if cos_angle >= 1.0 {
                0.0
            } else if cos_angle <= -1.0 {
                PI
            } else {
                acos(cos_angle)
            };
            total_angle += this_angle;
        }

        // return when the accumulated angle exceeds the tolerance
        if total_angle != 0.0 && total_angle > angle_tol_rad {
            out.max_turning_angle_deg = total_angle * 180.0 / PI;
            return Ok(out);
        }

        // set for next iteration
        last_vec = curr_vec;
    }

    // set output
    out.max_chord_height_squared = max_segment_ch_sq;
    out.max_turning_angle_deg = total_angle * 180.0 / PI;

    // The distance check below bounds the error of a span that approximates a
    // non-B-spline curve. A span cut from a B-spline curve is an exact piece of it: the
    // check would only measure rounding differences, so skip it.
    if exact_span {
        return Ok(out);
    }

    let mut efo = FindCurveDistanceExtrema::new(k, bez, original, interval, 10.0 * EFF_ZERO);
    let mut solver = LocalSolve1d::with_interval(interval, false);

    // Step along the curve: when the function changes sign, call the local solver.
    let num_steps = 20u32;
    let denom = f64::from(num_steps);
    let approx_tol_sq = approx_tol * approx_tol;
    let mut pt1 = Vec3::UNDEF;
    let mut pt2 = Vec3::UNDEF;

    // function value at start of curve (status ignored)
    let mut prev_t = interval.min();
    let mut fv = FunctionValue::default();
    ignored(efo.evaluate(prev_t, &mut fv, true))?;
    let mut prev_f = fv.f;

    for ii in 1..=num_steps {
        let frac = f64::from(ii) / denom;
        let param = interval.evaluate(frac);

        let mut fv = FunctionValue::default();
        ignored(efo.evaluate(param, &mut fv, true))?;
        let f = fv.f;
        if fv.found_answer {
            if let Some(p) = ignored(bez.evaluate_point(k, param))? {
                pt1 = p;
            }
            if let Some(p) = ignored(original.evaluate_point(k, param))? {
                pt2 = p;
            }
            let dist_sq = pt1.distance_squared(pt2);
            if dist_sq > approx_tol_sq {
                out.curve_distance = sqrt(dist_sq);
                break;
            }
        } else if f * prev_f < 0.0 {
            let guess = prev_t - prev_f * (param - prev_t) / (f - prev_f);
            if ignored(solver.solve_it(&mut efo, guess, 10.0 * EFF_ZERO))?.is_some() {
                // (upstream measures at the step parameter, not the solution)
                if let Some(p) = ignored(bez.evaluate_point(k, param))? {
                    pt1 = p;
                }
                if let Some(p) = ignored(original.evaluate_point(k, param))? {
                    pt2 = p;
                }
                let dist_sq = pt1.distance_squared(pt2);
                if dist_sq > approx_tol_sq {
                    out.curve_distance = sqrt(dist_sq);
                    break;
                }
            }
        }

        prev_t = param;
        prev_f = f;
    }

    Ok(out)
}

/// The cubic Hermite approximation of `curve` over each knot span
/// (`sm_BuildBezierSpans`): end points and end tangents scaled by a third of the
/// span length.
///
/// # Errors
///
/// `SM_ERR` for fewer than two knots; [`SolidError::Unported`] from the
/// evaluations (whose other failures upstream ignores).
pub fn build_bezier_spans(k: &Kernel, curve: &Curve, knots: &[f64]) -> Result<Vec<Nurb>> {
    if knots.len() < 2 {
        return Err(SolidError::sm_err(OP_BUILD));
    }
    let mut out = Vec::with_capacity(knots.len() - 1);
    let mut pv = [Vec3::UNDEF; 2];

    let mut param = knots.first().copied().unwrap_or(0.0);
    evaluate_ignored(k, curve, param, true, &mut pv)?; // first eval: FromLeft == TRUE

    for &next in knots.iter().skip(1) {
        // control points
        let prev_param = param;
        param = next;
        let delta_t = param - prev_param;

        let c0 = pv[0];
        let c1 = c0 + pv[1] * delta_t / 3.0;

        evaluate_ignored(k, curve, param, false, &mut pv)?;
        let c3 = pv[0];
        let c2 = c3 - pv[1] * delta_t / 3.0;

        let mut u = vec![prev_param; 4];
        u.extend([param; 4]);
        let pw = [c0, c1, c2, c3]
            .iter()
            .map(|c| CPoint::new(c.x, c.y, c.z, NOW))
            .collect();
        out.push(Nurb::new(pw, 3, u)?);
    }
    Ok(out)
}

/// One Bezier per non-zero span: of the NURB `nurb` when given, else the cubic
/// Hermite spans of `curve` over its knots (`sm_CreateBezierSegmentBlock`).
///
/// # Errors
///
/// `SM_ERR` for a curve without spans or a failing segment split; those of the
/// knot query and of [`build_bezier_spans`].
pub fn create_bezier_segment_block(
    k: &Kernel,
    nurb: Option<&Nurb>,
    curve: &Curve,
) -> Result<Vec<Nurb>> {
    if let Some(n) = nurb {
        let span_count = n.knot_vector().map_or(0, |k| k.span_count(n.degree()));
        if span_count < 1 {
            return Err(SolidError::sm_err(OP_BLOCK));
        }
        create_bez_segments(n).map_err(SolidError::nl_ser(OP_BLOCK))
    } else {
        // the knot vector matches CalculateContinuities (status ignored)
        let (knots, _) = knots_ignoring_status(curve.knots_with_status(k, None))?;
        if knots.len() < 2 {
            return Err(SolidError::sm_err(OP_BLOCK));
        }
        build_bezier_spans(k, curve, &knots)
    }
}

/// The average of two control points, missing `z` / `w` staying missing.
fn cpt_mid(a: CPoint, b: CPoint) -> CPoint {
    // Careful: do not add NL_NOZ or NL_NOW values together.
    CPoint::new(
        (a.x + b.x) / 2.0,
        (a.y + b.y) / 2.0,
        if a.z == NOZ { NOZ } else { (a.z + b.z) / 2.0 },
        if a.w == NOW { NOW } else { (a.w + b.w) / 2.0 },
    )
}

/// Splits the cubic Bezier `bez` of `curve` at its middle, the inner control
/// points from the curve's point and tangent there (`sm_SplitSegment`).
///
/// # Errors
///
/// `InvalidInput` for a Bezier that is not a cubic (upstream reads past it);
/// [`SolidError::Unported`] from the evaluation (whose other failures upstream
/// ignores).
pub fn split_segment(k: &Kernel, bez: &Nurb, curve: &Curve) -> Result<(Nurb, Nurb)> {
    let (Some(full), Some(p)) = (bez.knots().get(..8), bez.cpts().get(..4)) else {
        return Err(invalid(OP_SPLIT, "span is not a cubic Bezier"));
    };
    let (&[p0, p1, p2, p3], Some(&k0), Some(&k7)) = (p, full.first(), full.get(7)) else {
        return Err(invalid(OP_SPLIT, "span is not a cubic Bezier"));
    };

    let span_ivl = Extent1d::from_min_max(k0, k7);
    let mid = span_ivl.mid();
    let mut u1 = full.get(..4).map(<[f64]>::to_vec).unwrap_or_default();
    u1.extend([mid; 4]);
    let mut u2 = vec![mid; 4];
    u2.extend(full.get(4..8).unwrap_or_default());

    // control points
    let mut orig = [Vec3::UNDEF; 2];
    evaluate_ignored(k, curve, mid, false, &mut orig)?;
    let [e, d] = orig;

    // derivative multiplier: parameter span divided by 3 for cubic
    let scale = (mid - k0) / 3.0;

    let h1 = vec![
        p0,
        cpt_mid(p0, p1),
        CPoint::new(
            e.x - d.x * scale,
            e.y - d.y * scale,
            if e.z == NOZ { NOZ } else { e.z - d.z * scale },
            NOW,
        ),
        CPoint::new(e.x, e.y, e.z, NOW),
    ];
    let h2 = vec![
        CPoint::new(e.x, e.y, e.z, NOW),
        CPoint::new(
            e.x + d.x * scale,
            e.y + d.y * scale,
            if e.z == NOZ { NOZ } else { e.z + d.z * scale },
            NOW,
        ),
        cpt_mid(p2, p3),
        p3,
    ];
    Ok((Nurb::new(h1, 3, u1)?, Nurb::new(h2, 3, u2)?))
}

/// `SmFindCurveDistanceExtremaEFO::Evaluate` (co-ported from unit
/// `curve.curve_p4`): `F = (C2 - C1) . (C2' - C1')`, the derivative of half the
/// squared distance, and `F'`; found when `|F| <= tol`.
impl EvalFunction for FindCurveDistanceExtrema<'_> {
    fn evaluate(&mut self, t: f64, out: &mut FunctionValue, _signal_errors: bool) -> Result<()> {
        let k = self.kernel;
        // init output
        out.f = 0.0;
        out.f_prime = 0.0;
        out.found_answer = false;

        let mut pv1 = [Vec3::ZERO; 3];
        let mut pv2 = [Vec3::ZERO; 3];
        self.curve1.evaluate_into(k, t, true, true, &mut pv1)?;
        self.curve2.evaluate_into(k, t, true, true, &mut pv2)?;
        let (a0, a1, a2) = (at(&pv1, 0), at(&pv1, 1), at(&pv1, 2));
        let (b0, b1, b2) = (at(&pv2, 0), at(&pv2, 1), at(&pv2, 2));

        out.f = (b0 - a0).dot(b1 - a1);
        out.f_prime = (b1 - a1).dot(b1 - a1) + (b0 - a0).dot(b2 - a2);
        out.found_answer = out.f.abs() <= self.tol;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;
    use crate::nurbs::nurbs_crv::allocate_nurb_curve;

    fn cubic() -> Curve {
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0],
            [3.0, -1.0, 1.0],
            [4.0, 1.0, 0.0],
            [6.0, 0.0, 2.0],
        ];
        let mut cur = allocate_nurb_curve(4, 3, 8).unwrap();
        for (d, s) in cur.cpts_mut().iter_mut().zip(pts) {
            *d = CPoint::new(s[0], s[1], s[2], NOW);
        }
        cur.knots_mut()
            .copy_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0]);
        Curve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    #[test]
    fn type_queries() {
        let c: CurveCache<u32> = CurveCache::new(
            cubic(),
            Extent1d::from_min_max(0.0, 1.0),
            CurveCacheOptions::default(),
        );
        assert_eq!(c.type_code(), 14100);
        assert_eq!(c.type_string(), "SmCurveCache_TYPE");
        assert!(c.is_kind_of(TYPE));
        assert!(!c.is_kind_of(14000));
        assert_eq!(c.span_count(), 0);
        assert!(c.tree().is_none());
    }

    #[test]
    fn splits_stop_at_the_bound() {
        // a longest span no span passes and no minimum parametric ratio: the halving
        // would only stop at `SM_EFF_ZERO_SQRT` spans (millions of them); it stops at
        // MAX_TESS_SPLITS
        let k = Kernel::default();
        let opts = CurveCacheOptions {
            chord_height_tolerance: 0.01,
            max_3d_distance: 1e-9,
            min_parametric_ratio: 0.0,
            ..CurveCacheOptions::default()
        };
        let mut c: CurveCache<u32> =
            CurveCache::new(cubic(), Extent1d::from_min_max(0.0, 1.0), opts);
        assert!(matches!(
            c.tessellate(&k),
            Err(SolidError::InvalidInput {
                reason: "tessellation needs more than MAX_TESS_SPLITS splits",
                ..
            })
        ));
    }

    #[test]
    fn needs_a_tolerance() {
        let k = Kernel::default();
        let mut c: CurveCache<u32> = CurveCache::new(
            cubic(),
            Extent1d::from_min_max(0.0, 1.0),
            CurveCacheOptions::default(),
        );
        assert!(matches!(
            c.tessellate(&k),
            Err(SolidError::InvalidInput { .. })
        ));
    }

    #[test]
    fn spans_cover_the_interval() {
        let k = Kernel::default();
        for (full, aux) in [(false, false), (false, true), (true, true)] {
            let opts = CurveCacheOptions {
                chord_height_tolerance: 0.01,
                angle_tol_deg: 10.0,
                make_full_tree: full,
                compute_aux_tree_data: aux,
                ..CurveCacheOptions::default()
            };
            let mut c: CurveCache<u32> =
                CurveCache::new(cubic(), Extent1d::from_min_max(0.0, 1.0), opts);
            c.tessellate(&k).unwrap();
            let n = c.span_count();
            assert!(n > 2);
            let first = c.span(0).unwrap().interval();
            let last = c.span(n - 1).unwrap().interval();
            assert_eq!(first.min(), 0.0);
            assert_eq!(last.max(), 1.0);
            for w in c.spans().windows(2) {
                assert_eq!(w[0].interval().max(), w[1].interval().min());
            }
            let tree = c.tree().unwrap();
            let top = tree.node(tree.top_node().unwrap()).unwrap();
            assert!(top.bbox.min().x <= 0.0 && top.bbox.max().x >= 6.0);
            if aux {
                let a = tree.curve_aux(top).unwrap();
                assert_eq!(a.left, 0);
                if full {
                    assert_eq!(a.right as usize, n - 1);
                    assert_eq!(c.continuities().len(), 3);
                }
            }
        }
    }

    #[test]
    fn hermite_spans() {
        let k = Kernel::default();
        let h = HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(3.0, 3.0, 0.0),
            Vec3::new(4.0, 0.0, 0.0),
            Vec3::new(3.0, -3.0, 0.0),
            3,
        );
        let opts = CurveCacheOptions {
            chord_height_tolerance: 0.001,
            ..CurveCacheOptions::default()
        };
        let mut c: CurveCache<u32> =
            CurveCache::new(Curve::from(h), Extent1d::from_min_max(0.0, 1.0), opts);
        c.tessellate(&k).unwrap();
        assert!(c.span_count() > 1);
        let s = c.span(0).unwrap();
        assert_eq!(s.aux.ends[0], Vec3::new(0.0, 0.0, 0.0));
    }

    #[test]
    fn split_segment_halves() {
        let k = &Kernel::default();
        let h = Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            3,
        ));
        let segs = build_bezier_spans(k, &h, &[0.0, 1.0]).unwrap();
        let (a, b) = split_segment(k, &segs[0], &h).unwrap();
        assert_eq!(a.knots()[7], 0.5);
        assert_eq!(b.knots()[0], 0.5);
        assert_eq!(a.cpts()[3].x, 0.5);
        assert_eq!(b.cpts()[3].x, 1.0);
    }

    #[test]
    fn polygon_skips_shared_vertices() {
        let k = &Kernel::default();
        let h = Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            3,
        ));
        let segs = build_bezier_spans(k, &h, &[0.0, 0.5, 1.0]).unwrap();
        let poly = bezier_polygon(&segs, 0, 1).unwrap();
        assert_eq!(poly.len(), 7);
        assert!(bezier_polygon(&segs, 0, 2).is_err());
    }
}
