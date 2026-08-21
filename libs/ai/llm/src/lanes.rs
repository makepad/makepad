//! Scheduling policy for a multi-lane worker.
//!
//! One session serves N conversations. This module owns the *decisions* — who
//! is admitted, what the next step should be, when a lane retires — and knows
//! nothing about how a step is executed. The worker loop performs whatever
//! [`LaneScheduler::next_step`] returns and feeds the outcome back.
//!
//! That split is deliberate and load-bearing: multi-sequence decode only runs on
//! CUDA, so the executing half cannot be tested on a dev Mac at all. Keeping the
//! policy pure means the part most likely to be wrong — admission, chunk
//! boundaries, retirement, the advert counters — is unit-testable everywhere,
//! and the GPU window is spent on numerics rather than on bookkeeping bugs.
//!
//! It lives beside [`crate::slots`] rather than in a service crate because it
//! is slot scheduling, not service logic — and because keeping it here lets the
//! `llama-slot-probe` gates drive the ACTUAL shipping scheduler on real
//! hardware instead of replicating its call sequence.
//!
//! Design of record: `local/agent_state/qwen-parallel/batched-session-design.md`.

use std::collections::VecDeque;

use crate::slots::{SlotTable, StepPlan};

/// Tokens a lane generates before the scheduler re-examines the world.
///
/// Lanes join and leave at these boundaries, which is why the number matters:
/// it bounds how long a newly arrived conversation waits for a free lane to be
/// noticed. It matches the existing worker's streaming chunk so the cadence a
/// client sees is unchanged.
pub const CHUNK_TOKENS: usize = 24;

/// A request waiting for, or occupying, a lane.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneRequest {
    /// Caller's handle for this work, echoed back on every event.
    pub job: u64,
    /// Conversation identity. **Never a prompt hash**: prompt hashes collide on
    /// the game profile, where every player shares a system prompt, and under
    /// sticky affinity a collision would hand two players the same lane and
    /// therefore each other's KV.
    pub session: String,
    /// Tokens to ingest. With `reset_first` false this is the DELTA the
    /// session's existing state does not already hold, which is how prefix
    /// reuse survives: the worker owns that decision, the executor obeys it.
    pub prompt_tokens: Vec<i32>,
    /// Clear the session's single-sequence state before ingesting. False only
    /// for a solo-lane prefix hit.
    pub reset_first: bool,
    pub max_new: usize,
}

/// A lane that has been admitted.
#[derive(Clone, Debug)]
struct Lane {
    request: LaneRequest,
    /// Tokens emitted so far.
    produced: usize,
    /// Next token this lane will decode, from its prefill or its last step.
    next_token: Option<i32>,
    phase: LanePhase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LanePhase {
    /// Admitted, prompt not yet ingested.
    NeedsPrefill,
    Decoding,
    /// Finished or cancelled; retires at the next boundary.
    Done(LaneOutcome),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneOutcome {
    Complete,
    Cancelled,
}

/// What the worker should do next.
#[derive(Clone, Debug, PartialEq)]
pub enum LaneStep {
    /// Ingest a lane's prompt. One sequence, so it runs at `n_seqs == 1` and
    /// uses the same graphs as single-stream prefill.
    Prefill {
        lane: usize,
        kv_base: usize,
        state_row: usize,
        start: usize,
        tokens: Vec<i32>,
    },
    /// Decode one token for every lane in `plan`, in plan order.
    Decode { plan: StepPlan, tokens: Vec<i32> },
    /// Nothing to do. The worker should block for new work rather than spin.
    Idle,
}

/// Something the worker should report to a caller.
#[derive(Clone, Debug, PartialEq)]
pub enum LaneEvent {
    Token { job: u64, token: i32, produced: usize },
    /// A lane retired. Carries the lane index so the worker can release
    /// per-lane resources — notably the sampler stream, which must NOT be
    /// inherited by whoever takes the slot next.
    Finished {
        job: u64,
        lane: usize,
        outcome: LaneOutcome,
    },
}

/// Admission, stepping and retirement across N lanes.
pub struct LaneScheduler {
    table: SlotTable,
    lanes: Vec<Option<Lane>>,
    pending: VecDeque<LaneRequest>,
    queue_max: usize,
}

impl LaneScheduler {
    pub fn new(table: SlotTable, queue_max: usize) -> Self {
        let lanes = vec![None; table.len()];
        Self {
            table,
            lanes,
            pending: VecDeque::new(),
            queue_max,
        }
    }

    pub fn slots_total(&self) -> usize {
        self.lanes.len()
    }

    /// Lanes holding a conversation, generating or not.
    pub fn slots_claimed(&self) -> usize {
        self.lanes.iter().filter(|lane| lane.is_some()).count()
    }

    pub fn slots_free(&self) -> usize {
        self.slots_total() - self.slots_claimed()
    }

    /// Lanes that would contribute a column to the next decode step. This is
    /// the contention signal, and it excludes lanes that are merely parked.
    pub fn lanes_active(&self) -> usize {
        self.lanes
            .iter()
            .filter(|lane| {
                matches!(
                    lane.as_ref().map(|l| l.phase),
                    Some(LanePhase::Decoding) | Some(LanePhase::NeedsPrefill)
                )
            })
            .count()
    }

    pub fn queue_depth(&self) -> usize {
        self.pending.len()
    }

    pub fn is_idle(&self) -> bool {
        self.slots_claimed() == 0 && self.pending.is_empty()
    }

    /// Offer work. `Err` when the queue is full — the caller must refuse the
    /// request honestly rather than let it wait unbounded.
    pub fn submit(&mut self, request: LaneRequest) -> Result<(), LaneRequest> {
        if self.pending.len() >= self.queue_max {
            return Err(request);
        }
        self.pending.push_back(request);
        Ok(())
    }

    /// Move queued work into free lanes. Called at every step boundary, which
    /// is what makes joining cheap and bounded by [`CHUNK_TOKENS`].
    pub fn admit_pending(&mut self) {
        while !self.pending.is_empty() {
            let Some(index) = self.table.admit() else {
                break;
            };
            let request = self.pending.pop_front().expect("non-empty");
            self.lanes[index] = Some(Lane {
                request,
                produced: 0,
                next_token: None,
                phase: LanePhase::NeedsPrefill,
            });
        }
    }

    /// Mark a lane's work cancelled. It retires at the next boundary rather
    /// than mid-step, so a cancel can never tear down state a step is using.
    pub fn cancel(&mut self, job: u64) {
        for lane in self.lanes.iter_mut().flatten() {
            if lane.request.job == job && !matches!(lane.phase, LanePhase::Done(_)) {
                lane.phase = LanePhase::Done(LaneOutcome::Cancelled);
            }
        }
        self.pending.retain(|request| request.job != job);
    }

    /// Decide the next thing to do.
    ///
    /// Prefill takes priority over decode, and one lane at a time: a prompt
    /// chunk is a wide batch of its own, and mixing it into a decode step would
    /// need a ragged batch the recurrent scan cannot express.
    pub fn next_step(&mut self) -> LaneStep {
        self.admit_pending();
        for (index, lane) in self.lanes.iter().enumerate() {
            let Some(lane) = lane else { continue };
            if lane.phase == LanePhase::NeedsPrefill {
                let slot = self.table.slot(index).expect("admitted lane has a slot");
                return LaneStep::Prefill {
                    lane: index,
                    kv_base: slot.kv_base(),
                    state_row: slot.live_state_row(),
                    start: slot.fill(),
                    tokens: lane.request.prompt_tokens.clone(),
                };
            }
        }
        match self.table.plan_step() {
            Some(plan) => {
                let tokens = plan
                    .slots
                    .iter()
                    .map(|step| {
                        self.lanes[step.slot]
                            .as_ref()
                            .and_then(|lane| lane.next_token)
                            .unwrap_or(0)
                    })
                    .collect();
                LaneStep::Decode { plan, tokens }
            }
            None => LaneStep::Idle,
        }
    }

    /// Report a completed prefill: the lane ingested `count` tokens and its
    /// first generated token is `first_token`.
    pub fn on_prefilled(&mut self, lane: usize, count: usize, first_token: i32) {
        let _ = self.table.advance(lane, count);
        let _ = self.table.begin_decoding(lane);
        if let Some(slot) = self.lanes.get_mut(lane).and_then(|l| l.as_mut()) {
            slot.next_token = Some(first_token);
            slot.phase = LanePhase::Decoding;
        }
    }

    /// Report a completed decode step. `sampled[i]` is the token sampled for
    /// `plan.slots[i]`. Returns what the worker should tell callers.
    ///
    /// The token a lane just DECODED is the one it emits; `sampled` is what it
    /// will decode next. Conflating the two drops the first token of every
    /// generation and appends one that was never produced.
    pub fn on_decoded(&mut self, plan: &StepPlan, sampled: &[i32]) -> Vec<LaneEvent> {
        let mut events = Vec::new();
        for (step, &next) in plan.slots.iter().zip(sampled) {
            let index = step.slot;
            let Some(lane) = self.lanes.get_mut(index).and_then(|l| l.as_mut()) else {
                continue;
            };
            let emitted = match lane.next_token {
                Some(token) => token,
                None => continue,
            };
            lane.produced += 1;
            lane.next_token = Some(next);
            let _ = self.table.advance(index, 1);
            events.push(LaneEvent::Token {
                job: lane.request.job,
                token: emitted,
                produced: lane.produced,
            });
            if lane.produced >= lane.request.max_new {
                lane.phase = LanePhase::Done(LaneOutcome::Complete);
            }
        }
        events
    }

    /// Report a whole CHUNK produced by the solo speculative path.
    ///
    /// The batched path decodes one token per lane per step and emits the
    /// token it just consumed; the speculative path runs the session's own
    /// loop and hands back the tokens it GENERATED. So this emits them
    /// directly rather than going through the feed-one/emit-previous dance,
    /// and advances the lane by the whole chunk.
    pub fn on_generated(&mut self, lane: usize, tokens: &[i32]) -> Vec<LaneEvent> {
        let mut events = Vec::new();
        let Some(slot) = self.lanes.get_mut(lane).and_then(|l| l.as_mut()) else {
            return events;
        };
        for &token in tokens {
            slot.produced += 1;
            events.push(LaneEvent::Token {
                job: slot.request.job,
                token,
                produced: slot.produced,
            });
        }
        if slot.produced >= slot.request.max_new {
            slot.phase = LanePhase::Done(LaneOutcome::Complete);
        }
        let _ = self.table.advance(lane, tokens.len());
        events
    }

    /// Seed the token a lane will decode next.
    ///
    /// Needed when a lane hands over from the speculative path to the batched
    /// one: the speculative path leaves its next token implicit in the
    /// session's logits, while the batched path needs it explicit.
    pub fn set_next_token(&mut self, lane: usize, token: i32) {
        if let Some(slot) = self.lanes.get_mut(lane).and_then(|l| l.as_mut()) {
            slot.next_token = Some(token);
        }
    }

    /// Whether `lane` is the only claimed lane and nothing is queued behind it.
    ///
    /// The condition for taking the solo speculative path: no other lane can
    /// join before the chunk completes, so the session's own single-sequence
    /// state stays authoritative for its duration.
    pub fn is_solo(&self, lane: usize) -> bool {
        self.pending.is_empty()
            && self.slots_claimed() == 1
            && self
                .lanes
                .get(lane)
                .map(|l| l.is_some())
                .unwrap_or(false)
    }

    /// Whether `lane`'s request asked for the session to be cleared before its
    /// prompt is ingested. False only for a solo-lane prefix hit, where the
    /// prompt tokens are the delta on top of state already resident.
    pub fn reset_requested(&self, lane: usize) -> bool {
        self.lanes
            .get(lane)
            .and_then(|l| l.as_ref())
            .map(|slot| slot.request.reset_first)
            .unwrap_or(true)
    }

    /// Tokens `lane` still owes its caller.
    pub fn remaining(&self, lane: usize) -> usize {
        self.lanes
            .get(lane)
            .and_then(|l| l.as_ref())
            .map(|slot| slot.request.max_new.saturating_sub(slot.produced))
            .unwrap_or(0)
    }

    /// Retire finished and cancelled lanes, freeing their slots.
    pub fn reap(&mut self) -> Vec<LaneEvent> {
        let mut events = Vec::new();
        for index in 0..self.lanes.len() {
            let outcome = match self.lanes[index].as_ref().map(|lane| lane.phase) {
                Some(LanePhase::Done(outcome)) => outcome,
                _ => continue,
            };
            let job = self.lanes[index]
                .as_ref()
                .map(|lane| lane.request.job)
                .unwrap_or_default();
            self.lanes[index] = None;
            let _ = self.table.retire(index);
            events.push(LaneEvent::Finished {
                job,
                lane: index,
                outcome,
            });
        }
        events
    }

    /// Everything an observer needs to describe this worker's capacity.
    pub fn counts(&self) -> LaneCounts {
        LaneCounts {
            slots_total: self.slots_total(),
            slots_claimed: self.slots_claimed(),
            slots_free: self.slots_free(),
            lanes_active: self.lanes_active(),
            queue_depth: self.queue_depth(),
        }
    }
}

/// A snapshot of lane occupancy, for whatever wants to advertise it.
///
/// The scheduler reports; it does not know what `/health` is. That keeps this
/// module free of any service dependency, which is what lets it live next to
/// the slot table and be driven by the on-hardware gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneCounts {
    pub slots_total: usize,
    /// Lanes holding a conversation's KV, generating or not.
    pub slots_claimed: usize,
    pub slots_free: usize,
    /// Lanes generating right now — the contention signal.
    pub lanes_active: usize,
    pub queue_depth: usize,
}

/// Drives a real batched session from the scheduler's decisions.
///
/// Deliberately thin: every decision lives in [`LaneScheduler`], which is
/// tested everywhere, and this only performs them. Multi-sequence decode is
/// CUDA-only, so this half cannot be exercised on a dev Mac — keeping it free
/// of policy is what stops that gap from mattering.
pub struct LaneExecutor {
    session: crate::LlamaSession,
    scheduler: LaneScheduler,
    /// One sampler stream per lane. Per-lane rather than per-session because
    /// interleaved lanes would otherwise draw from one stream in an order that
    /// depends on who else was talking — the same bug the per-chunk re-seed
    /// had, one level up.
    samplers: Vec<Option<crate::LlamaSamplerState>>,
    params: crate::LlamaSamplingParams,
    /// Slot 0's history currently lives in the session's OWN single-sequence
    /// state, so the speculative path can run against it.
    ///
    /// The two paths disagree about where a lane's history is: the speculative
    /// one advances the session's token list, the batched one writes through
    /// slot rows and does not. So this is set only while slot 0 is the sole
    /// occupant, and cleared the moment the batch widens — after which that
    /// turn finishes unspeculated and the next one re-establishes it.
    solo_native: bool,
    /// Invoked with fresh counts after every step.
    ///
    /// A callback rather than "the caller remembers to ask": a forgotten
    /// publish leaves `/health` advertising stale occupancy, which is a silent
    /// failure — the box keeps claiming free lanes it no longer has, and a
    /// scheduler believes it.
    on_counts: Option<Box<dyn FnMut(LaneCounts) + Send>>,
}

/// The one lane whose history can live in the session's own single-sequence
/// state, and therefore the only lane that can speculate in phase 1. Slot 0
/// because its `kv_base` is 0, which is exactly where the session-native path
/// writes.
pub const SOLO_LANE: usize = 0;

impl LaneExecutor {
    fn sampler_for(&mut self, lane: usize) -> &mut crate::LlamaSamplerState {
        let params = self.params;
        self.samplers[lane]
            .get_or_insert_with(|| crate::LlamaSamplerState::new(params.seed ^ (lane as u64 + 1)))
    }

    pub fn new(
        session: crate::LlamaSession,
        scheduler: LaneScheduler,
        params: crate::LlamaSamplingParams,
    ) -> Self {
        let samplers = vec![None; scheduler.slots_total()];
        Self {
            session,
            scheduler,
            samplers,
            params,
            solo_native: false,
            on_counts: None,
        }
    }

    /// Report lane occupancy after every step. The service wires `/health` here.
    pub fn on_counts(mut self, sink: impl FnMut(LaneCounts) + Send + 'static) -> Self {
        self.on_counts = Some(Box::new(sink));
        self
    }

    pub fn scheduler(&mut self) -> &mut LaneScheduler {
        &mut self.scheduler
    }

    /// True when there is nothing to do and the worker should block for work
    /// rather than spin.
    pub fn is_idle(&self) -> bool {
        self.scheduler.is_idle()
    }

    /// Whether the next step will run the solo speculative path.
    pub fn is_speculating(&self) -> bool {
        self.solo_native && self.scheduler.is_solo(SOLO_LANE)
    }

    /// Perform one scheduler step. Returns the events to report to callers.
    pub fn step(&mut self) -> Result<Vec<LaneEvent>, String> {
        let mut events = Vec::new();
        // HANDOVER, before anything reads the plan. If slot 0 was running
        // session-native and is no longer alone, its next token is still
        // implicit in the session's logits; the batched path needs it
        // explicit. Miss this and the lane feeds a zero token — a plausible
        // word, not an error.
        if self.solo_native && !self.scheduler.is_solo(SOLO_LANE) {
            let next = {
                let logits = self
                    .session
                    .last_logits()
                    .ok_or_else(|| "handover with no logits to sample".to_string())?
                    .to_vec();
                let params = self.params;
                let sampler = self.sampler_for(SOLO_LANE);
                sampler
                    .sample_logits(&logits, params)
                    .map_err(|e| format!("handover sample: {e}"))?
            };
            self.scheduler.set_next_token(SOLO_LANE, next);
            self.solo_native = false;
        }
        match self.scheduler.next_step() {
            LaneStep::Idle => {}
            LaneStep::Prefill {
                lane,
                kv_base,
                state_row,
                start,
                tokens,
            } if lane == SOLO_LANE && self.scheduler.is_solo(lane) => {
                // Session-native ingest, so the speculative loop can run
                // against it. `reset_first` is the worker's prefix decision.
                if self.scheduler.reset_requested(lane) {
                    self.session.reset().map_err(|e| format!("reset: {e}"))?;
                }
                self.session
                    .append_tokens(&tokens)
                    .map_err(|e| format!("solo prefill: {e}"))?;
                let logits = self
                    .session
                    .last_logits()
                    .ok_or_else(|| "solo prefill produced no logits".to_string())?
                    .to_vec();
                let first = {
                    let params = self.params;
                    let sampler = self.sampler_for(lane);
                    sampler
                        .sample_logits(&logits, params)
                        .map_err(|e| format!("lane {lane} sample: {e}"))?
                };
                self.solo_native = true;
                self.scheduler.on_prefilled(lane, tokens.len(), first);
            }
            LaneStep::Prefill {
                lane,
                kv_base,
                state_row,
                start,
                tokens,
            } => {
                let logits = self
                    .session
                    .prefill_slot_chunk(lane, kv_base, state_row, start, &tokens)
                    .map_err(|e| format!("lane {lane} prefill: {e}"))?;
                // A lane's stream is seeded once, when it is admitted, and
                // carried for the whole generation.
                let sampler = self.samplers[lane].get_or_insert_with(|| {
                    crate::LlamaSamplerState::new(self.params.seed ^ (lane as u64 + 1))
                });
                let first = sampler
                    .sample_logits(&logits, self.params)
                    .map_err(|e| format!("lane {lane} sample: {e}"))?;
                self.scheduler.on_prefilled(lane, tokens.len(), first);
            }
            LaneStep::Decode { plan, .. }
                if self.solo_native
                    && plan.slots.len() == 1
                    && plan.slots[0].slot == SOLO_LANE =>
            {
                // The session's own loop: speculation, chunked so the caller
                // keeps its cancel and streaming boundaries.
                let want = CHUNK_TOKENS.min(self.scheduler.remaining(SOLO_LANE)).max(1);
                let generated = {
                    let params = self.params;
                    let sampler = self.samplers[SOLO_LANE]
                        .get_or_insert_with(|| crate::LlamaSamplerState::new(params.seed));
                    self.session
                        .continue_sampled_with(want, params, sampler)
                        .map_err(|e| format!("solo decode: {e}"))?
                };
                events.extend(
                    self.scheduler
                        .on_generated(SOLO_LANE, &generated.token_ids),
                );
            }
            LaneStep::Decode { plan, tokens } => {
                let rows = self
                    .session
                    .step_slots(&plan, &tokens)
                    .map_err(|e| format!("batched decode: {e}"))?;
                let mut sampled = Vec::with_capacity(rows.len());
                for (row, step) in rows.iter().zip(&plan.slots) {
                    let lane = step.slot;
                    let sampler = self.samplers[lane].get_or_insert_with(|| {
                        crate::LlamaSamplerState::new(self.params.seed ^ (lane as u64 + 1))
                    });
                    sampled.push(
                        sampler
                            .sample_logits(row, self.params)
                            .map_err(|e| format!("lane {lane} sample: {e}"))?,
                    );
                }
                events.extend(self.scheduler.on_decoded(&plan, &sampled));
            }
        }
        for event in self.scheduler.reap() {
            if let LaneEvent::Finished { lane, .. } = &event {
                // Drop the retired lane's stream so whoever takes the slot next
                // starts fresh instead of continuing a stranger's sequence.
                self.samplers[*lane] = None;
            }
            events.push(event);
        }
        if let Some(sink) = self.on_counts.as_mut() {
            sink(self.scheduler.counts());
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler(slots: usize, queue_max: usize) -> LaneScheduler {
        let table = SlotTable::new(slots, 512, 1, 0).expect("table");
        LaneScheduler::new(table, queue_max)
    }

    fn request(job: u64, prompt: &[i32], max_new: usize) -> LaneRequest {
        LaneRequest {
            job,
            session: format!("session-{job}"),
            prompt_tokens: prompt.to_vec(),
            reset_first: true,
            max_new,
        }
    }

    /// Drive one lane from prefill to completion, collecting its tokens.
    fn run_to_completion(sched: &mut LaneScheduler, steps: usize) -> Vec<LaneEvent> {
        let mut events = Vec::new();
        for _ in 0..steps {
            match sched.next_step() {
                LaneStep::Prefill { lane, tokens, .. } => {
                    sched.on_prefilled(lane, tokens.len(), 900 + lane as i32);
                }
                LaneStep::Decode { plan, .. } => {
                    let sampled: Vec<i32> = plan.slots.iter().map(|s| 500 + s.slot as i32).collect();
                    events.extend(sched.on_decoded(&plan, &sampled));
                }
                LaneStep::Idle => {}
            }
            events.extend(sched.reap());
        }
        events
    }

    #[test]
    fn an_empty_scheduler_asks_the_worker_to_block() {
        let mut sched = scheduler(4, 8);
        assert_eq!(sched.next_step(), LaneStep::Idle);
        assert!(sched.is_idle(), "a worker that spins on Idle burns a core");
    }

    #[test]
    fn a_submitted_job_prefills_before_it_decodes() {
        let mut sched = scheduler(4, 8);
        sched.submit(request(1, &[10, 11, 12], 4)).expect("submit");
        match sched.next_step() {
            LaneStep::Prefill { lane, start, tokens, kv_base, .. } => {
                assert_eq!(lane, 0, "first job takes the zero-base lane");
                assert_eq!(kv_base, 0);
                assert_eq!(start, 0);
                assert_eq!(tokens, vec![10, 11, 12]);
            }
            other => panic!("expected prefill, got {other:?}"),
        }
    }

    #[test]
    fn the_queue_refuses_rather_than_growing_without_bound() {
        let mut sched = scheduler(1, 2);
        sched.submit(request(1, &[1], 1)).expect("first");
        sched.submit(request(2, &[1], 1)).expect("second");
        let refused = sched
            .submit(request(3, &[1], 1))
            .expect_err("a full queue must refuse");
        assert_eq!(refused.job, 3, "the refusal must hand the work back");
        assert_eq!(sched.queue_depth(), 2);
    }

    #[test]
    fn a_lane_emits_the_token_it_decoded_not_the_one_it_will_decode_next() {
        // Off-by-one here drops the first token of every generation and
        // appends one that was never produced, which reads as the model
        // behaving oddly rather than as a scheduler bug.
        let mut sched = scheduler(1, 4);
        sched.submit(request(7, &[1, 2], 2)).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 42);

        let LaneStep::Decode { plan, tokens } = sched.next_step() else {
            panic!("expected decode");
        };
        assert_eq!(tokens, vec![42], "the lane decodes its prefill's token");
        let events = sched.on_decoded(&plan, &[43]);
        assert_eq!(
            events,
            vec![LaneEvent::Token { job: 7, token: 42, produced: 1 }],
            "it emits 42 — the token it just decoded — not 43"
        );
    }

    #[test]
    fn a_lane_finishes_at_its_token_budget_and_frees_its_slot() {
        let mut sched = scheduler(2, 4);
        sched.submit(request(1, &[1, 2], 3)).expect("submit");
        let events = run_to_completion(&mut sched, 8);
        let tokens: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, LaneEvent::Token { .. }))
            .collect();
        assert_eq!(tokens.len(), 3, "exactly max_new tokens, no more");
        assert!(events.contains(&LaneEvent::Finished {
            job: 1,
            lane: 0,
            outcome: LaneOutcome::Complete
        }));
        assert_eq!(sched.slots_claimed(), 0, "a finished lane frees its slot");
        assert_eq!(sched.slots_free(), 2);
    }

    #[test]
    fn several_lanes_decode_in_one_step() {
        let mut sched = scheduler(4, 8);
        for job in 1..=3 {
            sched
                .submit(request(job, &[1, 2, 3], 4))
                .expect("submit");
        }
        // Three prefills, then the decode batches all three at once.
        for _ in 0..3 {
            let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
                panic!("expected prefill");
            };
            sched.on_prefilled(lane, tokens.len(), 100 + lane as i32);
        }
        let LaneStep::Decode { plan, tokens } = sched.next_step() else {
            panic!("expected decode");
        };
        assert_eq!(plan.slots.len(), 3, "all three lanes in ONE step");
        assert_eq!(tokens, vec![100, 101, 102], "each lane's own token");
        assert_eq!(sched.lanes_active(), 3);
    }

    #[test]
    fn a_waiting_job_joins_as_soon_as_a_lane_frees() {
        let mut sched = scheduler(1, 4);
        sched.submit(request(1, &[1], 1)).expect("submit");
        sched.submit(request(2, &[1], 1)).expect("submit");
        // Submission only enqueues; admission happens at a step boundary,
        // which is what makes joining cheap and bounded.
        sched.admit_pending();
        assert_eq!(sched.slots_claimed(), 1, "one lane, one occupant");
        assert_eq!(sched.queue_depth(), 1, "second job waits for the only lane");

        let events = run_to_completion(&mut sched, 12);
        assert!(events.contains(&LaneEvent::Finished {
            job: 1,
            lane: 0,
            outcome: LaneOutcome::Complete
        }));
        assert!(
            events.contains(&LaneEvent::Finished {
                job: 2,
                lane: 0,
                outcome: LaneOutcome::Complete
            }),
            "the queued job must be admitted once lane 0 frees, not stranded"
        );
        assert_eq!(sched.queue_depth(), 0);
    }

    #[test]
    fn cancelling_retires_at_a_boundary_and_never_mid_step() {
        let mut sched = scheduler(2, 4);
        sched.submit(request(1, &[1, 2], 50)).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 5);

        sched.cancel(1);
        // The lane is still claimed until reap: tearing it down inside a step
        // would free state the step is reading.
        assert_eq!(sched.slots_claimed(), 1);
        let events = sched.reap();
        assert_eq!(
            events,
            vec![LaneEvent::Finished {
                job: 1,
                lane: 0,
                outcome: LaneOutcome::Cancelled
            }]
        );
        assert_eq!(sched.slots_claimed(), 0);
    }

    #[test]
    fn cancelling_queued_work_drops_it_without_ever_claiming_a_lane() {
        let mut sched = scheduler(1, 4);
        sched.submit(request(1, &[1], 9)).expect("submit");
        sched.submit(request(2, &[1], 9)).expect("submit");
        sched.admit_pending();
        assert_eq!(sched.queue_depth(), 1, "job 2 is queued behind the one lane");
        sched.cancel(2);
        assert_eq!(sched.queue_depth(), 0, "cancelled work leaves the queue");
        sched.admit_pending();
        assert_eq!(sched.slots_claimed(), 1, "only job 1 ever takes a lane");
    }

    #[test]
    fn parked_and_generating_lanes_are_counted_separately() {
        // The distinction /health exposes: a lane holding KV but not generating
        // costs no speed, and collapsing the two would make the advert useless
        // for choosing between boxes.
        let mut sched = scheduler(4, 8);
        sched.submit(request(1, &[1, 2], 1)).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 7);
        assert_eq!(sched.slots_claimed(), 1);
        assert_eq!(sched.lanes_active(), 1);

        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        sched.on_decoded(&plan, &[8]);
        // Budget of 1 reached, so it is Done but not yet reaped: still holding
        // its slot, no longer contributing a column.
        assert_eq!(sched.slots_claimed(), 1);
        assert_eq!(sched.lanes_active(), 0);
        sched.reap();
        assert_eq!(sched.slots_claimed(), 0);
    }

    #[test]
    fn retirement_names_the_lane_so_per_lane_state_can_be_released() {
        // The executor keys its per-lane sampler stream off this. Without the
        // index it cannot tell which stream to drop, and the next occupant of
        // the slot silently continues a stranger's RNG sequence — reproducible
        // only for whoever happened to be there before.
        let mut sched = scheduler(4, 8);
        sched.submit(request(1, &[1], 1)).expect("submit");
        sched.submit(request(2, &[1], 1)).expect("submit");
        sched.admit_pending();
        sched.cancel(1);
        sched.cancel(2);
        let mut finished: Vec<(u64, usize)> = sched
            .reap()
            .into_iter()
            .filter_map(|e| match e {
                LaneEvent::Finished { job, lane, .. } => Some((job, lane)),
                _ => None,
            })
            .collect();
        finished.sort();
        assert_eq!(
            finished,
            vec![(1, 0), (2, 1)],
            "each retirement must name the lane it freed"
        );
    }

    #[test]
    fn a_sole_occupant_is_solo_and_a_shared_box_is_not() {
        // The condition that decides whether a lane may take the speculative
        // path. It has to include the QUEUE: a lane that is alone right now
        // but has work waiting behind it would be joined mid-chunk.
        let mut sched = scheduler(4, 8);
        sched.submit(request(1, &[1, 2], 20)).expect("submit");
        sched.admit_pending();
        assert!(sched.is_solo(0), "sole occupant, nothing queued");

        sched.submit(request(2, &[1], 20)).expect("submit");
        assert!(
            !sched.is_solo(0),
            "work is queued behind it, so it is about to be joined"
        );
        sched.admit_pending();
        assert!(!sched.is_solo(0), "and now genuinely shares the box");
    }

    #[test]
    fn a_generated_chunk_emits_every_token_and_advances_the_lane() {
        // The speculative path hands back tokens it already produced, unlike
        // the batched path which emits the token it just consumed. Getting
        // this wrong drops or duplicates a whole chunk, not one token.
        let mut sched = scheduler(2, 4);
        sched.submit(request(5, &[1, 2], 10)).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 0);

        let events = sched.on_generated(0, &[41, 42, 43]);
        assert_eq!(
            events,
            vec![
                LaneEvent::Token { job: 5, token: 41, produced: 1 },
                LaneEvent::Token { job: 5, token: 42, produced: 2 },
                LaneEvent::Token { job: 5, token: 43, produced: 3 },
            ]
        );
        assert_eq!(sched.remaining(0), 7, "ten asked, three delivered");
    }

    #[test]
    fn a_generated_chunk_still_stops_at_the_budget() {
        let mut sched = scheduler(2, 4);
        sched.submit(request(6, &[1], 2)).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 0);
        // A speculative round can overshoot the budget; the lane must still
        // finish rather than run on.
        sched.on_generated(0, &[1, 2, 3]);
        assert_eq!(sched.remaining(0), 0);
        assert!(matches!(
            sched.reap().first(),
            Some(LaneEvent::Finished { job: 6, .. })
        ));
    }

    #[test]
    fn a_prefix_hit_asks_the_session_not_to_reset() {
        let mut sched = scheduler(2, 4);
        let mut hit = request(9, &[7, 8], 4);
        hit.reset_first = false;
        sched.submit(hit).expect("submit");
        sched.admit_pending();
        assert!(
            !sched.reset_requested(0),
            "a prefix hit must not clear the state it is reusing"
        );
        // An unclaimed lane defaults to resetting: safer to re-ingest than to
        // decode on top of whatever a previous conversation left behind.
        assert!(sched.reset_requested(1));
    }

    #[test]
    fn lanes_never_exceed_the_table() {
        let mut sched = scheduler(2, 16);
        for job in 1..=6 {
            sched.submit(request(job, &[1], 5)).expect("submit");
        }
        sched.admit_pending();
        assert_eq!(sched.slots_claimed(), 2, "only two lanes exist");
        assert_eq!(sched.queue_depth(), 4, "the rest wait honestly");
    }
}
