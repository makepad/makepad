//! `SmFace` queries, part 4: classification of a face's loops by counting the crossings of
//! a classification curve (`SmFace::ClassifyLoops`), the healer's loop analysis that
//! tolerates no-area loops, missing seams and mislabelled loops.
//!
//! Upstream: `source/SMLib/src/SmFace.cpp`, `source/SMLib/inc/SmFace.h`.
//!
//! | Rust                         | Upstream                                              |
//! |------------------------------|-------------------------------------------------------|
//! | [`classify_loops`]           | `SmFace::ClassifyLoops`                               |
//! | [`ClassifyLoopsOut`]         | its out-params `reClassifiable`, `rClassifyLoopIOs`, `reClassifyCrv_StartType`, `reClassifyCrv_StopType`, `rOrderedNoAreaIndices`, `rpLooptreeRoot` |
//! | [`ZEROED_CLASSIFY_LOOP_IO`]  | the record `SmTArray<SmClassifyLoopIO>::SetSize` adds |
//! | `impl LoopObject for EntityRef` | the `SmObject*` of a touch connected to a loop or of a vertex point classification |
//!
//! Deviations:
//! - **Object model.** The face is a record of a [`Brep`]; the function takes the B-rep
//!   and the face's id. The classification and touch handles are the caller's
//!   [`LoopObject`] (`EntityRef` in the port); a touch's `m_pConnectedTo` is read as a
//!   loop with [`LoopObject::as_loop`], a vertex point classification with
//!   [`LoopObject::as_vertex`]. A handle that is not a loop where upstream casts it to
//!   `SmLoop*`, or not a vertex where it calls `IsLoopVertex`, is
//!   [`SolidError::InvalidInput`].
//! - **Statuses and out-params.** `SM_SUCCESS` is `Ok(())`, `SM_ERR` (no loops, no
//!   classification touch list to walk) [`SolidError::sm_err`], the
//!   `SM_ERR_INVALID_INPUT` checks [`SolidError::InvalidInput`] with the reasons
//!   [`NOT_THIS_FACE`] and [`NO_SURFACE`]. `rLoops` and the out-params are passed in and
//!   returned through `loops` and `out`, so they hold what upstream leaves in them on
//!   every return (an early return leaves the caller's values).
//! - **`SmTArray::SetSize`** fills new records with zero bytes (`smos_Calloc` / `memset`,
//!   no constructor): `rClassifyLoopIOs` grows with [`ZEROED_CLASSIFY_LOOP_IO`] (type 0,
//!   `m_bDegen_Loop` and `m_bArea_Loop` `FALSE`), not with `SmClassifyLoopIO()`.
//! - **Marks.** `SmNewMarkAndLock` on the face context marks classified loops; a fresh
//!   locked mark leaves every loop unmarked, so the port keeps the marked loops in a
//!   local set. Upstream leaves the mark on the loops (not observable here).
//! - **`SmBoolean` tests** keep upstream's truth values: `m_bClosedU && ..` and
//!   `if(m_bArea_Loop)` are true for `UNSURE` (only [`Tri::False`] is false), `== TRUE` /
//!   `== FALSE` compare exactly.
//! - **Looptree.** The tree is a [`Looptree`] arena; `m_pClassifyLoopIO` is the index of
//!   the record in `out.classify_loop_ios`; `m_pLoopProps` stays unset (`NULL`), as
//!   upstream. As upstream, each pass over a target loop deletes the tree of the pass
//!   before, so the tree of the last pass is returned; it is built locally and handed
//!   to `out` only when every loop has been classified (an early return leaves `None`).
//! - **Dropped** (no effect on results in the reference build, which defines neither
//!   `SM_DEBUG_CODE` nor active asserts): the `SM_DEBUG_CODE` dumps and graphics, every
//!   `SM_ASSERT*` and `SM_DBG_WARN`, `ebClassifyCrvClosed3d` (read only by asserts),
//!   `pParent` of `FindItem` (never read).
//! - **Bisector curves.** `sms_FindAndClassifyLoopBisector` (`query.face_p3`) hands the
//!   curves it makes to a [`qf3::LoopObjectCurves`] over the handle type, kept for the
//!   pass (upstream: the curves the caller deletes). A handle type that cannot name the
//!   face and the curves ([`LoopObject::from_face`],
//!   [`LoopObject::from_classify_curve`]; `EntityRef` cannot name curves) cannot run
//!   the bisector: `ClassifyLoops` then returns [`SolidError::InvalidInput`]
//!   ([`qf3::NO_CURVE_HANDLES`]) instead of ignoring it as a status.
//! - **Touch lists.** Upstream's raw and done touch list pointers name either the face
//!   props' seam lists (borrowed) or the lists computed for a bisector or for UV
//!   containment (owned for the pass).
//! - **Undefined behaviour upstream** is [`SolidError::InvalidInput`]: an index read
//!   outside `rClassifyLoopIOs`, the face props' loop props or a touch list (odd length),
//!   an empty seam classification read at its first or last interval, a loop without
//!   loop use.
//! - No recursion ([`Looptree::find_item`] bounds its own).

use std::collections::BTreeSet;

use crate::error::{ignored, EntityRef, Result, SolidError};
use crate::heal::face_props::{
    ClassifyLoopIo, ClassifyLoopsOut, ContainmentType, FaceProps, HealNotYetFaceType, HealerOpType,
    ItemId, LoopObject, Looptree, LooptreeItem, LoopuseType, PropStage, Tri, SS_UMAX, SS_UMIN,
    SS_VMAX, SS_VMIN, UNDEF_ULONG,
};
use crate::intersect::curve_class::p1::{
    get_connected_touches, remove_non_crossings, CrvDirOnFaceType, CurveClassification, TouchData,
};
use crate::intersect::curve_class::p2::ResetOptions;
use crate::intersect::point_class::{PointClassType, PointClassification};
use crate::kernel::Kernel;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::query::face::p2::CurveOnClassifyCache;
use crate::query::face::p3 as qf3;
use crate::topology::face::p2 as face_p2;
use crate::topology::{loopuse, vertex, Brep, FaceId, LoopId, LoopuseStart, VertexId};

const OP: &str = "query::face::p4::classify_loops";

/// The reason of the `SM_ERR_INVALID_INPUT` return for a first loop that is not on the
/// face ("Given Loop not attached to this face").
pub const NOT_THIS_FACE: &str = "given loop not attached to this face";

/// The reason of the `SM_ERR_INVALID_INPUT` return for a face without surface ("Given
/// Loop->Face not attached to Surface").
pub const NO_SURFACE: &str = "given loop's face not attached to a surface";

/// The magic loop index upstream pushes for a pole without vertex (`999`).
const NO_VERTEX_POLE: u64 = 999;

/// The `SmClassifyLoopIO` record `SmTArray::SetSize` adds: zero bytes, not the
/// constructor's values (`m_tLoopuseType` 0, every `SmBoolean` `FALSE`).
pub const ZEROED_CLASSIFY_LOOP_IO: ClassifyLoopIo = ClassifyLoopIo {
    loop_: None,
    loopuse_type: LoopuseType::Zero,
    orient: OrientType::Unknown,
    degen_loop: Tri::False,
    area_loop: Tri::False,
    containment: ContainmentType::Unknown,
};

impl LoopObject for EntityRef {
    fn from_loop(l: LoopId) -> Self {
        Self::Loop(l)
    }

    fn as_loop(&self) -> Option<LoopId> {
        match self {
            Self::Loop(l) => Some(*l),
            _ => None,
        }
    }

    fn as_vertex(&self) -> Option<VertexId> {
        match self {
            Self::Vertex(v) => Some(*v),
            _ => None,
        }
    }

    fn from_face(f: FaceId) -> Option<Self> {
        Some(Self::Face(f))
    }
}

fn invalid(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// `SmBoolean` used as a condition: anything but `FALSE` (so `UNSURE` too).
const fn truthy(t: Tri) -> bool {
    !matches!(t, Tri::False)
}

const fn is_odd(n: u64) -> bool {
    n % 2 == 1
}

fn index(i: u64) -> Result<usize> {
    usize::try_from(i).map_err(|_| invalid("index out of range"))
}

fn io_at(ios: &[ClassifyLoopIo], i: u64) -> Result<&ClassifyLoopIo> {
    ios.get(index(i)?)
        .ok_or_else(|| invalid("classify loop record index out of range"))
}

fn io_at_mut(ios: &mut [ClassifyLoopIo], i: u64) -> Result<&mut ClassifyLoopIo> {
    ios.get_mut(index(i)?)
        .ok_or_else(|| invalid("classify loop record index out of range"))
}

fn touch_at<O: Copy>(list: &[TouchData<O>], i: usize) -> Result<TouchData<O>> {
    list.get(i)
        .copied()
        .ok_or_else(|| invalid("touch list index out of range"))
}

/// `pLoop->GetLoopuse()->GetLoopuseType()`.
fn loop_loopuse_type(b: &Brep, l: LoopId) -> Result<LoopuseType> {
    let Some(lu) = b.loop_(l)?.loopuse else {
        return Err(invalid("loop without loop use"));
    };
    Ok(match b.loopuse(lu)?.start {
        LoopuseStart::Unknown => LoopuseType::Unknown,
        LoopuseStart::Edgeuse(_) => LoopuseType::Edgeuse,
        LoopuseStart::Vertexuse(_) => LoopuseType::Vertexuse,
    })
}

/// `pLoop->GetLoopuse()->IsVertexLoopuse()`.
fn loop_is_vertex_loopuse(b: &Brep, l: LoopId) -> Result<bool> {
    let Some(lu) = b.loop_(l)?.loopuse else {
        return Err(invalid("loop without loop use"));
    };
    loopuse::is_vertex_loopuse(b, lu)
}

/// The loop a touch's `m_pConnectedTo` names (`(SmLoop*)` cast; `None` for `NULL`).
fn touch_loop<O: LoopObject>(t: &TouchData<O>) -> Result<Option<LoopId>> {
    match t.connected_to {
        None => Ok(None),
        Some(o) => o
            .as_loop()
            .map(Some)
            .ok_or_else(|| invalid("touch not connected to a loop")),
    }
}

/// The pole type at one end of the classification curve: `SM_CD_POLE` when the end is
/// classified to a vertex (`IsLoopVertex` is evaluated, both of its outcomes give
/// `SM_CD_POLE`), else `SM_CD_POLE_NO_VERTEX`.
fn pole_end_type<O: LoopObject>(b: &Brep, pc: &PointClassification<O>) -> Result<CrvDirOnFaceType> {
    if pc.point_class() == PointClassType::Vertex {
        let v = pc
            .vertex_object()
            .and_then(|o| o.as_vertex())
            .ok_or_else(|| invalid("vertex classification without vertex"))?;
        // SM_CD_POLE_LOOP_VERTEX / SM_CD_POLE_EDGE_VERTEX: upstream stopped
        // distinguishing these two cases
        let _loop_vertex = vertex::is_loop_vertex(b, v)?;
        Ok(CrvDirOnFaceType::Pole)
    } else {
        Ok(CrvDirOnFaceType::PoleNoVertex)
    }
}

/// Which classification curve and done-touch list classify the current target loop.
#[derive(Clone, Copy)]
enum ClassifyCrv {
    /// The face props' U seam classification (`m_pCrvClassU`, `m_sDoneTouchListU`).
    SeamU,
    /// The face props' V seam classification.
    SeamV,
    /// The caller's scratch classification of a bisector (`rCrvClassification`).
    Bisector,
}

/// `sms_FindAndClassifyLoopBisector(ii, rLoops, rClassifyLoopIOs, eMarkType, pFace,
/// pSurface, &rFaceProps, sLocalContext, &rCrvClassification, pCrvOnSurf, pUVTrimLine,
/// bStartOnSeam, bStopOnSeam, ebClosed3d)`: the seam flags are the caller's variables,
/// which the bisector sets (and leaves set when it fails) as upstream's references.
#[allow(clippy::too_many_arguments)]
fn find_and_classify_loop_bisector<O: LoopObject>(
    k: &Kernel,
    b: &Brep,
    face: FaceId,
    target: usize,
    loops: &[LoopId],
    ios: &[ClassifyLoopIo],
    marked: &BTreeSet<LoopId>,
    props: &FaceProps<O>,
    curves: &mut qf3::LoopObjectCurves<'_, O>,
    classification: &mut CurveClassification<O>,
    start_on_seam: &mut bool,
    stop_on_seam: &mut bool,
    cache: &mut CurveOnClassifyCache,
) -> Result<()> {
    let mut out = qf3::LoopBisector {
        start_on_seam: *start_on_seam,
        stop_on_seam: *stop_on_seam,
        closed_3d: None,
        has_curve: false,
    };
    let r = qf3::find_and_classify_loop_bisector_cached(
        k,
        b,
        face,
        target,
        loops,
        ios,
        marked,
        props,
        curves,
        classification,
        &mut out,
        cache,
    );
    *start_on_seam = out.start_on_seam;
    *stop_on_seam = out.stop_on_seam;
    r
}

/// Crossings found by [`get_loop_crossings`]: the touch lists for the two crossing
/// directions, in the order upstream returns them.
type LoopCrossings<O> = (Vec<TouchData<O>>, Vec<TouchData<O>>);

/// `SER(pCrvClassification->GetLoopCrossings(rLoops, &rFaceProps, &sCrossingCounts,
/// &sRawTouchDirList, &sDoneTouchDirList))`: the raw and the done touch lists (the
/// counts are not read by `ClassifyLoops`). `curves` are the bisector curves
/// `classification` names.
fn get_loop_crossings<O: LoopObject>(
    k: &Kernel,
    curves: &qf3::LoopObjectCurves<'_, O>,
    classification: &CurveClassification<O>,
    loops: &[LoopId],
    props: &mut FaceProps<O>,
) -> Result<LoopCrossings<O>> {
    let handles: Vec<O> = loops.iter().map(|&l| O::from_loop(l)).collect();
    let c = classification.get_loop_crossings(k, curves, &handles, Some(props))?;
    if let Some(e) = c.status {
        return Err(e);
    }
    Ok((c.raw_touches, c.done_touches))
}

/// `SER(pCrvClassification->GetLoopTouches(rLoops, &rFaceProps, sRawTouchDirList))`:
/// the complete encounters of the bisector `classification` with the loops.
fn get_loop_touches<O: LoopObject>(
    k: &Kernel,
    curves: &qf3::LoopObjectCurves<'_, O>,
    classification: &CurveClassification<O>,
    loops: &[LoopId],
    props: &mut FaceProps<O>,
) -> Result<Vec<TouchData<O>>> {
    let handles: Vec<O> = loops.iter().map(|&l| O::from_loop(l)).collect();
    classification.get_loop_touches(k, curves, &handles, Some(props))
}

/// The UV containment view of a raw touch list: a copy whose seam ends take the
/// opposite of their inward neighbour's direction, reduced to transverse crossings
/// (`SmTouchData::RemoveNonCrossings`). `None` when an inward neighbour is neither in
/// nor out (coincidence establishes no outside starting state).
fn uv_containment_touches<O: Copy>(raw: &[TouchData<O>]) -> Result<Option<Vec<TouchData<O>>>> {
    let mut done = raw.to_vec();
    let touch_count = done.len();
    if touch_count > 0 {
        for end in 0..2 {
            let (outside, inside) = if end == 0 {
                (0, 1)
            } else {
                (touch_count - 1, touch_count.wrapping_sub(2))
            };
            if touch_at(&done, outside)?.loop_dir != CrvDirOnFaceType::Seam {
                continue;
            }
            let inside_dir = touch_at(&done, inside)?.loop_dir;
            // Coincidence does not establish an outside starting state.
            if inside_dir != CrvDirOnFaceType::In && inside_dir != CrvDirOnFaceType::Out {
                return Ok(None);
            }
            done.get_mut(outside)
                .ok_or_else(|| invalid("touch list index out of range"))?
                .loop_dir = if inside_dir == CrvDirOnFaceType::In {
                CrvDirOnFaceType::Out
            } else {
                CrvDirOnFaceType::In
            };
        }
    }
    remove_non_crossings(&mut done)?;
    Ok(Some(done))
}

/// The orientation of an entering or pairing touch: `SM_OT_SAME` for an arrive-out /
/// depart-in touch, else `SM_OT_OPPOSITE`.
fn touch_orient<O: Copy>(done: &[TouchData<O>], j1: usize) -> Result<OrientType> {
    let arrive = touch_at(done, j1)?;
    let depart = touch_at(done, j1 + 1)?;
    Ok(
        if arrive.loop_dir == CrvDirOnFaceType::Out && depart.loop_dir == CrvDirOnFaceType::In {
            OrientType::Same
        } else {
            OrientType::Opposite
        },
    )
}

/// `SmFace::ClassifyLoops`: classifies the loops `loops` of `face` (all of the face's
/// loops when empty; `loops` then receives them) by the touches of classification curves
/// that cross them: per loop, whether it encloses area, its orientation and containment
/// (`out.classify_loop_ios`), the start and stop types of the first classification
/// curve, the no-area loops in domain order and the loop containment tree. `props` are
/// the face's props after `SetProps_Stage2` (computed here when not yet) with the loop
/// props of `SetProps_Stage3` (its seam touch lists classify the loops that cross a
/// seam); its `not_yet_face_type` records an unclassifiable face. `scratch` is reset
/// before and after use.
///
/// A face whose loops cannot be classified returns `Ok(())` with
/// `out.classifiable` other than [`HealNotYetFaceType::NoProbs`].
///
/// # Errors
///
/// `SM_ERR` ([`SolidError::sm_err`]) for a face without loops or a classification curve
/// without touches to walk; [`SolidError::InvalidInput`] ([`NOT_THIS_FACE`],
/// [`NO_SURFACE`]) when the first loop is not on `face` or the face has no surface, and
/// where upstream's behaviour is undefined (module docs); [`SolidError::InvalidInput`]
/// ([`qf3::NO_CURVE_HANDLES`]) when a target loop that crosses no seam meets a handle
/// type that cannot name the bisector's curves; the errors of the bisector, of
/// [`CurveClassification::get_loop_touches`], [`CurveClassification::get_loop_crossings`]
/// and [`remove_non_crossings`]; the errors of the topology queries and of
/// [`FaceProps::set_props_stage2`]'s [`SolidError::Unported`].
pub fn classify_loops<O: LoopObject>(
    k: &Kernel,
    b: &Brep,
    face: FaceId,
    loops: &mut Vec<LoopId>,
    out: &mut ClassifyLoopsOut,
    scratch: &mut CurveClassification<O>,
    props: &mut FaceProps<O>,
) -> Result<()> {
    let mut cache = CurveOnClassifyCache::default();
    classify_loops_cached(k, b, face, loops, out, scratch, props, &mut cache)
}

/// [`classify_loops`] on the query state `cache`, shared by every loop's bisector: the UV
/// trim curves, tolerance growth and gap caches they make stay on its snapshot for the
/// caller to keep, as upstream's on the face's own B-rep.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn classify_loops_cached<O: LoopObject>(
    k: &Kernel,
    b: &Brep,
    face: FaceId,
    loops: &mut Vec<LoopId>,
    out: &mut ClassifyLoopsOut,
    scratch: &mut CurveClassification<O>,
    props: &mut FaceProps<O>,
    cache: &mut CurveOnClassifyCache,
) -> Result<()> {
    // Fill Tgt Loops when input LoopList is empty
    if loops.is_empty() {
        *loops = face_p2::loops(b, face, false, None)?;
        if loops.is_empty() {
            return Err(SolidError::sm_err(OP));
        }
    }

    // init output
    out.classifiable = HealNotYetFaceType::NoProbs;
    out.looptree = None;
    out.classify_loop_ios
        .resize(loops.len(), ZEROED_CLASSIFY_LOOP_IO);
    out.start_type = CrvDirOnFaceType::Uninit;
    out.stop_type = CrvDirOnFaceType::Uninit;
    out.ordered_no_area_indices.clear();

    // init input - make sure we have FaceProps after Fix_TolSizes() (SM_HO_ALL assumes
    // all healable probs can be ignored now; the status is not checked)
    if props.prop_stage < PropStage::Stage2 {
        let face_index = if props.face_index == UNDEF_ULONG {
            0
        } else {
            props.face_index
        };
        ignored(props.set_props_stage2(k, b, face, face_index, HealerOpType::All))?;
    }
    scratch.reset(k, &ResetOptions::default())?;

    // check state - valid pFace and pSurface values
    let first = *loops.first().ok_or_else(|| invalid("no loops"))?;
    let loop_face = loopuse::loop_face(b, first)?;
    if loop_face != Some(face) {
        return Err(invalid(NOT_THIS_FACE));
    }
    if b.face(face)?.surface.is_none() {
        return Err(invalid(NO_SURFACE));
    }

    // Mark for upcoming iteration (enables multiple Loop classifies per iteration when
    // the ClassifyCrv happens to XSect multiple Loops)
    let mut marked: BTreeSet<LoopId> = BTreeSet::new();

    let mut no_area_loop_cnt: u64 = 0;
    let mut stack: Vec<u64> = Vec::new();
    let mut item_stack: Vec<ItemId> = Vec::new();
    let mut start_on_seam = false;
    let mut stop_on_seam = false;
    let n_loops = loops.len();

    // Keep incomplete trees local until every loop has been classified.
    let mut looptree: Option<Looptree> = None;

    // Complete seams allow a bounded UV containment walk. Each endpoint's classification
    // identifies whether that UV side has a boundary edgeuse.
    let complete_seams = (truthy(props.closed_u) || truthy(props.closed_v))
        && props.bad_missing_seam_dir == SurfParamType::Neither
        && props.bad_crossed_seam_dir == SurfParamType::Neither;

    // for every target Loop - until they are all classified - some passes may classify
    // more than one loop at a time
    for ii in 0..n_loops {
        let loop_ii = *loops.get(ii).ok_or_else(|| invalid("loop index"))?;

        // skip loops already classified
        if marked.contains(&loop_ii) {
            continue;
        }

        let uv_containment = complete_seams;

        if loop_is_vertex_loopuse(b, loop_ii)? {
            let io = out
                .classify_loop_ios
                .get_mut(ii)
                .ok_or_else(|| invalid("classify loop record index out of range"))?;
            io.loop_ = Some(loop_ii);
            io.loopuse_type = LoopuseType::Vertexuse;
            marked.insert(loop_ii);
        }

        // find a 'good' ClassifyCrv and classify the loops against it
        let doubly_periodic = props.closed_u.is_true() && props.closed_v.is_true();
        let loop_props = |props: &FaceProps<O>| {
            props
                .loop_props
                .get(ii)
                .copied()
                .ok_or_else(|| invalid("loop props index out of range"))
        };
        let crosses_u = truthy(props.closed_u) && loop_props(props)?.seam_crossing_cnt_u > 0;
        let crv: ClassifyCrv;
        let mut bisector_curves: Option<qf3::LoopObjectCurves<'_, O>> = None;
        if crosses_u && truthy(props.closed_v) && loop_props(props)?.seam_crossing_cnt_v > 0 {
            // a Loop on a Torus crossing both Seams: use the U touch lists unless the
            // loop winds once about V only
            let lp = loop_props(props)?;
            // (upstream's first two branches, both the U lists: windings above 1 about
            // both seams, or once about U)
            crv = if (lp.seam_winding_cnt_u > 1 && lp.seam_winding_cnt_v > 1)
                || lp.seam_winding_cnt_u == 1
            {
                ClassifyCrv::SeamU
            } else {
                ClassifyCrv::SeamV
            };
            start_on_seam = true;
            stop_on_seam = true;
        } else if crosses_u {
            // Srf is U Periodic and Loop crosses SeamU
            crv = ClassifyCrv::SeamU;
            start_on_seam = doubly_periodic;
            stop_on_seam = doubly_periodic;
        } else if truthy(props.closed_v) && loop_props(props)?.seam_crossing_cnt_v > 0 {
            // Srf is V Periodic and Loop crosses SeamV
            crv = ClassifyCrv::SeamV;
            start_on_seam = doubly_periodic;
            stop_on_seam = doubly_periodic;
        } else {
            // loop does not cross an existing seam ClassifyCrv (the status is not
            // checked upstream; a handle type that cannot name the bisector's curves is
            // an error of the caller, not a status)
            let curves = bisector_curves.insert(qf3::LoopObjectCurves::new(b, face)?);
            ignored(find_and_classify_loop_bisector(
                k,
                b,
                face,
                ii,
                loops,
                &out.classify_loop_ios,
                &marked,
                props,
                curves,
                scratch,
                &mut start_on_seam,
                &mut stop_on_seam,
                cache,
            ))?;
            if scratch.size() == 0 {
                continue;
            }
            crv = ClassifyCrv::Bisector;
        }
        // Upstream's crossings read the one B-rep, which the bisector's classification
        // just edited (UV trim curves made, tolerances raised; the vertex sectors then
        // use those trims): resolve the bisector's handles on the query snapshot that
        // holds those edits.
        let bisector_curves = bisector_curves.map(|c| c.rebind(cache.snapshot(b)));

        // upstream holds pointers to the face's touch lists (seams) or to the lists it
        // just computed (bisector, UV containment): borrow the former, own the latter
        let mut own_raw: Option<Vec<TouchData<O>>> = None;
        let mut own_done: Option<Vec<TouchData<O>>> = None;

        // Reuse complete encounters from Stage 2. Periodic counts remain in their own
        // cache; containment interprets a local copy of the UV boundary hits.
        if uv_containment {
            if let (ClassifyCrv::Bisector, Some(curves)) = (crv, bisector_curves.as_ref()) {
                own_raw = Some(get_loop_touches(k, curves, scratch, loops, props)?);
            }
            let raw: &[TouchData<O>] = match (&own_raw, crv) {
                (Some(r), _) => r,
                (None, ClassifyCrv::SeamU) => &props.raw_touch_list_u,
                (None, ClassifyCrv::SeamV) => &props.raw_touch_list_v,
                (None, ClassifyCrv::Bisector) => return Err(invalid("no raw touch list")),
            };
            let Some(done) = uv_containment_touches(raw)? else {
                out.classifiable = HealNotYetFaceType::NoClassifyZeroCrosscnt;
                scratch.reset(k, &ResetOptions::default())?;
                return Ok(());
            };
            own_done = Some(done);
        } else if let (ClassifyCrv::Bisector, Some(curves)) = (crv, bisector_curves.as_ref()) {
            let (raw, done) = get_loop_crossings(k, curves, scratch, loops, props)?;
            own_raw = Some(raw);
            own_done = Some(done);
        }
        let raw: &[TouchData<O>] = match (&own_raw, crv) {
            (Some(r), _) => r,
            (None, ClassifyCrv::SeamU) => &props.raw_touch_list_u,
            (None, ClassifyCrv::SeamV) => &props.raw_touch_list_v,
            (None, ClassifyCrv::Bisector) => return Err(invalid("no raw touch list")),
        };
        let done: &[TouchData<O>] = match (&own_done, crv) {
            (Some(d), _) => d,
            (None, ClassifyCrv::SeamU) => &props.done_touch_list_u,
            (None, ClassifyCrv::SeamV) => &props.done_touch_list_v,
            (None, ClassifyCrv::Bisector) => return Err(invalid("no done touch list")),
        };

        // for every Loop being classified - Get UVTrimLine/Loop crossings to Set Loop
        // LoopuseType, Area/NoArea, Orientation, and containment props
        for jj in ii..n_loops {
            let loop_jj = *loops.get(jj).ok_or_else(|| invalid("loop index"))?;

            // skip loops already classified
            if marked.contains(&loop_jj) {
                continue;
            }

            let this_touches = get_connected_touches(Some(O::from_loop(loop_jj)), done);
            let crossing_cnt = (this_touches.len() / 2) as u64;

            // check state - LoopCrossingCount should be greater than 0 after strut
            // removal, NoArea_Loops only happen on closed surfaces
            if crossing_cnt == 0
                || (is_odd(crossing_cnt) && props.closed_u.is_false() && props.closed_v.is_false())
            {
                // the loop with no crossings is the TgtLoop for this iteration: a
                // problem face
                if ii == jj {
                    if crossing_cnt == 0 {
                        out.classifiable = HealNotYetFaceType::NoClassifyZeroCrosscnt;
                        props.not_yet_face_type = HealNotYetFaceType::NoClassifyZeroCrosscnt;
                        scratch.reset(k, &ResetOptions::default())?;
                        return Ok(());
                    }
                    if is_odd(crossing_cnt)
                        && props.closed_u.is_false()
                        && props.closed_v.is_false()
                    {
                        out.classifiable = HealNotYetFaceType::NoClassifyOddCrosscnt;
                        props.not_yet_face_type = HealNotYetFaceType::NoClassifyOddCrosscnt;
                        scratch.reset(k, &ResetOptions::default())?;
                        return Ok(());
                    }
                }

                // skip nonTgtLoop candidates - they'll become the TgtLoop and get
                // classified on a subsequent iteration
                continue;
            }

            // 1. Set ClassifyLoopIO->Loop and ClassifyLoopIO->m_tLoopuseType
            let loopuse_type = loop_loopuse_type(b, loop_jj)?;
            let io = out
                .classify_loop_ios
                .get_mut(jj)
                .ok_or_else(|| invalid("classify loop record index out of range"))?;
            io.loop_ = Some(loop_jj);
            io.loopuse_type = loopuse_type;

            // 2. Set ClassifyLoopIO->Area_Loop: an odd crossing count is a NoArea_Loop
            io.area_loop = Tri::from(!is_odd(crossing_cnt));
            if io.area_loop == Tri::False {
                no_area_loop_cnt += 1;
            }

            marked.insert(loop_jj);
        }

        // the first classification sets the ClassifyCrv start and stop types and orders
        // the NoArea_Loops
        if ii == 0 {
            let classification: &CurveClassification<O> = match crv {
                ClassifyCrv::SeamU => props.crv_class_u.as_ref(),
                ClassifyCrv::SeamV => props.crv_class_v.as_ref(),
                ClassifyCrv::Bisector => Some(&*scratch),
            }
            .ok_or_else(|| invalid("no seam classification"))?;
            let ivl_cnt = classification.size();

            let mut start_seam_type = CrvDirOnFaceType::SeamNoEdgeuse;
            let mut stop_seam_type = CrvDirOnFaceType::SeamNoEdgeuse;
            if let (Some(first), Some(last)) = (raw.first(), raw.last()) {
                if first.loop_dir == CrvDirOnFaceType::Seam {
                    start_seam_type = CrvDirOnFaceType::Seam;
                }
                if last.loop_dir == CrvDirOnFaceType::Seam {
                    stop_seam_type = CrvDirOnFaceType::Seam;
                }
            }

            out.start_type = if start_on_seam {
                start_seam_type
            } else if props.poles & (SS_UMIN | SS_VMIN) != 0 {
                pole_end_type(b, &classification.at(0)?.start)?
            } else {
                CrvDirOnFaceType::None
            };
            out.stop_type = if stop_on_seam {
                stop_seam_type
            } else if props.poles & (SS_UMAX | SS_VMAX) != 0 {
                let last = ivl_cnt
                    .checked_sub(1)
                    .ok_or_else(|| invalid("empty classification"))?;
                pole_end_type(b, &classification.at(last)?.end)?
            } else {
                CrvDirOnFaceType::None
            };

            // order the NoArea_Loops by the classification curve (two touches per
            // crossing)
            if no_area_loop_cnt > 0 {
                for touch in done.iter().step_by(2) {
                    // (SmLoop*) cast: a handle that is no loop is not found either
                    let target = touch.connected_to.and_then(|o| o.as_loop());
                    // rLoops.FindElement: the index stays 0 when not found
                    let loop_indx = target
                        .and_then(|t| loops.iter().position(|&l| l == t))
                        .unwrap_or(0);
                    let area = out
                        .classify_loop_ios
                        .get(loop_indx)
                        .ok_or_else(|| invalid("classify loop record index out of range"))?
                        .area_loop;
                    if area == Tri::False && !out.ordered_no_area_indices.contains(&loop_indx) {
                        out.ordered_no_area_indices.push(loop_indx);
                    }
                }
            }
        }

        // walk the done touch list, building the Looptree from the entries and exits
        let done_size = done.len();
        let odd_no_area = out.ordered_no_area_indices.len() % 2 == 1;
        let start_with_no_vertex_pole =
            out.start_type == CrvDirOnFaceType::PoleNoVertex && odd_no_area;
        let end_with_no_vertex_pole =
            out.stop_type == CrvDirOnFaceType::PoleNoVertex && odd_no_area;

        // Complete seams bound the UV walk even when its 3D curve is closed. Otherwise
        // seam endpoints require a cyclic walk with a selected IN entry.
        let is_seam_type = |t: CrvDirOnFaceType| {
            t == CrvDirOnFaceType::Seam || t == CrvDirOnFaceType::SeamNoEdgeuse
        };
        let cyclic_walk =
            !uv_containment && (is_seam_type(out.start_type) || is_seam_type(out.stop_type));

        // set lIncIndx to find the start point on a closed-periodic classify curve: a
        // bounded walk starts at the 1st DoneTouchDirList entry, either on a nonperiodic
        // surface or in the UV domain enclosed by complete seams
        let mut inc_indx: Option<usize> = None;
        if cyclic_walk {
            let mut jj = 0;
            while jj < done_size {
                if touch_at(done, jj)?.loop_dir == CrvDirOnFaceType::Out
                    && touch_at(done, jj + 1)?.loop_dir == CrvDirOnFaceType::In
                {
                    let loop_indx = touch_at(done, jj)?.loop_index;
                    if (no_area_loop_cnt > 0
                        && io_at(&out.classify_loop_ios, loop_indx)?.area_loop == Tri::False)
                        || no_area_loop_cnt == 0
                    {
                        inc_indx = Some(jj);
                        break;
                    }
                }
                jj += 2;
            }
        } else {
            inc_indx = Some(0);
        }

        let iter_cnt = done_size + usize::from(end_with_no_vertex_pole);
        stack.clear();
        item_stack.clear();
        looptree = None;
        if start_with_no_vertex_pole {
            stack.push(NO_VERTEX_POLE);
        }

        let Some(inc_indx) = inc_indx.filter(|_| done_size != 0) else {
            // unexpected empty DoneTouchDirList or invalid lIncIndx
            return Err(SolidError::sm_err(OP));
        };

        let mut jj = 0usize;
        let mut j1 = inc_indx;
        while jj < iter_cnt {
            // the extra iteration pairs a NoVertex_Pole with a NoArea_Loop
            let (p_loop, loop_indx) = if jj == done_size {
                (None, NO_VERTEX_POLE)
            } else {
                let t = touch_at(done, j1)?;
                (touch_loop(&t)?, t.loop_index)
            };
            let io_indx = if p_loop.is_some() {
                Some(index(loop_indx)?)
            } else {
                None
            };

            let stack_cnt = stack.len();
            let last_indx = stack.last().copied().unwrap_or(UNDEF_ULONG);
            let (entering, exiting, pairing) = if stack_cnt == 0 {
                // A bounded walk starts geometrically outside, even when the outer loop
                // has reversed winding. Preserve that orientation error. Cyclic walks
                // still require their independently selected IN entry.
                if cyclic_walk
                    && !(touch_at(done, j1)?.loop_dir == CrvDirOnFaceType::Out
                        && touch_at(done, j1 + 1)?.loop_dir == CrvDirOnFaceType::In)
                    && !(io_at(&out.classify_loop_ios, loop_indx)?.area_loop == Tri::False
                        && start_with_no_vertex_pole)
                {
                    // Without an outside starting state, depth zero cannot establish
                    // containment. Leave this face unclassified.
                    out.classifiable = HealNotYetFaceType::NoLoopTreeRoot;
                    scratch.reset(k, &ResetOptions::default())?;
                    return Ok(());
                }
                (true, false, false)
            } else {
                let no_area = |i: u64| -> Result<bool> {
                    Ok(io_at(&out.classify_loop_ios, i)?.area_loop == Tri::False)
                };
                // paired a loop to itself
                let exiting_same_loop = last_indx == loop_indx && last_indx != NO_VERTEX_POLE;
                // paired an entering NoArea_Loop with an exiting NoArea_Loop
                let exiting_pair_no_area_loops = last_indx != loop_indx
                    && (last_indx != NO_VERTEX_POLE && no_area(last_indx)?)
                    && (loop_indx != NO_VERTEX_POLE && no_area(loop_indx)?);
                // paired an entering NoArea_Loop with an exiting NoVertex_Pole (only on
                // the extra iteration)
                let exiting_pair_no_area_no_vertex_pole = jj == done_size
                    && (last_indx != NO_VERTEX_POLE && no_area(last_indx)?)
                    && loop_indx == NO_VERTEX_POLE;
                // paired an entering NoVertex_Pole with an exiting NoArea_Loop
                let exiting_pair_no_vertex_pole_no_area = last_indx == NO_VERTEX_POLE
                    && (loop_indx != NO_VERTEX_POLE && no_area(loop_indx)?);
                // paired a NoVertex_Pole with a NoVertex_Pole
                let exiting_pair_no_vertex_poles =
                    last_indx == NO_VERTEX_POLE && loop_indx == NO_VERTEX_POLE;

                let exiting = exiting_same_loop;
                let pairing = exiting_pair_no_area_loops
                    || exiting_pair_no_area_no_vertex_pole
                    || exiting_pair_no_vertex_pole_no_area
                    || exiting_pair_no_vertex_poles;
                let entering = !exiting_same_loop
                    && !exiting_pair_no_area_loops
                    && !exiting_pair_no_area_no_vertex_pole
                    && !exiting_pair_no_vertex_pole_no_area
                    && !exiting_pair_no_vertex_poles;
                (entering, exiting, pairing)
            };

            if entering {
                if let Some(io_i) = io_indx {
                    // EnterEvent: Orient block (a subsequent entry is only asserted
                    // upstream)
                    let orient = touch_orient(done, j1)?;
                    let io = io_at_mut(&mut out.classify_loop_ios, io_i as u64)?;
                    if io.orient == OrientType::Unknown {
                        io.orient = orient;
                    }

                    // EnterEvent: Containment Block
                    let containment = if truthy(io.area_loop) {
                        if stack_cnt == 0 {
                            ContainmentType::OuterLoop
                        } else if stack_cnt == 1 {
                            ContainmentType::InnerLoop
                        } else if stack_cnt % 2 == 0 {
                            ContainmentType::NestedLoopEven
                        } else {
                            ContainmentType::NestedLoopOdd
                        }
                    } else if io.containment != ContainmentType::Unknown {
                        // entering the 1st of an expected pair of NoAreaLoops
                        io.containment
                    } else if stack_cnt == 0 {
                        // 1st entry - expect BotLoop
                        ContainmentType::BotLoop
                    } else {
                        ContainmentType::Error
                    };
                    if io.containment == ContainmentType::Unknown {
                        io.containment = containment;
                    }
                }

                // magic number '999' = entering face through a NoVertex_Pole to be
                // paired with a NoArea_Loop
                stack.push(loop_indx);
                if let Some(tree) = looptree.as_mut() {
                    let root = tree
                        .root()
                        .ok_or_else(|| invalid("looptree without root"))?;
                    let in_tree = if let Some(found) = tree.find_item(root, p_loop)? {
                        found.item
                    } else {
                        // Loop is not yet in the Looptree
                        let new_item = tree.push(LooptreeItem::with_loop(p_loop, None, io_indx));
                        match item_stack.last() {
                            None => tree.add_sibling(root, new_item)?,
                            Some(&top_of_stack) => tree.add_child(top_of_stack, new_item)?,
                        }
                        new_item
                    };
                    item_stack.push(in_tree);
                } else {
                    let mut tree = Looptree::new();
                    let root = tree.push(LooptreeItem::with_loop(p_loop, None, io_indx));
                    item_stack.push(root);
                    looptree = Some(tree);
                }
            } else if pairing {
                if let Some(io_i) = io_indx {
                    let orient = touch_orient(done, j1)?;
                    let io = io_at_mut(&mut out.classify_loop_ios, io_i as u64)?;
                    if io.orient == OrientType::Unknown {
                        io.orient = orient;
                    }
                    let containment = if stack_cnt == 1 {
                        ContainmentType::TopLoop
                    } else {
                        ContainmentType::Error
                    };
                    if io.containment == ContainmentType::Unknown {
                        io.containment = containment;
                    }
                }
                stack.pop();
                if let Some(top_of_stack) = item_stack.pop() {
                    let tree = looptree
                        .as_mut()
                        .ok_or_else(|| invalid("item stack without looptree"))?;
                    tree.item_mut(top_of_stack)?
                        .add_partner(p_loop, None, io_indx);
                }
            } else if exiting {
                stack.pop();
                item_stack.pop();
            }

            jj += 2;
            j1 = (j1 + 2) % done_size;
        }

        // An unmatched entry leaves only a partial nesting tree. Do not expose it as a
        // containment result for a face that could not be classified.
        if uv_containment && !stack.is_empty() {
            out.classifiable = HealNotYetFaceType::NoLoopTreeRoot;
            scratch.reset(k, &ResetOptions::default())?;
            return Ok(());
        }
    }

    if looptree.is_none() {
        // failed to build the Looptree
        out.classifiable = HealNotYetFaceType::NoLoopTreeRoot;
    }

    // don't ReSet pCrvClassification: it might point to the seam classifications of
    // the face props
    scratch.reset(k, &ResetOptions::default())?;

    // Transfer the completed tree to the caller.
    out.looptree = looptree;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::extent2d::Extent2d;
    use crate::geom::vector2d::Vec2;
    use crate::math::{Point3, Vec3};
    use crate::surface::plane::Plane;
    use crate::surface::surface::Surface;
    use crate::topology::{Face, Faceuse, FaceuseId, Loop, Loopuse, LoopuseId, VertexuseId};

    /// A face with `kinds.len()` loops (`true`: a vertex loop) on a plane.
    fn face_with_loops(b: &mut Brep, kinds: &[bool], surface: bool) -> FaceId {
        let s = b.add_shell().unwrap();
        let dom = Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0);
        let plane = Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Vec3::X,
            Vec3::Y,
            Vec2::new(1.0, 1.0),
            &dom,
        );
        let f = b
            .add_face_record(Face {
                surface: surface.then(|| Surface::from(plane)),
                faceuse: Some(FaceuseId::from_index(0)),
                uv_domain: dom,
                tolerance: 1e-6,
                ..Face::new(s)
            })
            .unwrap();
        b.add_faceuse_record(Faceuse {
            shell: Some(s),
            face: Some(f),
            mate: None,
            orientation: OrientType::Same,
            loopuses: (0..kinds.len() as u32).map(LoopuseId::from_index).collect(),
            ..Faceuse::default()
        })
        .unwrap();
        for (i, &vertex_loop) in kinds.iter().enumerate() {
            let i = i as u32;
            b.add_loopuse_record(Loopuse {
                faceuse: Some(FaceuseId::from_index(0)),
                loop_: Some(LoopId::from_index(i)),
                mate: None,
                start: if vertex_loop {
                    LoopuseStart::Vertexuse(Some(VertexuseId::from_index(i)))
                } else {
                    LoopuseStart::Edgeuse(None)
                },
                ..Loopuse::default()
            })
            .unwrap();
            b.add_loop_record(Loop {
                face: f,
                edges: Vec::new(),
                loopuse: Some(LoopuseId::from_index(i)),
            })
            .unwrap();
        }
        f
    }

    fn touch(l: u32, dir: CrvDirOnFaceType) -> TouchData<EntityRef> {
        TouchData {
            connected_to: Some(EntityRef::Loop(LoopId::from_index(l))),
            loop_dir: dir,
            loop_index: u64::from(l),
            param: 0.0,
            pt_classify_index: 0,
            pt_classify: None,
        }
    }

    /// Stage-2 props of a U-periodic face whose loops each cross the U seam once.
    fn seam_props(k: &Kernel, n: usize, done: Vec<TouchData<EntityRef>>) -> FaceProps<EntityRef> {
        let mut p: FaceProps<EntityRef> = FaceProps::for_face(FaceId::from_index(0), 0);
        p.prop_stage = PropStage::Stage3A;
        p.closed_u = Tri::True;
        p.closed_v = Tri::False;
        p.poles = 0;
        let mut lp = crate::heal::face_props::LoopProps::new();
        lp.seam_crossing_cnt_u = 1;
        lp.seam_winding_cnt_u = 1;
        lp.seam_crossing_cnt_v = 0;
        lp.seam_winding_cnt_v = 0;
        p.loop_props = vec![lp; n];
        let mut cc = CurveClassification::new(k);
        cc.set_size(k, 3).unwrap();
        p.crv_class_u = Some(cc);
        p.done_touch_list_u = done;
        p
    }

    fn run(
        b: &Brep,
        f: FaceId,
        props: &mut FaceProps<EntityRef>,
        loops: &mut Vec<LoopId>,
    ) -> (Result<()>, ClassifyLoopsOut) {
        let k = Kernel::default();
        let mut out = ClassifyLoopsOut::default();
        let mut scratch = CurveClassification::new(&k);
        let r = classify_loops(&k, b, f, loops, &mut out, &mut scratch, props);
        (r, out)
    }

    #[test]
    fn missing_seam_cylinder_pairs_bot_and_top_loops() {
        // the two circles of a cylinder face without seam edge (prog_test-style
        // MakeFaceWithCurves case; the C++ gives Bot/Top, Same/Opposite)
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = face_with_loops(&mut b, &[false, false], true);
        let done = vec![
            touch(0, CrvDirOnFaceType::Out),
            touch(0, CrvDirOnFaceType::In),
            touch(1, CrvDirOnFaceType::In),
            touch(1, CrvDirOnFaceType::Out),
        ];
        let mut props = seam_props(&k, 2, done);
        let mut loops = Vec::new();
        let (r, out) = run(&b, f, &mut props, &mut loops);
        r.unwrap();
        assert_eq!(loops.len(), 2);
        assert_eq!(out.classifiable, HealNotYetFaceType::NoProbs);
        assert_eq!(out.start_type, CrvDirOnFaceType::None);
        assert_eq!(out.stop_type, CrvDirOnFaceType::None);
        assert_eq!(out.ordered_no_area_indices, vec![0, 1]);
        let io0 = out.classify_loop_ios[0];
        let io1 = out.classify_loop_ios[1];
        assert_eq!(
            (io0.orient, io0.area_loop, io0.containment),
            (OrientType::Same, Tri::False, ContainmentType::BotLoop)
        );
        assert_eq!(
            (io1.orient, io1.area_loop, io1.containment),
            (OrientType::Opposite, Tri::False, ContainmentType::TopLoop)
        );
        assert_eq!(io0.loopuse_type, LoopuseType::Edgeuse);
        let tree = out.looptree.unwrap();
        let root = tree.item(tree.root().unwrap()).unwrap();
        assert_eq!(root.loop_, Some(LoopId::from_index(0)));
        assert_eq!(root.partner_loop, Some(LoopId::from_index(1)));
        assert_eq!(root.classify_loop_io, Some(0));
        assert_eq!(root.partner_classify_loop_io, Some(1));
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn uv_containment_flips_seam_ends_against_their_inward_neighbours() {
        let raw = vec![
            touch(0, CrvDirOnFaceType::Seam),
            touch(0, CrvDirOnFaceType::In),
            touch(0, CrvDirOnFaceType::Out),
            touch(0, CrvDirOnFaceType::Seam),
        ];
        let done = uv_containment_touches(&raw).unwrap().unwrap();
        let dirs: Vec<_> = done.iter().map(|t| t.loop_dir).collect();
        assert_eq!(
            dirs,
            vec![
                CrvDirOnFaceType::Out,
                CrvDirOnFaceType::In,
                CrvDirOnFaceType::Out,
                CrvDirOnFaceType::In
            ]
        );
        // the raw list is the caller's to keep
        assert_eq!(raw[0].loop_dir, CrvDirOnFaceType::Seam);

        // a coincident inward neighbour establishes no outside state
        let raw = vec![
            touch(0, CrvDirOnFaceType::Seam),
            touch(0, CrvDirOnFaceType::On),
        ];
        assert!(uv_containment_touches(&raw).unwrap().is_none());
        assert!(uv_containment_touches::<EntityRef>(&[])
            .unwrap()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn zero_crossings_is_a_problem_face() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = face_with_loops(&mut b, &[false, false], true);
        let done = vec![
            touch(1, CrvDirOnFaceType::Out),
            touch(1, CrvDirOnFaceType::In),
        ];
        let mut props = seam_props(&k, 2, done);
        let mut loops = Vec::new();
        let (r, out) = run(&b, f, &mut props, &mut loops);
        r.unwrap();
        assert_eq!(out.classifiable, HealNotYetFaceType::NoClassifyZeroCrosscnt);
        assert_eq!(
            props.not_yet_face_type,
            HealNotYetFaceType::NoClassifyZeroCrosscnt
        );
        // records SetSize adds are zero bytes
        assert_eq!(out.classify_loop_ios, vec![ZEROED_CLASSIFY_LOOP_IO; 2]);
    }

    #[test]
    fn checks_face_and_surface() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = face_with_loops(&mut b, &[false], false);
        let mut props = seam_props(&k, 1, Vec::new());
        let mut loops = Vec::new();
        let (r, _) = run(&b, f, &mut props, &mut loops);
        assert_eq!(r, Err(invalid(NO_SURFACE)));

        let mut b = Brep::new();
        let f = face_with_loops(&mut b, &[false], true);
        let mut loops = vec![LoopId::from_index(0)];
        let (r, _) = run(&b, FaceId::from_index(7), &mut props, &mut loops);
        assert!(r.is_err());
        let _ = f;
    }

    #[test]
    fn no_loops_is_sm_err() {
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = face_with_loops(&mut b, &[], true);
        let mut props = seam_props(&k, 0, Vec::new());
        let mut loops = Vec::new();
        let mut out = ClassifyLoopsOut {
            ordered_no_area_indices: vec![9],
            ..ClassifyLoopsOut::default()
        };
        let mut scratch = CurveClassification::new(&k);
        let r = classify_loops(&k, &b, f, &mut loops, &mut out, &mut scratch, &mut props);
        assert_eq!(r, Err(SolidError::sm_err(OP)));
        // the out-params keep the caller's values
        assert_eq!(out.ordered_no_area_indices, vec![9]);
    }

    #[test]
    fn loop_without_seam_crossing_needs_curve_handles() {
        // the bisector needs handles for its curves, which EntityRef cannot name
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = face_with_loops(&mut b, &[false], true);
        let mut props = seam_props(&k, 1, Vec::new());
        props.loop_props[0].seam_crossing_cnt_u = 0;
        let mut loops = Vec::new();
        let (r, _) = run(&b, f, &mut props, &mut loops);
        assert!(matches!(
            r,
            Err(SolidError::InvalidInput { reason, .. }) if reason == qf3::NO_CURVE_HANDLES
        ));
    }

    #[test]
    fn odd_no_area_loop_pairs_with_pole_without_vertex() {
        // a sphere cap: one no-area loop and the pole without vertex at the curve start
        let k = Kernel::default();
        let mut b = Brep::new();
        let f = face_with_loops(&mut b, &[false], true);
        let done = vec![
            touch(0, CrvDirOnFaceType::Out),
            touch(0, CrvDirOnFaceType::In),
        ];
        let mut props = seam_props(&k, 1, done);
        props.poles = SS_VMIN | SS_VMAX;
        let mut loops = Vec::new();
        let (r, out) = run(&b, f, &mut props, &mut loops);
        r.unwrap();
        // the C++ gives start and stop SM_CD_POLE_NO_VERTEX, TopLoop, Same
        assert_eq!(out.start_type, CrvDirOnFaceType::PoleNoVertex);
        assert_eq!(out.stop_type, CrvDirOnFaceType::PoleNoVertex);
        let io = out.classify_loop_ios[0];
        assert_eq!(
            (io.orient, io.containment),
            (OrientType::Same, ContainmentType::TopLoop)
        );
        // the loop pairs with the pole on the stack; the extra iteration enters the
        // pole at the curve's end: the tree is that pole vertex
        assert_eq!(out.classifiable, HealNotYetFaceType::NoProbs);
        let tree = out.looptree.unwrap();
        assert_eq!(tree.len(), 1);
        let root = tree.item(tree.root().unwrap()).unwrap();
        assert!(root.pole_vertex);
        assert_eq!(root.loop_, None);
    }
}
