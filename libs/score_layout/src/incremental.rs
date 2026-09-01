//! Incremental relayout: the seam that makes an editor feel instant.
//!
//! A one-note change should usually invalidate one measure and one system,
//! not the whole score. The engine caches, per measure, the spacing summary
//! keyed by a caller-supplied revision, and per system, the solved spring
//! widths plus the break-DP bookkeeping (edge cost, fitness class, and the
//! winner's cost margin over the runner-up).
//!
//! On an edit the engine walks a three-level ladder, cheapest first:
//!
//! 1. **Summarize** only measures whose revision changed.
//! 2. **Keep the breaks** when every affected system is still feasible, its
//!    fitness class is unchanged (so downstream continuity terms cannot
//!    move), and the total cost increase stays inside the stored DP margin
//!    (hysteresis: a break that barely wins keeps winning, which avoids
//!    reflow flicker). Only the affected systems' springs are re-solved.
//! 3. **Rebreak with resync** otherwise: rerun the line DP forward from the
//!    start of the first affected system, and stop as soon as a chosen
//!    boundary past the last dirty measure lands on an old system start
//!    with the same incoming fitness class — from there the old solution is
//!    provably still self-consistent and is spliced back verbatim.
//!
//! Work is therefore bounded by the edit's neighbourhood, not the score
//! length; the tests assert exact operation counts for each rung.

use crate::breaking::{class_of, BreakRule, EdgeContext, LineWidths, MeasureSpacing};
use crate::sp::Sp;
use crate::spacing::{solve_spacing, SpacingColumn, SpacingSolution};
use crate::style::LayoutStyle;
use std::ops::Range;

/// One measure as the incremental engine sees it: a revision stamp plus the
/// abstract spacing inputs. The caller (the score model) owns revisions and
/// bumps one whenever anything inside the measure changes.
#[derive(Clone, PartialEq, Debug)]
pub struct MeasureSource {
    /// Caller-owned change stamp; equality means "unchanged".
    pub revision: u64,
    /// The measure's onset columns (springs and rods).
    pub columns: Vec<SpacingColumn>,
    /// Break semantics at the measure's right boundary.
    pub break_right: BreakRule,
    /// Penalty for splitting spanners at that boundary.
    pub spanner_penalty: f64,
}

/// A laid-out system with its solved column widths.
#[derive(Clone, PartialEq, Debug)]
pub struct SystemLayout {
    /// The measures on this system.
    pub measures: Range<usize>,
    /// The usable width it was broken against.
    pub target: Sp,
    /// Natural width of its content.
    pub natural: Sp,
    /// Adjustment ratio applied.
    pub adjustment: f64,
    /// False for a ragged final system.
    pub justified: bool,
    /// Solved widths for every column of the system, in order.
    pub solution: SpacingSolution,
    /// The system's break-DP edge cost.
    pub edge_cost: f64,
    /// Fitness class of `adjustment` (drives the next system's continuity).
    pub class: usize,
}

/// Operation counters for the last [`IncrementalLayout::layout`] call.
/// Tests assert on these; callers can use them for perf telemetry.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RelayoutStats {
    /// True when everything was recomputed from scratch.
    pub full: bool,
    /// True when the line DP ran (level 3).
    pub rebreak: bool,
    /// True when the rerun DP spliced back into the old solution early.
    pub resynced: bool,
    /// Measures whose spacing summary was recomputed.
    pub measures_summarized: usize,
    /// Systems whose springs were re-solved.
    pub systems_spaced: usize,
    /// Break-DP edges evaluated.
    pub dp_edges: usize,
}

/// The incremental layout engine. Create once per document view and feed it
/// the full measure list each time; it diffs by revision.
#[derive(Default)]
pub struct IncrementalLayout {
    style: Option<LayoutStyle>,
    widths: Option<LineWidths>,
    revisions: Vec<u64>,
    summaries: Vec<MeasureSpacing>,
    systems: Vec<SystemLayout>,
    total_cost: f64,
    /// Runner-up total minus winner total from the last full DP;
    /// `f64::INFINITY` when no runner-up existed, 0 after a splice.
    margin: f64,
    stats: RelayoutStats,
}

impl IncrementalLayout {
    /// New empty engine.
    pub fn new() -> IncrementalLayout {
        IncrementalLayout::default()
    }

    /// Counters for the most recent [`IncrementalLayout::layout`] call.
    pub fn stats(&self) -> RelayoutStats {
        self.stats
    }

    /// The current systems (valid after the first `layout` call).
    pub fn systems(&self) -> &[SystemLayout] {
        &self.systems
    }

    /// Total break cost of the current solution.
    pub fn total_cost(&self) -> f64 {
        self.total_cost
    }

    /// Lay the score out, reusing everything the revision diff allows.
    pub fn layout(
        &mut self,
        measures: &[MeasureSource],
        widths: LineWidths,
        style: &LayoutStyle,
    ) -> &[SystemLayout] {
        self.stats = RelayoutStats::default();
        let full = self.style.as_ref() != Some(style)
            || self.widths != Some(widths)
            || self.revisions.len() != measures.len()
            || self.systems.is_empty() && !measures.is_empty();
        if full {
            self.full_layout(measures, widths, style);
            return &self.systems;
        }
        let dirty: Vec<usize> = (0..measures.len())
            .filter(|&i| self.revisions[i] != measures[i].revision)
            .collect();
        if dirty.is_empty() {
            return &self.systems;
        }
        // Level 1: re-summarize the dirty measures only.
        for &i in &dirty {
            self.summaries[i] = summarize(&measures[i]);
            self.revisions[i] = measures[i].revision;
            self.stats.measures_summarized += 1;
        }
        let style_owned = style.clone();
        let affected: Vec<usize> = self
            .systems
            .iter()
            .enumerate()
            .filter(|(_, s)| dirty.iter().any(|&d| s.measures.contains(&d)))
            .map(|(i, _)| i)
            .collect();
        // Level 2: try to keep the breaks.
        if self.try_keep_breaks(measures, widths, &style_owned, &affected) {
            return &self.systems;
        }
        // Level 3: rebreak from the first affected system, resyncing early.
        let last_dirty = *dirty.last().unwrap();
        if !self.rebreak_from(measures, widths, &style_owned, affected[0], last_dirty) {
            // The suffix had no strict solution: fall back to a full
            // (emergency-capable) layout.
            self.full_layout(measures, widths, style);
        }
        &self.systems
    }

    fn full_layout(&mut self, measures: &[MeasureSource], widths: LineWidths, style: &LayoutStyle) {
        self.stats.full = true;
        self.stats.rebreak = true;
        self.style = Some(style.clone());
        self.widths = Some(widths);
        self.revisions = measures.iter().map(|m| m.revision).collect();
        self.summaries = measures.iter().map(summarize).collect();
        self.stats.measures_summarized = measures.len();
        let res = crate::breaking::break_lines(&self.summaries, widths, &style.breaking, 2);
        self.stats.dp_edges += res.edges_evaluated;
        let plan = match res.alternatives.first() {
            Some(p) => p.clone(),
            None => {
                self.systems.clear();
                self.total_cost = 0.0;
                self.margin = f64::INFINITY;
                return;
            }
        };
        self.margin = res
            .alternatives
            .get(1)
            .map(|a| a.total_cost - plan.total_cost)
            .unwrap_or(f64::INFINITY);
        self.total_cost = plan.total_cost;
        self.systems = plan
            .systems
            .iter()
            .map(|sp| {
                self.stats.systems_spaced += 1;
                let width = sp.solved_width();
                SystemLayout {
                    measures: sp.measures.clone(),
                    target: sp.target,
                    natural: sp.natural,
                    adjustment: sp.adjustment,
                    justified: sp.justified,
                    solution: solve_system(measures, sp.measures.clone(), width),
                    edge_cost: sp.cost,
                    class: class_of(sp.adjustment),
                }
            })
            .collect();
    }

    fn try_keep_breaks(
        &mut self,
        measures: &[MeasureSource],
        widths: LineWidths,
        style: &LayoutStyle,
        affected: &[usize],
    ) -> bool {
        let ctx = EdgeContext::new(&self.summaries, widths, &style.breaking, false);
        let mut updates = Vec::with_capacity(affected.len());
        let mut delta = 0.0;
        for &si in affected {
            self.stats.dp_edges += 1;
            let sys = &self.systems[si];
            let prev_class = if si == 0 { None } else { Some(self.systems[si - 1].class) };
            let Some(core) = ctx.core(sys.measures.start, sys.measures.end) else {
                return false;
            };
            if core.class != sys.class {
                return false;
            }
            let cost = ctx.cost(&core, prev_class);
            delta += cost - sys.edge_cost;
            updates.push((si, core, cost));
        }
        if delta > 0.95 * self.margin {
            return false;
        }
        for (si, core, cost) in updates {
            let sys = &mut self.systems[si];
            sys.natural = core.natural;
            sys.adjustment = core.r_used;
            sys.justified = core.justified;
            sys.edge_cost = cost;
            let width = if core.justified { core.target } else { core.natural };
            sys.solution = solve_system(measures, sys.measures.clone(), width);
            self.stats.systems_spaced += 1;
        }
        self.total_cost += delta;
        if delta > 0.0 {
            self.margin = (self.margin - delta).max(0.0);
        }
        true
    }

    /// Forward DP from the start of the first affected system; returns
    /// false when no strict solution exists for the suffix.
    fn rebreak_from(
        &mut self,
        measures: &[MeasureSource],
        widths: LineWidths,
        style: &LayoutStyle,
        first_sys: usize,
        last_dirty: usize,
    ) -> bool {
        self.stats.rebreak = true;
        let st = &style.breaking;
        let n = self.summaries.len();
        let start = self.systems[first_sys].measures.start;
        let incoming = if first_sys == 0 { None } else { Some(self.systems[first_sys - 1].class) };
        let ctx = EdgeContext::new(&self.summaries, widths, st, false);
        let nclass = st.class_reps.len();

        // Splice targets: old system starts strictly past the last dirty
        // measure, keyed by boundary, requiring the class of the system
        // that preceded them in the old solution.
        let mut splice_at: Vec<Option<(usize, usize)>> = vec![None; n + 1]; // boundary -> (old sys index, required class)
        for (t, sys) in self.systems.iter().enumerate().skip(first_sys + 1) {
            if sys.measures.start > last_dirty {
                splice_at[sys.measures.start] = Some((t, self.systems[t - 1].class));
            }
        }

        #[derive(Clone, Copy)]
        struct Cand {
            cost: f64,
            prev_boundary: usize,
            prev_class: usize,
            r: f64,
            justified: bool,
            target: Sp,
            natural: Sp,
            minimum: Sp,
            edge_cost: f64,
            used: bool,
        }
        let empty = Cand {
            cost: f64::INFINITY,
            prev_boundary: 0,
            prev_class: 0,
            r: 0.0,
            justified: true,
            target: Sp(0.0),
            natural: Sp(0.0),
            minimum: Sp(0.0),
            edge_cost: 0.0,
            used: false,
        };
        let seed_class = incoming.unwrap_or(0);
        let mut states = vec![vec![empty; nclass]; n + 1];
        states[start][seed_class] = Cand { cost: 0.0, used: true, ..empty };

        // Forced boundaries in the suffix.
        let mut last_forced = start;
        let mut last_forced_before = vec![start; n + 1];
        for j in (start + 1)..=n {
            if j >= 2 && matches!(self.summaries[j - 2].break_right, BreakRule::Forced) && j - 1 > start {
                last_forced = j - 1;
            }
            last_forced_before[j] = last_forced;
        }

        let reconstruct = |states: &Vec<Vec<Cand>>, end: usize, class: usize| -> Vec<SystemLayout> {
            let mut out = Vec::new();
            let mut j = end;
            let mut c = class;
            while j > start {
                let cand = &states[j][c];
                out.push(SystemLayout {
                    measures: cand.prev_boundary..j,
                    target: cand.target,
                    natural: cand.natural,
                    adjustment: cand.r,
                    justified: cand.justified,
                    solution: SpacingSolution {
                        force: 0.0,
                        widths: Vec::new(),
                        natural_width: cand.natural,
                        minimum_width: cand.minimum,
                        fit: crate::spacing::SpacingFit::Exact,
                    },
                    edge_cost: cand.edge_cost,
                    class: c,
                });
                let (pj, pc) = (cand.prev_boundary, cand.prev_class);
                j = pj;
                c = pc;
            }
            out.reverse();
            out
        };

        for j in (start + 1)..=n {
            if j < n && matches!(self.summaries[j - 1].break_right, BreakRule::Forbidden) {
                continue;
            }
            let lo = last_forced_before[j];
            for i in (lo..j).rev() {
                if j - i > st.max_measures_per_system {
                    break;
                }
                if ctx.min_width(i, j) > widths.at(i).0 {
                    break;
                }
                if states[i].iter().all(|c| !c.used) {
                    continue;
                }
                self.stats.dp_edges += 1;
                let Some(core) = ctx.core(i, j) else { continue };
                for pc in 0..nclass {
                    if !states[i][pc].used {
                        continue;
                    }
                    // Continuity: the seed state at `start` carries the
                    // incoming class (None at the score head).
                    let prev = if i == start { incoming } else { Some(pc) };
                    let edge_cost = ctx.cost(&core, prev);
                    let total = states[i][pc].cost + edge_cost;
                    let slot = &mut states[j][core.class];
                    let better = !slot.used
                        || total < slot.cost
                        || (total == slot.cost
                            && (i, pc) < (slot.prev_boundary, slot.prev_class));
                    if better {
                        *slot = Cand {
                            cost: total,
                            prev_boundary: i,
                            prev_class: pc,
                            r: core.r_used,
                            justified: core.justified,
                            target: core.target,
                            natural: core.natural,
                            minimum: core.minimum,
                            edge_cost,
                            used: true,
                        };
                    }
                }
            }
            // Resync: the first boundary past the dirty region that lands
            // on an old system start with a matching incoming class lets us
            // splice the old tail back on.
            if j < n {
                if let Some((old_t, need_class)) = splice_at[j] {
                    if states[j][need_class].used {
                        let mut new_systems = reconstruct(&states, j, need_class);
                        for sys in &mut new_systems {
                            let width = if sys.justified { sys.target } else { sys.natural };
                            sys.solution = solve_system(measures, sys.measures.clone(), width);
                            self.stats.systems_spaced += 1;
                        }
                        let prefix_cost: f64 =
                            self.systems[..first_sys].iter().map(|s| s.edge_cost).sum();
                        let new_cost = states[j][need_class].cost;
                        let suffix_cost: f64 =
                            self.systems[old_t..].iter().map(|s| s.edge_cost).sum();
                        let mut rebuilt =
                            Vec::with_capacity(first_sys + new_systems.len() + self.systems.len() - old_t);
                        rebuilt.extend_from_slice(&self.systems[..first_sys]);
                        rebuilt.extend(new_systems);
                        rebuilt.extend_from_slice(&self.systems[old_t..]);
                        self.systems = rebuilt;
                        self.total_cost = prefix_cost + new_cost + suffix_cost;
                        // The spliced solution is hysteresis, not a proven
                        // optimum: force the DP on the next edit here.
                        self.margin = 0.0;
                        self.stats.resynced = true;
                        return true;
                    }
                }
            }
        }
        // No splice: take the best final state, if any.
        let mut finals: Vec<(usize, f64)> = states[n]
            .iter()
            .enumerate()
            .filter(|(_, c)| c.used)
            .map(|(c, cand)| (c, cand.cost))
            .collect();
        finals.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let Some(&(class, cost)) = finals.first() else { return false };
        let runner = finals.get(1).map(|&(_, c)| c);
        let mut new_systems = reconstruct(&states, n, class);
        for sys in &mut new_systems {
            let width = if sys.justified { sys.target } else { sys.natural };
            sys.solution = solve_system(measures, sys.measures.clone(), width);
            self.stats.systems_spaced += 1;
        }
        let prefix_cost: f64 = self.systems[..first_sys].iter().map(|s| s.edge_cost).sum();
        self.systems.truncate(first_sys);
        self.systems.extend(new_systems);
        self.total_cost = prefix_cost + cost;
        self.margin = runner.map(|r| r - cost).unwrap_or(f64::INFINITY);
        true
    }
}

fn summarize(m: &MeasureSource) -> MeasureSpacing {
    MeasureSpacing::from_columns(&m.columns, m.break_right, m.spanner_penalty)
}

fn solve_system(measures: &[MeasureSource], range: Range<usize>, width: Sp) -> SpacingSolution {
    let cols: Vec<SpacingColumn> = measures[range]
        .iter()
        .flat_map(|m| m.columns.iter().cloned())
        .collect();
    solve_spacing(&cols, width)
}

#[cfg(test)]
mod incremental_tests {
    use super::*;
    use crate::style::SpacingStyle;

    /// A measure of four quarter-note columns; `skew` moves natural width
    /// between the first two columns without changing any measure summary
    /// (sum-preserving, flexes untouched) — the shape of a "one note moved"
    /// edit.
    fn measure(revision: u64, skew: f64) -> MeasureSource {
        let st = SpacingStyle::default();
        let mut columns: Vec<SpacingColumn> = (0..4)
            .map(|_| SpacingColumn::from_duration(0.25, Sp(0.5), Sp(1.0), &st))
            .collect();
        columns[0].natural += Sp(skew);
        columns[1].natural -= Sp(skew);
        MeasureSource {
            revision,
            columns,
            break_right: BreakRule::Allowed,
            spanner_penalty: 0.0,
        }
    }

    /// A hand-sized measure with explicit natural/minimum, one column.
    fn simple(revision: u64, natural: f64, minimum: f64, stretch: f64) -> MeasureSource {
        MeasureSource {
            revision,
            columns: vec![SpacingColumn {
                natural: Sp(natural),
                minimum: Sp(minimum),
                stretch_flex: stretch,
                shrink_flex: natural - minimum,
                headroom: Sp(0.0),
                duration_class: None,
            }],
            break_right: BreakRule::Allowed,
            spanner_penalty: 0.0,
        }
    }

    fn widths_bits(systems: &[SystemLayout]) -> Vec<Vec<u64>> {
        systems
            .iter()
            .map(|s| s.solution.widths.iter().map(|w| w.0.to_bits()).collect())
            .collect()
    }

    #[test]
    fn first_layout_is_full_and_tiles() {
        let style = LayoutStyle::default();
        let ms: Vec<MeasureSource> = (0..20).map(|i| measure(i as u64 + 1, 0.0)).collect();
        let mut eng = IncrementalLayout::new();
        let systems = eng.layout(&ms, LineWidths::uniform(Sp(40.0)), &style).to_vec();
        let stats = eng.stats();
        assert!(stats.full);
        assert_eq!(stats.measures_summarized, 20);
        assert_eq!(stats.systems_spaced, systems.len());
        let mut at = 0;
        for s in &systems {
            assert_eq!(s.measures.start, at);
            at = s.measures.end;
            // Springs solved to the system's decided width.
            let want = if s.justified { s.target } else { s.natural };
            assert!((s.solution.total().0 - want.0).abs() < 1e-9);
        }
        assert_eq!(at, 20);
    }

    #[test]
    fn unchanged_input_does_no_work() {
        let style = LayoutStyle::default();
        let ms: Vec<MeasureSource> = (0..20).map(|i| measure(i as u64 + 1, 0.0)).collect();
        let mut eng = IncrementalLayout::new();
        eng.layout(&ms, LineWidths::uniform(Sp(40.0)), &style);
        let before = widths_bits(eng.systems());
        eng.layout(&ms, LineWidths::uniform(Sp(40.0)), &style);
        let stats = eng.stats();
        assert!(!stats.full && !stats.rebreak);
        assert_eq!(stats.measures_summarized, 0);
        assert_eq!(stats.systems_spaced, 0);
        assert_eq!(stats.dp_edges, 0);
        assert_eq!(widths_bits(eng.systems()), before);
    }

    /// The headline property: a one-measure edit on a 200-measure score
    /// re-summarizes exactly one measure, re-solves exactly one system,
    /// touches one DP edge, and leaves every other system bit-identical.
    #[test]
    fn local_edit_does_bounded_work() {
        let style = LayoutStyle::default();
        let mut ms: Vec<MeasureSource> = (0..200).map(|i| measure(i as u64 + 1, 0.0)).collect();
        let mut eng = IncrementalLayout::new();
        eng.layout(&ms, LineWidths::uniform(Sp(40.0)), &style);
        let full_edges = eng.stats().dp_edges;
        let before = widths_bits(eng.systems());
        let edited_sys = eng
            .systems()
            .iter()
            .position(|s| s.measures.contains(&57))
            .unwrap();

        // Move ink inside measure 57 without changing its summary.
        ms[57] = measure(1000, 0.4);
        eng.layout(&ms, LineWidths::uniform(Sp(40.0)), &style);
        let stats = eng.stats();
        assert!(!stats.full);
        assert!(!stats.rebreak, "sum-preserving edit must keep the breaks");
        assert_eq!(stats.measures_summarized, 1);
        assert_eq!(stats.systems_spaced, 1);
        assert_eq!(stats.dp_edges, 1);
        assert!(stats.dp_edges * 20 < full_edges, "not bounded vs full {}", full_edges);
        let after = widths_bits(eng.systems());
        assert_eq!(before.len(), after.len());
        for (i, (b, a)) in before.iter().zip(&after).enumerate() {
            if i == edited_sys {
                assert_ne!(b, a, "edited system should re-solve differently");
            } else {
                assert_eq!(b, a, "untouched system {} changed", i);
            }
        }
    }

    #[test]
    fn small_cost_change_within_margin_keeps_breaks() {
        let style = LayoutStyle::default();
        // 30 measures, 3 per system with r = 0.2; alternatives are far
        // worse, so the DP margin is huge.
        let mut ms: Vec<MeasureSource> = (0..30).map(|i| simple(i as u64 + 1, 10.0, 8.0, 5.0)).collect();
        let mut eng = IncrementalLayout::new();
        eng.layout(&ms, LineWidths::uniform(Sp(33.0)), &style);
        let breaks: Vec<usize> = eng.systems().iter().map(|s| s.measures.end).collect();
        // Nudge one measure a little: same fitness class, tiny cost delta.
        ms[5] = simple(1000, 10.3, 8.0, 5.0);
        eng.layout(&ms, LineWidths::uniform(Sp(33.0)), &style);
        let stats = eng.stats();
        assert!(!stats.rebreak, "in-margin edit must not rebreak");
        assert_eq!(stats.systems_spaced, 1);
        let same: Vec<usize> = eng.systems().iter().map(|s| s.measures.end).collect();
        assert_eq!(breaks, same);
        // The re-solved system still meets its width.
        let sys = &eng.systems()[1];
        let want = if sys.justified { sys.target } else { sys.natural };
        assert!((sys.solution.total().0 - want.0).abs() < 1e-9);
    }

    #[test]
    fn class_changing_edit_rebreaks_and_resyncs() {
        let style = LayoutStyle::default();
        let mut ms: Vec<MeasureSource> = (0..30).map(|i| simple(i as u64 + 1, 10.0, 8.0, 5.0)).collect();
        let mut eng = IncrementalLayout::new();
        eng.layout(&ms, LineWidths::uniform(Sp(33.0)), &style);
        let full_edges = eng.stats().dp_edges;
        let before = widths_bits(eng.systems());
        let before_breaks: Vec<usize> = eng.systems().iter().map(|s| s.measures.end).collect();
        assert_eq!(before_breaks[0], 3, "expected 3-measure systems");

        // Fatten measure 5 drastically: its system flips fitness class,
        // forcing a rebreak from boundary 3.
        ms[5] = simple(1000, 20.0, 16.0, 5.0);
        eng.layout(&ms, LineWidths::uniform(Sp(33.0)), &style);
        let stats = eng.stats();
        assert!(!stats.full);
        assert!(stats.rebreak);
        assert!(stats.resynced, "expected an early splice back into the old plan");
        assert!(
            stats.dp_edges * 3 < full_edges,
            "resync work {} not bounded vs full {}",
            stats.dp_edges,
            full_edges
        );
        // Everything before the edited region and after the splice is
        // bit-identical to the old layout.
        let after = widths_bits(eng.systems());
        assert_eq!(after[0], before[0], "system before the edit changed");
        // Find where the tail realigns: compare from the ends.
        let mut tail = 0;
        while tail < before.len().min(after.len())
            && before[before.len() - 1 - tail] == after[after.len() - 1 - tail]
        {
            tail += 1;
        }
        assert!(tail >= 6, "expected a long untouched tail, got {}", tail);
        // The plan still tiles.
        let mut at = 0;
        for s in eng.systems() {
            assert_eq!(s.measures.start, at);
            at = s.measures.end;
        }
        assert_eq!(at, 30);
    }

    #[test]
    fn style_or_width_change_forces_full() {
        let style = LayoutStyle::default();
        let ms: Vec<MeasureSource> = (0..12).map(|i| measure(i as u64 + 1, 0.0)).collect();
        let mut eng = IncrementalLayout::new();
        eng.layout(&ms, LineWidths::uniform(Sp(40.0)), &style);
        eng.layout(&ms, LineWidths::uniform(Sp(44.0)), &style);
        assert!(eng.stats().full, "width change must trigger a full layout");
        let mut style2 = style.clone();
        style2.breaking.continuity_weight *= 2.0;
        eng.layout(&ms, LineWidths::uniform(Sp(44.0)), &style2);
        assert!(eng.stats().full, "style change must trigger a full layout");
    }

    #[test]
    fn deterministic_across_engines() {
        let style = LayoutStyle::default();
        let mut ms: Vec<MeasureSource> = (0..40).map(|i| measure(i as u64 + 1, 0.0)).collect();
        let mut a = IncrementalLayout::new();
        let mut b = IncrementalLayout::new();
        a.layout(&ms, LineWidths::uniform(Sp(40.0)), &style);
        b.layout(&ms, LineWidths::uniform(Sp(40.0)), &style);
        ms[11] = measure(999, 0.3);
        a.layout(&ms, LineWidths::uniform(Sp(40.0)), &style);
        b.layout(&ms, LineWidths::uniform(Sp(40.0)), &style);
        assert_eq!(widths_bits(a.systems()), widths_bits(b.systems()));
        assert_eq!(a.total_cost().to_bits(), b.total_cost().to_bits());
    }
}
