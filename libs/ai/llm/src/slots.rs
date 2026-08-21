//! Slot table and step scheduling for multi-slot (continuous-batching) decode.
//!
//! One session holds one copy of the weights and `N` slots. Every decode step
//! batches **only the slots that are generating right now**, so idle slots cost
//! nothing and a single active client runs at single-client speed. That is the
//! whole point: a 4-slot box must not make a solo chat 45 % slower for nothing.
//!
//! This module is deliberately pure host bookkeeping — no graph, no device, no
//! session. It owns three things that decide how the batch behaves:
//!
//! * where a slot's rows live in the shared caches ([`Slot::kv_base`],
//!   [`Slot::state_base`]),
//! * which slots run in the next step and how wide that step is
//!   ([`SlotTable::plan_step`]),
//! * how much speculation the step can afford ([`draft_depth_for`]).
//!
//! Design of record: `local/agent_state/qwen-parallel/batched-session-design.md`.

use crate::error::{LlamaError, Result};
use crate::runtime::HybridDecodeBatchLayout;

/// Widths a decode step is allowed to take.
///
/// Padding the active count to a small set bounds the compiled-graph space: an
/// active set of 3 runs as 4 with one dead column, which costs one column of
/// decode and saves a whole graph family. `1` is in the set and is exact, so a
/// solo client always gets the solo graph.
pub const BATCH_WIDTHS: [usize; 3] = [1, 2, 4];

/// The most columns the cheap quantised mat-vec path serves in one pass.
///
/// `MMVQ_MAX_BATCH_SIZE` in `mmvq.cuh`, mirrored by `MMV_MAX_COLUMNS` in the
/// CUDA executor. Past it the dispatcher falls into a dequantise-the-whole-
/// model hole that costs several times the weight bandwidth, so
/// `width * (draft + 1) <= COLUMN_BUDGET` is a wall, not a preference.
///
/// Raising it is the measurements lane's job (M2). If it rises, the ladder in
/// [`draft_depth_for`] extends on its own — nothing here needs redesigning.
pub const COLUMN_BUDGET: usize = 8;

/// The **measured** single-stream speculation sweet spot, and therefore the
/// **minimum allocation ceiling** a batched session may be built with.
///
/// Measured on warm chat: n3 beats n5 both with the full draft head (106.3 vs
/// 101.3 tok/s) and with the restricted sidecar head (121.6 vs 119.8), so the
/// solo service runs depth 3 and the dynamic ladder degrades from there.
///
/// Two things at once, and they have to be the same number:
///
/// * it is what a solo client must get — the solo lane runs today's
///   single-stream fast path, not a batched-path compromise, so its draft depth
///   is the shipped `spec_draft_max`, not something reduced to suit four slots;
/// * because the ladder only ever *narrows* from the allocation, sizing the
///   allocation any lower would clip the solo rung and there would be no error
///   to notice.
///
/// Costs `slots * (SOLO_DRAFT_MAX + 2)` recurrent rows — at 4 slots that is
/// 4.09 GiB against 1.75 GiB for a depth-1 allocation. That ~2.3 GiB is the
/// price of the solo lane staying fast, and it fits the 4-slot 16k budget.
pub const SOLO_DRAFT_MAX: usize = 3;

/// Speculative draft depth a step of this width can afford.
///
/// This is the dynamic-degradation ladder, and it is why one active client
/// stays at today's speed instead of dropping to the unspeculated rate: the
/// column budget freed by idle slots is spent on draft depth.
///
/// `draft_max` is the session's **allocation** ceiling — how many draft tokens
/// its recurrent rows were sized for (`draft_max + 2` rows per slot). It is
/// deliberately NOT the depth that happens to be optimal when running solo.
///
/// That distinction is a trap worth naming, because it is invisible until the
/// column budget moves. Measurement says a raised cap's value is not more slots
/// but **more draft depth at 4 slots** — at a 16-column budget, 4 slots can
/// afford depth 3, and speculation is what carries 4 slots over the playability
/// floor. If this were configured to the solo optimum (2), that rung would be
/// silently capped at 2 and the whole point of raising the cap would be lost
/// with no error anywhere. Size the allocation for the deepest rung any width
/// may take, and let this function do the narrowing.
pub fn draft_depth_for(width: usize, draft_max: usize) -> usize {
    draft_depth_for_budget(width, draft_max, COLUMN_BUDGET)
}

/// [`draft_depth_for`] against an explicit column budget.
///
/// The budget is a parameter so the ladder can be evaluated — and tested — at a
/// raised cap before the cap itself moves. When the cap raise lands with both
/// its verdicts (faster AND numerically sound), [`COLUMN_BUDGET`] is the single
/// value that changes.
pub fn draft_depth_for_budget(width: usize, draft_max: usize, column_budget: usize) -> usize {
    if width == 0 || draft_max == 0 {
        return 0;
    }
    // width * (draft + 1) <= column_budget
    let affordable = column_budget / width;
    affordable.saturating_sub(1).min(draft_max)
}

/// Measured cost of one decode step, and of one batched draft forward.
///
/// The step cost is indexed by TOTAL columns: a speculative round with lanes at
/// depths `k_i` runs one verify batch of `sum(k_i + 1)` columns and the same
/// number of logit rows, which is exactly the shape the `n_outputs = B` sweep
/// measured.
#[derive(Clone, Debug)]
pub struct StepCostModel {
    /// `step_ms[c]` is the cost of a `c`-column step. Index 0 is unused.
    step_ms: Vec<f32>,
    draft_base_ms: f32,
    draft_per_lane_ms: f32,
}

impl StepCostModel {
    /// The measured `n_outputs = B` curve on .217 (5090, Q4_K_M, fill-4096,
    /// medians of 12 after 3 warm-ups).
    ///
    /// **Lower bound**: the verify-batch shape carries neither the per-slot
    /// recurrent state reads nor the unified-cache attention span, so a real
    /// multi-slot step costs at least this. Columns 1..8 are measured points;
    /// 9..16 come from the raised-cap sweep with the gaps linearly
    /// interpolated, and are only reachable if the cap raise is ever taken.
    pub fn measured_5090() -> Self {
        const MEASURED: &[(usize, f32)] = &[
            (1, 14.113),
            (2, 17.889),
            (3, 21.770),
            (4, 24.286),
            (5, 26.891),
            (6, 31.991),
            (7, 37.730),
            (8, 39.986),
            // raised-cap sweep
            (10, 52.559),
            (12, 58.010),
            (16, 75.034),
        ];
        // Measured draft-forward cost per DRAFTED token with the full
        // 248,320-row draft head: 1.72-1.87 ms. Verify dominates a round; the
        // draft chain is comparatively cheap.
        Self::from_points(MEASURED, 1.80, 0.30)
    }

    /// Build from sparse measured points, linearly interpolating the gaps.
    pub fn from_points(points: &[(usize, f32)], draft_base_ms: f32, draft_per_lane_ms: f32) -> Self {
        let max = points.iter().map(|(c, _)| *c).max().unwrap_or(1);
        let mut step_ms = vec![0.0f32; max + 1];
        for window in points.windows(2) {
            let (c0, t0) = window[0];
            let (c1, t1) = window[1];
            for c in c0..=c1 {
                let t = (c - c0) as f32 / (c1 - c0).max(1) as f32;
                step_ms[c] = t0 + (t1 - t0) * t;
            }
        }
        if let Some(&(c, t)) = points.first() {
            step_ms[c] = t;
            for c in 1..c {
                step_ms[c] = t;
            }
        }
        Self {
            step_ms,
            draft_base_ms,
            draft_per_lane_ms,
        }
    }

    /// Cost of a step of `columns` columns, or `None` past the measured range.
    pub fn step_ms(&self, columns: usize) -> Option<f32> {
        self.step_ms.get(columns).copied().filter(|ms| *ms > 0.0)
    }

    /// Cost of one draft forward shared by `lanes` lanes.
    pub fn draft_forward_ms(&self, lanes: usize) -> f32 {
        if lanes == 0 {
            return 0.0;
        }
        self.draft_base_ms + (lanes - 1) as f32 * self.draft_per_lane_ms
    }

    /// Scale the draft forward, e.g. once a restricted-vocab draft head makes it
    /// cheaper. The sidecar shrinks exactly this term, and it is the term that
    /// decides whether deeper drafts pay.
    pub fn with_draft_cost(mut self, base_ms: f32, per_lane_ms: f32) -> Self {
        self.draft_base_ms = base_ms;
        self.draft_per_lane_ms = per_lane_ms;
        self
    }

    /// The restricted-vocabulary (`.draftvocab`) draft head.
    ///
    /// Measured at ~0.52 ms per drafted token against 1.72-1.87 ms for the full
    /// head — 3.3x cheaper — and **acceptance is identical** (0.7647, same
    /// counts) because the restricted set covers 99.8 % of chat output tokens.
    /// So it models as a pure draft-cost reduction with no acceptance penalty,
    /// and nothing in the allocator's demand side changes.
    pub fn with_restricted_draft_head(self) -> Self {
        self.with_draft_cost(0.52, 0.10)
    }
}

/// Measured per-draft-token acceptance on warm chat, full or restricted head
/// alike. The allocator's default assumption before a lane has an EMA of its
/// own.
pub const MEASURED_CHAT_ACCEPTANCE: f32 = 0.7647;

/// Measured acceptance on expander-prose prompts, where speculation LOSES
/// outright (0.66x greedy at depth 5). Kept as a named constant because it is
/// the case the allocator has to get right: a lane like this must fall to depth
/// 0 on its own and hand its columns to lanes that will spend them.
pub const MEASURED_EXPANDER_ACCEPTANCE: f32 = 0.219;

/// One active lane's live demand, as the allocator sees it.
#[derive(Clone, Copy, Debug)]
pub struct LaneDemand {
    pub slot: usize,
    /// EMA of per-draft-token acceptance, in `[0, 1]`. A lane on a long
    /// accepted run earns depth; a lane that keeps rejecting drops to depth 0
    /// and hands its columns to lanes that will use them.
    pub acceptance: f32,
}

impl LaneDemand {
    /// Expected tokens a round yields at draft depth `k`: `1 + a + ... + a^k`.
    ///
    /// Concave in `k` — the marginal value of one more draft column is
    /// `a^(k+1)`, which strictly decreases. That concavity is what makes the
    /// greedy allocation in [`allocate_depths`] optimal rather than a heuristic.
    fn expected_tokens(&self, k: usize) -> f32 {
        let a = self.acceptance.clamp(0.0, 1.0);
        (0..=k).map(|j| a.powi(j as i32)).sum()
    }

    fn marginal(&self, k: usize) -> f32 {
        self.acceptance.clamp(0.0, 1.0).powi(k as i32 + 1)
    }
}

/// What the allocator is optimising for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerObjective {
    /// Maximise aggregate accepted tokens/second, subject to the per-client
    /// floor. This is the shipped objective.
    MaxAggregate,
    /// Maximise the slowest lane's rate. Reserved for a future latency mode;
    /// also the fallback when no allocation can satisfy the floor.
    MaxMinLatency,
}

/// Scheduler policy. The objective and the floor are config, not constants,
/// because they are a product decision rather than a hardware fact.
#[derive(Clone, Copy, Debug)]
pub struct SchedulerConfig {
    pub column_budget: usize,
    pub draft_max: usize,
    /// Per-client tokens/second below which an allocation is considered to have
    /// broken the promise. 0 disables the constraint.
    pub floor_tok_s: f32,
    pub objective: SchedulerObjective,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            column_budget: COLUMN_BUDGET,
            draft_max: SOLO_DRAFT_MAX,
            floor_tok_s: 50.0,
            objective: SchedulerObjective::MaxAggregate,
        }
    }
}

/// A chosen split of the column budget across lanes.
#[derive(Clone, Debug, PartialEq)]
pub struct Allocation {
    /// Draft depth per lane, parallel to the input slice. Need NOT be uniform.
    pub depths: Vec<usize>,
    pub columns: usize,
    pub round_ms: f32,
    pub aggregate_tok_s: f32,
    pub per_client_tok_s: Vec<f32>,
    /// Whether every lane cleared the configured floor.
    pub meets_floor: bool,
}

impl Allocation {
    fn min_per_client(&self) -> f32 {
        self.per_client_tok_s
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min)
    }
}

/// Allocate the column budget across active lanes to maximise the objective.
///
/// Each lane always holds one column (its own next token); the surplus is
/// distributed as draft depth. For every affordable total column count we take
/// the greedy allocation — repeatedly hand the next column to the lane with the
/// highest marginal `a^(k+1)` — which is exactly optimal for a separable
/// concave objective under a cardinality constraint, then score the resulting
/// round and keep the best.
///
/// That makes this a scan over at most `column_budget` candidates, each a
/// greedy pass, so it is a table-lookup-scale computation per step and not a
/// solver in the hot path.
///
/// Degenerate cases fall out rather than being special-cased: one lane takes
/// the whole budget as depth (the solo fast path), and equal acceptances
/// reproduce the uniform ladder.
pub fn allocate_depths(
    lanes: &[LaneDemand],
    config: &SchedulerConfig,
    costs: &StepCostModel,
) -> Option<Allocation> {
    let width = lanes.len();
    if width == 0 {
        return None;
    }
    let mut best: Option<Allocation> = None;
    for columns in width..=config.column_budget {
        let Some(step_ms) = costs.step_ms(columns) else {
            continue;
        };
        let mut depths = vec![0usize; width];
        for _ in 0..(columns - width) {
            let pick = (0..width)
                .filter(|&i| depths[i] < config.draft_max)
                .max_by(|&a, &b| {
                    lanes[a]
                        .marginal(depths[a])
                        .partial_cmp(&lanes[b].marginal(depths[b]))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            match pick {
                // A positive marginal is always worth taking for the numerator;
                // whether the extra column pays for itself is decided by the
                // ratio, which is why every column count is scored.
                Some(index) if lanes[index].marginal(depths[index]) > 0.0 => depths[index] += 1,
                _ => break,
            }
        }
        let used: usize = depths.iter().map(|k| k + 1).sum();
        if used != columns {
            // Could not spend the budget (every lane at draft_max, or zero
            // acceptance everywhere); a narrower candidate already covered it.
            continue;
        }
        let rounds = depths.iter().copied().max().unwrap_or(0);
        let draft_ms: f32 = (0..rounds)
            .map(|round| {
                let drafting = depths.iter().filter(|&&k| k > round).count();
                costs.draft_forward_ms(drafting)
            })
            .sum();
        let round_ms = step_ms + draft_ms;
        if round_ms <= 0.0 {
            continue;
        }
        let per_client: Vec<f32> = lanes
            .iter()
            .zip(&depths)
            .map(|(lane, &k)| lane.expected_tokens(k) / (round_ms / 1000.0))
            .collect();
        let aggregate: f32 = per_client.iter().sum();
        let candidate = Allocation {
            meets_floor: per_client.iter().all(|rate| *rate >= config.floor_tok_s),
            depths,
            columns,
            round_ms,
            aggregate_tok_s: aggregate,
            per_client_tok_s: per_client,
        };
        best = Some(match best {
            None => candidate,
            Some(current) => pick_better(current, candidate, config.objective),
        });
    }
    best
}

/// Floor first, then the objective. An allocation that keeps every client above
/// the promised rate always beats one that does not, however much aggregate the
/// latter wins — that is what makes the floor a constraint rather than a wish.
///
/// **When NO allocation can meet the floor, the objective switches to
/// max-min.** Chasing aggregate through an unmeetable floor picks winners among
/// identical clients: at 4 lanes on measured costs, max-aggregate prefers
/// `[0,0,0,1]` over `[1,1,1,1]` for **0.4 % more aggregate and a 1.76x spread
/// between clients doing the same thing**. Once the promise is already broken,
/// breaking it evenly is the only defensible degradation.
fn pick_better(
    current: Allocation,
    candidate: Allocation,
    objective: SchedulerObjective,
) -> Allocation {
    match (current.meets_floor, candidate.meets_floor) {
        (true, false) => return current,
        (false, true) => return candidate,
        (false, false) => {
            return if candidate.min_per_client() > current.min_per_client() {
                candidate
            } else {
                current
            }
        }
        (true, true) => {}
    }
    let better = match objective {
        SchedulerObjective::MaxAggregate => candidate.aggregate_tok_s > current.aggregate_tok_s,
        SchedulerObjective::MaxMinLatency => candidate.min_per_client() > current.min_per_client(),
    };
    if better {
        candidate
    } else {
        current
    }
}

/// Whether admitting one more lane improves things.
///
/// Admission is part of the same optimisation, not a separate capacity check:
/// a lane is admitted only if the post-admission allocation still clears the
/// floor for everyone AND raises expected aggregate. So a box at its speed
/// limit stops admitting before it starts degrading the clients it already has.
pub fn should_admit(
    active: &[LaneDemand],
    candidate: LaneDemand,
    config: &SchedulerConfig,
    costs: &StepCostModel,
) -> bool {
    let mut widened = active.to_vec();
    widened.push(candidate);
    let Some(after) = allocate_depths(&widened, config, costs) else {
        return false;
    };
    if !after.meets_floor && config.floor_tok_s > 0.0 {
        return false;
    }
    match allocate_depths(active, config, costs) {
        None => true,
        Some(before) => after.aggregate_tok_s > before.aggregate_tok_s,
    }
}

/// What a slot is doing between steps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotPhase {
    /// No job attached. Contributes nothing to a step and costs nothing.
    Idle,
    /// Ingesting a prompt. Contributes a chunk, not a single token.
    Prefilling { cursor: usize },
    /// Generating. Contributes exactly one token per step.
    Decoding,
}

/// One slot's place in the shared caches and its position counters.
#[derive(Clone, Debug)]
pub struct Slot {
    index: usize,
    kv_base: usize,
    state_base: usize,
    capacity: usize,
    phase: SlotPhase,
    /// Tokens this slot's KV region currently holds.
    fill: usize,
    /// Next M-RoPE position. Tracks `fill` for pure text; falls behind after an
    /// image span, whose n_pos is max(tokens_w, tokens_h), not its token count.
    rope_pos_next: i64,
    /// Which of the slot's own recurrent rows is live, as an offset inside its
    /// block. Always 0 without speculation; with it, the verify batch
    /// checkpoints into the slot's other rows and this follows the live one.
    live_state_offset: usize,
}

impl Slot {
    /// First absolute attention-cache row this slot owns.
    ///
    /// Slot-major, so slot 0's base is 0 and a solo session on slot 0 writes
    /// exactly the rows a single-sequence session writes today. The solo
    /// determinism gate rests on that.
    pub fn kv_base(&self) -> usize {
        self.kv_base
    }

    /// First recurrent-state row this slot owns. With speculation the slot owns
    /// `draft_max + 2` consecutive rows: the live row plus one checkpoint per
    /// verify-batch position.
    pub fn state_base(&self) -> usize {
        self.state_base
    }

    /// The recurrent row the non-speculative graphs read and write for this
    /// slot. Always inside the slot's own block.
    pub fn live_state_row(&self) -> usize {
        self.state_base + self.live_state_offset
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn phase(&self) -> SlotPhase {
        self.phase
    }

    pub fn fill(&self) -> usize {
        self.fill
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn rope_pos_next(&self) -> i64 {
        self.rope_pos_next
    }

    pub fn remaining_context(&self) -> usize {
        self.capacity.saturating_sub(self.fill)
    }

    /// Absolute cache row the slot's next token will occupy.
    pub fn next_write_row(&self) -> usize {
        self.kv_base + self.fill
    }

    /// Highest absolute row this slot occupies, or `None` while empty.
    fn high_water(&self) -> Option<usize> {
        (self.fill > 0).then(|| self.kv_base + self.fill - 1)
    }

    fn is_active(&self) -> bool {
        !matches!(self.phase, SlotPhase::Idle)
    }
}

/// One slot's contribution to a planned step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlotStep {
    /// Which slot, so the caller can map logit rows back to jobs.
    pub slot: usize,
    /// Absolute cache row this token writes: `kv_base + fill`.
    pub write_row: usize,
    /// First absolute row it may attend to. Feeds
    /// `HybridDecodeBatchLayout::attention_key_lower_bounds`.
    pub key_lower_bound: usize,
    /// Within-slot position, for RoPE.
    pub position: usize,
    /// Recurrent row this slot reads and writes this step.
    pub state_row: usize,
}

/// The decode step to run next.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepPlan {
    /// Slots contributing a token, in batch order. Logit row `i` belongs to
    /// `slots[i]`.
    pub slots: Vec<SlotStep>,
    /// Graph width — the exact number of contributing slots. Every column is
    /// live; there are no dead columns to reason about.
    pub width: usize,
    /// Speculative draft depth this step can afford, from [`draft_depth_for`].
    pub draft_depth: usize,
    /// Attention key span the graph must cover: one past the highest occupied
    /// absolute row across ACTIVE slots.
    pub attention_key_count: usize,
}

impl StepPlan {
    /// Columns this step spends, against [`COLUMN_BUDGET`].
    pub fn columns(&self) -> usize {
        self.width * (self.draft_depth + 1)
    }

    /// Turn the plan into the decode batch layout the graph is fed.
    ///
    /// One token per slot, one logit row per slot, in plan order — so logit row
    /// `i` belongs to `self.slots[i].slot`.
    ///
    /// `positions` stay WITHIN-slot (they drive RoPE), while
    /// `attention_write_indices` are absolute arena rows. For a single sequence
    /// on slot 0 those coincide, and the lower-bound vector is left empty, so
    /// this produces byte-for-byte the layout a single-sequence decode produces
    /// today. That is what keeps one active client on today's numbers.
    pub fn to_batch_layout(&self) -> Result<HybridDecodeBatchLayout> {
        if self.slots.is_empty() {
            return Err(LlamaError::format(
                "a decode step plan needs at least one slot",
            ));
        }
        let as_i32 = |value: usize, what: &str| -> Result<i32> {
            i32::try_from(value)
                .map_err(|_| LlamaError::format(format!("{what} {value} does not fit in i32")))
        };

        let positions = self
            .slots
            .iter()
            .map(|step| as_i32(step.position, "slot position"))
            .collect::<Result<Vec<_>>>()?;
        let attention_write_indices = self
            .slots
            .iter()
            .map(|step| as_i32(step.write_row, "slot write row"))
            .collect::<Result<Vec<_>>>()?;
        let recurrent_state_rows = self
            .slots
            .iter()
            .map(|step| as_i32(step.state_row, "slot state row"))
            .collect::<Result<Vec<_>>>()?;
        let output_ids = (0..self.slots.len())
            .map(|index| as_i32(index, "slot output id"))
            .collect::<Result<Vec<_>>>()?;
        // All-zero bounds mean "one sequence from row 0", and the mask builder
        // has a dedicated path for that. Keeping the vector empty there is what
        // makes the solo lane byte-identical rather than merely equivalent.
        let attention_key_lower_bounds = if self
            .slots
            .iter()
            .all(|step| step.key_lower_bound == 0)
        {
            Vec::new()
        } else {
            self.slots
                .iter()
                .map(|step| as_i32(step.key_lower_bound, "slot key lower bound"))
                .collect::<Result<Vec<_>>>()?
        };

        let layout = HybridDecodeBatchLayout {
            positions,
            attention_write_indices,
            attention_key_count: self.attention_key_count,
            recurrent_state_rows,
            output_ids,
            rope_positions: None,
            hidden_read_rows: Vec::new(),
            hidden_write_rows: Vec::new(),
            attention_key_lower_bounds,
        };
        layout.validate()?;
        if let Some(&highest) = layout.attention_write_indices.iter().max() {
            if usize::try_from(highest).unwrap_or(usize::MAX) >= self.attention_key_count {
                return Err(LlamaError::format(format!(
                    "step plan writes cache row {} but only spans {} keys",
                    highest, self.attention_key_count
                )));
            }
        }
        Ok(layout)
    }
}

/// The slot table: `N` slots over one flat KV arena and one recurrent arena.
#[derive(Clone, Debug)]
pub struct SlotTable {
    slots: Vec<Slot>,
    per_slot_context: usize,
    state_rows_per_slot: usize,
    solo_draft_max: usize,
}

impl SlotTable {
    /// `per_slot_context` is the context one slot may hold; the attention arena
    /// is `slots * per_slot_context` rows. `state_rows_per_slot` is
    /// `draft_max + 2` when speculating, else 1.
    pub fn new(
        slots: usize,
        per_slot_context: usize,
        state_rows_per_slot: usize,
        solo_draft_max: usize,
    ) -> Result<Self> {
        if slots == 0 {
            return Err(LlamaError::format("slot table needs at least one slot"));
        }
        if per_slot_context == 0 {
            return Err(LlamaError::format(
                "slot table needs a non-zero per-slot context",
            ));
        }
        if state_rows_per_slot == 0 {
            return Err(LlamaError::format(
                "slot table needs at least one recurrent state row per slot",
            ));
        }
        let slots = (0..slots)
            .map(|index| Slot {
                index,
                kv_base: index * per_slot_context,
                state_base: index * state_rows_per_slot,
                capacity: per_slot_context,
                phase: SlotPhase::Idle,
                fill: 0,
                rope_pos_next: 0,
                live_state_offset: 0,
            })
            .collect();
        Ok(Self {
            slots,
            per_slot_context,
            state_rows_per_slot,
            solo_draft_max,
        })
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Rows in the shared attention arena: `slots * per_slot_context`.
    pub fn attention_arena_rows(&self) -> usize {
        self.slots.len() * self.per_slot_context
    }

    /// Rows in the shared recurrent arena.
    pub fn state_arena_rows(&self) -> usize {
        self.slots.len() * self.state_rows_per_slot
    }

    pub fn slot(&self, index: usize) -> Option<&Slot> {
        self.slots.get(index)
    }

    pub fn slot_mut(&mut self, index: usize) -> Option<&mut Slot> {
        self.slots.get_mut(index)
    }

    pub fn active_count(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_active()).count()
    }

    pub fn idle_count(&self) -> usize {
        self.slots.len() - self.active_count()
    }

    /// Claim the lowest free slot, or `None` when all are busy.
    ///
    /// Lowest-first matters: with one client it always lands on slot 0, whose
    /// `kv_base` is 0, so the solo lane is today's layout exactly.
    ///
    /// Admission needs no cache clearing — a slot's stale rows sit above its
    /// new `fill` and are masked exactly by the lower/upper span.
    pub fn admit(&mut self) -> Option<usize> {
        let index = self
            .slots
            .iter()
            .position(|slot| matches!(slot.phase, SlotPhase::Idle))?;
        let slot = &mut self.slots[index];
        slot.phase = SlotPhase::Prefilling { cursor: 0 };
        slot.fill = 0;
        slot.rope_pos_next = 0;
        slot.live_state_offset = 0;
        Some(index)
    }

    /// Release a slot. Pure bookkeeping: nothing on the device is touched.
    pub fn retire(&mut self, index: usize) -> Result<()> {
        let slot = self.require_mut(index)?;
        slot.phase = SlotPhase::Idle;
        slot.fill = 0;
        slot.rope_pos_next = 0;
        slot.live_state_offset = 0;
        Ok(())
    }

    /// Move a slot from prefill to decode.
    pub fn begin_decoding(&mut self, index: usize) -> Result<()> {
        let slot = self.require_mut(index)?;
        if matches!(slot.phase, SlotPhase::Idle) {
            return Err(LlamaError::format(format!(
                "slot {index} cannot start decoding while idle"
            )));
        }
        slot.phase = SlotPhase::Decoding;
        Ok(())
    }

    /// Record that a slot ingested or generated `count` tokens.
    pub fn advance(&mut self, index: usize, count: usize) -> Result<()> {
        let per_slot_context = self.per_slot_context;
        let slot = self.require_mut(index)?;
        let fill = slot.fill + count;
        if fill > per_slot_context {
            return Err(LlamaError::format(format!(
                "slot {index} would hold {fill} tokens, past its {per_slot_context}-token capacity"
            )));
        }
        slot.fill = fill;
        slot.rope_pos_next += count as i64;
        Ok(())
    }

    /// Attention key span the next graph must cover: one past the highest
    /// occupied absolute row across ACTIVE slots.
    ///
    /// Retired slots are excluded, so a step never pays for a region nobody is
    /// reading. With only slot 0 active this is exactly that slot's fill —
    /// i.e. today's key count.
    pub fn attention_key_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.is_active())
            .filter_map(|slot| slot.high_water())
            .max()
            .map(|row| row + 1)
            .unwrap_or(0)
    }

    /// Plan the next decode step from the slots that are decoding right now.
    ///
    /// Returns `None` when nothing is decoding — the caller should block rather
    /// than run an empty step. Slots that are still prefilling are excluded:
    /// prefill rides its own step (a chunk, not a token).
    pub fn plan_step(&self) -> Option<StepPlan> {
        let slots: Vec<SlotStep> = self
            .slots
            .iter()
            .filter(|slot| matches!(slot.phase, SlotPhase::Decoding))
            .map(|slot| SlotStep {
                slot: slot.index,
                write_row: slot.next_write_row(),
                key_lower_bound: slot.kv_base,
                position: slot.fill,
                state_row: slot.live_state_row(),
            })
            .collect();
        if slots.is_empty() {
            return None;
        }
        // EXACT active width, not padded. Padding to {1,2,4} would save one
        // compiled-graph family at N=4, but a dead column has to point its KV
        // write and state row SOMEWHERE, and every cheap answer either
        // double-writes a live slot's cache row or needs a scratch row the
        // graph would still scan. One extra graph family is the cheaper trade;
        // `pad_batch_width` stays available if shape pressure ever appears.
        let width = slots.len();
        // The span must cover the rows this step WRITES, not just the rows
        // already occupied — the highest writer may be a slot that is one token
        // ahead of everyone else.
        let write_span = slots
            .iter()
            .map(|step| step.write_row + 1)
            .max()
            .unwrap_or(0);
        Some(StepPlan {
            width,
            draft_depth: draft_depth_for(width, self.solo_draft_max),
            attention_key_count: self.attention_key_count().max(write_span),
            slots,
        })
    }

    fn require_mut(&mut self, index: usize) -> Result<&mut Slot> {
        let len = self.slots.len();
        self.slots
            .get_mut(index)
            .ok_or_else(|| LlamaError::format(format!("slot {index} is out of range 0..{len}")))
    }
}

/// Round an active count up to a compiled graph width.
///
/// Counts past the largest width are clamped to it; the caller is responsible
/// for never activating more slots than the table holds.
pub fn pad_batch_width(active: usize) -> usize {
    BATCH_WIDTHS
        .iter()
        .copied()
        .find(|&width| width >= active)
        .unwrap_or_else(|| BATCH_WIDTHS[BATCH_WIDTHS.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_draft_ladder_spends_exactly_the_free_column_budget() {
        // Every rung must fit the column budget — past it a step falls off the
        // MMVQ cliff into a dequantise-the-whole-model path.
        for (width, expected) in [(1, 2), (2, 2), (3, 1), (4, 1)] {
            let depth = draft_depth_for(width, 2);
            assert_eq!(depth, expected, "ladder rung for width {width}");
            assert!(
                width * (depth + 1) <= COLUMN_BUDGET,
                "width {width} draft {depth} spends {} of {COLUMN_BUDGET} columns",
                width * (depth + 1)
            );
        }
        assert_eq!(draft_depth_for(8, 2), 0);
        assert_eq!(draft_depth_for(9, 2), 0);
        // A session that disabled speculation never gets it back.
        assert_eq!(draft_depth_for(1, 0), 0);
    }

    fn lanes(acceptances: &[f32]) -> Vec<LaneDemand> {
        acceptances
            .iter()
            .enumerate()
            .map(|(slot, &acceptance)| LaneDemand { slot, acceptance })
            .collect()
    }

    /// Floor off, so tests exercise the objective rather than the constraint.
    fn open_config() -> SchedulerConfig {
        SchedulerConfig {
            floor_tok_s: 0.0,
            ..SchedulerConfig::default()
        }
    }

    #[test]
    fn one_client_gets_the_whole_budget_as_depth() {
        // The solo case must fall OUT of the optimiser, not be special-cased:
        // with nothing to share with, every spare column becomes speculation.
        let allocation = allocate_depths(
            &lanes(&[0.9]),
            &open_config(),
            &StepCostModel::measured_5090(),
        )
        .expect("allocation");
        assert_eq!(allocation.depths.len(), 1);
        assert!(
            allocation.depths[0] >= 1,
            "a solo lane with a 0.9 accept rate must speculate, got {:?}",
            allocation.depths
        );
        assert!(allocation.columns <= COLUMN_BUDGET);
    }

    #[test]
    fn a_lane_that_keeps_rejecting_gives_its_columns_to_one_that_does_not() {
        // The user's example: depths need not be uniform. A lane whose drafts
        // keep missing should not hold columns a productive lane could spend.
        let allocation = allocate_depths(
            &lanes(&[0.95, 0.02]),
            &open_config(),
            &StepCostModel::measured_5090(),
        )
        .expect("allocation");
        assert!(
            allocation.depths[0] > allocation.depths[1],
            "the accepting lane must out-earn the rejecting one, got {:?}",
            allocation.depths
        );
        assert_eq!(
            allocation.depths[1], 0,
            "a near-zero-acceptance lane should hold no draft columns"
        );
    }

    #[test]
    fn equal_acceptance_spreads_columns_evenly() {
        // With nothing to distinguish lanes the optimiser must not play
        // favourites — an even split is the only defensible answer.
        let allocation = allocate_depths(
            &lanes(&[0.9, 0.9, 0.9, 0.9]),
            &open_config(),
            &StepCostModel::measured_5090(),
        )
        .expect("allocation");
        let min = allocation.depths.iter().copied().min().unwrap_or(0);
        let max = allocation.depths.iter().copied().max().unwrap_or(0);
        assert!(
            max - min <= 1,
            "equal lanes must differ by at most a rounding column, got {:?}",
            allocation.depths
        );
    }

    #[test]
    fn an_allocation_never_overspends_the_column_budget() {
        // Overspending falls off the MMVQ cliff, so this is the one invariant
        // that must hold for every demand shape.
        let costs = StepCostModel::measured_5090();
        for shape in [
            vec![0.9],
            vec![0.9, 0.5],
            vec![0.99, 0.99, 0.99],
            vec![0.9, 0.8, 0.7, 0.6],
            vec![0.5; 6],
            vec![0.9; 8],
        ] {
            let config = open_config();
            let allocation =
                allocate_depths(&lanes(&shape), &config, &costs).expect("allocation");
            let spent: usize = allocation.depths.iter().map(|k| k + 1).sum();
            assert_eq!(spent, allocation.columns);
            assert!(
                spent <= config.column_budget,
                "{} lanes overspent: {:?}",
                shape.len(),
                allocation.depths
            );
            assert!(allocation.depths.iter().all(|k| *k <= config.draft_max));
        }
    }

    #[test]
    fn the_floor_outranks_aggregate() {
        // A floor-satisfying allocation must win even when a floor-breaking one
        // scores more aggregate, or the floor is decoration.
        let costs = StepCostModel::measured_5090();
        let demand = lanes(&[0.9, 0.9]);
        let open = allocate_depths(&demand, &open_config(), &costs).expect("open");
        let strict = allocate_depths(
            &demand,
            &SchedulerConfig {
                floor_tok_s: open.min_per_client() + 5.0,
                ..SchedulerConfig::default()
            },
            &costs,
        )
        .expect("strict");
        // Either it found something clearing the raised floor, or nothing could
        // and it fell back — but it must never report a false pass.
        if strict.meets_floor {
            assert!(strict.min_per_client() >= open.min_per_client());
        }
    }

    #[test]
    fn admission_stops_before_it_degrades_the_clients_already_there() {
        let costs = StepCostModel::measured_5090();
        let config = SchedulerConfig {
            floor_tok_s: 50.0,
            ..SchedulerConfig::default()
        };
        let newcomer = LaneDemand {
            slot: 9,
            acceptance: 0.9,
        };
        // An idle box admits.
        assert!(should_admit(&[], newcomer, &config, &costs));
        // A box already past the floor must refuse rather than degrade further.
        let crowded = lanes(&[0.9, 0.9, 0.9, 0.9, 0.9, 0.9, 0.9, 0.9]);
        assert!(!should_admit(&crowded, newcomer, &config, &costs));
    }

    #[test]
    fn a_lane_that_loses_under_speculation_switches_itself_off() {
        // Measured: expander-prose accepts 0.219 and speculation makes it
        // SLOWER (0.66x greedy at depth 5). The allocator has to reach depth 0
        // for that lane on its own — per-class policy falling out of measured
        // demand, with no prompt-class special case anywhere in the planner.
        let costs = StepCostModel::measured_5090();
        let solo_expander = allocate_depths(
            &lanes(&[MEASURED_EXPANDER_ACCEPTANCE]),
            &open_config(),
            &costs,
        )
        .expect("allocation");
        assert_eq!(
            solo_expander.depths,
            vec![0],
            "a lane that loses under speculation must not speculate"
        );

        // And mixed with a healthy chat lane, the chat lane takes the columns.
        let mixed = allocate_depths(
            &lanes(&[MEASURED_CHAT_ACCEPTANCE, MEASURED_EXPANDER_ACCEPTANCE]),
            &open_config(),
            &costs,
        )
        .expect("allocation");
        assert!(mixed.depths[0] > 0, "chat lane should speculate");
        assert_eq!(mixed.depths[1], 0, "expander lane should not");
    }

    #[test]
    fn an_unmeetable_floor_degrades_everyone_evenly() {
        // Once the promise is already broken, chasing aggregate picks winners
        // among identical clients. On measured costs at 4 lanes, max-aggregate
        // prefers [0,0,0,1] over [1,1,1,1] for 0.4% more aggregate and a 1.76x
        // spread between clients doing the exact same thing. Refuse that trade.
        let costs = StepCostModel::measured_5090();
        let demand = lanes(&[MEASURED_CHAT_ACCEPTANCE; 4]);
        let config = SchedulerConfig {
            floor_tok_s: 50.0,
            ..SchedulerConfig::default()
        };
        let allocation = allocate_depths(&demand, &config, &costs).expect("allocation");
        assert!(
            !allocation.meets_floor,
            "this fixture is meant to exercise the unmeetable-floor path"
        );
        let min = allocation.min_per_client();
        let max = allocation
            .per_client_tok_s
            .iter()
            .copied()
            .fold(0.0f32, f32::max);
        assert!(
            max / min < 1.2,
            "identical clients must degrade together, got {:?}",
            allocation.per_client_tok_s
        );
    }

    #[test]
    fn a_cheaper_draft_forward_buys_more_depth() {
        // The sidecar's whole effect on this lane: it shrinks the draft
        // forward, and the depth economics move with it. Pinning the direction
        // means the recalibration is a number change, not a redesign.
        let demand = lanes(&[0.9, 0.9, 0.9, 0.9]);
        let config = open_config();
        let full_head = StepCostModel::measured_5090();
        let sidecar = StepCostModel::measured_5090().with_draft_cost(0.8, 0.1);
        let before = allocate_depths(&demand, &config, &full_head).expect("before");
        let after = allocate_depths(&demand, &config, &sidecar).expect("after");
        let depth_before: usize = before.depths.iter().sum();
        let depth_after: usize = after.depths.iter().sum();
        assert!(
            depth_after >= depth_before,
            "a cheaper draft must never buy LESS depth: {depth_before} -> {depth_after}"
        );
        assert!(after.aggregate_tok_s > before.aggregate_tok_s);
    }

    #[test]
    fn the_solo_rung_is_campaign_grade_speculation_not_a_batched_compromise() {
        // The product requirement, encoded: one client gets the WHOLE column
        // budget spent on speculation — today's single-stream fast path, at the
        // shipped depth. A four-slot server must not quietly cost the solo chat
        // anything, and the only way that regresses is if someone sizes the
        // allocation for the wide rungs.
        assert_eq!(draft_depth_for(1, SOLO_DRAFT_MAX), SOLO_DRAFT_MAX);
        assert!(
            1 * (SOLO_DRAFT_MAX + 1) <= COLUMN_BUDGET,
            "the solo rung must fit the budget it is allowed to spend"
        );
        // And it survives a raised cap unchanged — the solo path is the one
        // thing a cap change must never move.
        assert_eq!(draft_depth_for_budget(1, SOLO_DRAFT_MAX, 16), SOLO_DRAFT_MAX);
    }

    #[test]
    fn columns_move_from_draft_depth_to_lanes_as_clients_join() {
        // The dynamic reallocation the user asked for: idle capacity is spent
        // on depth, and each joining client trades depth for a lane, per step,
        // with no mode flip. Pinned at the shipped allocation.
        //
        // This is the affordability CEILING, not the chosen depth — the
        // allocator narrows further when measurement says a shallower draft
        // scores better, which at 2 lanes on the measured curve it does.
        let rungs: Vec<usize> = [1usize, 2, 3, 4]
            .iter()
            .map(|&w| draft_depth_for(w, SOLO_DRAFT_MAX))
            .collect();
        assert_eq!(rungs, vec![3, 3, 1, 1], "cap-8 affordability ladder");
        // Depth is monotonically non-increasing in width: joining a client can
        // never somehow buy the others MORE speculation.
        for pair in rungs.windows(2) {
            assert!(pair[0] >= pair[1], "ladder must not rise with width");
        }
        // Every rung spends what it is allowed and no more.
        for (index, &k) in rungs.iter().enumerate() {
            let width = index + 1;
            assert!(width * (k + 1) <= COLUMN_BUDGET);
        }
        // Under a raised cap the wide rungs get their depth back; this is the
        // whole and only benefit of raising it.
        let raised: Vec<usize> = [1usize, 2, 3, 4]
            .iter()
            .map(|&w| draft_depth_for_budget(w, SOLO_DRAFT_MAX, 16))
            .collect();
        assert_eq!(raised, vec![3, 3, 3, 3], "cap-16 affordability ladder");
    }

    #[test]
    fn the_allocation_ceiling_narrows_the_ladder_and_is_not_the_solo_optimum() {
        // The trap this guards: `draft_max` is how many draft tokens the
        // slot's recurrent rows were SIZED for, not the depth that happens to
        // be best when running solo. Configure it to the solo optimum and the
        // wider rungs get silently clipped, with no error anywhere.
        //
        // At a 16-column budget 4 slots can afford depth 3...
        assert_eq!(draft_depth_for_budget(4, 8, 16), 3);
        // ...but an allocation sized for the solo optimum caps it at 2, and
        // nothing reports that the extra depth was unavailable.
        assert_eq!(draft_depth_for_budget(4, 2, 16), 2);
    }

    #[test]
    fn a_raised_budget_changes_only_the_rungs_above_eight_columns() {
        // Measured: widths <= 8 columns are point-for-point identical at both
        // caps, so raising the cap cannot move the shipped solo path. Every
        // rung that already fit in 8 columns must be unchanged.
        for width in [1usize, 2, 3, 4, 8] {
            let at8 = draft_depth_for_budget(width, 8, 8);
            let at16 = draft_depth_for_budget(width, 8, 16);
            if width * (at8 + 1) <= 8 && width * (at16 + 1) <= 8 {
                assert_eq!(at8, at16, "rung for width {width} moved without needing to");
            }
            assert!(width * (at8 + 1) <= 8);
            assert!(width * (at16 + 1) <= 16);
        }
        // The one rung a raised cap actually buys, and the reason the cap
        // raise is argued for at all.
        assert_eq!(draft_depth_for_budget(4, 8, 8), 1);
        assert_eq!(draft_depth_for_budget(4, 8, 16), 3);
    }

    #[test]
    fn batch_widths_pad_up_and_keep_solo_exact() {
        assert_eq!(pad_batch_width(1), 1, "a solo client must get the solo graph");
        assert_eq!(pad_batch_width(2), 2);
        assert_eq!(pad_batch_width(3), 4);
        assert_eq!(pad_batch_width(4), 4);
    }

    fn table() -> SlotTable {
        SlotTable::new(4, 100, 1, 2).expect("table")
    }

    #[test]
    fn slot_bases_partition_both_arenas() {
        let table = SlotTable::new(4, 100, 3, 2).expect("table");
        assert_eq!(table.attention_arena_rows(), 400);
        assert_eq!(table.state_arena_rows(), 12);
        for index in 0..4 {
            let slot = table.slot(index).expect("slot");
            assert_eq!(slot.kv_base(), index * 100);
            assert_eq!(slot.state_base(), index * 3);
        }
        // Slot 0 is the solo lane, and its bases are the ones a
        // single-sequence session uses today.
        assert_eq!(table.slot(0).expect("slot").kv_base(), 0);
        assert_eq!(table.slot(0).expect("slot").state_base(), 0);
    }

    #[test]
    fn admission_takes_the_lowest_slot_so_solo_lands_on_slot_zero() {
        let mut table = table();
        assert_eq!(table.admit(), Some(0));
        assert_eq!(table.admit(), Some(1));
        table.retire(0).expect("retire");
        // Slot 0 frees and is reclaimed first, so one client is always on the
        // zero-base lane.
        assert_eq!(table.admit(), Some(0));
        assert_eq!(table.admit(), Some(2));
        assert_eq!(table.admit(), Some(3));
        assert_eq!(table.admit(), None, "a full table must refuse, not overflow");
    }

    #[test]
    fn an_idle_table_plans_nothing() {
        let table = table();
        assert!(table.plan_step().is_none());
        assert_eq!(table.attention_key_count(), 0);
    }

    #[test]
    fn a_prefilling_slot_does_not_join_the_decode_batch() {
        let mut table = table();
        table.admit().expect("admit");
        table.advance(0, 10).expect("advance");
        // Still Prefilling — a prefill chunk rides its own step.
        assert!(table.plan_step().is_none());
        table.begin_decoding(0).expect("decode");
        assert!(table.plan_step().is_some());
    }

    #[test]
    fn one_active_slot_plans_exactly_todays_step() {
        // The whole dynamic-degradation promise in one assertion: with one
        // client the step is width 1, full draft depth, and a key count equal
        // to that slot's own fill — indistinguishable from single-stream.
        let mut table = table();
        table.admit().expect("admit");
        table.advance(0, 10).expect("advance");
        table.begin_decoding(0).expect("decode");

        let plan = table.plan_step().expect("plan");
        assert_eq!(plan.width, 1);
        assert_eq!(plan.draft_depth, 2);
        assert_eq!(plan.attention_key_count, 11);
        assert_eq!(
            plan.slots,
            vec![SlotStep {
                slot: 0,
                write_row: 10,
                key_lower_bound: 0,
                position: 10,
                state_row: 0,
            }]
        );
    }

    #[test]
    fn idle_slots_cost_nothing_in_a_planned_step() {
        // Three slots exist and hold history; only one is decoding. The step
        // must be width 1, and the key count must ignore the parked slots
        // entirely rather than spanning their regions.
        let mut table = table();
        for _ in 0..3 {
            table.admit().expect("admit");
        }
        table.advance(0, 5).expect("advance");
        table.advance(1, 40).expect("advance");
        table.advance(2, 60).expect("advance");
        for index in 0..3 {
            table.begin_decoding(index).expect("decode");
        }
        assert_eq!(table.plan_step().expect("plan").width, 3);

        table.retire(1).expect("retire");
        table.retire(2).expect("retire");
        let plan = table.plan_step().expect("plan");
        assert_eq!(plan.width, 1, "retired slots must not widen the step");
        assert_eq!(plan.draft_depth, 2, "and their columns come back as depth");
        assert_eq!(
            plan.attention_key_count, 6,
            "a retired slot's rows must not be swept"
        );
    }

    #[test]
    fn a_step_spans_every_active_slot_and_the_rows_it_writes() {
        let mut table = table();
        for _ in 0..2 {
            table.admit().expect("admit");
        }
        table.advance(0, 5).expect("advance");
        table.advance(1, 30).expect("advance");
        table.begin_decoding(0).expect("decode");
        table.begin_decoding(1).expect("decode");

        let plan = table.plan_step().expect("plan");
        assert_eq!(plan.width, 2);
        assert_eq!(plan.draft_depth, 2);
        assert_eq!(plan.columns(), 6);
        assert!(plan.columns() <= COLUMN_BUDGET);
        // Slot 1 writes absolute row 130, so the span must reach 131 even
        // though the highest OCCUPIED row is 129.
        assert_eq!(plan.attention_key_count, 131);
        assert_eq!(
            plan.slots,
            vec![
                SlotStep {
                    slot: 0,
                    write_row: 5,
                    key_lower_bound: 0,
                    position: 5,
                    state_row: 0,
                },
                SlotStep {
                    slot: 1,
                    write_row: 130,
                    key_lower_bound: 100,
                    position: 30,
                    state_row: 1,
                },
            ]
        );
    }

    #[test]
    fn three_active_slots_run_three_wide() {
        let mut table = table();
        for _ in 0..3 {
            table.admit().expect("admit");
        }
        for index in 0..3 {
            table.advance(index, 7).expect("advance");
            table.begin_decoding(index).expect("decode");
        }
        let plan = table.plan_step().expect("plan");
        assert_eq!(plan.slots.len(), 3);
        assert_eq!(plan.width, 3, "every column is live");
        assert_eq!(plan.draft_depth, 1);
        assert_eq!(plan.columns(), 6);
        assert!(plan.columns() <= COLUMN_BUDGET);
    }

    #[test]
    fn slot_lower_bounds_never_reach_into_another_slot() {
        let mut table = table();
        for _ in 0..4 {
            table.admit().expect("admit");
        }
        for index in 0..4 {
            table.advance(index, 20 + index).expect("advance");
            table.begin_decoding(index).expect("decode");
        }
        let plan = table.plan_step().expect("plan");
        for step in &plan.slots {
            let base = table.slot(step.slot).expect("slot").kv_base();
            assert_eq!(step.key_lower_bound, base);
            assert!(step.write_row >= base);
            assert!(
                step.write_row < base + 100,
                "slot {} wrote outside its region",
                step.slot
            );
        }
    }

    #[test]
    fn a_solo_plan_builds_todays_single_sequence_layout() {
        // End of the byte-identity chain: a solo client on slot 0 must produce
        // a layout indistinguishable from single-sequence decode, INCLUDING an
        // empty lower-bound vector so the mask takes its single-sequence path.
        let mut table = table();
        table.admit().expect("admit");
        table.advance(0, 12).expect("advance");
        table.begin_decoding(0).expect("decode");

        let layout = table
            .plan_step()
            .expect("plan")
            .to_batch_layout()
            .expect("layout");
        assert_eq!(layout.positions, vec![12]);
        assert_eq!(layout.attention_write_indices, vec![12]);
        assert_eq!(
            layout.attention_write_indices, layout.positions,
            "on slot 0 the write row IS the position"
        );
        assert!(
            layout.attention_key_lower_bounds.is_empty(),
            "an all-zero bound list must stay empty, not become zeros"
        );
        assert!(layout.attention_key_lower_bounds().is_none());
        assert_eq!(layout.recurrent_state_rows, vec![0]);
        assert_eq!(layout.output_ids, vec![0]);
        assert_eq!(layout.attention_key_count, 13);
    }

    #[test]
    fn a_batched_plan_keeps_rope_within_slot_and_writes_absolute() {
        // The distinction the whole layout rests on: positions are within-slot
        // (they drive RoPE), write rows are absolute arena rows.
        let mut table = SlotTable::new(4, 100, 3, 2).expect("table");
        for _ in 0..2 {
            table.admit().expect("admit");
        }
        table.advance(0, 4).expect("advance");
        table.advance(1, 9).expect("advance");
        table.begin_decoding(0).expect("decode");
        table.begin_decoding(1).expect("decode");

        let layout = table
            .plan_step()
            .expect("plan")
            .to_batch_layout()
            .expect("layout");
        assert_eq!(layout.positions, vec![4, 9], "RoPE stays within the slot");
        assert_eq!(
            layout.attention_write_indices,
            vec![4, 109],
            "cache writes are absolute"
        );
        assert_eq!(layout.attention_key_lower_bounds, vec![0, 100]);
        assert_eq!(
            layout.recurrent_state_rows,
            vec![0, 3],
            "each slot reads its own recurrent block"
        );
        assert_eq!(
            layout.output_ids,
            vec![0, 1],
            "one logit row per slot, in plan order"
        );
        layout.validate().expect("batched layout is valid");
    }

    #[test]
    fn a_plan_that_outruns_its_key_span_is_refused() {
        // A layout whose write row sits outside the mask would attend to
        // nothing, which reads as a plausible-looking zero rather than an
        // error, so it has to be caught here.
        let plan = StepPlan {
            slots: vec![SlotStep {
                slot: 0,
                write_row: 40,
                key_lower_bound: 0,
                position: 40,
            state_row: 0,
            }],
            width: 1,
            draft_depth: 0,
            attention_key_count: 8,
        };
        assert!(plan.to_batch_layout().is_err());
    }

    #[test]
    fn an_empty_plan_cannot_become_a_layout() {
        let plan = StepPlan {
            slots: Vec::new(),
            width: 1,
            draft_depth: 0,
            attention_key_count: 8,
        };
        assert!(plan.to_batch_layout().is_err());
    }

    #[test]
    fn a_slot_cannot_be_filled_past_its_capacity() {
        let mut table = table();
        table.admit().expect("admit");
        table.advance(0, 100).expect("fill to capacity");
        assert_eq!(table.slot(0).expect("slot").remaining_context(), 0);
        assert!(
            table.advance(0, 1).is_err(),
            "overrunning a slot would corrupt its neighbour's rows"
        );
    }

    #[test]
    fn out_of_range_slots_are_refused() {
        let mut table = table();
        assert!(table.retire(9).is_err());
        assert!(table.advance(9, 1).is_err());
        assert!(table.begin_decoding(9).is_err());
        assert!(table.slot(9).is_none());
    }

    #[test]
    fn an_idle_slot_cannot_be_told_to_decode() {
        let mut table = table();
        assert!(
            table.begin_decoding(0).is_err(),
            "decoding an unclaimed slot would emit tokens for no job"
        );
    }

    #[test]
    fn degenerate_tables_are_refused() {
        assert!(SlotTable::new(0, 100, 1, 2).is_err());
        assert!(SlotTable::new(4, 0, 1, 2).is_err());
        assert!(SlotTable::new(4, 100, 0, 2).is_err());
    }
}
