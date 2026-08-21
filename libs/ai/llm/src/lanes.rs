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

/// Prompt tokens a lane ingests per prefill step, unless the caller says
/// otherwise. Matches the single-sequence prefill batch, which has always
/// chunked for the same reason.
///
/// 512 rather than a smaller number because a prefill chunk's cost is very
/// far from proportional to the tokens in it: the attention kernel walks the
/// lane's whole key span once per chunk, and the FFN of a 27B model at 64
/// columns is latency-bound rather than compute-bound. Measured on a 5090,
/// 4096 tokens into one lane (.217, `llama-slot-probe --prefill-rate`):
///
/// | chunk | lane 0 | lane 1 (base 65536) |
/// |---|---|---|
/// | 64 | 786 tok/s | 603 tok/s |
/// | 256 | 3130 | 1438 |
/// | 512 | 3575 | 1561 |
/// | 1024 | 3800 | 1727 |
///
/// So 64 was costing a factor of four and a half. 1024 buys a further 6 % for
/// twice the graph activations, which is the wrong side of a trade whose
/// failure mode is an out-of-memory at prefill on a box sized for the smaller
/// one.
pub const DEFAULT_PREFILL_CHUNK: usize = 512;

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
    /// Sampling settings for THIS request.
    ///
    /// Per-request, not per-executor: temperature and seed belong to the
    /// caller, and two chats sharing one setting would silently sample with
    /// each other's — the same shared-state bug class as the recurrent row and
    /// the carry ring, one level up.
    pub sampling: crate::LlamaSamplingParams,
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
    /// Prompt tokens already ingested. A prompt is prefilled in CHUNKS —
    /// ingesting eight thousand tokens in one graph builds activations
    /// proportional to `n_tokens x key_span`, and on a lane whose base is high
    /// in the arena that is gigabytes.
    ingested: usize,
    /// This lane's own history: the prompt it ingested plus everything it has
    /// generated, in order.
    ///
    /// Kept because the draft head has to be caught up over the tokens the
    /// model consumed but it has not, and those tokens are the LANE'S — there
    /// is no session-wide token list once more than one conversation is
    /// resident. Bounded by the per-slot context, so ~64 KB at 16k.
    tokens: Vec<i32>,
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
        /// True when this lane kept its caches and `tokens` is only the delta.
        /// Reported outward so a client can see its conversation stayed warm
        /// rather than infer it from a latency it cannot attribute.
        resumed: bool,
        /// True when this chunk finishes the prompt.
        ///
        /// Only then is there a token to sample: a middle chunk's logits
        /// predict the next PROMPT token, which is already known. Sampling
        /// anyway costs a full-vocabulary pass per chunk AND draws from the
        /// lane's RNG — which would make a conversation's output depend on how
        /// its prompt happened to be split.
        last: bool,
    },
    /// Decode one token for every lane in `plan`, in plan order.
    Decode { plan: StepPlan, tokens: Vec<i32> },
    /// Nothing to do. The worker should block for new work rather than spin.
    Idle,
}

/// Something the worker should report to a caller.
#[derive(Clone, Debug, PartialEq)]
pub enum LaneEvent {
    /// A lane ingested its prompt. `ingested` is what it actually put in,
    /// which is the DELTA when `resumed` — the two together are how a client
    /// learns its conversation stayed warm instead of guessing from latency.
    Prefilled {
        job: u64,
        ingested: usize,
        resumed: bool,
    },
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

/// What a parked slot still holds: whose conversation it was, and the exact
/// tokens its caches describe.
///
/// Survives retirement on purpose. Nothing on the device is erased when a lane
/// retires — only counters are reset — so a conversation coming back can decode
/// straight on from where it left off instead of re-ingesting itself.
#[derive(Clone, Debug)]
struct Parked {
    session: String,
    tokens: Vec<i32>,
}

/// Admission, stepping and retirement across N lanes.
pub struct LaneScheduler {
    table: SlotTable,
    lanes: Vec<Option<Lane>>,
    pending: VecDeque<LaneRequest>,
    queue_max: usize,
    /// Tokens a lane ingests per prefill step.
    ///
    /// The whole reason prefill is chunked at all: a prefill graph's
    /// activations scale with `n_tokens x attention_key_count`, and with a
    /// slot-major arena a high lane's key span is its base PLUS its fill — so
    /// an 8k prompt on lane 1 of a 128k-per-lane box asks for gigabytes of
    /// activations in a single graph. Chunking bounds the first factor, which
    /// is the only one a scheduler controls.
    prefill_chunk: usize,
    /// Per slot, what its caches still describe after its lane retired.
    ///
    /// PER SLOT, which is the whole point. One shared prefix cache belongs to
    /// whoever spoke last, so a second conversation interleaving with a first
    /// takes the first's append away — the user reads that as "it was fast,
    /// now it is slow, for no reason". A conversation that owns its own lane's
    /// history cannot have it stolen.
    parked: Vec<Option<Parked>>,
    /// Lanes whose prompt extended their parked history, so their caches were
    /// kept and the prefill is a delta. Reported through
    /// [`Self::reset_requested`], which the executor already consults.
    resumed: Vec<bool>,
    /// Tokens that END a turn. A property of the model, so every lane on one
    /// session shares them.
    ///
    /// The scheduler cannot ask a vocabulary anything, and until it was told
    /// these it ended a lane at `max_new` and NOWHERE else: a reply ran past
    /// its own end-of-turn token into a fresh one, forever, and the lane never
    /// retired. [`LaneExecutor`] fills these from the session so no caller can
    /// forget them.
    stop_tokens: Vec<i32>,
}

impl LaneScheduler {
    pub fn new(table: SlotTable, queue_max: usize) -> Self {
        let slots = table.len();
        let lanes = vec![None; slots];
        Self {
            table,
            lanes,
            pending: VecDeque::new(),
            queue_max,
            prefill_chunk: DEFAULT_PREFILL_CHUNK,
            parked: vec![None; slots],
            resumed: vec![false; slots],
            stop_tokens: Vec::new(),
        }
    }

    /// Tokens that end a lane's turn. Empty means "run to `max_new`", which is
    /// only ever right for a caller feeding synthetic tokens.
    pub fn with_stop_tokens(mut self, tokens: impl IntoIterator<Item = i32>) -> Self {
        self.stop_tokens = tokens.into_iter().collect();
        self
    }

    pub fn stop_tokens(&self) -> &[i32] {
        &self.stop_tokens
    }

    /// Tokens a lane ingests per prefill step. Set from the session's own
    /// prefill batch so the two agree.
    pub fn with_prefill_chunk(mut self, tokens: usize) -> Self {
        self.prefill_chunk = tokens.max(1);
        self
    }

    pub fn slots_total(&self) -> usize {
        self.lanes.len()
    }

    /// Lanes holding a conversation, generating or not.
    pub fn slots_claimed(&self) -> usize {
        self.lanes.iter().filter(|lane| lane.is_some()).count()
    }

    /// Whether `lane` currently holds a conversation.
    pub fn is_lane_claimed(&self, lane: usize) -> bool {
        self.lanes.get(lane).map(|l| l.is_some()).unwrap_or(false)
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
            let Some(request) = self.pending.front() else { break };
            // Sticky first: a slot still holding THIS conversation's tokens,
            // whose new prompt extends all of them, lets the turn append.
            let resume = self.resumable_slot(request);
            let Some(index) = resume.or_else(|| self.free_slot()) else {
                break;
            };
            let mut request = self.pending.pop_front().expect("non-empty");
            let history = match resume {
                Some(_) => {
                    // Keep the caches and prefill only what is new. Everything
                    // up to `fill` is already correct — including the recurrent
                    // state, which is why this is only ever offered for a
                    // prompt that extends the WHOLE history.
                    let parked = self.parked[index].take().expect("resumable slot is parked");
                    let delta = request.prompt_tokens[parked.tokens.len()..].to_vec();
                    request.prompt_tokens = delta;
                    let _ = self.table.resume(index);
                    self.resumed[index] = true;
                    parked.tokens
                }
                None => {
                    let _ = self.table.retire(index);
                    self.parked[index] = None;
                    self.resumed[index] = false;
                    let _ = self.table.admit_at(index);
                    Vec::new()
                }
            };
            self.lanes[index] = Some(Lane {
                request,
                produced: 0,
                next_token: None,
                phase: LanePhase::NeedsPrefill,
                ingested: 0,
                tokens: history,
            });
        }
    }

    /// A slot for a conversation that cannot resume one.
    ///
    /// Unparked slots first, lowest index — so a lone conversation lands on
    /// slot 0, whose `kv_base` is 0 and which is therefore the only lane that
    /// can run the session-native speculative path.
    ///
    /// A PARKED slot is taken only when nothing else is free, and taking it
    /// destroys an append somebody else was going to get. Preferring the
    /// unparked ones is what stops a passing conversation from evicting a
    /// player mid-session on a box with lanes to spare.
    fn free_slot(&self) -> Option<usize> {
        // A conversation arriving ALONE goes to slot 0 if it can, even over a
        // park.
        //
        // Slot 0 is the only lane whose history can live in the session's own
        // single-sequence state, and that state is what the session-native
        // speculative path decodes against — the measured 89-100 tok/s one. Any
        // other lane decodes through a slot, and on this box that is half the
        // rate. Preferring an unparked lane is right when someone else is
        // talking (it protects their append); when nobody else is talking there
        // is no append to protect from THIS turn, and the choice is between a
        // stranger's dormant cache and this speaker's decode rate.
        //
        // Only when the box is otherwise idle: with a second lane live, taking
        // slot 0 would not make this turn native anyway (`is_solo` is false),
        // so the eviction would buy nothing at all.
        let alone = self.pending.len() <= 1 && self.slots_claimed() == 0;
        if alone && self.lanes.first().map(|lane| lane.is_none()).unwrap_or(false) {
            return Some(0);
        }
        let unparked = (0..self.lanes.len())
            .find(|index| self.lanes[*index].is_none() && self.parked[*index].is_none());
        unparked.or_else(|| (0..self.lanes.len()).find(|index| self.lanes[*index].is_none()))
    }

    /// A parked slot this request may resume: same conversation, and a prompt
    /// that extends every token the slot's caches describe.
    ///
    /// The extension has to be TOTAL. Attention rows could be truncated to any
    /// prefix, but the delta-net state is a running scan and cannot be rewound
    /// — resuming at anything short of the full history would decode against a
    /// recurrent state belonging to tokens the prompt no longer contains, which
    /// is fluent output built on a conversation that did not happen.
    ///
    /// A prompt EQUAL to the history is refused too: a lane must ingest at
    /// least one token to have something to decode from.
    fn resumable_slot(&self, request: &LaneRequest) -> Option<usize> {
        if !self.can_resume(request) {
            return None;
        }
        (0..self.lanes.len()).find(|index| {
            self.lanes[*index].is_none()
                && self.parked[*index].as_ref().is_some_and(|parked| {
                    parked.session == request.session
                        && !parked.tokens.is_empty()
                        && request.prompt_tokens.len() > parked.tokens.len()
                        && request.prompt_tokens.starts_with(&parked.tokens)
                })
        })
    }

    /// A caller can still demand a clean slate; `reset_first` is its way of
    /// saying the resident state must not be trusted.
    fn can_resume(&self, request: &LaneRequest) -> bool {
        !request.reset_first
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
                // One CHUNK, not the whole prompt. The graph a prefill builds
                // is sized by `n_tokens x attention_key_count`, and with a
                // slot-major arena the key count is the lane's BASE plus its
                // fill — so an 8k prompt on a high lane at 128k per lane asks
                // for gigabytes of activations at once, and the allocation
                // fails. The single-sequence path has always chunked; this is
                // the slot path catching up to it.
                let end = (lane.ingested + self.prefill_chunk)
                    .min(lane.request.prompt_tokens.len());
                return LaneStep::Prefill {
                    lane: index,
                    kv_base: slot.kv_base(),
                    state_row: slot.live_state_row(),
                    start: slot.fill(),
                    tokens: lane.request.prompt_tokens[lane.ingested..end].to_vec(),
                    last: end >= lane.request.prompt_tokens.len(),
                    // Only the FIRST chunk may reset: the ones after it are
                    // appending to state this same lane just wrote.
                    resumed: self.resumed.get(index).copied().unwrap_or(false)
                        || lane.ingested > 0,
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
    ///
    /// A prompt whose very first sampled token is a stop token asks for an
    /// empty reply, and gets one. Decoding it instead would step past the end
    /// of the turn on token zero.
    pub fn on_prefilled(&mut self, lane: usize, count: usize, first_token: i32) -> Vec<LaneEvent> {
        let _ = self.table.advance(lane, count);
        let stops = self.stop_tokens.contains(&first_token);
        let mut complete = false;
        if let Some(slot) = self.lanes.get_mut(lane).and_then(|l| l.as_mut()) {
            // The chunk that just went in is now part of this lane's history.
            let from = slot.ingested.min(slot.request.prompt_tokens.len());
            let to = (slot.ingested + count).min(slot.request.prompt_tokens.len());
            let chunk: Vec<i32> = slot.request.prompt_tokens[from..to].to_vec();
            slot.tokens.extend_from_slice(&chunk);
            slot.ingested = to;
            // More prompt to go: stay in prefill. The token sampled from a
            // middle chunk's logits is meaningless — the prompt is not
            // finished, so there is nothing yet to continue from.
            complete = slot.ingested >= slot.request.prompt_tokens.len();
            if complete {
                slot.next_token = Some(first_token);
                slot.phase = if stops {
                    LanePhase::Done(LaneOutcome::Complete)
                } else {
                    LanePhase::Decoding
                };
            }
        }
        if complete {
            let _ = self.table.begin_decoding(lane);
        } else {
            // Not a decode step yet, and not an event either: the caller hears
            // about a prefill once, when it is actually done.
            return Vec::new();
        }
        let resumed = self.resumed.get(lane).copied().unwrap_or(false);
        self.lanes
            .get(lane)
            .and_then(|l| l.as_ref())
            .map(|slot| {
                vec![LaneEvent::Prefilled {
                    job: slot.request.job,
                    // The WHOLE turn's ingest, not the last chunk's — the
                    // number that says whether this conversation was warm.
                    ingested: slot.ingested,
                    resumed,
                }]
            })
            .unwrap_or_default()
    }

    /// Report a completed decode step. `sampled[i]` is the token sampled for
    /// `plan.slots[i]`. Returns what the worker should tell callers.
    ///
    /// The token a lane just DECODED is the one it emits; `sampled` is what it
    /// will decode next. Conflating the two drops the first token of every
    /// generation and appends one that was never produced.
    ///
    /// A lane ends here on **either** of two conditions, and dropping the
    /// second one is what made a reply run forever: `max_new` reached, or the
    /// next token being a stop token. The stop token is neither emitted nor
    /// decoded — the turn is over at the token before it.
    pub fn on_decoded(&mut self, plan: &StepPlan, sampled: &[i32]) -> Vec<LaneEvent> {
        let mut events = Vec::new();
        // Split borrows: the stop set and the lanes are separate fields, and
        // reading one while mutating the other is exactly what this needs.
        let stop_tokens = &self.stop_tokens;
        let lanes = &mut self.lanes;
        let table = &mut self.table;
        for (step, &next) in plan.slots.iter().zip(sampled) {
            let index = step.slot;
            let Some(lane) = lanes.get_mut(index).and_then(|l| l.as_mut()) else {
                continue;
            };
            let emitted = match lane.next_token {
                Some(token) => token,
                None => continue,
            };
            if stop_tokens.contains(&emitted) {
                // Only reachable if a stop token was seeded past the guards
                // above; end the turn rather than emit it.
                lane.phase = LanePhase::Done(LaneOutcome::Complete);
                continue;
            }
            lane.produced += 1;
            lane.tokens.push(emitted);
            lane.next_token = Some(next);
            let _ = table.advance(index, 1);
            events.push(LaneEvent::Token {
                job: lane.request.job,
                token: emitted,
                produced: lane.produced,
            });
            if lane.produced >= lane.request.max_new || stop_tokens.contains(&next) {
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
    ///
    /// `stopped` is the session's own verdict — it stopped for a reason other
    /// than running out of budget. The session drops the stop token rather
    /// than returning it, so the chunk can be SHORT, or empty, and still be
    /// the end of the turn. Without this the lane looked merely under-budget:
    /// it stayed claimed, produced nothing on every subsequent step, spun the
    /// worker, and — because a claimed lane is not solo — pushed the next
    /// conversation onto the unspeculated batched path.
    ///
    /// An empty chunk ends the lane whatever `stopped` says. A generator that
    /// returned nothing cannot make progress, and looping on it is a spin with
    /// no exit.
    pub fn on_generated(&mut self, lane: usize, tokens: &[i32], stopped: bool) -> Vec<LaneEvent> {
        let mut events = Vec::new();
        let stop_tokens = &self.stop_tokens;
        let Some(slot) = self.lanes.get_mut(lane).and_then(|l| l.as_mut()) else {
            return events;
        };
        for &token in tokens {
            if stop_tokens.contains(&token) {
                break;
            }
            slot.produced += 1;
            slot.tokens.push(token);
            events.push(LaneEvent::Token {
                job: slot.request.job,
                token,
                produced: slot.produced,
            });
        }
        if stopped || tokens.is_empty() || slot.produced >= slot.request.max_new {
            slot.phase = LanePhase::Done(LaneOutcome::Complete);
        }
        let _ = self.table.advance(lane, tokens.len());
        events
    }

    /// Everything a batched speculative round needs about the lanes in `plan`,
    /// in plan order.
    ///
    /// Every fact is read from the lane or its slot and passed as an argument;
    /// the session learns nothing about lanes and keeps its own single-sequence
    /// state for the solo path. That split is the whole defence against the
    /// bug class this lane keeps finding — a lane's history, its resume row and
    /// its draft-head fill are exactly the things two conversations must not
    /// share.
    ///
    /// Cross-checks the plan against the table rather than trusting it. A plan
    /// built before something else advanced a slot would address the right
    /// lane at the wrong position, which writes a token over a token and reads
    /// a mask that is one row short — no error, just a conversation that starts
    /// repeating itself.
    pub fn spec_lanes(&self, plan: &StepPlan) -> Result<Vec<crate::SpecLane<'_>>, String> {
        plan.slots
            .iter()
            .map(|step| {
                let index = step.slot;
                let lane = self
                    .lanes
                    .get(index)
                    .and_then(|lane| lane.as_ref())
                    .ok_or_else(|| {
                        format!("lane {index} is in a decode plan but holds no conversation")
                    })?;
                let first = lane.next_token.ok_or_else(|| {
                    format!("lane {index} is decoding with no token to decode")
                })?;
                let slot = self
                    .table
                    .slot(index)
                    .ok_or_else(|| format!("lane {index} has no slot"))?;
                if step.position != slot.fill() || step.key_lower_bound != slot.kv_base() {
                    return Err(format!(
                        "decode plan for lane {index} is stale: it says position {} base {}, \
                         the slot says {} and {}",
                        step.position,
                        step.key_lower_bound,
                        slot.fill(),
                        slot.kv_base()
                    ));
                }
                Ok(crate::SpecLane {
                    lane: index,
                    kv_base: slot.kv_base(),
                    state_base: slot.state_base(),
                    live_state_offset: slot.live_state_offset(),
                    fill: slot.fill(),
                    mtp_filled: slot.mtp_filled(),
                    tokens: &lane.tokens,
                    first,
                })
            })
            .collect()
    }

    /// Report a batched speculative round: `outcomes[i]` belongs to
    /// `plan.slots[i]`.
    ///
    /// A round commits between one and `depth + 1` tokens per lane, so this is
    /// neither [`on_decoded`](Self::on_decoded) (exactly one, and it emits the
    /// PREVIOUS one) nor [`on_generated`](Self::on_generated) (a whole chunk
    /// with no follow-on token). The round's first committed token is the one
    /// the lane was already decoding, and its `next` is drawn and carried
    /// forward rather than re-sampled.
    ///
    /// **The history takes every committed token even past `max_new`.** The
    /// tokens are in the lane's KV whatever the caller asked for, and a parked
    /// lane's history has to describe its caches exactly — a conversation that
    /// comes back and appends against a history one token short of its own
    /// cache decodes the rest of its life at the wrong offset.
    pub fn on_speculated(
        &mut self,
        plan: &StepPlan,
        outcomes: &[crate::SpecRoundOutcome],
    ) -> Result<Vec<LaneEvent>, String> {
        if outcomes.len() != plan.slots.len() {
            return Err(format!(
                "a {}-lane round reported {} outcomes",
                plan.slots.len(),
                outcomes.len()
            ));
        }
        let mut events = Vec::new();
        let stop_tokens = &self.stop_tokens;
        let lanes = &mut self.lanes;
        let table = &mut self.table;
        for (step, outcome) in plan.slots.iter().zip(outcomes) {
            let index = step.slot;
            let Some(lane) = lanes.get_mut(index).and_then(|lane| lane.as_mut()) else {
                continue;
            };
            let expected = lane.next_token.ok_or_else(|| {
                format!("lane {index} committed a round with no token to decode")
            })?;
            // The round's first committed token IS the token this lane was
            // decoding. If it is not, the outcomes are out of step with the
            // plan and every lane below this one is about to be given another
            // conversation's tokens.
            match outcome.committed.first() {
                Some(&first) if first == expected => {}
                Some(&first) => {
                    return Err(format!(
                        "lane {index} committed {first} while decoding {expected}: the round's \
                         outcomes are out of step with the plan"
                    ))
                }
                None => {
                    return Err(format!(
                        "lane {index} committed nothing; a round commits at least one token"
                    ))
                }
            }
            if let Some(stop) = outcome
                .committed
                .iter()
                .find(|token| stop_tokens.contains(token))
            {
                return Err(format!(
                    "lane {index} committed stop token {stop}: the round must end the turn \
                     before it rather than emit it"
                ));
            }
            for &token in &outcome.committed {
                // History first and unconditionally: it describes the CACHE.
                lane.tokens.push(token);
                if lane.produced >= lane.request.max_new {
                    continue;
                }
                lane.produced += 1;
                events.push(LaneEvent::Token {
                    job: lane.request.job,
                    token,
                    produced: lane.produced,
                });
            }
            lane.next_token = Some(outcome.next);
            table
                .advance(index, outcome.committed.len())
                .map_err(|e| format!("lane {index} advance: {e}"))?;
            table
                .adopt_state(index, outcome.live_state_offset, outcome.mtp_filled)
                .map_err(|e| format!("lane {index}: {e}"))?;
            if outcome.stop_reason.is_some()
                || lane.produced >= lane.request.max_new
                || stop_tokens.contains(&outcome.next)
            {
                lane.phase = LanePhase::Done(LaneOutcome::Complete);
            }
        }
        Ok(events)
    }

    /// Forget every parked conversation, because the device state their caches
    /// lived in has been cleared.
    ///
    /// [`crate::LlamaSession::reset`] zeroes the WHOLE shared cache — every
    /// slot's attention rows, the entire recurrent arena and the carry tensor —
    /// not just the sequence that asked for it. The solo path resets whenever a
    /// conversation cannot append to what the session already holds, and at
    /// that moment every parked lane's rows become zeros while this scheduler
    /// still believes they are resumable.
    ///
    /// Left unsaid, the next returning conversation appends its delta onto a
    /// cache full of zeros and answers a conversation that did not happen —
    /// fluently, with no error anywhere. The cost of saying it is that a
    /// conversation coming back after someone else's cold turn re-ingests
    /// itself, which is what it did before per-lane caches existed.
    pub fn invalidate_parked(&mut self) {
        for index in 0..self.parked.len() {
            if self.parked[index].take().is_some() {
                let _ = self.table.retire(index);
            }
        }
    }

    /// Take over a lane's speculative bookkeeping from the session-native path.
    ///
    /// The solo path advances the session's own resume row and draft-head fill
    /// every round and has no way to know it is sitting in a slot. Whatever
    /// hands it over has to carry both across, or the first batched step
    /// resumes lane 0 from a checkpoint belonging to a different number of
    /// committed tokens.
    pub fn adopt_native_state(
        &mut self,
        lane: usize,
        live_state_offset: usize,
        mtp_filled: usize,
    ) -> Result<(), String> {
        self.table
            .adopt_state(lane, live_state_offset, mtp_filled)
            .map_err(|e| format!("lane {lane} adopting session state: {e}"))
    }

    /// The slot a lane occupies, for callers that need its cache geometry.
    pub fn slot(&self, lane: usize) -> Option<&crate::slots::Slot> {
        self.table.slot(lane)
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
        // The SCHEDULER decides this, not the request. It is the only thing
        // that knows what the slot still holds and whether the prompt extends
        // it, and a request's own `reset_first` is only ever a veto.
        !self.resumed.get(lane).copied().unwrap_or(false)
    }

    /// Sampling settings for whoever holds `lane`.
    pub fn sampling_for(&self, lane: usize) -> Option<crate::LlamaSamplingParams> {
        self.lanes
            .get(lane)
            .and_then(|l| l.as_ref())
            .map(|slot| slot.request.sampling)
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
            // Park, do not clear: the rows are still there and the
            // conversation may come straight back. A cancelled turn parks too
            // — its tokens are as real as any other's.
            let held = self.lanes[index]
                .as_ref()
                .map(|lane| (lane.request.session.clone(), lane.tokens.clone()));
            self.lanes[index] = None;
            self.resumed[index] = false;
            match held {
                Some((session, tokens)) if !tokens.is_empty() => {
                    let _ = self.table.park(index);
                    self.parked[index] = Some(Parked { session, tokens });
                }
                _ => {
                    let _ = self.table.retire(index);
                    self.parked[index] = None;
                }
            }
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
    /// Cost of a step of C columns and of one shared draft forward, for the
    /// depth decision. Built once, and built knowing WHICH draft head this
    /// session loaded: the restricted sidecar is ~3.3x cheaper per drafted
    /// token at identical acceptance, which is exactly the term that decides
    /// whether a deeper batched round pays for itself.
    costs: crate::slots::StepCostModel,
    config: crate::slots::SchedulerConfig,
    /// `MAKEPAD_LLAMA_BATCH_SPEC_DEPTH`, when set: run every batched round at
    /// this depth instead of the modelled one.
    ///
    /// Here so the depth curve can be MEASURED on a box rather than modelled
    /// from one. The cost model is a lower bound taken from a synthetic sweep;
    /// the only way to find out what a real multi-lane round costs is to run
    /// each rung on the hardware, and a knob that needs a rebuild per rung is a
    /// measurement that never happens.
    forced_depth: Option<usize>,
    /// `MAKEPAD_LLAMA_BATCH_FORCE_ROUND`, when set: take the fused round even
    /// at depth 0, where the executor would otherwise take the plain step.
    ///
    /// A diagnostic, and a pointed one. At depth 0 the two paths produce the
    /// same tokens from the same columns and differ ONLY in which graph runs —
    /// the plain decode graph, or the one that checkpoints the recurrent state
    /// after every token. `.217` says the second costs 2.5x per column, and
    /// this is the knob that says whether that is the checkpointing graph
    /// itself or the multi-token recurrent scan it usually runs with.
    force_round: bool,
    /// Batched speculative rounds run since load, and the shape of the last
    /// one.
    ///
    /// Not statistics: a gate has to be able to prove the path it is testing
    /// actually ran. A two-lanes-both-speculating gate that silently fell back
    /// to the unspeculated step would pass, and would be certifying nothing.
    batched_spec_rounds: u64,
    last_batched_spec: Option<(usize, usize)>,
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
    /// Sampling settings for whoever holds `lane`, falling back to the
    /// executor's default when the lane is unclaimed.
    fn params_for(&self, lane: usize) -> crate::LlamaSamplingParams {
        self.scheduler.sampling_for(lane).unwrap_or(self.params)
    }

    /// The lane's RNG stream, seeded ONCE from its own request's seed.
    ///
    /// Seeded per lane rather than per executor so two chats with the same
    /// seed still get their own stream, and a chat's stream does not depend on
    /// which lane it happened to land in.
    fn sampler_for(&mut self, lane: usize) -> &mut crate::LlamaSamplerState {
        let seed = self.params_for(lane).seed;
        self.samplers[lane].get_or_insert_with(|| crate::LlamaSamplerState::new(seed))
    }

    pub fn new(
        session: crate::LlamaSession,
        scheduler: LaneScheduler,
        params: crate::LlamaSamplingParams,
    ) -> Self {
        let samplers = vec![None; scheduler.slots_total()];
        // Taken from the session rather than asked of the caller. A scheduler
        // with no stop tokens ends a lane only at `max_new`, and the failure
        // that causes is silent: a reply that runs past its own end-of-turn
        // token into a new one, and a lane that never retires.
        let scheduler = scheduler.with_stop_tokens(session.stop_tokens());
        let costs = if session.has_restricted_draft_head() {
            crate::slots::StepCostModel::measured_5090().with_restricted_draft_head()
        } else {
            crate::slots::StepCostModel::measured_5090()
        };
        let forced_depth = std::env::var("MAKEPAD_LLAMA_BATCH_SPEC_DEPTH")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok());
        Self {
            session,
            scheduler,
            samplers,
            params,
            solo_native: false,
            costs,
            config: crate::slots::SchedulerConfig::default(),
            forced_depth,
            force_round: std::env::var_os("MAKEPAD_LLAMA_BATCH_FORCE_ROUND").is_some(),
            batched_spec_rounds: 0,
            last_batched_spec: None,
            on_counts: None,
        }
    }

    /// Batched speculative rounds this executor has run.
    pub fn batched_spec_rounds(&self) -> u64 {
        self.batched_spec_rounds
    }

    /// `(width, uniform draft depth)` of the last batched speculative round, or
    /// `None` if no step has run one.
    pub fn last_batched_spec(&self) -> Option<(usize, usize)> {
        self.last_batched_spec
    }

    /// Report lane occupancy after every step. The service wires `/health` here.
    pub fn on_counts(mut self, sink: impl FnMut(LaneCounts) + Send + 'static) -> Self {
        self.on_counts = Some(Box::new(sink));
        self
    }

    /// The resident session, for callers that need its vocabulary or context
    /// bounds while the executor owns it.
    pub fn session(&self) -> &crate::LlamaSession {
        &self.session
    }

    /// Give the session back.
    ///
    /// An executor is scoped to a set of lanes and their streams; the session
    /// is the expensive thing, and on a box it is sixteen gigabytes of weights
    /// that took a minute to reach the device. A gate that measures several
    /// lane configurations has to be able to keep it across them, or most of
    /// its wall time is model loading and its results are one reload apart.
    pub fn into_session(self) -> crate::LlamaSession {
        self.session
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

    /// The uniform draft depth this step runs at, or 0 for a plain step.
    ///
    /// `plan.draft_depth` is the ladder's wall — what the column budget and the
    /// session's allocation allow at this width. This narrows it to what the
    /// measured cost curve says actually PAYS at this width, which at 3 and 4
    /// lanes on a 5090 with the full draft head is nothing: an extra draft
    /// column there buys fewer tokens than the wider verify batch costs.
    ///
    /// Clamped by every lane's remaining context too. A round writes
    /// `depth + 1` rows per lane before anything is committed, and the lane
    /// closest to its capacity is the one that decides how many rows the step
    /// may write.
    fn batched_depth(&self, plan: &StepPlan) -> usize {
        if !self.session.speculative_enabled() || plan.draft_depth == 0 {
            return 0;
        }
        let modelled = crate::slots::batched_draft_depth(
            plan.width,
            plan.draft_depth,
            crate::slots::BATCHED_CHAT_ACCEPTANCE,
            &self.config,
            &self.costs,
        );
        let depth = self.forced_depth.unwrap_or(modelled).min(plan.draft_depth);
        let room = plan
            .slots
            .iter()
            .filter_map(|step| self.scheduler.slot(step.slot))
            .map(|slot| slot.remaining_context())
            .min()
            .unwrap_or(0);
        depth.min(room.saturating_sub(1))
    }

    /// One batched speculative round: every lane in `plan` drafts, and ONE
    /// verify batch serves all of them.
    fn speculative_batch(
        &mut self,
        plan: &StepPlan,
        depth: usize,
    ) -> Result<Vec<LaneEvent>, String> {
        let params: Vec<crate::LlamaSamplingParams> = plan
            .slots
            .iter()
            .map(|step| self.params_for(step.slot))
            .collect();
        // Taken OUT of the per-lane table and put back after, because the round
        // needs them contiguous and mutable. A lane's stream is its own: two
        // lanes drawing from one would make a chat's output depend on when the
        // other one happened to sample.
        let mut streams: Vec<crate::LlamaSamplerState> = plan
            .slots
            .iter()
            .enumerate()
            .map(|(index, step)| {
                self.samplers[step.slot]
                    .take()
                    .unwrap_or_else(|| crate::LlamaSamplerState::new(params[index].seed))
            })
            .collect();
        let round = {
            let scheduler = &self.scheduler;
            let session = &mut self.session;
            let lanes = scheduler.spec_lanes(plan)?;
            session.speculative_round_slots(&lanes, depth, &params, &mut streams)
        };
        // Put the streams back BEFORE the error is propagated. A round that
        // failed must not also lose every participating conversation's RNG
        // position — the next turn would silently re-seed from the request
        // default and sample a different reply for the same seed.
        for (stream, step) in streams.into_iter().zip(&plan.slots) {
            self.samplers[step.slot] = Some(stream);
        }
        let outcomes = round.map_err(|e| format!("batched speculative round: {e}"))?;
        self.batched_spec_rounds += 1;
        self.last_batched_spec = Some((plan.width, depth));
        self.scheduler.on_speculated(plan, &outcomes)
    }

    /// Carry the session's own speculative bookkeeping onto the solo lane's
    /// slot.
    ///
    /// The session-native path moves its resume row and its draft-head fill
    /// every round and cannot see the slot table. Whatever the lane does next —
    /// a batched step because someone else joined, or a batched speculative
    /// round — reads those two numbers FROM the slot, so they have to be true
    /// there as well. A stale resume row does not fail: it decodes lane 0
    /// against the recurrent state of a different number of tokens, fluently.
    fn adopt_solo_state(&mut self) -> Result<(), String> {
        if !self.session.speculative_enabled() {
            return Ok(());
        }
        let offset = self.session.live_state_offset();
        let filled = self.session.draft_head_fill();
        self.scheduler.adopt_native_state(SOLO_LANE, offset, filled)
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
                let params = self.params_for(SOLO_LANE);
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
                resumed,
                last,
            } if lane == SOLO_LANE
                && self.scheduler.is_solo(lane)
                && (resumed || self.session.token_count() == start) =>
            {
                // Session-native ingest, so the speculative loop can run
                // against it. `reset_first` is the worker's prefix decision —
                // but it is a PER-TURN decision, and this arm runs once per
                // CHUNK: resetting on every chunk of a cold multi-chunk
                // prefill wiped the whole session (KV, delta-net state,
                // carry) 512 tokens at a time, leaving the model holding
                // only the last chunk — fluent, and amnesiac about
                // everything before it. Only a chunk that is NOT a resume
                // of the previous one (i.e. the first) may reset.
                if !resumed && self.scheduler.reset_requested(lane) {
                    // A reset clears the whole device cache, not this lane's
                    // share of it, so every parked conversation's rows become
                    // zeros. Say so before it happens: a parked lane that is
                    // still believed resumable afterwards appends its next
                    // delta onto zeros and answers fluently about nothing.
                    self.scheduler.invalidate_parked();
                    self.session.reset().map_err(|e| format!("reset: {e}"))?;
                }
                self.session
                    .append_tokens(&tokens)
                    .map_err(|e| format!("solo prefill: {e}"))?;
                // Only the LAST chunk has a token to sample. A middle chunk's
                // logits predict the next PROMPT token, which is already
                // known — sampling it would cost a full-vocabulary pass per
                // chunk and draw from this lane's RNG, making the reply depend
                // on how the prompt happened to be split.
                let first = if last {
                    let logits = self
                        .session
                        .last_logits()
                        .ok_or_else(|| "solo prefill produced no logits".to_string())?
                        .to_vec();
                    let params = self.params_for(lane);
                    let sampler = self.sampler_for(lane);
                    sampler
                        .sample_logits(&logits, params)
                        .map_err(|e| format!("lane {lane} sample: {e}"))?
                } else {
                    0
                };
                self.solo_native = true;
                events.extend(self.scheduler.on_prefilled(lane, tokens.len(), first));
                self.adopt_solo_state()?;
            }
            LaneStep::Prefill {
                lane,
                kv_base,
                state_row,
                start,
                tokens,
                resumed,
                last,
            } => {
                let _ = resumed;
                // A fresh conversation on a used slot inherits the previous
                // occupant's RECURRENT state — the scan resumes from the row it
                // is given, and admission only resets counters. Attention rows
                // are fine (the mask's lower/upper span excludes them exactly);
                // the running state is not, and the symptom is a fluent reply
                // conditioned on someone else's conversation.
                if start == 0 {
                    self.session
                        .clear_slot_state(lane)
                        .map_err(|e| format!("lane {lane} clear: {e}"))?;
                }
                let logits = self
                    .session
                    .prefill_slot_chunk(lane, kv_base, state_row, start, &tokens)
                    .map_err(|e| format!("lane {lane} prefill: {e}"))?;
                // A lane's stream is seeded once, when it is admitted, and
                // carried for the whole generation.
                let first = if last {
                    let params = self.params_for(lane);
                    let sampler = self.sampler_for(lane);
                    sampler
                        .sample_logits(&logits, params)
                        .map_err(|e| format!("lane {lane} sample: {e}"))?
                } else {
                    0
                };
                events.extend(self.scheduler.on_prefilled(lane, tokens.len(), first));
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
                    let params = self.params_for(SOLO_LANE);
                    let sampler = self.samplers[SOLO_LANE]
                        .get_or_insert_with(|| crate::LlamaSamplerState::new(params.seed));
                    self.session
                        .continue_sampled_with(want, params, sampler)
                        .map_err(|e| format!("solo decode: {e}"))?
                };
                // Two ways the turn is over, and the second is not a stop
                // reason: a chunk shorter than asked for, with no reason
                // given, means the session cannot make progress. The
                // single-lane worker has always broken on both; the lane
                // worker inherited neither.
                let stopped = generated.stop_reason != crate::LlamaStopReason::MaxNewTokens
                    || generated.token_ids.len() < want;
                events.extend(self.scheduler.on_generated(
                    SOLO_LANE,
                    &generated.token_ids,
                    stopped,
                ));
                // The chunk moved the session's resume row and its draft-head
                // fill. Slot 0's copies do not move on their own, and the next
                // step may well be a batched one that reads them.
                self.adopt_solo_state()?;
            }
            LaneStep::Decode { plan, tokens } => {
                // One depth for the whole step, or none at all. At depth 0 the
                // speculative round degenerates to one column per lane — the
                // same tokens the plain step produces, through a DIFFERENT
                // graph family (checkpointed state rows, hidden write rows).
                // Taking the plain step there keeps the byte-identity the
                // existing gates rest on and compiles one graph family fewer.
                let depth = self.batched_depth(&plan);
                if depth > 0 || (self.force_round && self.session.speculative_enabled()) {
                    events.extend(self.speculative_batch(&plan, depth)?);
                } else {
                    let rows = self
                        .session
                        .step_slots(&plan, &tokens)
                        .map_err(|e| format!("batched decode: {e}"))?;
                    let mut sampled = Vec::with_capacity(rows.len());
                    for (row, step) in rows.iter().zip(&plan.slots) {
                        let lane = step.slot;
                        let params = self.params_for(lane);
                        let sampler = self.samplers[lane]
                            .get_or_insert_with(|| crate::LlamaSamplerState::new(params.seed));
                        sampled.push(
                            sampler
                                .sample_logits(row, params)
                                .map_err(|e| format!("lane {lane} sample: {e}"))?,
                        );
                    }
                    events.extend(self.scheduler.on_decoded(&plan, &sampled));
                }
            }
        }
        for event in self.scheduler.reap() {
            if let LaneEvent::Finished { lane, .. } = &event {
                // Drop the retired lane's stream so whoever takes the slot next
                // starts fresh instead of continuing a stranger's sequence.
                self.samplers[*lane] = None;
                if *lane == SOLO_LANE {
                    // And forget that its history was session-native. Left set,
                    // the handover at the top of the next step fires for a lane
                    // that no longer exists: it samples stale logits and, in
                    // doing so, RE-CREATES `samplers[0]` — seeded from the
                    // executor default, because the lane is gone and has no
                    // request to take a seed from. The next conversation to
                    // land on lane 0 then finds a stream already there and
                    // silently inherits it, seed ignored. That is the retired
                    // stream leaking back in through the door reap just shut.
                    self.solo_native = false;
                }
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

    /// The end-of-turn token these tests stand in for `<|im_end|>` with.
    const EOS: i32 = 7;

    fn scheduler(slots: usize, queue_max: usize) -> LaneScheduler {
        let table = SlotTable::new(slots, 512, 1, 0).expect("table");
        LaneScheduler::new(table, queue_max).with_stop_tokens([EOS])
    }

    /// A scheduler whose slots are sized for speculation: `draft_max + 2`
    /// recurrent rows each, so a round has checkpoint planes to commit into.
    fn spec_scheduler(slots: usize, queue_max: usize) -> LaneScheduler {
        let table = SlotTable::new(slots, 512, crate::slots::SOLO_DRAFT_MAX + 2, crate::slots::SOLO_DRAFT_MAX)
            .expect("table");
        LaneScheduler::new(table, queue_max).with_stop_tokens([EOS])
    }

    /// What a round handed back for one lane.
    fn outcome(committed: &[i32], next: i32, resume: usize, filled: usize) -> crate::SpecRoundOutcome {
        crate::SpecRoundOutcome {
            committed: committed.to_vec(),
            next,
            live_state_offset: resume,
            mtp_filled: filled,
            drafted: committed.len().saturating_sub(1),
            accepted: committed.len().saturating_sub(1),
            stop_reason: None,
        }
    }

    /// Prefill one job and return the decode plan its lane is part of.
    fn prefilled(sched: &mut LaneScheduler, job: u64, prompt: &[i32], max_new: usize, first: i32) {
        sched.submit(request(job, prompt, max_new)).expect("submit");
        loop {
            match sched.next_step() {
                LaneStep::Prefill { lane, tokens, last, .. } => {
                    let token = if last { first } else { 0 };
                    sched.on_prefilled(lane, tokens.len(), token);
                    if last {
                        return;
                    }
                }
                other => panic!("expected prefill for job {job}, got {other:?}"),
            }
        }
    }

    fn request(job: u64, prompt: &[i32], max_new: usize) -> LaneRequest {
        LaneRequest {
            job,
            session: format!("session-{job}"),
            prompt_tokens: prompt.to_vec(),
            reset_first: true,
            max_new,
            sampling: crate::LlamaSamplingParams::default(),
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
        sched.on_prefilled(lane, tokens.len(), 70);
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
    /// LIVE, 2026-08-21: a user's poem streamed at 45 tok/s on a box whose solo
    /// path measures 89-100. Their conversation was on lane 1, because lane 0
    /// was PARKED by an earlier one and free_slot preferred an unparked lane —
    /// and only lane 0 can run the session-native speculative path.
    ///
    /// So a conversation arriving alone takes slot 0, park or no park. What it
    /// costs is a dormant conversation's cache; what it buys is this speaker's
    /// decode rate, and there is nobody else on the box to be slowed down.
    #[test]
    fn a_conversation_arriving_alone_lands_where_it_can_speculate() {
        let mut sched = scheduler(4, 4);
        // Someone talked, and left their history parked on slot 0.
        sched.submit(request(1, &[1, 2, 3], 2)).expect("submit");
        let _ = run_to_completion(&mut sched, 12);
        assert!(sched.is_idle(), "the first conversation retired");

        // A DIFFERENT conversation now arrives, alone.
        sched
            .submit(LaneRequest {
                session: "someone-else".to_string(),
                ..request(2, &[9, 9, 9], 2)
            })
            .expect("submit");
        sched.admit_pending();
        assert!(
            sched.is_solo(SOLO_LANE),
            "a lone arrival must land on the lane that can speculate, not beside it"
        );
    }

    /// The other half of the same rule: with someone else already talking,
    /// taking slot 0 would NOT make this turn native — `is_solo` is false
    /// either way — so the park is left alone and the newcomer goes elsewhere.
    #[test]
    fn a_second_conversation_does_not_evict_a_park_for_nothing() {
        let mut sched = scheduler(4, 4);
        sched.submit(request(1, &[1, 2, 3], 2)).expect("submit");
        let _ = run_to_completion(&mut sched, 12);

        // One conversation is live...
        sched
            .submit(LaneRequest { session: "a".to_string(), ..request(2, &[5, 5], 40) })
            .expect("submit");
        sched.admit_pending();
        let first = (0..4).find(|i| sched.is_lane_claimed(*i)).expect("claimed");
        // ...and a second arrives behind it.
        sched
            .submit(LaneRequest { session: "b".to_string(), ..request(3, &[7, 7], 40) })
            .expect("submit");
        sched.admit_pending();
        let second = (0..4)
            .find(|i| sched.is_lane_claimed(*i) && *i != first)
            .expect("second claimed");
        assert_ne!(first, second, "two conversations, two lanes");
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

        let events = sched.on_generated(0, &[41, 42, 43], false);
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
        sched.on_generated(0, &[1, 2, 3], false);
        assert_eq!(sched.remaining(0), 0);
        assert!(matches!(
            sched.reap().first(),
            Some(LaneEvent::Finished { job: 6, .. })
        ));
    }

    /// REGRESSION, 2026-08-21. Live `.165` chat: a user said "hi" and the
    /// reply never ended.
    ///
    /// The batched path ended a lane at `max_new` and nowhere else, so a reply
    /// ran straight through its own end-of-turn token and started a new turn,
    /// over and over, until the token budget ran out. Every symptom the box
    /// showed follows from it: the endless reply, the lane that stayed
    /// claimed, the next conversation queueing behind it, and — because a
    /// claimed lane is not solo — that conversation decoding on the
    /// unspeculated batched path at 69 tok/s instead of the ~110 solo one.
    #[test]
    fn a_reply_ends_at_its_stop_token_instead_of_running_to_the_budget() {
        let mut sched = scheduler(2, 4);
        // A generous budget, as a chat request has: the budget must NOT be
        // what ends this turn.
        sched.submit(request(1, &[1, 2], 512)).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 100);

        // Two real tokens, then the model wants to end the turn.
        let mut emitted = Vec::new();
        for next in [101, EOS] {
            let LaneStep::Decode { plan, .. } = sched.next_step() else {
                panic!("expected decode");
            };
            emitted.extend(sched.on_decoded(&plan, &[next]));
        }
        assert_eq!(
            emitted
                .iter()
                .filter_map(|e| match e {
                    LaneEvent::Token { token, .. } => Some(*token),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![100, 101],
            "the stop token is never emitted, and nothing follows it"
        );

        let finished = sched.reap();
        assert!(
            finished.contains(&LaneEvent::Finished {
                job: 1,
                lane: 0,
                outcome: LaneOutcome::Complete
            }),
            "the lane must finish on the stop token, got {finished:?}"
        );
        assert_eq!(sched.slots_claimed(), 0, "and free its slot");
        assert!(sched.is_idle(), "and leave the worker with nothing to spin on");
        assert_eq!(
            sched.next_step(),
            LaneStep::Idle,
            "a retired lane must not keep planning steps"
        );
    }

    /// REGRESSION, same incident, the other half.
    ///
    /// The solo speculative path DOES stop at the stop token — the session
    /// drops it and returns a short chunk. The scheduler never heard about it,
    /// saw only "under budget", and kept the lane. That lane then produced
    /// nothing on every following step: the worker spun, the slot stayed
    /// claimed, and the next turn was no longer solo.
    #[test]
    fn a_solo_chunk_that_stopped_finishes_the_lane_even_when_it_came_up_short() {
        let mut sched = scheduler(2, 4);
        sched.submit(request(2, &[1], 512)).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 100);

        let events = sched.on_generated(0, &[41, 42], true);
        assert_eq!(events.len(), 2, "both tokens are still the lane's output");
        assert!(
            sched
                .reap()
                .contains(&LaneEvent::Finished {
                    job: 2,
                    lane: 0,
                    outcome: LaneOutcome::Complete
                }),
            "a stopped chunk ends the turn even 510 tokens under budget"
        );
        assert!(sched.is_idle());
    }

    /// A generator that returns nothing cannot make progress. Looping on it is
    /// a spin with no exit, and it is what pinned a core on the live box.
    #[test]
    fn a_solo_chunk_that_produced_nothing_ends_the_lane_rather_than_spinning() {
        let mut sched = scheduler(2, 4);
        sched.submit(request(3, &[1], 512)).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 100);

        assert!(sched.on_generated(0, &[], false).is_empty());
        assert!(!sched.reap().is_empty(), "an empty chunk retires the lane");
        assert!(sched.is_idle(), "and the worker gets to block again");
    }

    /// An empty reply is a reply. Decoding the stop token instead would step
    /// past the end of the turn on token zero.
    #[test]
    fn a_prompt_whose_first_token_is_a_stop_token_yields_an_empty_reply() {
        let mut sched = scheduler(2, 4);
        sched.submit(request(4, &[1, 2], 512)).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), EOS);
        assert_eq!(sched.lanes_active(), 0, "it is over before it decodes");
        assert!(sched.reap().contains(&LaneEvent::Finished {
            job: 4,
            lane: 0,
            outcome: LaneOutcome::Complete
        }));
        assert!(sched.is_idle());
    }

    /// The whole point of retiring properly: the NEXT conversation is solo
    /// again, so it takes the speculative fast path instead of the batched
    /// one. This is the 69-vs-110 tok/s the box measured.
    #[test]
    fn a_finished_turn_leaves_the_next_one_solo() {
        let mut sched = scheduler(4, 4);
        sched.submit(request(1, &[1], 512)).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 100);
        sched.on_generated(0, &[41], true);
        sched.reap();

        sched.submit(request(2, &[1], 512)).expect("submit");
        let LaneStep::Prefill { lane, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        assert!(
            sched.is_solo(lane),
            "a lane left claimed by the previous turn is what forced the \
             batched path onto a single conversation"
        );
    }

    /// REGRESSION, 2026-08-21. Live chat showed "loading" on every message.
    ///
    /// The slot prefill sent the WHOLE prompt in one graph. A prefill graph's
    /// activations scale with `n_tokens x attention_key_count`, and with a
    /// slot-major arena a lane's key count is its BASE plus its fill — so an
    /// eight-thousand-token prompt on lane 1 of a 128k-per-lane box asked for
    /// 7.5 GB of activations at once. The allocation failed, the job errored,
    /// the backend unloaded, and the NEXT message re-booted the model: the
    /// "loading 13%" the player saw was a session boot on every turn.
    ///
    /// The single-sequence path has always chunked. This is the slot path
    /// doing the same, and the property is simply that no step ever offers
    /// more than the chunk.
    #[test]
    fn a_long_prompt_is_prefilled_in_chunks() {
        let mut sched = scheduler(2, 4).with_prefill_chunk(8);
        let prompt: Vec<i32> = (0..20).collect();
        sched.submit(request(1, &prompt, 4)).expect("submit");

        let mut offered = Vec::new();
        let mut starts = Vec::new();
        for _ in 0..5 {
            match sched.next_step() {
                LaneStep::Prefill { lane, start, tokens, .. } => {
                    assert!(tokens.len() <= 8, "a step must never exceed the chunk");
                    starts.push(start);
                    offered.extend_from_slice(&tokens);
                    let events = sched.on_prefilled(lane, tokens.len(), 700);
                    // Only the LAST chunk finishes the prefill and reports it.
                    if offered.len() < prompt.len() {
                        assert!(
                            events.is_empty(),
                            "a middle chunk is not a finished prefill"
                        );
                    }
                }
                LaneStep::Decode { .. } => break,
                LaneStep::Idle => panic!("idle mid-prefill"),
            }
        }
        assert_eq!(offered, prompt, "every token goes in, once, in order");
        assert_eq!(starts, vec![0, 8, 16], "each chunk resumes where the last stopped");
    }

    #[test]
    fn only_the_last_chunk_of_a_prefill_is_asked_for_a_token() {
        // A middle chunk's logits predict the next PROMPT token, which is
        // already known. Sampling one costs a full-vocabulary pass per chunk
        // and DRAWS FROM THE LANE'S RNG — which would make a conversation's
        // reply depend on how its prompt happened to be split, and a chunk
        // size is not something a chat should be able to feel.
        let mut sched = scheduler(2, 4).with_prefill_chunk(4);
        let prompt: Vec<i32> = (0..10).collect();
        sched.submit(request(1, &prompt, 4)).expect("submit");
        let mut lasts = Vec::new();
        for _ in 0..3 {
            let LaneStep::Prefill { lane, tokens, last, .. } = sched.next_step() else {
                break;
            };
            lasts.push(last);
            sched.on_prefilled(lane, tokens.len(), 700);
        }
        assert_eq!(lasts, vec![false, false, true], "only the final chunk");
    }

    #[test]
    fn only_the_first_chunk_of_a_prefill_may_reset() {
        // Chunks after the first are appending to state THIS lane just wrote.
        // Resetting on chunk two would throw away chunk one and prefill the
        // rest at position zero — a prompt with its beginning missing.
        let mut sched = scheduler(2, 4).with_prefill_chunk(4);
        let prompt: Vec<i32> = (0..10).collect();
        sched.submit(request(1, &prompt, 4)).expect("submit");
        let mut resets = Vec::new();
        for _ in 0..3 {
            let LaneStep::Prefill { lane, tokens, resumed, .. } = sched.next_step() else {
                break;
            };
            resets.push(!resumed);
            sched.on_prefilled(lane, tokens.len(), 700);
        }
        assert_eq!(
            resets,
            vec![true, false, false],
            "reset on the first chunk only"
        );
    }

    #[test]
    fn a_lane_with_nothing_parked_resets_whatever_the_caller_asked_for() {
        // `reset_first: false` is a VETO, not an instruction. A caller cannot
        // know what a lane is holding — only the scheduler does — and honouring
        // "do not reset" over an empty or foreign lane decodes on top of
        // whatever was left behind.
        let mut sched = scheduler(2, 4);
        let mut hit = request(9, &[7, 8], 4);
        hit.reset_first = false;
        sched.submit(hit).expect("submit");
        sched.admit_pending();
        assert!(
            sched.reset_requested(0),
            "nothing was parked, so there is nothing to resume"
        );
        assert!(sched.reset_requested(1), "and an unclaimed lane always resets");
    }

    /// Park a lane by running its turn to completion, then bring the same
    /// conversation back with a longer prompt.
    fn park_then_return(
        sched: &mut LaneScheduler,
        session: &str,
        first: &[i32],
        second: &[i32],
    ) -> LaneStep {
        let mut a = request(1, first, 1);
        a.session = session.to_string();
        sched.submit(a).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 100);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        sched.on_decoded(&plan, &[101]);
        sched.reap();

        let mut b = request(2, second, 4);
        b.session = session.to_string();
        b.reset_first = false;
        sched.submit(b).expect("submit");
        sched.next_step()
    }

    #[test]
    fn a_conversation_that_comes_back_appends_instead_of_re_ingesting() {
        // The user's own words: "the first message is slow cause of the
        // context load but then it should just be an append."
        let mut sched = scheduler(2, 4);
        // The lane ends up holding [7, 8] (the prompt) + [100] (the token it
        // emitted). The next prompt extends all three.
        let step = park_then_return(&mut sched, "player-1", &[7, 8], &[7, 8, 100, 55, 66]);
        match step {
            LaneStep::Prefill { lane, start, tokens, kv_base, .. } => {
                assert_eq!(lane, 0, "it comes back to the lane that holds its tokens");
                assert_eq!(kv_base, 0);
                assert_eq!(start, 3, "and resumes at the position it left off");
                assert_eq!(tokens, vec![55, 66], "ingesting ONLY what is new");
            }
            other => panic!("expected a resumed prefill, got {other:?}"),
        }
        assert!(
            !sched.reset_requested(0),
            "resuming must not clear the state it is resuming from"
        );
    }

    #[test]
    fn a_prompt_that_does_not_extend_the_history_gets_a_clean_lane() {
        // Attention rows could be truncated to any prefix. The delta-net state
        // cannot — it is a running scan — so resuming at anything short of the
        // full history would decode against a recurrent state belonging to
        // tokens the prompt no longer contains. Fluent output, conversation
        // that never happened.
        let mut sched = scheduler(2, 4);
        let step = park_then_return(&mut sched, "player-1", &[7, 8], &[7, 9, 100, 55]);
        match step {
            LaneStep::Prefill { start, tokens, .. } => {
                assert_eq!(start, 0, "a diverging prompt starts over");
                assert_eq!(tokens, vec![7, 9, 100, 55], "and re-ingests everything");
            }
            other => panic!("expected a cold prefill, got {other:?}"),
        }
        assert!(sched.reset_requested(0));
    }

    #[test]
    fn a_prompt_equal_to_its_history_still_ingests_a_token() {
        // A lane must ingest at least one token to have something to decode
        // from. A zero-length delta would be admitted and then stall.
        let mut sched = scheduler(2, 4);
        let step = park_then_return(&mut sched, "player-1", &[7, 8], &[7, 8, 100]);
        match step {
            LaneStep::Prefill { start, tokens, .. } => {
                assert_eq!(start, 0);
                assert_eq!(tokens, vec![7, 8, 100]);
            }
            other => panic!("expected a cold prefill, got {other:?}"),
        }
    }

    /// A park is safe from a conversation that is INTERLEAVING with it — which
    /// is the case it was built for, and the case that still holds.
    ///
    /// It is not safe from a conversation arriving when the box is idle: that
    /// one takes slot 0, because slot 0 is the only lane that can speculate and
    /// a dormant cache is worth less than the live speaker's decode rate. See
    /// `a_conversation_arriving_alone_lands_where_it_can_speculate`. Here the
    /// first conversation is still LIVE when the second arrives, so its park is
    /// untouched — and the assertion below is that its append survives.
    #[test]
    fn another_conversation_cannot_steal_a_parked_append() {
        // THE reason this is per lane. One shared prefix belongs to whoever
        // spoke last, so a second conversation interleaving with a first takes
        // the first's append away — and the user reads that as "it was fast,
        // now it is slow, for no reason".
        let mut sched = scheduler(2, 4);
        let mut a = request(1, &[7, 8], 1);
        a.session = "player-1".to_string();
        sched.submit(a).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 100);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        // Somebody else arrives WHILE player 1 is still holding its lane, which
        // is what "interleaving" means and the only shape the park has to
        // survive.
        let mut b = request(2, &[40, 41], 1);
        b.session = "player-2".to_string();
        sched.submit(b).expect("submit");
        // ADMITTED while player 1 still holds its lane. That is the whole of
        // "interleaving": the second conversation is admitted next to a live
        // one, so the idle rule does not apply and the park is not a candidate.
        sched.admit_pending();
        sched.on_decoded(&plan, &[101]);
        sched.reap();
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        assert_ne!(lane, 0, "a new conversation must not take lane 0's parked tokens");
        sched.on_prefilled(lane, tokens.len(), 200);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        sched.on_decoded(&plan, &[201]);
        sched.reap();

        // Player 1 returns and STILL appends.
        let mut back = request(3, &[7, 8, 100, 55], 4);
        back.session = "player-1".to_string();
        back.reset_first = false;
        sched.submit(back).expect("submit");
        match sched.next_step() {
            LaneStep::Prefill { lane, start, tokens, .. } => {
                assert_eq!(lane, 0);
                assert_eq!(start, 3);
                assert_eq!(tokens, vec![55], "the interleaved turn took nothing away");
            }
            other => panic!("expected a resumed prefill, got {other:?}"),
        }
    }

    #[test]
    fn each_lane_samples_with_its_own_settings() {
        // Temperature and seed belong to the caller. Two chats sharing one
        // setting would silently sample with each other's — the same
        // shared-state class as the recurrent row and the carry ring.
        let mut sched = scheduler(2, 4);
        let mut hot = request(1, &[1], 4);
        hot.sampling.temperature = 1.3;
        hot.sampling.seed = 111;
        let mut cold = request(2, &[1], 4);
        cold.sampling.temperature = 0.0;
        cold.sampling.seed = 222;
        sched.submit(hot).expect("submit");
        sched.submit(cold).expect("submit");
        sched.admit_pending();

        let a = sched.sampling_for(0).expect("lane 0 params");
        let b = sched.sampling_for(1).expect("lane 1 params");
        assert_eq!(a.temperature, 1.3);
        assert_eq!(a.seed, 111);
        assert_eq!(b.temperature, 0.0);
        assert_eq!(b.seed, 222);
        assert!(sched.sampling_for(9).is_none(), "no lane, no settings");
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

    #[test]
    fn a_speculative_round_emits_every_token_it_committed() {
        // A round commits between one and depth+1 tokens per lane, and it
        // commits the token the lane was ALREADY decoding first. Emitting the
        // previous one instead — the batched path's contract — would drop the
        // first token of every generation.
        let mut sched = spec_scheduler(2, 4);
        prefilled(&mut sched, 1, &[10, 11], 16, 100);
        prefilled(&mut sched, 2, &[20, 21], 16, 200);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        assert_eq!(plan.width, 2, "both lanes are decoding");
        let events = sched
            .on_speculated(
                &plan,
                &[
                    outcome(&[100, 101, 102], 103, 2, 3),
                    outcome(&[200], 201, 0, 3),
                ],
            )
            .expect("round");
        let lane_a: Vec<i32> = events
            .iter()
            .filter_map(|e| match e {
                LaneEvent::Token { job: 1, token, .. } => Some(*token),
                _ => None,
            })
            .collect();
        let lane_b: Vec<i32> = events
            .iter()
            .filter_map(|e| match e {
                LaneEvent::Token { job: 2, token, .. } => Some(*token),
                _ => None,
            })
            .collect();
        assert_eq!(lane_a, vec![100, 101, 102], "three tokens for one verify");
        assert_eq!(lane_b, vec![200], "a lane whose drafts all missed still moves");

        // And the next step feeds each lane the token its OWN round drew.
        let LaneStep::Decode { plan, tokens } = sched.next_step() else {
            panic!("expected decode");
        };
        let by_slot: Vec<(usize, i32)> = plan
            .slots
            .iter()
            .zip(&tokens)
            .map(|(step, token)| (step.slot, *token))
            .collect();
        assert_eq!(by_slot, vec![(0, 103), (1, 201)]);
        assert_eq!(plan.slots[0].position, 5, "2 prompt + 3 committed");
        assert_eq!(plan.slots[1].position, 3, "2 prompt + 1 committed");
    }

    #[test]
    fn a_round_moves_the_resume_row_and_the_draft_head_of_its_own_lane() {
        // Both are per-lane facts the next round reads back. A lane that took
        // its neighbour's resume row would decode against a recurrent state
        // belonging to a different conversation — fluently.
        let mut sched = spec_scheduler(2, 4);
        prefilled(&mut sched, 1, &[10, 11], 16, 100);
        prefilled(&mut sched, 2, &[20, 21], 16, 200);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        sched
            .on_speculated(
                &plan,
                &[outcome(&[100, 101], 102, 1, 3), outcome(&[200], 201, 0, 3)],
            )
            .expect("round");
        assert_eq!(sched.slot(0).expect("slot 0").live_state_offset(), 1);
        assert_eq!(sched.slot(1).expect("slot 1").live_state_offset(), 0);
        assert_eq!(sched.slot(0).expect("slot 0").mtp_filled(), 3);
        assert_eq!(sched.slot(1).expect("slot 1").mtp_filled(), 3);
        // The state row the NEXT plan hands the graph is inside each slot's own
        // block, never the neighbour's.
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        assert_eq!(plan.slots[0].state_row, 1, "slot 0 resumes at its own offset 1");
        assert_eq!(plan.slots[1].state_row, crate::slots::SOLO_DRAFT_MAX + 2);
    }

    #[test]
    fn a_round_that_overruns_the_budget_still_records_what_the_cache_holds() {
        // The tokens are in the KV whether or not the caller wanted them, and a
        // parked lane's history has to describe its caches EXACTLY. One token
        // short and the conversation comes back, appends at the wrong offset,
        // and decodes the rest of its life one position out.
        let mut sched = spec_scheduler(2, 4);
        let mut req = request(1, &[10, 11], 2);
        req.session = "player-1".to_string();
        sched.submit(req).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 100);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        let events = sched
            .on_speculated(&plan, &[outcome(&[100, 101, 102], 103, 2, 3)])
            .expect("round");
        let emitted: Vec<i32> = events
            .iter()
            .filter_map(|e| match e {
                LaneEvent::Token { token, .. } => Some(*token),
                _ => None,
            })
            .collect();
        assert_eq!(emitted, vec![100, 101], "the caller asked for two");
        assert_eq!(
            sched.slot(0).expect("slot").fill(),
            5,
            "but the cache holds all five"
        );
        sched.reap();

        // Coming back, the history it must extend is the CACHE's, not the
        // caller's: prompt + all three committed tokens.
        let mut back = request(2, &[10, 11, 100, 101, 102, 55], 4);
        back.session = "player-1".to_string();
        back.reset_first = false;
        sched.submit(back).expect("submit");
        match sched.next_step() {
            LaneStep::Prefill { lane, start, tokens, .. } => {
                assert_eq!(lane, 0, "back to the lane holding its tokens");
                assert_eq!(start, 5);
                assert_eq!(tokens, vec![55], "ingesting only what is new");
            }
            other => panic!("expected a resumed prefill, got {other:?}"),
        }
    }

    #[test]
    fn a_round_out_of_step_with_its_plan_is_refused() {
        // Outcomes are matched to lanes positionally. If they ever slip, every
        // lane below the slip is handed another conversation's tokens — so this
        // fails loudly rather than emitting them.
        let mut sched = spec_scheduler(2, 4);
        prefilled(&mut sched, 1, &[10, 11], 16, 100);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        let error = sched
            .on_speculated(&plan, &[outcome(&[999], 1000, 0, 2)])
            .expect_err("a round starting on the wrong token must be refused");
        assert!(error.contains("out of step"), "{error}");
        let short = sched
            .on_speculated(&plan, &[])
            .expect_err("an outcome per lane, or none of it is trustworthy");
        assert!(short.contains("reported 0 outcomes"), "{short}");
    }

    #[test]
    fn a_round_may_not_commit_a_stop_token() {
        // The turn ends at the token BEFORE its end-of-turn marker; the round
        // is supposed to have dropped it. If one arrives anyway, emitting it
        // would print `<|im_end|>` into a player's chat.
        let mut sched = spec_scheduler(2, 4);
        prefilled(&mut sched, 1, &[10, 11], 16, 100);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        let error = sched
            .on_speculated(&plan, &[outcome(&[100, EOS], 101, 1, 3)])
            .expect_err("a committed stop token must be refused");
        assert!(error.contains("stop token"), "{error}");
    }

    #[test]
    fn a_round_that_stopped_ends_the_turn_and_frees_the_lane() {
        let mut sched = spec_scheduler(2, 4);
        prefilled(&mut sched, 1, &[10, 11], 16, 100);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        let mut done = outcome(&[100, 101], EOS, 1, 3);
        done.stop_reason = Some(crate::LlamaStopReason::EndOfSequence);
        sched.on_speculated(&plan, &[done]).expect("round");
        let events = sched.reap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LaneEvent::Finished { job: 1, .. })),
            "a stopped round must retire its lane rather than decode past the turn"
        );
        assert_eq!(sched.lanes_active(), 0);
    }

    #[test]
    fn a_reset_forgets_every_parked_conversation() {
        // `LlamaSession::reset` clears the WHOLE device cache, not one
        // sequence's share of it. A parked lane still believed resumable
        // afterwards appends its next delta onto zeros and answers, fluently,
        // about a conversation that did not happen.
        let mut sched = spec_scheduler(2, 4);
        let mut first = request(1, &[7, 8], 1);
        first.session = "player-1".to_string();
        sched.submit(first).expect("submit");
        let LaneStep::Prefill { lane, tokens, .. } = sched.next_step() else {
            panic!("expected prefill");
        };
        sched.on_prefilled(lane, tokens.len(), 100);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        sched.on_decoded(&plan, &[101]);
        sched.reap();

        sched.invalidate_parked();

        let mut back = request(2, &[7, 8, 100, 55], 4);
        back.session = "player-1".to_string();
        back.reset_first = false;
        sched.submit(back).expect("submit");
        match sched.next_step() {
            LaneStep::Prefill { start, tokens, .. } => {
                assert_eq!(start, 0, "the caches it would have resumed are zeros now");
                assert_eq!(tokens, vec![7, 8, 100, 55], "so it re-ingests everything");
            }
            other => panic!("expected a cold prefill, got {other:?}"),
        }
    }

    #[test]
    fn a_lane_cannot_adopt_a_resume_row_from_another_slot() {
        // The solo path hands its resume row over by number. A number past the
        // slot's own block is the neighbour's state, and reading it is silent.
        let mut sched = spec_scheduler(2, 4);
        prefilled(&mut sched, 1, &[10, 11], 16, 100);
        let rows = crate::slots::SOLO_DRAFT_MAX + 2;
        sched
            .adopt_native_state(0, rows - 1, 2)
            .expect("the last row of its own block is its own");
        let error = sched
            .adopt_native_state(0, rows, 2)
            .expect_err("one past the block is the next slot's state");
        assert!(error.contains("another slot's state"), "{error}");
        // A draft head that looks AHEAD of the model is handled, not refused,
        // and the difference matters: the resume row above changes what the
        // model computes, while the draft fill only changes how often a
        // proposal survives. Speculative rejection sampling emits exactly the
        // target distribution whatever the drafter says.
        //
        // This used to assert an error. It reached a player as
        // `slot 0 would put its draft head at 8366 tokens, ahead of the
        // model's 1024` on every ~20k prompt, because a slot changing hands
        // carries the previous occupant's fill across. Restarting the draft
        // head costs one catch-up; failing the turn costs the turn.
        sched
            .adopt_native_state(0, 0, 99)
            .expect("a stale draft fill must not fail the handover");
        assert_eq!(
            sched.slot(0).expect("slot").mtp_filled(),
            0,
            "restart from zero, not from the model's fill: the rows below it \
             hold the previous occupant's tokens too"
        );
    }

    #[test]
    fn a_stale_decode_plan_is_refused_rather_than_decoded() {
        // A plan built before something else advanced a slot addresses the
        // right lane at the wrong position: it writes a token over a token and
        // reads a mask one row short. No error, just a conversation that starts
        // repeating itself.
        let mut sched = spec_scheduler(2, 4);
        prefilled(&mut sched, 1, &[10, 11], 16, 100);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        sched
            .on_speculated(&plan, &[outcome(&[100, 101], 102, 1, 3)])
            .expect("round");
        // The same plan again: the slot has moved on.
        let error = sched
            .spec_lanes(&plan)
            .expect_err("a stale plan must be refused");
        assert!(error.contains("stale"), "{error}");
    }

    #[test]
    fn a_round_reads_each_lanes_own_history_and_geometry() {
        let mut sched = spec_scheduler(2, 4);
        prefilled(&mut sched, 1, &[10, 11], 16, 100);
        prefilled(&mut sched, 2, &[20, 21, 22], 16, 200);
        let LaneStep::Decode { plan, .. } = sched.next_step() else {
            panic!("expected decode");
        };
        let lanes = sched.spec_lanes(&plan).expect("lanes");
        assert_eq!(lanes.len(), 2);
        assert_eq!(lanes[0].tokens, &[10, 11]);
        assert_eq!(lanes[1].tokens, &[20, 21, 22]);
        assert_eq!(lanes[0].first, 100, "each lane starts from its own token");
        assert_eq!(lanes[1].first, 200);
        assert_eq!(lanes[0].kv_base, 0);
        assert_eq!(lanes[1].kv_base, 512, "slot-major, one per-slot context apart");
        assert_eq!(lanes[0].state_base, 0);
        assert_eq!(lanes[1].state_base, crate::slots::SOLO_DRAFT_MAX + 2);
        // The invariant a round validates for itself: a lane's history is
        // exactly what its cache holds.
        for lane in &lanes {
            assert_eq!(lane.tokens.len(), lane.fill);
        }
    }
}
