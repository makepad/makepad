//! `SmCurve`, part 8: the global curve/curve intersector `GlobalCurveIntersect`
//! (analytic shortcuts, the leaf box intersection test, the coincidence check, the
//! curve/curve global solve and the heuristics that combine, split, drop and convert
//! the solutions it finds).
//!
//! Upstream: `source/SMLib/src/SmCurve.cpp`, `source/SMLib/inc/SmCurve.h`.
//!
//! | Rust                                                    | Upstream                        |
//! |---------------------------------------------------------|---------------------------------|
//! | [`Curve::global_curve_intersect`], [`CurveIntersectOptions`] | `SmCurve::GlobalCurveIntersect` |
//!
//! Deviations:
//! - **Out-params.** The solutions are returned as a [`SolutionArray`].
//! - **Objects.** Solutions name the caller's handles: [`CurveIntersectOptions::handles`]
//!   for this curve and the other curve (upstream `this`, `&crOtherCurve`; also the
//!   owner objects of their cache trees) and
//!   [`CurveIntersectOptions::analytic_handles`] for the analytic replacements the
//!   curve caches make of non-analytic curves (upstream the caches' `SmLine` /
//!   `SmCircle`). Where upstream casts a solution object back to its curve
//!   (`sm_RefineCCSolutions`, the spurious point check) the handle is resolved by
//!   equality, in the order this curve, other curve, this analytic, other analytic;
//!   a handle naming none of them is `SM_ERR_NULL_POINTER`, as upstream's `NER` on a
//!   failed cast.
//!   A caller that gives no handles at all gets solutions without objects; the solve
//!   itself then names stand-ins for the four curves (`operand_handles`) reporting
//!   their natural intervals, so `IdenticalSolutions` merges within 1/10000th of
//!   them and `sm_RefineCCSolutions` resolves each curve, as upstream.
//! - **Caches.** The caches are built for the call with
//!   [`make_curve_cache`](crate::curve::curve::p5::make_curve_cache) (see unit
//!   `curve.curve_p5`), or kept across calls in slots the caller lends
//!   (`global_curve_intersect_cached`, as upstream keeps them on the curves; the
//!   composite curve offsetter uses it for its offset pairs, and
//!   `global_curve_intersect_unnamed_cached` serves callers that name no handles,
//!   such as the boundary curves of `SmSurface::CoincidenceCheck`); the same curve (by
//!   identity, [`core::ptr::eq`]) shares one cache and so one tree, which makes the
//!   intersection a self intersection, as upstream's shared object cache. A cache that cannot be built is
//!   `SM_ERR_NULL_POINTER` (upstream `NER`).
//!   Callers holding topology may supply each curve owner's zone tolerance to the
//!   lazy build without changing intersection tolerance or copying geometry.
//! - **Leaf pair solve.** Upstream calls `LocalSolve` on the intersection test
//!   solver directly, which appends to the solution array the solver still points
//!   at; the port lends the array to the solver for that call.
//! - **Ignored statuses.** `sm_RefineCCSolutions`, `ClassifyIntersectingRangeSolution`,
//!   `SetMinMax` and the `EvaluatePoint`s of the solution combination and spurious
//!   point checks ignore their status upstream; so does the port (a failed
//!   evaluation leaves `Vec3::UNDEF`, a failed classification is not degenerate),
//!   except [`SolidError::Unported`], which is always propagated.
//! - **Debug only code.** The `SM_DEBUG_CODE` blocks, the assertions and the loop
//!   that only warns about consecutive point solutions within tolerance
//!   (`SM_DBG_WARN`, no output) are not ported. The obsolete `while (!bDone2)` loop
//!   around the range solution check runs exactly once upstream (nothing clears the
//!   flag) and is a single pass here.
//! - **Upstream quirks kept.** Trimming the intervals at a coincident range ending
//!   at the other curve's interval end sets this curve's interval
//!   (`sIvl1.SetMinMax(sIvl2.GetMin(), ...)`), and the second gap drop of the
//!   solution combination evaluates the other curve inside its trimmed interval
//!   (`sIvl2.Evaluate(0.38)`), as upstream. `bIsDegenerate` keeps its value from one
//!   range solution to the next when the classification is skipped.

use crate::curve::curve::p1::refine_cc_solutions;
use crate::curve::curve::p2::{at, CciGlobalSolver, Curve};
use crate::curve::curve::p4::{analytic_curve_intersect, point_or_undef, AnalyticOperand};
use crate::curve::curve::p5::make_curve_cache_with_zone_tol;
use crate::curve::curve_cache::CurveCache;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::global_solver::{
    Branch, GlobalSolve, SolutionRequested, SolveRequest, SolverOperation, MAX_SOLVER_TREES,
};
use crate::geom::solution_array::{
    OperandObject, Solution, SolutionArray, SolutionEnd, SolutionObject, SolutionType,
};
use crate::geom::tol::{are_same, scaled_zero2};
use crate::geom::tree::Tree;
use crate::kernel::Kernel;
use crate::math::{max2, min2, min4, BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::ProjectionType;

const OP_GCI: &str = "curve::curve::p8::Curve::global_curve_intersect";

/// `SM_BIG_ULONG`: no solution index yet.
const NO_INDEX: usize = usize::MAX;

/// The optional inputs of [`Curve::global_curve_intersect`].
#[derive(Debug, Clone, Copy)]
pub struct CurveIntersectOptions<O> {
    /// `bSkipGlobalCoincidenceCheck` (upstream default `FALSE`).
    pub skip_global_coincidence_check: bool,
    /// The handles of this curve and the other curve, named by the solutions. With
    /// these and [`CurveIntersectOptions::analytic_handles`] all `None` the solutions
    /// name no objects, but are merged and refined as upstream's (which always name
    /// the curves).
    pub handles: [Option<O>; 2],
    /// The handles of the analytic replacements the curve caches make of
    /// non-analytic curves (an `SmLine` of a straight B-spline), named by the
    /// solutions of their analytic intersection.
    pub analytic_handles: [Option<O>; 2],
}

impl<O> Default for CurveIntersectOptions<O> {
    fn default() -> Self {
        Self {
            skip_global_coincidence_check: false,
            handles: [None, None],
            analytic_handles: [None, None],
        }
    }
}

/// `SM_IS_BETWEEN(c, a, b)`: `c` strictly inside the open interval between `a` and
/// `b` shrunk by their scaled zero.
fn is_between(c: f64, a: f64, b: f64) -> bool {
    let z = scaled_zero2(&a, &b);
    ((a + z) < c && c < (b - z)) || ((b + z) < c && c < (a - z))
}

/// Solution `i` (a copy), `InvalidInput` past the size.
fn sol_at<O: SolutionObject>(sols: &SolutionArray<O>, i: usize) -> Result<Solution<O>> {
    sols.get_at(i)
}

/// Replaces solution `i`.
fn put_at<O: SolutionObject>(sols: &mut SolutionArray<O>, i: usize, s: &Solution<O>) -> Result<()> {
    sols.set_at(i, s)
}

/// The curves a solution object can name (see the module deviations).
struct Named<'a, O> {
    handles: [Option<O>; 2],
    curves: [&'a Curve; 2],
    analytic_handles: [Option<O>; 2],
    analytic: [Option<&'a Curve>; 2],
}

impl<'a, O: SolutionObject> Named<'a, O> {
    /// The curve named by `obj`, if any.
    fn curve_of(&self, obj: Option<O>) -> Option<&'a Curve> {
        if obj == self.handles[0] {
            return Some(self.curves[0]);
        }
        if obj == self.handles[1] {
            return Some(self.curves[1]);
        }
        for (h, c) in self.analytic_handles.iter().zip(self.analytic) {
            if let Some(c) = c {
                if obj == *h {
                    return Some(c);
                }
            }
        }
        None
    }
}

/// `sm_MoveContactsToCurveEnds` (the fork's source fork@601d804, opt-in
/// [`UpstreamFixes::curve_contacts_at_curve_ends`](crate::kernel::UpstreamFixes::curve_contacts_at_curve_ends),
/// WR-455): a point solution whose contact runs on to an end of either curve's interval
/// is at that end. It moves there when that end lies within `dist_tol` of the other
/// curve and the two curves stay within `dist_tol` of each other over the whole stretch
/// between the end and the solution, measured on each curve.
fn move_contacts_to_curve_ends<O: SolutionObject>(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    other: &Curve,
    other_interval: &Extent1d,
    dist_tol: f64,
    sols: &mut SolutionArray<O>,
) -> Result<()> {
    for ii in 0..sols.len() {
        let mut sol = sol_at(sols, ii)?;
        if sol.solution_type != SolutionType::SingleValue {
            continue;
        }
        let t1 = sol.start.parameters[0];
        let t2 = sol.start.parameters[1];
        let mut moved = false;
        // which 0: an end of this curve, dropped to the other; 1: an end of the other,
        // dropped to this
        for which in 0..2 {
            if moved {
                break;
            }
            let (end_curve, drop_curve, end_ivl, drop_ivl) = if which == 0 {
                (curve, other, interval, other_interval)
            } else {
                (other, curve, other_interval, interval)
            };
            let (sol_on_end, sol_on_drop) = if which == 0 { (t1, t2) } else { (t2, t1) };
            for end in 0..2 {
                if moved {
                    break;
                }
                let end_t = if end == 0 {
                    end_ivl.min()
                } else {
                    end_ivl.max()
                };
                if end_t == sol_on_end {
                    continue; // already there
                }
                let end_pt = end_curve.evaluate_point(k, end_t)?;
                let dropped = drop_curve.drop_point(
                    k,
                    drop_ivl,
                    end_pt,
                    None,
                    dist_tol,
                    Some(sol_on_drop),
                    SolverOperation::Minimize,
                )?;
                if !dropped.success || dropped.distance.is_nan() || dropped.distance > dist_tol {
                    continue;
                }
                let drop_t = dropped.parameter;
                // the curves within tolerance of one another over the whole stretch, on
                // each curve (one way alone passes a closed curve's whole turn against the
                // one point of a line it touches at its start)
                let mut stretch = Extent1d::from_value(end_t);
                stretch.add_value(sol_on_end);
                let mut drop_stretch = Extent1d::from_value(drop_t);
                drop_stretch.add_value(sol_on_drop);
                let (drop_at_min, drop_at_max) = if end_t < sol_on_end {
                    (drop_t, sol_on_drop)
                } else {
                    (sol_on_drop, drop_t)
                };
                let gap = end_curve.curve_max_distance_between(
                    k,
                    &stretch,
                    drop_curve,
                    drop_at_min,
                    drop_at_max,
                    0,
                    Some(dist_tol),
                )?;
                if gap.distance.is_nan() || gap.distance > dist_tol {
                    continue;
                }
                let (end_at_min, end_at_max) = if drop_t < sol_on_drop {
                    (end_t, sol_on_end)
                } else {
                    (sol_on_end, end_t)
                };
                let gap = drop_curve.curve_max_distance_between(
                    k,
                    &drop_stretch,
                    end_curve,
                    end_at_min,
                    end_at_max,
                    0,
                    Some(dist_tol),
                )?;
                if gap.distance.is_nan() || gap.distance > dist_tol {
                    continue;
                }
                sol.start.parameters[0] = if which == 0 { end_t } else { drop_t };
                sol.start.parameters[1] = if which == 0 { drop_t } else { end_t };
                sol.start.solution_value = dropped.distance;
                put_at(sols, ii, &sol)?;
                moved = true;
            }
        }
    }
    Ok(())
}

/// Runs `sm_RefineCCSolutions` on `sols` (status ignored).
fn refine<O: SolutionObject>(
    k: &Kernel,
    sols: &mut SolutionArray<O>,
    named: &Named<'_, O>,
) -> Result<()> {
    ignored(refine_cc_solutions(k, sols, |o| named.curve_of(o)))?;
    Ok(())
}

/// The analytic curve of a cache (`GetAnalyticCurve`), copied out of the cache.
fn cache_analytic<O: SolutionObject>(
    k: &Kernel,
    cache: &mut CurveCache<O>,
) -> Result<Option<Curve>> {
    Ok(cache.analytic_curve(k)?.cloned())
}

/// The cache tree (`GetTree`, `NER`).
fn cache_tree<O: SolutionObject>(cache: &CurveCache<O>) -> Result<&Tree<O>> {
    cache.tree().ok_or(SolidError::null_pointer(OP_GCI))
}

/// `GetOrCreateObjectCache` for a curve (`NER` on failure). `zone_tol_3d` is the zone
/// tolerance the caller resolved for the curve's owner: an edge curve's cache
/// tessellates at half that tolerance ([`make_curve_cache_with_zone_tol`]), as
/// upstream's `SmTol::GetApproxTol3d(&m_crCurve)` reads it from the curve's edge
/// (`SmCurveCache.cpp:1496`). `None` preserves the curve's recorded owner tolerance,
/// falling back to the system tolerance when the curve has none.
fn build_cache<O: SolutionObject>(
    k: &Kernel,
    curve: &Curve,
    handle: Option<O>,
    zone_tol_3d: Option<f64>,
) -> Result<CurveCache<O>> {
    match make_curve_cache_with_zone_tol::<O>(
        k,
        curve,
        handle,
        zone_tol_3d.or_else(|| curve.owner_zone_tol_3d()),
    ) {
        Ok(c) => Ok(c),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Err(SolidError::null_pointer(OP_GCI)),
    }
}

/// The stand-in objects of a curve/curve solve whose caller names none: this curve,
/// the other curve (the same object when `other` is this curve, as upstream's
/// pointers) and their caches' analytic replacements (reporting the natural
/// interval of the curve they replace).
pub(crate) fn operand_handles(this: &Curve, other: &Curve) -> [OperandObject; 4] {
    let this_ivl = this.natural_interval();
    let other_ivl = other.natural_interval();
    if core::ptr::eq(this, other) {
        [
            OperandObject::Curve(0, this_ivl),
            OperandObject::Curve(0, this_ivl),
            OperandObject::Curve(2, this_ivl),
            OperandObject::Curve(2, this_ivl),
        ]
    } else {
        [
            OperandObject::Curve(0, this_ivl),
            OperandObject::Curve(1, other_ivl),
            OperandObject::Curve(2, this_ivl),
            OperandObject::Curve(3, other_ivl),
        ]
    }
}

impl Curve {
    /// `GlobalCurveIntersect`: the intersection points (`start.parameters`: this
    /// curve's, then the other curve's parameter) and coincident ranges of this
    /// curve over `interval` with `other` over `other_interval` within `dist_tol`.
    /// Two analytic curves (or curves whose caches have analytic replacements) are
    /// intersected analytically when that settles the answer; curves with disjoint
    /// cache boxes have none. A single leaf pair whose intersection is far from
    /// tangency is solved alone. Otherwise a coincidence covering a whole curve is
    /// the answer, coincident ends trim the intervals, and the curve/curve global
    /// solver finds the intersections, which are then combined (touching or
    /// overlapping ranges, ranges whose gap stays within tolerance, close points),
    /// culled (short ranges next to point solutions, points next to long ranges,
    /// worse points converging to better ones) and classified (short or tangent
    /// ranges become points). The same curve object as `other` is a self
    /// intersection.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when a cache cannot be built or a solution names an
    /// unknown handle; those of the analytic intersections, the global solves, the
    /// coincidence check, the drops, the gap measure and the evaluations upstream
    /// checks; [`SolidError::Unported`] from unported callees.
    pub fn global_curve_intersect<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        dist_tol: f64,
        opts: &CurveIntersectOptions<O>,
    ) -> Result<SolutionArray<O>> {
        // no handles: upstream's solutions still name this curve and the other one
        // (whose natural intervals IdenticalSolutions reads, and Refine resolves)
        if opts.handles == [None, None] && opts.analytic_handles == [None, None] {
            let own = operand_handles(self, other);
            let inner = CurveIntersectOptions {
                skip_global_coincidence_check: opts.skip_global_coincidence_check,
                handles: [Some(own[0]), Some(own[1])],
                analytic_handles: [Some(own[2]), Some(own[3])],
            };
            return Ok(self
                .global_curve_intersect(k, interval, other, other_interval, dist_tol, &inner)?
                .without_objects());
        }
        self.global_curve_intersect_body(
            k,
            interval,
            other,
            other_interval,
            dist_tol,
            opts,
            &mut None,
            &mut None,
            [None, None],
        )
    }

    /// [`Curve::global_curve_intersect`] keeping the curves' caches in `cache1` (this
    /// curve's) and `cache2` (the other curve's; unused when `other` is this curve)
    /// across calls, as upstream keeps them on the curves (`GetOrCreateObjectCache`):
    /// a cache is built where the intersection first needs it (with the same errors)
    /// and reused by later calls given the same slot. A slot must belong to its curve,
    /// unchanged since the cache was built, and to the same handle in `opts.handles`
    /// (the cache names it in its solutions); the build is deterministic and the
    /// intersection only adds the analytic replacement a fresh cache would make, so a
    /// reused cache gives the same solutions as a fresh one. Without handles the slots
    /// are not used.
    ///
    /// # Errors
    ///
    /// As [`Curve::global_curve_intersect`].
    pub(crate) fn global_curve_intersect_cached<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        dist_tol: f64,
        opts: &CurveIntersectOptions<O>,
        cache1: &mut Option<CurveCache<O>>,
        cache2: &mut Option<CurveCache<O>>,
    ) -> Result<SolutionArray<O>> {
        self.global_curve_intersect_cached_with_zone_tols(
            k,
            interval,
            other,
            other_interval,
            dist_tol,
            opts,
            cache1,
            cache2,
            [None, None],
        )
    }

    /// [`Curve::global_curve_intersect_cached`] with the zone tolerances the caller
    /// resolved for the curves' owners. Upstream builds each curve's cache on the curve
    /// object itself, so an edge curve tessellates at half the edge's tolerance
    /// (`SmTol::GetApproxTol3d(&m_crCurve)`, `SmCurveCache.cpp:1496`); a caller whose
    /// handle geometry is not that owned object supplies the tolerance here. A slot is
    /// used exactly as in [`Curve::global_curve_intersect_cached`]: nothing is built
    /// before the body reaches it, and a `None` tolerance builds as that method does.
    /// A populated slot must also match its supplied owner tolerance. These hints
    /// govern the two intersection-tree slots only; nested coincidence and drop
    /// routines retain their existing curve-owner context.
    ///
    /// # Errors
    ///
    /// As [`Curve::global_curve_intersect`].
    pub(crate) fn global_curve_intersect_cached_with_zone_tols<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        dist_tol: f64,
        opts: &CurveIntersectOptions<O>,
        cache1: &mut Option<CurveCache<O>>,
        cache2: &mut Option<CurveCache<O>>,
        zone_tols: [Option<f64>; 2],
    ) -> Result<SolutionArray<O>> {
        if opts.handles == [None, None] && opts.analytic_handles == [None, None] {
            let own = operand_handles(self, other);
            let inner = CurveIntersectOptions {
                skip_global_coincidence_check: opts.skip_global_coincidence_check,
                handles: [Some(own[0]), Some(own[1])],
                analytic_handles: [Some(own[2]), Some(own[3])],
            };
            return Ok(self
                .global_curve_intersect_cached_with_zone_tols(
                    k,
                    interval,
                    other,
                    other_interval,
                    dist_tol,
                    &inner,
                    &mut None,
                    &mut None,
                    zone_tols,
                )?
                .without_objects());
        }
        self.global_curve_intersect_body(
            k,
            interval,
            other,
            other_interval,
            dist_tol,
            opts,
            cache1,
            cache2,
            zone_tols,
        )
    }

    /// [`Curve::global_curve_intersect`] for a caller that names no handles, keeping
    /// the curves' caches in `cache1` (this curve's) and `cache2` (the other curve's;
    /// unused when `other` is this curve) across calls, as
    /// [`Curve::global_curve_intersect_cached`] does. The caches name the stand-in
    /// handles of the unnamed solve (`operand_handles`), so a slot must belong to its
    /// curve, unchanged since the cache was built, and keep its role: `cache1` always
    /// holds a cache of a `self` curve and `cache2` one of an `other` curve.
    ///
    /// # Errors
    ///
    /// As [`Curve::global_curve_intersect`].
    pub(crate) fn global_curve_intersect_unnamed_cached<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        dist_tol: f64,
        skip_global_coincidence_check: bool,
        cache1: &mut Option<CurveCache<OperandObject>>,
        cache2: &mut Option<CurveCache<OperandObject>>,
    ) -> Result<SolutionArray<O>> {
        let own = operand_handles(self, other);
        let inner = CurveIntersectOptions {
            skip_global_coincidence_check,
            handles: [Some(own[0]), Some(own[1])],
            analytic_handles: [Some(own[2]), Some(own[3])],
        };
        Ok(self
            .global_curve_intersect_body(
                k,
                interval,
                other,
                other_interval,
                dist_tol,
                &inner,
                cache1,
                cache2,
                [None, None],
            )?
            .without_objects())
    }

    /// The body of [`Curve::global_curve_intersect`] for a caller giving handles, with
    /// the cache slots of [`Curve::global_curve_intersect_cached`]. `zone_tols` are the
    /// zone tolerances the caller resolved for the curves' owners, used only where a
    /// slot's cache is built ([`build_cache`]); `None` preserves the corresponding
    /// curve's recorded owner tolerance.
    fn global_curve_intersect_body<O: SolutionObject>(
        &self,
        k: &Kernel,
        interval: &Extent1d,
        other: &Self,
        other_interval: &Extent1d,
        dist_tol: f64,
        opts: &CurveIntersectOptions<O>,
        cache1_slot: &mut Option<CurveCache<O>>,
        cache2_slot: &mut Option<CurveCache<O>>,
        zone_tols: [Option<f64>; 2],
    ) -> Result<SolutionArray<O>> {
        let [h_this, h_other] = opts.handles;
        let same_curve = core::ptr::eq(self, other);

        // init outputs
        let mut sols: SolutionArray<O> = SolutionArray::new();

        // locals
        let mut needs_more_intersections = true;

        let base_named = Named {
            handles: opts.handles,
            curves: [self, other],
            analytic_handles: opts.analytic_handles,
            analytic: [None, None],
        };

        // special case: Line/Line, Line/Ellipse, Ellipse/Ellipse intersections
        if self.is_analytic() && other.is_analytic() {
            // get Line/Line intersection
            let r = analytic_curve_intersect(
                k,
                AnalyticOperand::Curve(self),
                interval,
                AnalyticOperand::Curve(other),
                other_interval,
                dist_tol,
                h_this,
                h_other,
            )?;
            sols.reset();
            for s in &r.solutions {
                sols.add(s);
            }
            needs_more_intersections = r.needs_more_intersections;

            // Refine Range solutions.
            refine(k, &mut sols, &base_named)?;

            // exit when Line/Line solutions are complete
            if !needs_more_intersections {
                return Ok(sols);
            }
        }

        // get target curves' caches (kept in the caller's slots, see
        // `global_curve_intersect_cached`), each at its owner's zone tolerance when the
        // caller resolved one
        if cache1_slot.is_none() {
            *cache1_slot = Some(build_cache(k, self, h_this, zone_tols[0])?);
        }
        if !same_curve && cache2_slot.is_none() {
            *cache2_slot = Some(build_cache(k, other, h_other, zone_tols[1])?);
        }
        let cache1 = cache1_slot
            .as_mut()
            .ok_or(SolidError::null_pointer(OP_GCI))?;
        let mut cache2_own = if same_curve {
            None
        } else {
            Some(
                cache2_slot
                    .as_mut()
                    .ok_or(SolidError::null_pointer(OP_GCI))?,
            )
        };

        // get curve spatial decomposition trees from each cache
        {
            let tree1 = cache_tree(cache1)?;
            let tree2 = cache_tree(cache2_own.as_deref().unwrap_or(cache1))?;

            // no work - topNode bounding boxes are disjoint
            let root1 = tree1.node(tree1.top_node().ok_or(SolidError::null_pointer(OP_GCI))?)?;
            let root2 = tree2.node(tree2.top_node().ok_or(SolidError::null_pointer(OP_GCI))?)?;
            let mut bbox = root1.bbox;
            bbox.expand_absolute(dist_tol);
            if bbox.are_disjoint(&root2.bbox, 0.0) {
                return Ok(sols);
            }
        }

        // special case: curves converted to Analytic/Analytic intersection by
        // SmCurve::CreateAnalyticCurve(). Note, GetAnalyticCurve() can be expensive,
        // so call it only as many times as necessary.
        let analy1 = cache_analytic(k, cache1)?;
        let mut analy2: Option<Curve> = None;
        if let Some(a1) = analy1.as_ref() {
            // (the same curve shares its cache, so its analytic curve is the same
            // object: a self intersection, which the line intersector skips)
            if let Some(c2) = cache2_own.as_deref_mut() {
                analy2 = cache_analytic(k, c2)?;
            }
            let a2_ref = if same_curve {
                Some(a1)
            } else {
                analy2.as_ref()
            };
            if let Some(a2) = a2_ref {
                // (the cache's analytic curve is the curve itself when it is analytic)
                let a1_handle = if self.is_analytic() {
                    h_this
                } else {
                    opts.analytic_handles[0]
                };
                let a2_handle = if other.is_analytic() {
                    h_other
                } else {
                    opts.analytic_handles[1]
                };

                // execute analytic/analytic intersection
                let r = analytic_curve_intersect(
                    k,
                    AnalyticOperand::Curve(a1),
                    interval,
                    AnalyticOperand::Curve(a2),
                    other_interval,
                    dist_tol,
                    a1_handle,
                    a2_handle,
                )?;
                sols.reset();
                for s in &r.solutions {
                    sols.add(s);
                }
                needs_more_intersections = r.needs_more_intersections;

                // exit when analytic/analytic intersection is complete
                if !needs_more_intersections {
                    return Ok(sols);
                }
            }
        }

        if !needs_more_intersections {
            return Ok(sols);
        }

        let named = Named {
            analytic: [
                if self.is_analytic() {
                    None
                } else {
                    analy1.as_ref()
                },
                if other.is_analytic() {
                    None
                } else if same_curve {
                    analy1.as_ref()
                } else {
                    analy2.as_ref()
                },
            ],
            ..base_named
        };

        let tree1 = cache_tree(cache1)?;
        let tree2 = cache_tree(cache2_own.as_deref().unwrap_or(cache1))?;

        // arrive here after line/line and analytic/analytic special cases were skipped
        // execute general curve/curve intersection solver

        // Set up a global solver to find all the leafNode/leafNode possible
        // intersection cases (see SmCCIGlobalSolver::LocalSolve,
        // SM_SO_INTERSECTION_TEST).
        let mut gs_test = CciGlobalSolver::new(
            tree1,
            tree2,
            *interval,
            *other_interval,
            false,
            ProjectionType::Parallel,
        )
        .with_curves(self, other, k);
        gs_test.solve_it(
            &SolveRequest {
                operation: SolverOperation::IntersectionTest,
                solution_requested: SolutionRequested::All,
                tolerance_3d: dist_tol,
                best_answer_so_far_sq: BIG_DOUBLE,
                opt_vectors: &[],
            },
            &mut sols,
        )?;

        // exit condition: no leafNode/leafNode possible solutions
        if sols.is_empty() {
            return Ok(sols);
        }

        // when leafNode/leafNode possible solutions were found
        {
            let sol = sol_at(&sols, 0)?;

            // check for easy cases: No Solution or 1 Solution far from tangency
            // note: sol.start[6] = number of intersecting leafNode pairs
            if sol.start.param(6)? < 1.00001 {
                // only a single node found
                gs_test
                    .base
                    .set_solver_operation(SolverOperation::Intersect);
                let mut branch: Branch = [None; MAX_SOLVER_TREES];

                // load branch with single leaf/leaf intersection SmTreeNodes
                branch[0] = sol.nodes[0];
                branch[1] = sol.nodes[1];
                sols.reset();

                // find LeafNode/LeafNode intersection (into the solver's array)
                std::mem::swap(&mut gs_test.base.solutions, &mut sols);
                let solved = gs_test.local_solve(&branch);
                std::mem::swap(&mut gs_test.base.solutions, &mut sols);
                solved?;
                let mut finished = true;

                // when just one leafNode/leafNode possible solution was found
                if sols.len() == 1 {
                    let mut sol2 = sol_at(&sols, 0)?;
                    sol2.num_objects = 2;
                    sol2.objects[0] = h_this;
                    sol2.objects[1] = h_other;
                    put_at(&mut sols, 0, &sol2)?;

                    // when solution value is only close to dist_tol and not extremely tight
                    if sol2.solution_type == SolutionType::SingleValue
                        && sol2.start.solution_value > dist_tol / 1000.0
                    {
                        // get the curve/curve intersection point positions and tangents
                        let pv1 = self.evaluate(k, sol2.start.parameters[0], 1, true, true)?;
                        let pv2 = other.evaluate(k, sol2.start.parameters[1], 1, true, true)?;
                        let t1 = at(&pv1, 1);
                        let t2 = at(&pv2, 1);

                        // get angle between curve tangents
                        // If this or other are degenerate, the angle fails
                        let ang_degree = match t1.angle_between(t2) {
                            Ok(rad) => rad * 180.0 / PI,
                            Err(_) => 90.0,
                        };

                        // when the tangents are within 20 degrees of one another
                        // (possibly coincident)
                        if ang_degree < 20.0 || ang_degree > 160.0 {
                            // clear the Finished bit
                            finished = false;
                        }
                    }
                } else if sols.len() > 1 {
                    // else several solutions: clear the Finished bit
                    finished = false;
                }

                // exit criteria - No Solution or 1 intersection Solution far from tangency
                if finished {
                    return Ok(sols);
                }
            }

            // Arrive here if the IntersectionTest found more than one solution, or
            // just one solution that is within 20 degree of tangent. Prepare for the
            // general Intersection.
            sols.reset();
        }

        // See if all of one of the curves is coincident with the other
        if !same_curve {
            // don't look for global coincidence when looking for self-intersecting curves
            let mut needs_more_work = true; // GlobalCoincidenceChecker() might set it to False.
            if !opts.skip_global_coincidence_check {
                // execute search for coincidence
                let r = self.global_coincidence_checker(
                    k,
                    interval,
                    other,
                    other_interval,
                    dist_tol,
                    false,
                    opts.handles,
                )?;
                needs_more_work = r.additional_work_needed;
                sols.reset();
                for s in &r.solutions {
                    sols.add(s);
                }
            }

            if !needs_more_work {
                return Ok(sols);
            }
        }

        // If we found solutions in the coincidence checker - we can decrease the size
        // of the intervals being checked to those portions of the curve without
        // coincidence.

        // create intervals to be trimmed to coincident region
        let mut ivl1 = *interval;
        let mut ivl2 = *other_interval;
        let mut done = false;

        // for every coincident solution
        for ii in 0..sols.len() {
            let sol = sol_at(&sols, ii)?;
            if sol.solution_type != SolutionType::RangeOfValues {
                continue;
            }
            let (s0, s1) = (sol.start.parameters[0], sol.start.parameters[1]);
            let (e0, e1) = (sol.end.parameters[0], sol.end.parameters[1]);

            // when solution starts at ivl1.min and stops short of ivl1.max
            if are_same(ivl1.min(), s0) && e0 < ivl1.max() {
                // trim ivl1 to remove solution range (status ignored)
                let _ = ivl1.set_min_max(e0, ivl1.max());

                // for zero length regions - curve is entirely coincident - no more work
                if ivl1.length().abs() < EFF_ZERO {
                    done = true;
                    break;
                }
            }

            // when solution ends at ivl1.max and starts after ivl1.min
            if are_same(ivl1.max(), e0) && ivl1.min() < s0 {
                // trim ivl1 to remove solution range (status ignored)
                let _ = ivl1.set_min_max(ivl1.min(), s0);

                // for zero length regions - curve is entirely coincident - no more work
                if ivl1.length().abs() < EFF_ZERO {
                    done = true;
                    break;
                }
            }

            // let ivl_tmp = solution range, so that we know which is min/max.
            let mut ivl_tmp = Extent1d::from_value(s1);
            ivl_tmp.add_value(e1);

            // when solution starts at ivl2.min and stops short of ivl2.max
            if are_same(ivl2.min(), ivl_tmp.min()) && ivl_tmp.max() < ivl2.max() {
                // trim ivl2 to remove solution range (status ignored)
                let _ = ivl2.set_min_max(ivl_tmp.max(), ivl2.max());

                // for zero length regions - curve is entirely coincident - no more work
                if ivl2.length().abs() < EFF_ZERO {
                    done = true;
                    break;
                }
            }

            // when solution ends at ivl2.max and starts after ivl2.min
            if are_same(ivl2.max(), ivl_tmp.max()) && ivl2.min() < ivl_tmp.min() {
                // trim (upstream: sIvl1) to remove solution range (status ignored)
                let _ = ivl1.set_min_max(ivl2.min(), ivl_tmp.min());

                // for zero length regions - curve is entirely coincident - no more work
                if ivl2.length().abs() < EFF_ZERO {
                    done = true;
                    break;
                }
            }
        }

        // exit condition - a coincidence was found that includes all of one curve or
        // the other
        if done {
            return Ok(sols);
        }

        // arrive here - need to do a general curve/curve intersection

        // Remove the GlobalCoincidenceChecker point solutions with a gap bigger than
        // SM_EFF_ZERO: the global solver finds the actual intersections next.
        for ii in (1..=sols.len()).rev() {
            let sol = sol_at(&sols, ii - 1)?;
            if sol.solution_type == SolutionType::SingleValue && sol.start.solution_value > EFF_ZERO
            {
                sols.remove_at(ii - 1, 1);
            }
        }

        // construct global solver with the curve trees
        let mut gs =
            CciGlobalSolver::new(tree1, tree2, ivl1, ivl2, false, ProjectionType::Parallel)
                .with_curves(self, other, k);

        // traverse curve and surface sub-division trees
        gs.solve_it(
            &SolveRequest {
                operation: SolverOperation::Intersect,
                solution_requested: SolutionRequested::All,
                tolerance_3d: dist_tol,
                best_answer_so_far_sq: BIG_DOUBLE,
                opt_vectors: &[],
            },
            &mut sols,
        )?;

        // Refine Range solutions.
        refine(k, &mut sols, &named)?;
        if k.fixes.curve_contacts_at_curve_ends {
            move_contacts_to_curve_ends(
                k,
                self,
                interval,
                other,
                other_interval,
                dist_tol,
                &mut sols,
            )?;
        }

        // Now sort through the intersections looking for cases to combine, split, or
        // convert from 'coincident maybe' intervals to point intersections.

        let self_intersect = core::ptr::eq(tree1, tree2);
        if self_intersect {
            // Remove all self intersections from beginning of list
            while let Some(curr) = sols.get(0) {
                if curr.start.parameters[0] > curr.start.parameters[1]
                    || (curr.start.parameters[0] - curr.start.parameters[1]).abs() < EFF_ZERO
                {
                    sols.remove_at(0, 1);
                } else {
                    break;
                }
            }
        }

        // phase 1: merge coincident intervals and point solutions close enough to be
        // the same point, compressing the array in place; keep a last interval index
        // and a last point index.
        let mut last_interval_index: usize;
        let mut last_point_index: usize;
        if sols
            .get(0)
            .is_some_and(|s| s.solution_type == SolutionType::RangeOfValues)
        {
            last_interval_index = 0;
            last_point_index = NO_INDEX;
        } else {
            last_interval_index = NO_INDEX;
            last_point_index = 0;
        }

        // We'll use these for checking when solutions are across a seam:
        let this_whole_domain = self.natural_interval();
        let other_whole_domain = other.natural_interval();

        // for every solution after the first - see if it can be combined with the
        // last processed solution of the same type
        let mut ii = 1;
        while ii < sols.len() {
            let curr = sol_at(&sols, ii)?;
            let curr_start = curr.start;
            let curr_is_range = curr.solution_type == SolutionType::RangeOfValues;

            // get index of last solution of same type as curr
            let last_index = if curr_is_range {
                last_interval_index
            } else {
                last_point_index
            };

            // get last solution
            let last_slot = if last_index == NO_INDEX {
                0
            } else {
                last_index
            };
            let mut last = sol_at(&sols, last_slot)?;
            let last_end = if last.solution_type == SolutionType::RangeOfValues {
                last.end
            } else {
                last.start
            };

            // For self intersection we have duplicates; eliminate those.
            if self_intersect
                && (curr.start.parameters[0] > curr.start.parameters[1]
                    || (curr.start.parameters[0] - curr.start.parameters[1]).abs() < EFF_ZERO)
            {
                ii += 1;
                continue;
            }

            let do_combine = self.decide_combine(
                k,
                other,
                interval,
                other_interval,
                &ivl2,
                dist_tol,
                &this_whole_domain,
                &other_whole_domain,
                self_intersect,
                last_index == NO_INDEX,
                &last,
                &last_end,
                &curr,
            )?;

            // when pair of solutions are marked for combination
            if do_combine {
                // Expand last interval to cover curr interval
                if curr_is_range {
                    if last.end.parameters[0] < curr.end.parameters[0] {
                        last.end = curr.end;
                        put_at(&mut sols, last_slot, &last)?;
                    }
                } else if curr_start.solution_value < last_end.solution_value {
                    // working with point solutions: keep the better solution
                    put_at(&mut sols, last_slot, &curr)?;
                }
                // else: take no action - which throws away curr
            } else {
                // Keep the current solution but squeeze it down into the array to
                // keep the array packed
                let next = if last_interval_index == NO_INDEX {
                    last_point_index.wrapping_add(1)
                } else if last_point_index == NO_INDEX {
                    last_interval_index.wrapping_add(1)
                } else {
                    last_interval_index.max(last_point_index).wrapping_add(1)
                };
                if curr_is_range {
                    last_interval_index = next;
                } else {
                    last_point_index = next;
                }
                if next != ii {
                    put_at(&mut sols, next, &curr)?;
                }
            }
            ii += 1;
        }

        // Set the array size after all combining has been done.
        if !sols.is_empty() {
            let last_index = if last_interval_index == NO_INDEX {
                last_point_index
            } else if last_point_index == NO_INDEX {
                last_interval_index
            } else {
                last_interval_index.max(last_point_index)
            };
            sols.set_size(last_index.wrapping_add(1));
        }

        // Phase 2: remove coincident segments which include or are adjacent to point
        // intersections (nearly tangent intersecting curves often lie within a
        // coincident segment: keep the point, discard the segment).
        Self::cull_ranges_near_points(&mut sols, tree1, tree2, interval)?;

        // Check adjacent point solutions: a 'precise' solution with a not-so-good
        // solution very close may be an artifact of the local intersector; when the
        // gap steadily decreases from the bad solution to the tight one, delete the
        // bad one. [B298] (Assumes the solutions are sorted on the first parameter.)
        Self::cull_spurious_points(
            k,
            &mut sols,
            &named,
            &this_whole_domain,
            &other_whole_domain,
        )?;

        // check each coincident interval solution: very short or diverging intervals
        // become point solutions.
        self.classify_ranges(k, &mut sols, other, interval, other_interval)?;

        Ok(sols)
    }

    /// Whether `curr` combines with `last` (the last kept solution of its type,
    /// ending at `last_end`) in [`Curve::global_curve_intersect`]: never across the
    /// ends of a closed curve; touching ends combine (with exceptions for ranges);
    /// close points combine unless across a seam; overlapping ranges combine; ranges
    /// separated by an opening combine when the curves stay within tolerance of each
    /// other through it.
    fn decide_combine<O: SolutionObject>(
        &self,
        k: &Kernel,
        other: &Self,
        interval: &Extent1d,
        other_interval: &Extent1d,
        ivl2: &Extent1d,
        dist_tol: f64,
        this_whole_domain: &Extent1d,
        other_whole_domain: &Extent1d,
        self_intersect: bool,
        no_previous: bool,
        last: &Solution<O>,
        last_end: &SolutionEnd,
        curr: &Solution<O>,
    ) -> Result<bool> {
        let curr_start = &curr.start;
        let le0 = last_end.parameters[0];
        let le1 = last_end.parameters[1];
        let cs0 = curr_start.parameters[0];
        let cs1 = curr_start.parameters[1];

        // no previous solution of same type - keep solution
        if no_previous {
            return Ok(false);
        }

        // handle start/end closed OtherCurve case
        if (le0 - cs0).abs() < EFF_ZERO_SQRT
            && (le1 - cs1).abs() * 10.0 > (other_interval.max() - other_interval.min())
        {
            // don't combine because OtherCurve is closed
            return Ok(false);
        }

        // handle start/end closed ThisCurve case
        if (le1 - cs1).abs() < EFF_ZERO_SQRT
            && (le0 - cs0).abs() * 10.0 > (interval.max() - interval.min())
        {
            // don't combine because ThisCurve is closed
            return Ok(false);
        }

        // When Solution endPoints touch on either curve - combine solutions.
        if are_same(le0, cs0) || are_same(le1, cs1) {
            let mut do_combine = true;

            // for interval solutions - make some exceptions
            if curr.solution_type == SolutionType::RangeOfValues {
                let start_dev1 = last.start.solution_value;
                let end_dev1 = last.end.solution_value;

                // don't combine when end of last is much better solution than start
                if start_dev1 > EFF_ZERO && start_dev1 > end_dev1 * 100000.0 {
                    do_combine = false;
                }
                let start_dev2 = curr.start.solution_value;
                let end_dev2 = curr.end.solution_value;

                // don't combine when start of curr is much better solution than end
                if end_dev2 > EFF_ZERO && end_dev2 > start_dev2 * 100000.0 {
                    do_combine = false;
                }

                // If the smaller segment is less than 1/1000 of the other then just
                // combine them anyhow.
                let last_leng = max2(EFF_ZERO, last.end.parameters[0] - last.start.parameters[0]);
                let curr_leng = max2(EFF_ZERO, curr.end.parameters[0] - curr.start.parameters[0]);

                if last_leng > curr_leng * 1000.00 || curr_leng > last_leng * 1000.00 {
                    do_combine = true;
                }
            }
            return Ok(do_combine);
        }

        // for point solutions only
        if curr.solution_type == SolutionType::SingleValue {
            // evaluate solution points (statuses ignored)
            let pt1 = point_or_undef(k, self, cs0)?;
            let pt2 = point_or_undef(k, self, le0)?;
            let dist2 = pt1.distance_squared(pt2);

            // combine solutions when points are close enough together
            if dist2 < dist_tol * dist_tol {
                // Same position, check whether they're at opposite ends of the domain
                // (seam). [B88]
                let mut across_seam = false;
                if (cs0 - le0).abs() > 0.9 * this_whole_domain.length() {
                    across_seam = true;
                }
                if (cs1 - le1).abs() > 0.9 * other_whole_domain.length() {
                    across_seam = true;
                }
                if !across_seam {
                    return Ok(true);
                }
            }
        }

        // for range solutions only
        if curr.solution_type == SolutionType::RangeOfValues {
            // When SolutionIntervals overlap on both curves - combine solutions
            if le0 >= cs0 {
                return Ok(is_between(
                    cs1,
                    last.start.parameters[1],
                    last.end.parameters[1],
                ));
            }

            // arrive here when Solution Intervals are separated by an opening; see if
            // the curves are coincident throughout the solution opening

            // get SolutionOpening = curve parameter intervals from last end to curr start
            let mut interval1 = Extent1d::new();
            interval1.add_value(le0);
            interval1.add_value(cs0);
            let mut interval2 = Extent1d::new();
            interval2.add_value(le1);
            interval2.add_value(cs1);

            // Evaluate and Drop a Curve1 SolutionOpening MidPoint to Curve2
            let pt_on1 = self.evaluate_point(k, interval1.evaluate(0.45))?;
            let guess = interval2.evaluate(0.5);
            let dropped = other.drop_point(
                k,
                &interval2,
                pt_on1,
                None,
                dist_tol,
                Some(guess),
                SolverOperation::Minimize,
            )?;

            // don't combine solutions when SolutionGap MidPoint does not drop
            if !dropped.success {
                return Ok(false);
            }

            if self_intersect {
                // with self intersection the orientation must be opposite
                let pv1 = self.evaluate(k, interval1.evaluate(0.45), 1, true, true)?;
                let pv2 = other.evaluate(k, dropped.parameter, 1, true, true)?;
                let t1 = at(&pv1, 1);
                let t2 = at(&pv2, 1);
                if t1.dot(t2) > 0.0 {
                    return Ok(false); // same orientation can not be self intersection
                }
            }

            // Evaluate and drop a Curve2 SolutionOpening nearMidPoint to Curve1
            // (upstream evaluates the trimmed interval sIvl2)
            let pt_on2 = other.evaluate_point(k, ivl2.evaluate(0.38))?;
            let guess = interval1.evaluate(0.5);
            let dropped = self.drop_point(
                k,
                &interval1,
                pt_on2,
                None,
                dist_tol,
                Some(guess),
                SolverOperation::Minimize,
            )?;

            // don't combine solutions when nearMidPoint fails to drop
            if !dropped.success {
                return Ok(false);
            }

            // If SolutionOpening Interval is less than 1000th of curve intervals
            // assume the curves are coincident between solutions and combine
            if interval1.length() < interval.length() / 1000.0
                && interval2.length() < other_interval.length() / 1000.0
            {
                return Ok(true);
            }

            // look for curve/curve distance at any maxima point within the opening
            let max_degree = self.degree()?.max(other.degree()?);
            let gap = self.curve_max_distance_between(
                k,
                &interval1,
                other,
                le1,
                cs1,
                max_degree as u64 + 3,
                Some(dist_tol),
            )?;

            // when curve/curve gap distance is less than tolerance at maxima
            if gap.distance < dist_tol {
                return Ok(true);
            }
        }

        // If made it to here then we don't combine
        Ok(false)
    }

    /// Phase 2 of [`Curve::global_curve_intersect`]: marks every range solution next
    /// to or containing a point solution (by the point's subdivision spans) for
    /// removal when short (else marks the point when it is inside the range, or close
    /// to an end and twice worse), then compresses the array, turning culled ranges
    /// with exact ends into point solutions.
    fn cull_ranges_near_points<O: SolutionObject>(
        sols: &mut SolutionArray<O>,
        tree1: &Tree<O>,
        tree2: &Tree<O>,
        interval: &Extent1d,
    ) -> Result<()> {
        let num_sols = sols.len();
        let mut cull = vec![false; num_sols];
        let mut compress = false;

        // search for interval solutions
        for ii in 0..num_sols {
            let inv_sol = sol_at(sols, ii)?;

            // skip point-solutions solutions
            if inv_sol.solution_type != SolutionType::RangeOfValues {
                continue;
            }

            let inv_start = inv_sol.start;
            let inv_end = inv_sol.end;
            let mut inv_span1 = Extent1d::new();
            inv_span1.add_value(inv_start.parameters[0]);
            inv_span1.add_value(inv_end.parameters[0]);
            let mut inv_span2 = Extent1d::new();
            inv_span2.add_value(inv_start.parameters[1]);
            inv_span2.add_value(inv_end.parameters[1]);

            // search for point solutions within or adjacent to this interval solution
            for jj in 0..num_sols {
                if ii == jj {
                    continue;
                }

                let pt_sol = sol_at(sols, jj)?;

                // skip interval solutions
                if pt_sol.solution_type != SolutionType::SingleValue {
                    continue;
                }

                let pt_start = pt_sol.start;

                // Get the cache subdivision intervals containing this point if
                // available
                let pt_ivl1 = match pt_sol.nodes[0] {
                    Some(n) => tree1.curve_aux(tree1.node(n)?)?.interval,
                    None => Extent1d::from_min_max(pt_start.parameters[0], pt_start.parameters[0]),
                };
                let pt_ivl2 = match pt_sol.nodes[1] {
                    Some(n) => tree2.curve_aux(tree2.node(n)?)?.interval,
                    None => Extent1d::from_min_max(pt_start.parameters[1], pt_start.parameters[1]),
                };

                // check for overlap between Interval SolSpan and Point SubDivisionSpan
                // (a disjoint intersection is Init())
                let xsect1 = pt_ivl1
                    .intersect(&inv_span1)
                    .unwrap_or_else(|_| Extent1d::new());
                let xsect2 = pt_ivl2
                    .intersect(&inv_span2)
                    .unwrap_or_else(|_| Extent1d::new());

                if xsect1.is_init() || xsect2.is_init() {
                    continue;
                }

                // Now InvSpan is adjacent to or contains solution or its sub-division
                // end-points. Delete the range interval only if it's 'short': 1/1000
                // of the whole domain.
                let close_param = interval.length() / 1000.0;

                if inv_span1.length() < close_param {
                    // Delete the range interval.
                    if let Some(c) = cull.get_mut(ii) {
                        *c = true;
                    }
                    compress = true;
                } else {
                    // Range is long enough. See about deleting the point solution.
                    let pt_param1 = pt_sol.start.parameters[0];
                    let delete_point = if pt_param1 < inv_span1.min() {
                        let dist = inv_span1.min() - pt_param1;
                        dist < close_param
                            && pt_sol.start.solution_value > 2.0 * inv_start.solution_value
                    } else if pt_param1 > inv_span1.max() {
                        let dist = pt_param1 - inv_span1.max();
                        dist < close_param
                            && pt_sol.start.solution_value > 2.0 * inv_end.solution_value
                    } else {
                        // Point sol is inside the range sol: delete it.
                        true
                    };
                    if delete_point {
                        if let Some(c) = cull.get_mut(jj) {
                            *c = true;
                        }
                        compress = true;
                    }
                }

                if cull.get(ii).copied().unwrap_or(false) {
                    break;
                }

                // quit searching when point solutions get past interval solutions
                if pt_start.parameters[0] > inv_span1.max()
                    || pt_start.parameters[1] > inv_span2.max()
                {
                    break;
                }
            }
        }

        // compress the solutions array to remove all unwanted interval solutions;
        // culled ranges with very accurate ends become point solutions.
        if compress {
            let mut next_index = 0;
            let mut ii = 0;
            while ii < sols.len() {
                // omit solutions to drop
                if cull.get(ii).copied().unwrap_or(false) {
                    let mut sol = sol_at(sols, ii)?;
                    // except when endPoints are good enough to keep
                    if sol.solution_type == SolutionType::RangeOfValues {
                        let start_dev = sol.start.solution_value;
                        let end_dev = sol.end.solution_value;

                        if start_dev < EFF_ZERO && end_dev < EFF_ZERO {
                            // change range solution into 2 point solutions
                            let mut first = sol;
                            first.solution_type = SolutionType::SingleValue;
                            sol.solution_type = SolutionType::SingleValue;
                            sol.start = sol.end;
                            put_at(sols, ii, &sol)?;
                            sols.insert_at(ii + 1, &first, 1);
                            if let Some(c) = cull.get_mut(ii) {
                                *c = false;
                            }
                            let at = (ii + 1).min(cull.len());
                            cull.insert(at, false);
                        } else if start_dev < EFF_ZERO {
                            // change range solution into point solution
                            sol.solution_type = SolutionType::SingleValue;
                            put_at(sols, ii, &sol)?;
                            if let Some(c) = cull.get_mut(ii) {
                                *c = false;
                            }
                        } else if end_dev < EFF_ZERO {
                            // change range solution into point solution
                            sol.solution_type = SolutionType::SingleValue;
                            sol.start = sol.end;
                            put_at(sols, ii, &sol)?;
                            if let Some(c) = cull.get_mut(ii) {
                                *c = false;
                            }
                        } else {
                            // all done with this solution - omit it
                            ii += 1;
                            continue;
                        }
                    } else {
                        // all done with this solution - omit it
                        ii += 1;
                        continue;
                    }
                }

                // move current solution to next open array spot
                if ii != next_index {
                    let s = sol_at(sols, ii)?;
                    put_at(sols, next_index, &s)?;
                }
                next_index += 1;
                ii += 1;
            }

            // Set the array size after discarding unwanted interval solutions.
            if !sols.is_empty() {
                sols.set_size(next_index);
            }
        }
        Ok(())
    }

    /// The spurious point check of [`Curve::global_curve_intersect`]: of two nearby
    /// point solutions with very different values, the worse is removed when the
    /// curves' gap at the midpoint is smaller than at the worse one.
    fn cull_spurious_points<O: SolutionObject>(
        k: &Kernel,
        sols: &mut SolutionArray<O>,
        named: &Named<'_, O>,
        domain_a: &Extent1d,
        domain_b: &Extent1d,
    ) -> Result<()> {
        let max_param_delta_a = domain_a.length() / 100.0; // mainly to avoid seams.
        let max_param_delta_b = domain_b.length() / 100.0;
        let num_sols = sols.len();
        for ii in 0..num_sols.saturating_sub(1) {
            let sol_a = sol_at(sols, ii)?;

            // Check only single solutions.
            if sol_a.solution_type != SolutionType::SingleValue {
                continue;
            }

            // Skip already-marked solutions (see below).
            if sol_a.start.solution_value > BIG_DOUBLE / 2.0 {
                continue;
            }

            let param_a1 = sol_a.start.parameters[0];
            let param_b1 = sol_a.start.parameters[1];

            // Double-loop on all solution pairs.
            for jj in ii + 1..num_sols {
                // (re-read: an earlier pair may have marked it)
                let sol_a = sol_at(sols, ii)?;
                let sol_b = sol_at(sols, jj)?;

                // Check only single solutions.
                if sol_b.solution_type != SolutionType::SingleValue {
                    continue;
                }

                // Skip already-'deleted' solutions (see below).
                if sol_b.start.solution_value > BIG_DOUBLE / 2.0 {
                    continue;
                }

                let param_a2 = sol_b.start.parameters[0];
                let param_b2 = sol_b.start.parameters[1];

                if (param_a2 - param_a1).abs() > max_param_delta_a {
                    break; // go on to next outer-loop iteration.
                }
                if (param_b2 - param_b1).abs() > max_param_delta_b {
                    break;
                }

                // Ok, the sols are fairly close on both curves. Check solution
                // values: we're looking for the case where they are very different.
                let val_a = sol_a.start.solution_value;
                let val_b = sol_b.start.solution_value;
                if val_a < val_b * 100.0 && val_a > val_b / 100.0 {
                    continue;
                }

                // Ok, one sol is much better than the other. Check the distances at
                // both solutions and at the midpoint.
                let curve_a = named
                    .curve_of(sol_a.objects[0])
                    .ok_or(SolidError::null_pointer(OP_GCI))?;
                let curve_b = named
                    .curve_of(sol_a.objects[1])
                    .ok_or(SolidError::null_pointer(OP_GCI))?;
                let pt_a = point_or_undef(k, curve_a, param_a1)?;
                let pt_b = point_or_undef(k, curve_b, param_b1)?;
                let dist1 = pt_a.distance(pt_b);

                let pt_a = point_or_undef(k, curve_a, param_a2)?;
                let pt_b = point_or_undef(k, curve_b, param_b2)?;
                let dist2 = pt_a.distance(pt_b);

                let t = (param_a1 + param_a2) / 2.0;
                let pt_a = point_or_undef(k, curve_a, t)?;
                let t = (param_b1 + param_b2) / 2.0;
                let pt_b = point_or_undef(k, curve_b, t)?;
                let dist_mid = pt_a.distance(pt_b);

                if dist1 < dist2 && dist_mid < dist2 {
                    // Remove the second solution (marked; deleted afterwards).
                    let mut s = sol_b;
                    s.start.solution_value = BIG_DOUBLE;
                    put_at(sols, jj, &s)?;
                } else if dist2 < dist1 && dist_mid < dist1 {
                    // Remove the first solution.
                    let mut s = sol_a;
                    s.start.solution_value = BIG_DOUBLE;
                    put_at(sols, ii, &s)?;
                }
            }
        }

        // Now remove all marked solutions.
        let mut done = false;
        while !done {
            done = true;
            for ii in 0..sols.len() {
                if sol_at(sols, ii)?.start.solution_value > BIG_DOUBLE / 2.0 {
                    sols.remove_at(ii, 1);
                    done = false;
                    break;
                }
            }
        }
        Ok(())
    }

    /// The range solution check of [`Curve::global_curve_intersect`]: ranges with
    /// exact ends are kept; very short ranges and ranges classified as tangent points
    /// (`ClassifyIntersectingRangeSolution`) become point solutions; long ranges are
    /// kept.
    fn classify_ranges<O: SolutionObject>(
        &self,
        k: &Kernel,
        sols: &mut SolutionArray<O>,
        other: &Self,
        interval: &Extent1d,
        other_interval: &Extent1d,
    ) -> Result<()> {
        // locals: curve/solution metrics
        let mut this_length = BIG_DOUBLE;
        let mut other_length = BIG_DOUBLE;
        let mut is_degenerate = false;

        // for every SM_ST_RANGE_OF_VALUES solution
        for ii in 0..sols.len() {
            let mut curr = sol_at(sols, ii)?;

            // skip SM_ST_SINGLE_VALUE solutions
            if curr.solution_type != SolutionType::RangeOfValues {
                continue;
            }

            // locals - intersection StartPoint, Start/End Deviations, Interval
            // ParameterLengths
            let dev_start = curr.start.solution_value;
            let dev_end = curr.end.solution_value;
            let mut ivl = Extent1d::from_value(curr.start.parameters[0]);
            ivl.add_value(curr.end.parameters[0]);
            let mut o_ivl = Extent1d::from_value(curr.start.parameters[1]);
            o_ivl.add_value(curr.end.parameters[1]);
            let ivl_length = ivl.length();
            let o_ivl_length = o_ivl.length();
            let pnt = self.evaluate_point(k, curr.start.parameters[0])?;

            // locals: Curve 3d and solution range lengths
            if this_length == BIG_DOUBLE {
                this_length = self.approximate_length(k, interval, 5, false)?.0;
            }
            if other_length == BIG_DOUBLE {
                other_length = other.approximate_length(k, other_interval, 5, false)?.0;
            }
            let seg_leng = self.approximate_length(k, &ivl, 5, false)?.0;

            // Scaled Zeros in 3D and both Parameter spaces
            let scaled_zero = EFF_ZERO * (1.0 + pnt.max_dimension());
            let scaled_ps_tol = 10000.0 * EFF_ZERO * (1.0 + interval.max_dimension());
            let scaled_ps_tol2 = 10000.0 * EFF_ZERO * (1.0 + other_interval.max_dimension());

            // keep nonZero intervals whose ends are exact intersections to ScaledZero
            if dev_start < scaled_zero
                && dev_end < scaled_zero
                && ivl_length > scaled_ps_tol
                && o_ivl_length > scaled_ps_tol2
            {
                continue;
            }

            // See if solution range is a large interval: at least 1/10 the length of
            // the shorter curve.
            let large_interval = seg_leng > min2(this_length, other_length) / 10.0;

            // See if solution range is a small interval: less than ScaledZero in
            // either curve domain
            let small_interval = ivl_length < scaled_ps_tol || o_ivl_length < scaled_ps_tol2;

            // for nonDegenerate intervals
            if !small_interval {
                // if GapVector is diverging over length of solution range change
                // solution to a tangent point intersection (status ignored)
                let (deg, point) =
                    ignored(self.classify_intersecting_range_solution(k, other, &curr))?
                        .unwrap_or((false, None));
                is_degenerate = deg;
                if let Some(p) = point {
                    curr.start.solution_value = p.solution_value;
                    curr.start.parameters[0] = p.parameters[0];
                    curr.start.parameters[1] = p.parameters[1];
                }
                put_at(sols, ii, &curr)?;
            }

            // keep interval if it is at least 1/10 the length of the shorter curve.
            if large_interval && !is_degenerate {
                continue;
            }

            // Change very small and diverging intervals into a single pt solutions.
            if small_interval || is_degenerate {
                // When working with short solution intervals with good solutions on
                // both ends load the one closest to a curve interval boundary, else
                // load the solution with smallest deviation into start
                if small_interval
                    && curr.start.solution_value < EFF_ZERO
                    && curr.end.solution_value < EFF_ZERO
                {
                    let del_start0 = curr.start.parameters[0] - interval.min();
                    let del_end0 = interval.max() - curr.end.parameters[0];

                    let del_start_beg1 = curr.start.parameters[1] - other_interval.min();
                    let del_start_end1 = other_interval.max() - curr.start.parameters[1];
                    let del_end_beg1 = curr.end.parameters[1] - other_interval.min();
                    let del_end_end1 = other_interval.max() - curr.end.parameters[1];
                    let del1_min = min4(del_start_beg1, del_start_end1, del_end_beg1, del_end_end1);

                    let at_start_of_1st_curve = del_start0 < EFF_ZERO;
                    let at_end_of_1st_curve = del_end0 < EFF_ZERO;
                    let other_1st_closest_to_start = del_end_beg1 == del1_min;
                    let other_1st_closest_to_end = del_end_end1 == del1_min;

                    // use the end Solution when it does not start on the 1st curve
                    // beg param and ends on the 1st curve end param or on the 2nd
                    // curve beg/end param
                    let use_end_solution = !at_start_of_1st_curve
                        && (at_end_of_1st_curve
                            || other_1st_closest_to_start
                            || other_1st_closest_to_end);
                    if use_end_solution {
                        curr.start = curr.end;
                    }
                } else if curr.start.solution_value > curr.end.solution_value {
                    curr.start = curr.end;
                }

                // change the solutionType
                curr.solution_type = SolutionType::SingleValue;
                put_at(sols, ii, &curr)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::hermite_curve::HermiteCurve;
    use crate::geom::solution_array::ObjectKind;
    use crate::geom::vector3d::Vec3;
    use crate::nurbs::b_spline_curve::BSplineCurve;
    use crate::nurbs::nurbs_crv::allocate_nurb_curve;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct H(u8);

    impl SolutionObject for H {
        fn kind(&self) -> ObjectKind {
            ObjectKind::Curve
        }
    }

    fn polyline(pts: &[[f64; 3]]) -> Curve {
        let n = pts.len() - 1;
        let mut cur = allocate_nurb_curve(n, 1, n + 2).unwrap();
        for (d, s) in cur.cpts_mut().iter_mut().zip(pts) {
            d.x = s[0];
            d.y = s[1];
            d.z = s[2];
            d.w = -f64::MAX;
        }
        let mut u = vec![0.0, 0.0];
        for i in 1..n {
            u.push(i as f64 / n as f64);
        }
        u.extend([1.0, 1.0]);
        cur.knots_mut().copy_from_slice(&u);
        Curve::from(BSplineCurve::from_nurb(cur, 3, false))
    }

    fn opts() -> CurveIntersectOptions<H> {
        CurveIntersectOptions {
            handles: [Some(H(0)), Some(H(1))],
            analytic_handles: [Some(H(2)), Some(H(3))],
            ..CurveIntersectOptions::default()
        }
    }

    fn unit() -> Extent1d {
        Extent1d::from_min_max(0.0, 1.0)
    }

    #[test]
    fn helpers() {
        assert!(is_between(0.5, 0.0, 1.0));
        assert!(is_between(0.5, 1.0, 0.0));
        assert!(!is_between(0.0, 0.0, 1.0));
        assert_eq!(min4(3.0, 2.0, 4.0, 1.0), 1.0);
    }

    #[test]
    fn far_apart_curves_have_no_solutions() {
        let k = Kernel::default();
        let a = polyline(&[[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 0.0, 0.0]]);
        let b = polyline(&[[0.0, 50.0, 0.0], [1.0, 51.0, 0.0], [2.0, 50.0, 0.0]]);
        match a.global_curve_intersect(&k, &unit(), &b, &unit(), 1e-6, &opts()) {
            Ok(s) => assert!(s.is_empty()),
            Err(e) => assert!(e.is_unported()),
        }
    }

    #[test]
    fn crossing_hermites() {
        let k = Kernel::default();
        let a = Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            3,
        ));
        let b = Curve::from(HermiteCurve::new(
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 2.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            3,
        ));
        // reaches the Newton intersection (unit curve.curve_p5) or finds the crossing
        match a.global_curve_intersect(&k, &unit(), &b, &unit(), 1e-6, &opts()) {
            Ok(s) => assert!(!s.is_empty()),
            Err(e) => assert!(e.is_unported()),
        }
    }

    /// Reused cache slots give the solutions of fresh caches, call after call, also
    /// for a self intersection (one slot).
    #[test]
    fn cached_slots_match_fresh_caches() {
        let k = Kernel::default();
        let a = Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            3,
        ));
        let b = polyline(&[[0.0, 0.2, 0.0], [1.0, 0.1, 0.0], [2.0, 0.3, 0.0]]);
        let fresh = a.global_curve_intersect(&k, &unit(), &b, &unit(), 1e-6, &opts());
        let fresh_self = a.global_curve_intersect(&k, &unit(), &a, &unit(), 1e-6, &opts());
        let (mut slot_a, mut slot_b) = (None, None);
        for _ in 0..3 {
            let cached = a.global_curve_intersect_cached(
                &k,
                &unit(),
                &b,
                &unit(),
                1e-6,
                &opts(),
                &mut slot_a,
                &mut slot_b,
            );
            assert_eq!(format!("{cached:?}"), format!("{fresh:?}"));
            let cached_self = a.global_curve_intersect_cached(
                &k,
                &unit(),
                &a,
                &unit(),
                1e-6,
                &opts(),
                &mut slot_a,
                &mut None,
            );
            assert_eq!(format!("{cached_self:?}"), format!("{fresh_self:?}"));
        }
        if fresh.is_ok() {
            assert!(slot_a.is_some() && slot_b.is_some());
        }
    }

    /// The unnamed solve with reused cache slots gives the solutions of the
    /// uncached unnamed solve, call after call.
    #[test]
    fn unnamed_cached_slots_match_fresh_caches() {
        let k = Kernel::default();
        let a = Curve::from(HermiteCurve::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            3,
        ));
        let b = polyline(&[[0.0, 0.2, 0.0], [1.0, 0.1, 0.0], [2.0, 0.3, 0.0]]);
        let none = CurveIntersectOptions::<H>::default();
        let fresh = a.global_curve_intersect(&k, &unit(), &b, &unit(), 1e-6, &none);
        let (mut slot_a, mut slot_b) = (None, None);
        for _ in 0..3 {
            let cached: Result<SolutionArray<H>> = a.global_curve_intersect_unnamed_cached(
                &k,
                &unit(),
                &b,
                &unit(),
                1e-6,
                false,
                &mut slot_a,
                &mut slot_b,
            );
            assert_eq!(format!("{cached:?}"), format!("{fresh:?}"));
        }
        if fresh.is_ok() {
            assert!(slot_a.is_some() && slot_b.is_some());
        }
    }
}
