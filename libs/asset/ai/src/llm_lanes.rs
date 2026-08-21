//! Scheduling policy for a multi-lane LLM worker.
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
//! Design of record: `local/agent_state/qwen-parallel/batched-session-design.md`.

use std::collections::VecDeque;

use makepad_ai_llm::{SlotTable, StepPlan};

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
    pub prompt_tokens: Vec<i32>,
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

    /// Publish the current counters to `/health`.
    pub fn publish_advert(&self) {
        crate::lane_advert::set_live(self.slots_claimed() as u64, self.lanes_active() as u64);
    }
}

/// Drives a real batched session from the scheduler's decisions.
///
/// Deliberately thin: every decision lives in [`LaneScheduler`], which is
/// tested everywhere, and this only performs them. Multi-sequence decode is
/// CUDA-only, so this half cannot be exercised on a dev Mac — keeping it free
/// of policy is what stops that gap from mattering.
pub struct LaneExecutor {
    session: makepad_ai_llm::LlamaSession,
    scheduler: LaneScheduler,
    /// One sampler stream per lane. Per-lane rather than per-session because
    /// interleaved lanes would otherwise draw from one stream in an order that
    /// depends on who else was talking — the same bug the per-chunk re-seed
    /// had, one level up.
    samplers: Vec<Option<makepad_ai_llm::LlamaSamplerState>>,
    params: makepad_ai_llm::LlamaSamplingParams,
}

impl LaneExecutor {
    pub fn new(
        session: makepad_ai_llm::LlamaSession,
        scheduler: LaneScheduler,
        params: makepad_ai_llm::LlamaSamplingParams,
    ) -> Self {
        let samplers = vec![None; scheduler.slots_total()];
        Self {
            session,
            scheduler,
            samplers,
            params,
        }
    }

    pub fn scheduler(&mut self) -> &mut LaneScheduler {
        &mut self.scheduler
    }

    /// True when there is nothing to do and the worker should block for work
    /// rather than spin.
    pub fn is_idle(&self) -> bool {
        self.scheduler.is_idle()
    }

    /// Perform one scheduler step. Returns the events to report to callers.
    pub fn step(&mut self) -> Result<Vec<LaneEvent>, String> {
        let mut events = Vec::new();
        match self.scheduler.next_step() {
            LaneStep::Idle => {}
            LaneStep::Prefill {
                lane,
                kv_base,
                state_row,
                start,
                tokens,
            } => {
                let logits = self
                    .session
                    .prefill_slot_chunk(kv_base, state_row, start, &tokens)
                    .map_err(|e| format!("lane {lane} prefill: {e}"))?;
                // A lane's stream is seeded once, when it is admitted, and
                // carried for the whole generation.
                let sampler = self.samplers[lane].get_or_insert_with(|| {
                    makepad_ai_llm::LlamaSamplerState::new(self.params.seed ^ (lane as u64 + 1))
                });
                let first = sampler
                    .sample_logits(&logits, self.params)
                    .map_err(|e| format!("lane {lane} sample: {e}"))?;
                self.scheduler.on_prefilled(lane, tokens.len(), first);
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
                        makepad_ai_llm::LlamaSamplerState::new(self.params.seed ^ (lane as u64 + 1))
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
        self.scheduler.publish_advert();
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
