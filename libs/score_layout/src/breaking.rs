//! Optimal line (system) and page breaking.
//!
//! Greedy packing is unstable: a one-measure edit can leave a sparse last
//! system. Here every legal boundary is a graph node and dynamic programming
//! chooses globally. The edge cost for a candidate system combines cubic
//! stretch badness in the Knuth-Plass demerits form, break-semantics
//! penalties supplied per boundary, a density-continuity term that keeps
//! adjacent systems equally gray, spanner-disruption penalties, and
//! final-system fill rules.
//!
//! The adjustment ratio of the previous system enters the cost only through
//! one of four *fitness classes*, each with a representative ratio; the DP
//! state is `(boundary, class)`, which keeps it small and — because the cost
//! then depends on nothing else — makes the DP **exactly optimal** for the
//! quantized cost (verified against exhaustive search in tests).
//!
//! Line breaking can return several good complete alternatives, not just
//! one: a locally excellent system set can produce an orphan page, so the
//! page breaker must be allowed to choose among near-optimal line solutions
//! ([`plan_document`] wires the two levels together).
//!
//! Complexity: O(n · w · c²· k) for `n` measures, feasible window `w`
//! (bounded by rod minima against the line width and by a hard style cap),
//! `c = 4` classes and `k` kept alternatives. A counting test pins this
//! down on a long score.

use crate::sp::Sp;
use crate::spacing::SpacingColumn;
use crate::style::BreakStyle;
use std::ops::Range;

/// Break semantics at a measure's right boundary.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum BreakRule {
    /// An ordinary legal boundary.
    #[default]
    Allowed,
    /// A mandatory break (explicit system/page break): edges may not span
    /// it. Modeled as a hard constraint rather than a reward, which is
    /// equivalent and keeps costs finite.
    Forced,
    /// Breaking here is illegal (e.g. mid-measure-group).
    Forbidden,
    /// Legal but charged (positive: inside a beam/tuplet/slur; negative:
    /// a strong section boundary that welcomes a break).
    Penalty(f64),
}

/// Cached spacing summary of one measure, the unit of break decisions.
#[derive(Clone, PartialEq, Debug)]
pub struct MeasureSpacing {
    /// Natural width: sum of the measure's column widths at zero force.
    pub natural: Sp,
    /// The smallest width any force can reach (rod floor).
    pub minimum: Sp,
    /// Stretch capacity: width gained at unit stretching force.
    pub stretch: Sp,
    /// Shrink capacity: `natural - minimum` (positive).
    pub shrink: Sp,
    /// Break semantics at this measure's right boundary.
    pub break_right: BreakRule,
    /// Cost of breaking at the right boundary while spanners (slurs, ties,
    /// hairpins, octave lines) would be split there.
    pub spanner_penalty: f64,
}

impl MeasureSpacing {
    /// Summarize a measure's onset columns for the breaker.
    pub fn from_columns(
        columns: &[SpacingColumn],
        break_right: BreakRule,
        spanner_penalty: f64,
    ) -> MeasureSpacing {
        let natural: f64 = columns.iter().map(|c| c.minimum.0.max(c.natural.0)).sum();
        let floor: f64 = columns
            .iter()
            .map(|c| if c.shrink_flex > 0.0 { c.minimum.0 } else { c.minimum.0.max(c.natural.0) })
            .sum();
        let stretch: f64 = columns.iter().map(|c| c.stretch_flex).sum();
        MeasureSpacing {
            natural: Sp(natural),
            minimum: Sp(floor),
            stretch: Sp(stretch),
            shrink: Sp(natural - floor),
            break_right,
            spanner_penalty,
        }
    }
}

/// Usable music widths per system. The first system is often indented
/// (instrument names); all others share one width.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LineWidths {
    /// Usable width of the first system.
    pub first: Sp,
    /// Usable width of every following system.
    pub rest: Sp,
}

impl LineWidths {
    /// The same width everywhere.
    pub fn uniform(w: Sp) -> LineWidths {
        LineWidths { first: w, rest: w }
    }

    pub(crate) fn at(&self, start: usize) -> Sp {
        if start == 0 {
            self.first
        } else {
            self.rest
        }
    }
}

/// One chosen system of a line-break solution.
#[derive(Clone, PartialEq, Debug)]
pub struct SystemPlan {
    /// The measures on this system.
    pub measures: Range<usize>,
    /// The usable width this system is justified to.
    pub target: Sp,
    /// Natural width of its content.
    pub natural: Sp,
    /// Minimum (rod-floor) width of its content.
    pub minimum: Sp,
    /// Adjustment ratio actually applied (0 for a ragged final system).
    pub adjustment: f64,
    /// False for a ragged final system left at natural width.
    pub justified: bool,
    /// This system's edge cost (its share of the plan total).
    pub cost: f64,
}

impl SystemPlan {
    /// The width the system should actually occupy: the target when
    /// justified, the natural width when ragged.
    pub fn solved_width(&self) -> Sp {
        if self.justified {
            self.target
        } else {
            self.natural
        }
    }
}

/// One complete line-break alternative.
#[derive(Clone, PartialEq, Debug)]
pub struct LinePlan {
    /// The systems, in order; their ranges tile `0..n`.
    pub systems: Vec<SystemPlan>,
    /// Total cost (sum of system costs).
    pub total_cost: f64,
}

/// Result of line breaking.
#[derive(Clone, PartialEq, Debug)]
pub struct LineBreakResult {
    /// Up to the requested number of best alternatives, cheapest first.
    /// Empty only when no legal solution exists even in the emergency pass.
    pub alternatives: Vec<LinePlan>,
    /// Number of DP edges evaluated — the complexity tests assert on it.
    pub edges_evaluated: usize,
    /// True when the strict pass found no feasible solution and infeasible
    /// stretch ratios were admitted at saturated badness.
    pub emergency: bool,
}

/// Which fitness class an adjustment ratio falls in.
pub(crate) fn class_of(r: f64) -> usize {
    if r < -0.5 {
        0
    } else if r < 0.5 {
        1
    } else if r < 1.0 {
        2
    } else {
        3
    }
}

/// Everything about one candidate edge except the continuity term.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EdgeCore {
    pub r_used: f64,
    pub class: usize,
    pub base_cost: f64,
    pub justified: bool,
    pub target: Sp,
    pub natural: Sp,
    pub minimum: Sp,
}

/// Shared evaluation context: prefix sums make each edge O(1).
pub(crate) struct EdgeContext<'a> {
    measures: &'a [MeasureSpacing],
    pref_nat: Vec<f64>,
    pref_min: Vec<f64>,
    pref_stretch: Vec<f64>,
    pref_shrink: Vec<f64>,
    widths: LineWidths,
    style: &'a BreakStyle,
    emergency: bool,
}

impl<'a> EdgeContext<'a> {
    pub(crate) fn new(
        measures: &'a [MeasureSpacing],
        widths: LineWidths,
        style: &'a BreakStyle,
        emergency: bool,
    ) -> EdgeContext<'a> {
        let n = measures.len();
        let mut pref_nat = vec![0.0; n + 1];
        let mut pref_min = vec![0.0; n + 1];
        let mut pref_stretch = vec![0.0; n + 1];
        let mut pref_shrink = vec![0.0; n + 1];
        for (k, m) in measures.iter().enumerate() {
            pref_nat[k + 1] = pref_nat[k] + m.natural.0;
            pref_min[k + 1] = pref_min[k] + m.minimum.0;
            pref_stretch[k + 1] = pref_stretch[k] + m.stretch.0;
            pref_shrink[k + 1] = pref_shrink[k] + m.shrink.0;
        }
        EdgeContext { measures, pref_nat, pref_min, pref_stretch, pref_shrink, widths, style, emergency }
    }

    pub(crate) fn measure_count(&self) -> usize {
        self.measures.len()
    }

    /// Minimum (rod-floor) width of measures `i..j`.
    pub(crate) fn min_width(&self, i: usize, j: usize) -> f64 {
        self.pref_min[j] - self.pref_min[i]
    }

    /// Evaluate the edge for a system holding measures `i..j`, ending the
    /// score iff `j == n`. Returns `None` when infeasible.
    pub(crate) fn core(&self, i: usize, j: usize) -> Option<EdgeCore> {
        let st = self.style;
        let n = self.measures.len();
        let is_final = j == n;
        let target = self.widths.at(i);
        let natural = self.pref_nat[j] - self.pref_nat[i];
        let minimum = self.pref_min[j] - self.pref_min[i];
        let surplus = target.0 - natural;
        let r = if surplus >= 0.0 {
            let cap = self.pref_stretch[j] - self.pref_stretch[i];
            if cap > 0.0 {
                surplus / cap
            } else if surplus == 0.0 {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            let cap = self.pref_shrink[j] - self.pref_shrink[i];
            if cap > 0.0 {
                surplus / cap
            } else {
                f64::NEG_INFINITY
            }
        };

        // Ragged final system: below the justify threshold the last system
        // keeps its natural width; sparseness below the fill floor is
        // charged instead of stretch badness.
        let fill = if target.0 > 0.0 { natural / target.0 } else { 1.0 };
        let ragged = is_final && surplus > 0.0 && fill < st.last_fill_justify;

        let mut cost;
        let r_used;
        if ragged {
            r_used = 0.0;
            let short = (st.last_fill_min - fill).max(0.0);
            cost = st.demerit_offset * st.demerit_offset + st.last_fill_weight * short * short;
        } else {
            let feasible = r.is_finite() && r >= st.min_ratio && r <= st.max_ratio;
            if !feasible && !self.emergency {
                return None;
            }
            r_used = if r.is_finite() {
                r.clamp(st.min_ratio, st.max_ratio)
            } else if r > 0.0 {
                st.max_ratio
            } else {
                st.min_ratio
            };
            // An infeasible edge admitted by the emergency pass pays the
            // saturated badness, not the badness of its clamped ratio.
            let b = if feasible {
                (st.badness_scale * r.abs().powi(3)).min(st.badness_cap)
            } else {
                st.badness_cap
            };
            let d = st.demerit_offset + b;
            let over = (r_used.abs() - 1.0).max(0.0);
            cost = d * d + st.overstretch_weight * over * over;
        }

        if is_final {
            if j - i == 1 && n > 1 {
                cost += st.orphan_measure_penalty;
            }
        } else {
            match self.measures[j - 1].break_right {
                BreakRule::Allowed | BreakRule::Forced => {}
                BreakRule::Forbidden => return None,
                BreakRule::Penalty(p) => cost += p,
            }
            cost += self.measures[j - 1].spanner_penalty;
            if j + 1 == n {
                // Breaking here leaves a one-measure widow as the final
                // system.
                cost += st.widow_break_penalty;
            }
        }

        Some(EdgeCore {
            r_used,
            class: class_of(r_used),
            base_cost: cost,
            justified: !ragged,
            target,
            natural: Sp(natural),
            minimum: Sp(minimum),
        })
    }

    /// Full edge cost given the previous system's fitness class (`None`
    /// for the first system: no continuity term).
    pub(crate) fn cost(&self, core: &EdgeCore, prev_class: Option<usize>) -> f64 {
        match prev_class {
            None => core.base_cost,
            Some(pc) => {
                let d = core.r_used - self.style.class_reps[pc];
                core.base_cost + self.style.continuity_weight * d * d
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Cand {
    cost: f64,
    prev_boundary: usize,
    prev_class: usize,
    prev_rank: usize,
    // The system that *ends* at this state:
    sys_start: usize,
    r: f64,
    justified: bool,
    target: Sp,
    natural: Sp,
    minimum: Sp,
    edge_cost: f64,
}

/// Break `measures` into systems, returning up to `alternatives` best
/// complete solutions (at least 1). See the module docs for the cost model.
pub fn break_lines(
    measures: &[MeasureSpacing],
    widths: LineWidths,
    style: &BreakStyle,
    alternatives: usize,
) -> LineBreakResult {
    let k = alternatives.max(1);
    let mut edges = 0usize;
    let (states, emergency) = {
        let ctx = EdgeContext::new(measures, widths, style, false);
        let states = run_dp(&ctx, k, &mut edges);
        if has_solution(&states, measures.len()) {
            (states, false)
        } else {
            let ctx = EdgeContext::new(measures, widths, style, true);
            (run_dp(&ctx, k, &mut edges), true)
        }
    };
    let n = measures.len();
    let mut finals: Vec<(usize, usize)> = Vec::new(); // (class, rank)
    for (c, list) in states[n].iter().enumerate() {
        for rank in 0..list.len() {
            finals.push((c, rank));
        }
    }
    finals.sort_by(|a, b| {
        let ca = states[n][a.0][a.1].cost;
        let cb = states[n][b.0][b.1].cost;
        ca.total_cmp(&cb).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1))
    });
    let alternatives = finals
        .into_iter()
        .take(k)
        .map(|(c, rank)| reconstruct(&states, n, c, rank))
        .collect();
    LineBreakResult { alternatives, edges_evaluated: edges, emergency }
}

fn has_solution(states: &[Vec<Vec<Cand>>], n: usize) -> bool {
    n == 0 || states[n].iter().any(|l| !l.is_empty())
}

fn run_dp(ctx: &EdgeContext, k: usize, edges: &mut usize) -> Vec<Vec<Vec<Cand>>> {
    let n = ctx.measure_count();
    let nclass = ctx.style.class_reps.len();
    let mut states: Vec<Vec<Vec<Cand>>> = vec![vec![Vec::new(); nclass]; n + 1];
    // Boundary 0: the virtual start, one zero-cost candidate in class 0.
    states[0][0].push(Cand {
        cost: 0.0,
        prev_boundary: 0,
        prev_class: 0,
        prev_rank: 0,
        sys_start: 0,
        r: 0.0,
        justified: true,
        target: Sp(0.0),
        natural: Sp(0.0),
        minimum: Sp(0.0),
        edge_cost: 0.0,
    });
    if n == 0 {
        return states;
    }
    // Forced boundaries bound how far back an edge may reach.
    let mut last_forced_before = vec![0usize; n + 1];
    for j in 1..=n {
        last_forced_before[j] = if j >= 2 && matches!(ctx.measures[j - 2].break_right, BreakRule::Forced) {
            j - 1
        } else {
            last_forced_before[j - 1]
        };
    }
    for j in 1..=n {
        if j < n && matches!(ctx.measures[j - 1].break_right, BreakRule::Forbidden) {
            continue;
        }
        let lo = last_forced_before[j];
        let mut new_cands: Vec<Cand> = Vec::new();
        for i in (lo..j).rev() {
            if j - i > ctx.style.max_measures_per_system {
                break;
            }
            if !ctx.emergency && ctx.min_width(i, j) > ctx.widths.at(i).0 {
                // Rod floors already exceed the line: this edge and every
                // longer one is overfull past r = -1.
                break;
            }
            if states[i].iter().all(|l| l.is_empty()) {
                continue;
            }
            *edges += 1;
            let Some(core) = ctx.core(i, j) else { continue };
            let prev_classes: &[Option<usize>] = if i == 0 {
                &[None]
            } else {
                &[Some(0), Some(1), Some(2), Some(3)]
            };
            for &pc in prev_classes {
                let list = &states[i][pc.unwrap_or(0)];
                if list.is_empty() {
                    continue;
                }
                let edge_cost = ctx.cost(&core, pc);
                for (rank, cand) in list.iter().enumerate() {
                    new_cands.push(Cand {
                        cost: cand.cost + edge_cost,
                        prev_boundary: i,
                        prev_class: pc.unwrap_or(0),
                        prev_rank: rank,
                        sys_start: i,
                        r: core.r_used,
                        justified: core.justified,
                        target: core.target,
                        natural: core.natural,
                        minimum: core.minimum,
                        edge_cost,
                    });
                }
            }
        }
        // Deterministic K-best per class.
        new_cands.sort_by(|a, b| {
            a.cost
                .total_cmp(&b.cost)
                .then(a.prev_boundary.cmp(&b.prev_boundary))
                .then(a.prev_class.cmp(&b.prev_class))
                .then(a.prev_rank.cmp(&b.prev_rank))
        });
        for cand in new_cands {
            let list = &mut states[j][class_of(cand.r)];
            if list.len() < k {
                list.push(cand);
            }
        }
    }
    states
}

fn reconstruct(states: &[Vec<Vec<Cand>>], n: usize, class: usize, rank: usize) -> LinePlan {
    let mut systems = Vec::new();
    let mut j = n;
    let mut c = class;
    let mut r = rank;
    let mut total = 0.0;
    while j > 0 {
        let cand = &states[j][c][r];
        systems.push(SystemPlan {
            measures: cand.sys_start..j,
            target: cand.target,
            natural: cand.natural,
            minimum: cand.minimum,
            adjustment: cand.r,
            justified: cand.justified,
            cost: cand.edge_cost,
        });
        total += cand.edge_cost;
        let (pj, pc, pr) = (cand.prev_boundary, cand.prev_class, cand.prev_rank);
        j = pj;
        c = pc;
        r = pr;
    }
    systems.reverse();
    LinePlan { systems, total_cost: total }
}

// ---------------------------------------------------------------------------
// Page breaking
// ---------------------------------------------------------------------------

/// Page-turn quality at the boundary after a system.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TurnRule {
    /// An ordinary legal page boundary.
    #[default]
    Allowed,
    /// A welcome turn (e.g. after rests): earns the good-turn bonus.
    Good,
    /// Turning here is forbidden (inside a tightly bound construct).
    Forbidden,
}

/// Vertical summary of one system, as the page breaker sees it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SystemVertical {
    /// Skyline-derived height of the system block itself.
    pub height: Sp,
    /// Preferred gap between this system and the previous one when they
    /// share a page (ignored for the first system of a page).
    pub gap_natural: Sp,
    /// Hard minimum for that gap.
    pub gap_min: Sp,
    /// Stretch capacity of that gap at unit vertical force (0 opts out of
    /// vertical justification through this gap).
    pub gap_stretch: Sp,
    /// Turn quality at the boundary after this system.
    pub turn_after: TurnRule,
}

/// Vertical page geometry.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PageSpec {
    /// Usable music height between the page margins.
    pub usable_height: Sp,
}

/// One chosen page.
#[derive(Clone, PartialEq, Debug)]
pub struct PageFill {
    /// The systems on this page.
    pub systems: Range<usize>,
    /// Vertical adjustment ratio applied to the gaps (0 for a ragged page).
    pub adjustment: f64,
    /// False when the page is left at natural stack height.
    pub justified: bool,
    /// This page's edge cost.
    pub cost: f64,
}

/// A complete page-break solution.
#[derive(Clone, PartialEq, Debug)]
pub struct PagePlan {
    /// The pages, in order; their ranges tile the systems.
    pub pages: Vec<PageFill>,
    /// Total cost.
    pub total_cost: f64,
    /// Number of DP edges evaluated.
    pub edges_evaluated: usize,
    /// True when the strict pass failed and overflow was admitted.
    pub emergency: bool,
}

/// Distribute systems onto pages with the vertical analogue of the line DP:
/// gaps are springs with rods, page badness is cubic in the vertical
/// adjustment ratio, and turn/orphan semantics enter as penalties.
pub fn break_pages(systems: &[SystemVertical], page: PageSpec, style: &BreakStyle) -> PagePlan {
    let mut edges = 0usize;
    let (result, emergency) = {
        let r = page_dp(systems, page, style, false, &mut edges);
        if r.is_some() {
            (r, false)
        } else {
            (page_dp(systems, page, style, true, &mut edges), true)
        }
    };
    match result {
        Some((pages, total_cost)) => PagePlan { pages, total_cost, edges_evaluated: edges, emergency },
        None => PagePlan { pages: Vec::new(), total_cost: 0.0, edges_evaluated: edges, emergency },
    }
}

fn page_dp(
    systems: &[SystemVertical],
    page: PageSpec,
    style: &BreakStyle,
    emergency: bool,
    edges: &mut usize,
) -> Option<(Vec<PageFill>, f64)> {
    let n = systems.len();
    if n == 0 {
        return Some((Vec::new(), 0.0));
    }
    let nclass = style.class_reps.len();
    // Prefix sums; gap k is between systems k-1 and k.
    let mut pref_h = vec![0.0; n + 1];
    for (k, s) in systems.iter().enumerate() {
        pref_h[k + 1] = pref_h[k] + s.height.0;
    }
    // Gap k (1 <= k < n) sits between systems k-1 and k and is described by
    // systems[k]; pref index k sums gaps 1..=k.
    let mut pref_gap = vec![0.0; n + 1];
    let mut pref_gap_min = vec![0.0; n + 1];
    let mut pref_gap_stretch = vec![0.0; n + 1];
    for k in 1..=n {
        let (gn, gm, gs) = if k < n {
            (systems[k].gap_natural.0, systems[k].gap_min.0, systems[k].gap_stretch.0)
        } else {
            (0.0, 0.0, 0.0)
        };
        pref_gap[k] = pref_gap[k - 1] + gn;
        pref_gap_min[k] = pref_gap_min[k - 1] + gm;
        pref_gap_stretch[k] = pref_gap_stretch[k - 1] + gs;
    }
    #[derive(Clone, Copy)]
    struct PCand {
        cost: f64,
        prev: usize,
        prev_class: usize,
        r: f64,
        justified: bool,
        start: usize,
        edge_cost: f64,
        used: bool,
    }
    let empty = PCand {
        cost: f64::INFINITY,
        prev: 0,
        prev_class: 0,
        r: 0.0,
        justified: true,
        start: 0,
        edge_cost: 0.0,
        used: false,
    };
    let mut states = vec![vec![empty; nclass]; n + 1];
    states[0][0] = PCand { cost: 0.0, used: true, ..empty };
    for j in 1..=n {
        for i in (0..j).rev() {
            if j - i > style.max_systems_per_page {
                break;
            }
            let is_final = j == n;
            if j < n && systems[j - 1].turn_after == TurnRule::Forbidden {
                continue;
            }
            if states[i].iter().all(|c| !c.used) {
                continue;
            }
            let h = page.usable_height.0;
            let stack = pref_h[j] - pref_h[i];
            // Internal gaps of a page holding systems i..j are gap indices
            // i+1..=j-1.
            let gaps = pref_gap[j - 1] - pref_gap[i];
            let gap_min = pref_gap_min[j - 1] - pref_gap_min[i];
            let gap_stretch = pref_gap_stretch[j - 1] - pref_gap_stretch[i];
            let natural = stack + gaps;
            let minimum = stack + gap_min;
            *edges += 1;
            if minimum > h && !emergency {
                break; // overflow; longer edges only get worse
            }
            let surplus = h - natural;
            let fill = if h > 0.0 { natural / h } else { 1.0 };
            // A page is left at natural height when nothing offers stretch,
            // or when it is the final page below its fill floor.
            let ragged = surplus > 0.0
                && (gap_stretch <= 0.0 || (is_final && fill < style.page_last_fill_min));
            let r_used;
            let feasible;
            if ragged {
                r_used = 0.0;
                feasible = true;
            } else {
                let r = if surplus >= 0.0 {
                    if gap_stretch > 0.0 { surplus / gap_stretch } else { 0.0 }
                } else {
                    let shrink = gaps - gap_min;
                    if shrink > 0.0 { surplus / shrink } else { f64::NEG_INFINITY }
                };
                feasible = r.is_finite() && r >= style.min_ratio && r <= style.max_ratio;
                if !feasible && !emergency {
                    continue;
                }
                r_used = if r.is_finite() {
                    r.clamp(style.min_ratio, style.max_ratio)
                } else {
                    style.min_ratio
                };
            }
            // A small constant per page (the line DP's demerit offset,
            // squared) makes fewer, fuller pages win whenever costs would
            // otherwise tie — e.g. for callers that opt out of vertical
            // justification entirely.
            let per_page = style.demerit_offset * style.demerit_offset;
            let mut base = per_page
                + if feasible {
                    style.badness_scale * r_used.abs().powi(3)
                } else {
                    style.badness_cap
                };
            if ragged && !is_final {
                // A middle page left short of its height is a hard defect;
                // charge its emptiness. Only the final page may run short
                // for free.
                let empty = (1.0 - fill).max(0.0);
                base += style.page_sparse_weight * empty * empty;
            }
            if is_final {
                if j - i == 1 && n > 1 {
                    base += style.page_orphan_penalty;
                }
            } else if systems[j - 1].turn_after == TurnRule::Good {
                base += style.good_turn_bonus;
            }
            for pc in 0..nclass {
                if !states[i][pc].used {
                    continue;
                }
                let cont = if i == 0 {
                    0.0
                } else {
                    let d = r_used - style.class_reps[pc];
                    style.page_continuity_weight * d * d
                };
                let edge_cost = base + cont;
                let total = states[i][pc].cost + edge_cost;
                let slot = &mut states[j][class_of(r_used)];
                let better = !slot.used
                    || total < slot.cost
                    || (total == slot.cost && (i, pc) < (slot.prev, slot.prev_class));
                if better {
                    *slot = PCand {
                        cost: total,
                        prev: i,
                        prev_class: pc,
                        r: r_used,
                        justified: !ragged,
                        start: i,
                        edge_cost,
                        used: true,
                    };
                }
            }
        }
    }
    // Best final state.
    let mut best: Option<(usize, f64)> = None;
    for (c, cand) in states[n].iter().enumerate() {
        if cand.used {
            let better = match best {
                None => true,
                Some((bc, bcost)) => {
                    cand.cost < bcost || (cand.cost == bcost && c < bc)
                }
            };
            if better {
                best = Some((c, cand.cost));
            }
        }
    }
    let (mut c, total) = best?;
    let mut pages = Vec::new();
    let mut j = n;
    while j > 0 {
        let cand = states[j][c];
        pages.push(PageFill {
            systems: cand.start..j,
            adjustment: cand.r,
            justified: cand.justified,
            cost: cand.edge_cost,
        });
        j = cand.prev;
        c = cand.prev_class;
    }
    pages.reverse();
    Some((pages, total))
}

/// A joint line+page decision.
#[derive(Clone, PartialEq, Debug)]
pub struct DocumentPlan {
    /// Which line alternative won.
    pub line_index: usize,
    /// The winning line-break solution.
    pub lines: LinePlan,
    /// The page solution for it.
    pub pages: PagePlan,
    /// Combined cost that was minimized.
    pub total_cost: f64,
}

/// Two-level breaking: line DP hands the page DP its best few complete
/// alternatives instead of freezing one choice, and the joint minimum wins.
/// `height_of` maps a chosen system to its vertical summary (in real use:
/// solve the system's vertical skylines; in tests: any deterministic rule).
pub fn plan_document(
    measures: &[MeasureSpacing],
    widths: LineWidths,
    page: PageSpec,
    style: &BreakStyle,
    alternatives: usize,
    mut height_of: impl FnMut(&SystemPlan) -> SystemVertical,
) -> Option<DocumentPlan> {
    let lines = break_lines(measures, widths, style, alternatives);
    let mut best: Option<DocumentPlan> = None;
    for (idx, alt) in lines.alternatives.iter().enumerate() {
        let verts: Vec<SystemVertical> = alt.systems.iter().map(&mut height_of).collect();
        let pages = break_pages(&verts, page, style);
        if pages.pages.is_empty() && !verts.is_empty() {
            continue;
        }
        let total = alt.total_cost + pages.total_cost;
        let better = match &best {
            None => true,
            Some(b) => total < b.total_cost,
        };
        if better {
            best = Some(DocumentPlan {
                line_index: idx,
                lines: alt.clone(),
                pages,
                total_cost: total,
            });
        }
    }
    best
}

#[cfg(test)]
mod breaking_tests {
    use super::*;
    use crate::style::BreakStyle;
    use crate::testutil::Lcg;

    fn m(natural: f64, minimum: f64, stretch: f64) -> MeasureSpacing {
        MeasureSpacing {
            natural: Sp(natural),
            minimum: Sp(minimum),
            stretch: Sp(stretch),
            shrink: Sp(natural - minimum),
            break_right: BreakRule::Allowed,
            spanner_penalty: 0.0,
        }
    }

    #[test]
    fn single_system_hand_worked() {
        // Three measures, natural 30 total, stretch capacity 15, width 33:
        // r = 0.2, b = 100 * 0.008 = 0.8, demerits = (10.8)^2 = 116.64.
        // fill = 30/33 = 0.909 >= 0.70, so the final system justifies.
        let st = BreakStyle::default();
        let ms = vec![m(10.0, 8.0, 5.0); 3];
        let res = break_lines(&ms, LineWidths::uniform(Sp(33.0)), &st, 1);
        assert!(!res.emergency);
        let plan = &res.alternatives[0];
        assert_eq!(plan.systems.len(), 1);
        let sys = &plan.systems[0];
        assert_eq!(sys.measures, 0..3);
        assert!(sys.justified);
        assert!((sys.adjustment - 0.2).abs() < 1e-12);
        assert!((plan.total_cost - 116.64).abs() < 1e-9);
    }

    #[test]
    fn ragged_final_system() {
        // Two systems; the second holds one-third of a line: fill < 0.70,
        // so it stays at natural width (unjustified, r = 0).
        let st = BreakStyle::default();
        let mut ms = vec![m(10.0, 8.0, 5.0); 8];
        // Forced break after measure 5 pins the split 6 + 2.
        ms[5].break_right = BreakRule::Forced;
        let res = break_lines(&ms, LineWidths::uniform(Sp(62.0)), &st, 1);
        assert!(!res.emergency);
        let plan = &res.alternatives[0];
        assert_eq!(plan.systems.len(), 2);
        let last = plan.systems.last().unwrap();
        assert_eq!(last.measures, 6..8);
        assert!(!last.justified, "sparse final system must stay ragged");
        assert_eq!(last.adjustment, 0.0);
        assert_eq!(last.solved_width(), last.natural);
    }

    #[test]
    fn widow_and_orphan_steer_to_even_split() {
        // Four equal measures, width 25: 2+2 must beat 3+1 (widow + orphan
        // + bad compression) and 1+3 variants.
        let st = BreakStyle::default();
        let ms = vec![m(10.0, 8.0, 5.0); 4];
        let res = break_lines(&ms, LineWidths::uniform(Sp(25.0)), &st, 1);
        assert!(!res.emergency);
        let plan = &res.alternatives[0];
        let splits: Vec<_> = plan.systems.iter().map(|s| s.measures.clone()).collect();
        assert_eq!(splits, vec![0..2, 2..4]);
    }

    #[test]
    fn forced_and_forbidden_semantics() {
        let st = BreakStyle::default();
        let mut ms = vec![m(10.0, 8.0, 5.0); 8];
        ms[3].break_right = BreakRule::Forced; // boundary 4 mandatory
        ms[1].break_right = BreakRule::Forbidden; // boundary 2 illegal
        let res = break_lines(&ms, LineWidths::uniform(Sp(33.0)), &st, 3);
        assert!(!res.alternatives.is_empty());
        for alt in &res.alternatives {
            let boundaries: Vec<usize> = alt.systems.iter().map(|s| s.measures.end).collect();
            assert!(boundaries.contains(&4), "forced boundary skipped: {:?}", boundaries);
            assert!(!boundaries[..boundaries.len() - 1].contains(&2), "forbidden boundary used");
            // Systems tile the score.
            let mut at = 0;
            for s in &alt.systems {
                assert_eq!(s.measures.start, at);
                at = s.measures.end;
            }
            assert_eq!(at, 8);
        }
    }

    /// Exhaustive verification of DP optimality: for every partition of a
    /// small score into systems, compute the exact same edge costs, and the
    /// DP total must equal the exhaustive minimum.
    #[test]
    fn dp_matches_exhaustive_on_random_instances() {
        let mut rng = Lcg::new(0x5eed_0010);
        let st = BreakStyle::default();
        let mut checked = 0;
        for _case in 0..120 {
            let n = 3 + (rng.next_u64() % 7) as usize;
            let ms: Vec<MeasureSpacing> = (0..n)
                .map(|_| {
                    let nat = 6.0 + 6.0 * rng.next_f64();
                    let min = nat * (0.45 + 0.3 * rng.next_f64());
                    let mut mm = m(nat, min, nat * (0.4 + 0.4 * rng.next_f64()));
                    if rng.next_f64() < 0.2 {
                        mm.break_right = BreakRule::Penalty(600.0 * rng.next_f64() - 100.0);
                    }
                    if rng.next_f64() < 0.2 {
                        mm.spanner_penalty = 300.0 * rng.next_f64();
                    }
                    mm
                })
                .collect();
            let width = Sp(9.0 * (2.0 + 2.0 * rng.next_f64()));
            let res = break_lines(&ms, LineWidths::uniform(width), &st, 1);
            if res.emergency || res.alternatives.is_empty() {
                continue;
            }
            let dp_total = res.alternatives[0].total_cost;
            // Exhaustive enumeration over the 2^(n-1) interior boundary
            // subsets, using the identical edge evaluator.
            let ctx = EdgeContext::new(&ms, LineWidths::uniform(width), &st, false);
            let mut best = f64::INFINITY;
            for mask in 0u32..(1 << (n - 1)) {
                let mut bounds = vec![0usize];
                for b in 1..n {
                    if mask & (1 << (b - 1)) != 0 {
                        bounds.push(b);
                    }
                }
                bounds.push(n);
                let mut total = 0.0;
                let mut prev_class: Option<usize> = None;
                let mut ok = true;
                for w in bounds.windows(2) {
                    match ctx.core(w[0], w[1]) {
                        Some(core) => {
                            total += ctx.cost(&core, prev_class);
                            prev_class = Some(core.class);
                        }
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok && total < best {
                    best = total;
                }
            }
            assert!(
                (dp_total - best).abs() < 1e-6,
                "DP {} vs exhaustive {} on {} measures width {}",
                dp_total,
                best,
                n,
                width.0
            );
            checked += 1;
        }
        assert!(checked > 60, "too few feasible instances checked: {}", checked);
    }

    #[test]
    fn k_best_alternatives_are_valid_and_sorted() {
        let mut rng = Lcg::new(0x5eed_0011);
        let st = BreakStyle::default();
        let ms: Vec<MeasureSpacing> = (0..12)
            .map(|_| {
                let nat = 6.0 + 6.0 * rng.next_f64();
                m(nat, nat * 0.6, nat * 0.6)
            })
            .collect();
        let res = break_lines(&ms, LineWidths::uniform(Sp(30.0)), &st, 4);
        assert!(res.alternatives.len() >= 2, "expected multiple alternatives");
        let mut seen: Vec<Vec<usize>> = Vec::new();
        let mut last_cost = f64::NEG_INFINITY;
        for alt in &res.alternatives {
            assert!(alt.total_cost >= last_cost - 1e-9, "alternatives not sorted");
            last_cost = alt.total_cost;
            let mut at = 0;
            for s in &alt.systems {
                assert_eq!(s.measures.start, at);
                at = s.measures.end;
            }
            assert_eq!(at, 12);
            let key: Vec<usize> = alt.systems.iter().map(|s| s.measures.end).collect();
            assert!(!seen.contains(&key), "duplicate alternative {:?}", key);
            seen.push(key);
            // Plan total equals the sum of its system costs.
            let sum: f64 = alt.systems.iter().map(|s| s.cost).sum();
            assert!((sum - alt.total_cost).abs() < 1e-9);
        }
    }

    /// The DP scans a bounded window: rod floors cap how many measures fit
    /// a line, so work is O(n * window), not O(n^2).
    #[test]
    fn bounded_work_on_long_scores() {
        let st = BreakStyle::default();
        let n = 1200;
        let ms = vec![m(6.0, 4.8, 3.0); n];
        let res = break_lines(&ms, LineWidths::uniform(Sp(40.0)), &st, 1);
        assert!(!res.emergency);
        assert!(!res.alternatives.is_empty());
        // Window: floor(40 / 4.8) = 8 measures fit above the rod floor, so
        // at most ~9 edges end at each boundary.
        assert!(
            res.edges_evaluated <= 15 * n,
            "edges {} not linear in n = {}",
            res.edges_evaluated,
            n
        );
        let plan = &res.alternatives[0];
        let mut at = 0;
        for s in &plan.systems {
            assert_eq!(s.measures.start, at);
            at = s.measures.end;
        }
        assert_eq!(at, n);
    }

    #[test]
    fn emergency_pass_handles_impossible_width() {
        // A measure wider than the line: strict pass fails, emergency pass
        // still returns a plan with saturated badness.
        let st = BreakStyle::default();
        let ms = vec![m(10.0, 9.0, 2.0), m(30.0, 28.0, 2.0), m(10.0, 9.0, 2.0)];
        let res = break_lines(&ms, LineWidths::uniform(Sp(20.0)), &st, 1);
        assert!(res.emergency);
        assert_eq!(res.alternatives.len(), 1);
        let mut at = 0;
        for s in &res.alternatives[0].systems {
            assert_eq!(s.measures.start, at);
            at = s.measures.end;
        }
        assert_eq!(at, 3);
    }

    fn sys(h: f64, gap_stretch: f64, turn: TurnRule) -> SystemVertical {
        SystemVertical {
            height: Sp(h),
            gap_natural: Sp(4.0),
            gap_min: Sp(2.0),
            gap_stretch: Sp(gap_stretch),
            turn_after: turn,
        }
    }

    #[test]
    fn page_dp_prefers_filled_pages_and_final_short_page() {
        let st = BreakStyle::default();
        // Five systems of height 20 on 50-high pages: two per page fits
        // with mild stretch; the last page holds one system, ragged.
        let systems = vec![sys(20.0, 8.0, TurnRule::Allowed); 5];
        let plan = break_pages(&systems, PageSpec { usable_height: Sp(50.0) }, &st);
        assert!(!plan.emergency);
        let ranges: Vec<_> = plan.pages.iter().map(|p| p.systems.clone()).collect();
        assert_eq!(ranges, vec![0..2, 2..4, 4..5]);
        let last = plan.pages.last().unwrap();
        assert!(!last.justified, "short final page must not stretch");
        assert_eq!(last.adjustment, 0.0);
        // Middle pages are justified with the expected ratio:
        // (50 - 44) / 8 = 0.75.
        assert!((plan.pages[0].adjustment - 0.75).abs() < 1e-12);
        assert!(plan.pages[0].justified);
    }

    #[test]
    fn page_turn_rules_respected() {
        let st = BreakStyle::default();
        let mut systems = vec![sys(20.0, 8.0, TurnRule::Allowed); 6];
        // Forbid the natural 2/2/2 boundaries; allow (and reward) 3/3.
        systems[1].turn_after = TurnRule::Forbidden;
        systems[3].turn_after = TurnRule::Forbidden;
        systems[2].turn_after = TurnRule::Good;
        let plan = break_pages(&systems, PageSpec { usable_height: Sp(76.0) }, &st);
        assert!(!plan.emergency);
        let ranges: Vec<_> = plan.pages.iter().map(|p| p.systems.clone()).collect();
        assert_eq!(ranges, vec![0..3, 3..6]);
    }

    #[test]
    fn plan_document_takes_joint_minimum() {
        // The winner must minimize line cost + page cost over the returned
        // alternatives, verified by recomputing every alternative by hand.
        let mut rng = Lcg::new(0x5eed_0012);
        let st = BreakStyle::default();
        let ms: Vec<MeasureSpacing> = (0..14)
            .map(|_| {
                let nat = 6.0 + 6.0 * rng.next_f64();
                m(nat, nat * 0.6, nat * 0.6)
            })
            .collect();
        let widths = LineWidths::uniform(Sp(30.0));
        let page = PageSpec { usable_height: Sp(50.0) };
        let height_of = |sp: &SystemPlan| {
            // Deterministic pseudo-height from content: more measures,
            // taller system.
            sys(16.0 + 2.0 * (sp.measures.len() as f64), 8.0, TurnRule::Allowed)
        };
        let doc = plan_document(&ms, widths, page, &st, 4, height_of).unwrap();
        let lines = break_lines(&ms, widths, &st, 4);
        let mut best = f64::INFINITY;
        let mut best_idx = 0;
        for (idx, alt) in lines.alternatives.iter().enumerate() {
            let verts: Vec<SystemVertical> = alt.systems.iter().map(height_of).collect();
            let pages = break_pages(&verts, page, &st);
            let total = alt.total_cost + pages.total_cost;
            if total < best {
                best = total;
                best_idx = idx;
            }
        }
        assert_eq!(doc.line_index, best_idx);
        assert!((doc.total_cost - best).abs() < 1e-9);
    }

    #[test]
    fn deterministic_bitwise() {
        let mut rng = Lcg::new(0x5eed_0013);
        let ms: Vec<MeasureSpacing> = (0..30)
            .map(|_| {
                let nat = 6.0 + 6.0 * rng.next_f64();
                m(nat, nat * 0.6, nat * 0.5)
            })
            .collect();
        let st = BreakStyle::default();
        let a = break_lines(&ms, LineWidths::uniform(Sp(31.0)), &st, 3);
        let b = break_lines(&ms, LineWidths::uniform(Sp(31.0)), &st, 3);
        assert_eq!(a.alternatives.len(), b.alternatives.len());
        for (x, y) in a.alternatives.iter().zip(&b.alternatives) {
            assert_eq!(x.total_cost.to_bits(), y.total_cost.to_bits());
            let xb: Vec<usize> = x.systems.iter().map(|s| s.measures.end).collect();
            let yb: Vec<usize> = y.systems.iter().map(|s| s.measures.end).collect();
            assert_eq!(xb, yb);
        }
    }
}
