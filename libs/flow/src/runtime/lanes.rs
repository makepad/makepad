//! Run lanes: the long-lived loops that execute runs for the engine.
//!
//! A lane owns every [`RunTask`] it was handed, creates each on its own
//! thread (a task's Splash VM is thread-affine) and steps them cooperatively.
//! It never sleeps while a run can progress. A full event channel holds the
//! run back until the engine drains it, behind a bounded outbox: what one
//! synchronous `start` or `step` says while the channel is full waits there,
//! up to `OUTBOX_MAX` events, `OUTBOX_META_MAX` of metadata and the engine's
//! media budget of values, and the channel itself carries at most the
//! engine's `lane_max_bytes` at a time. No budget ever refuses the lone
//! event at the head of a queue or of an empty channel, so a run always
//! progresses. Every event is first cut to the per-event telemetry limits;
//! semantic fields are never cut, an event they make too large is refused
//! explicitly. Past the outbox bounds, progress and deltas are dropped and
//! counted; a lifecycle event past them ends the run instead, so the run's
//! terminal `RunFinished` (the one event a run may always add, with its own
//! bounded summary) reports the loss rather than the store silently missing
//! a node's outcome.
//!
//! These are job-domain workers. Provider executors may still make
//! synchronous network calls on them; a lane is not a realtime owner.
//!
//! A run that panics is finished as `Failed` and the lane carries on. If the
//! lane itself ends, however it ends, it raises its `exited` flag. The flag
//! is the lossless signal (the event channel may be full at that moment);
//! the engine reads it every turn, fails the lane's remaining runs and stops
//! dispatching to it.

use crate::engine::results::ResultStore;
use crate::engine::{NetPolicy, RunEvent, RunId, RunInput, RunState, RunStep, RunTask, Seams, EVENT_META_MAX};
use crate::instance::InstanceId;
use crate::values::Value;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::time::Duration;

/// How long a lane parks when none of its runs can progress. An answer, a
/// cancel flag or an executor result is noticed within this interval.
const IDLE_PARK: Duration = Duration::from_millis(20);
/// Most answers taken for one run per turn. An instance parks on one Ask at
/// a time, so more than a few means a misbehaving caller.
const ANSWER_BUDGET: usize = 8;
/// Most events one run holds back for the engine. One `step` emits at most a
/// few events per node it touched, so this is far above any graph the
/// source-size limit admits; it is the bound, not the expectation. Values
/// travel as `Arc` bytes, so an entry is a small record.
const OUTBOX_MAX: usize = 4_096;
/// Most metadata bytes one run holds back for the engine, behind the head
/// of its queue. Media bytes (values) have their own budget, the engine's
/// `run_media_max_bytes`: values travel as shared `Arc` bytes but count in
/// full while queued, because a queued event keeps them alive.
const OUTBOX_META_MAX: usize = 16 * 1024 * 1024;

/// Who a run belongs to; shared by every event of that run.
pub(crate) struct RunTag {
    pub lane: usize,
    pub run_id: RunId,
    pub instance: InstanceId,
    pub flow: String,
}

pub(crate) struct LaneEvent {
    pub tag: Arc<RunTag>,
    pub event: RunEvent,
    /// `event.payload_bytes()`, counted in the lane channel's byte budget
    /// until the engine takes the event.
    pub bytes: usize,
}

pub(crate) struct LaneStart {
    pub tag: Arc<RunTag>,
    pub input: RunInput,
    pub cancel: Arc<AtomicBool>,
    pub answers: Receiver<(String, Value)>,
}

pub(crate) struct LaneContext {
    /// Raised when the lane loop ends, for any reason.
    pub exited: Arc<AtomicBool>,
    pub commands: Receiver<LaneStart>,
    pub events: SyncSender<LaneEvent>,
    /// Payload bytes in the event channel right now, every lane together;
    /// the engine subtracts what it takes.
    pub lane_bytes: Arc<AtomicUsize>,
    pub lane_max_bytes: usize,
    /// Most value bytes one event, and one run's queued events, may carry.
    pub media_max_bytes: usize,
    /// Rung after events were queued so the engine worker wakes at once.
    pub doorbell: SyncSender<()>,
    pub seams: Seams,
    pub net: NetPolicy,
    pub results: Option<Arc<dyn ResultStore>>,
}

struct LaneRun {
    tag: Arc<RunTag>,
    /// `None` once the run finished (its last events may still be in `outbox`).
    task: Option<RunTask>,
    answers: Receiver<(String, Value)>,
    outbox: Outbox,
}

/// Events a run said that the engine has not taken yet, bounded in count,
/// in metadata bytes and in media bytes. The head of the queue is never
/// refused for lack of room: a budget bounds what waits behind it.
struct Outbox {
    queue: VecDeque<(RunEvent, usize, usize)>,
    queued_meta: usize,
    queued_media: usize,
    /// The run's cancel flag: raised when a lifecycle event had no room.
    cancel: Arc<AtomicBool>,
    /// Progress and deltas dropped for lack of room.
    dropped_observations: u64,
    /// Lifecycle events dropped for lack of room; the run was cancelled
    /// when the first was.
    dropped_lifecycle: u64,
    /// Bytes cut from telemetry to fit the per-event limits.
    cut_bytes: usize,
}

impl Outbox {
    fn new(cancel: Arc<AtomicBool>) -> Self {
        Self {
            queue: VecDeque::new(),
            queued_meta: 0,
            queued_media: 0,
            cancel,
            dropped_observations: 0,
            dropped_lifecycle: 0,
            cut_bytes: 0,
        }
    }

    fn has_room(&self, meta: usize, media: usize, media_max: usize) -> bool {
        self.queue.is_empty()
            || (self.queue.len() < OUTBOX_MAX
                && self.queued_meta + meta <= OUTBOX_META_MAX
                && self.queued_media + media <= media_max)
    }

    fn push(&mut self, event: RunEvent, meta: usize, media: usize) {
        self.queued_meta += meta;
        self.queued_media += media;
        self.queue.push_back((event, meta, media));
    }

    fn push_front(&mut self, event: RunEvent, meta: usize, media: usize) {
        self.queued_meta += meta;
        self.queued_media += media;
        self.queue.push_front((event, meta, media));
    }

    fn pop_front(&mut self) -> Option<(RunEvent, usize, usize)> {
        let (event, meta, media) = self.queue.pop_front()?;
        self.queued_meta -= meta;
        self.queued_media -= media;
        Some((event, meta, media))
    }

    /// The engine's notes for the terminal event: what this run lost.
    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if self.dropped_lifecycle > 0 {
            notes.push(format!(
                "the run's event backlog overflowed: {} lifecycle events were lost and the run was cancelled",
                self.dropped_lifecycle
            ));
        }
        if self.dropped_observations > 0 {
            notes.push(format!("{} progress or delta events were dropped under backlog", self.dropped_observations));
        }
        if self.cut_bytes > 0 {
            notes.push(format!("{} bytes of event text were cut to the per-event limits", self.cut_bytes));
        }
        notes
    }
}

/// Take the channel's byte budget for one event, or say it is full. An
/// empty channel takes any admitted event, however large: a budget bounds
/// what queues behind an event, never whether the event can travel at all.
fn reserve_lane_bytes(context: &LaneContext, bytes: usize) -> bool {
    context
        .lane_bytes
        .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current == 0 || current + bytes <= context.lane_max_bytes).then_some(current + bytes)
        })
        .is_ok()
}

/// A single event past the per-event limits (metadata after its telemetry
/// was cut, or media past the admission budget). Nothing semantic is
/// trimmed: progress is dropped; a node's result becomes that node's
/// failure, said in bounded words, and the run is cancelled; a run's plan
/// or an ask's choices that cannot be carried end the run; a terminal event
/// keeps its identity, loses its payload and says so.
fn refuse_oversize(event: RunEvent, meta: usize, media: usize, media_max: usize, outbox: &mut Outbox) -> Option<RunEvent> {
    let why = if media > media_max {
        format!("{media} bytes of values exceed the media admission budget of {media_max} bytes")
    } else {
        format!("{meta} bytes of event metadata exceed the event limit of {EVENT_META_MAX} bytes")
    };
    match event {
        RunEvent::NodeProgress { .. } | RunEvent::NodeDelta { .. } => {
            outbox.dropped_observations += 1;
            None
        }
        RunEvent::NodeDone { node, .. } => {
            outbox.cancel.store(true, Ordering::SeqCst);
            Some(RunEvent::NodeFailed { node, error: format!("the node's outputs were refused: {why}") })
        }
        RunEvent::NodeWaiting { node, options, .. } => {
            outbox.cancel.store(true, Ordering::SeqCst);
            Some(RunEvent::NodeFailed {
                node,
                error: format!("the ask's {} options were refused: {why}", options.len()),
            })
        }
        RunEvent::RunStarted { planned_nodes, .. } => {
            outbox.cancel.store(true, Ordering::SeqCst);
            Some(RunEvent::RunFinished {
                state: RunState::Failed,
                secs: 0.0,
                outputs: Vec::new(),
                http_log: Vec::new(),
                warnings: vec![format!("the run's plan of {} nodes was refused: {why}", planned_nodes.len())],
            })
        }
        RunEvent::RunFinished { secs, mut warnings, .. } => {
            warnings.push(format!("the run's outputs were not delivered: {why}"));
            Some(RunEvent::RunFinished {
                state: RunState::Failed,
                secs,
                outputs: Vec::new(),
                http_log: Vec::new(),
                warnings,
            })
        }
        RunEvent::NodeStarted { .. }
        | RunEvent::NodeAnswered { .. }
        | RunEvent::NodeFailed { .. }
        | RunEvent::NodeSkipped { .. } => {
            // Bounded texts and ids from a size-limited source: these cannot
            // reach the limit. If one ever does, its loss is a lifecycle
            // loss, reported by the terminal event.
            outbox.dropped_lifecycle += 1;
            outbox.cancel.store(true, Ordering::SeqCst);
            None
        }
    }
}

/// Raises the lane's `exited` flag when the loop ends, unwinding included.
/// The doorbell only hurries the engine; the flag is what it relies on.
struct ExitNotice {
    exited: Arc<AtomicBool>,
    doorbell: SyncSender<()>,
}

impl Drop for ExitNotice {
    fn drop(&mut self) {
        self.exited.store(true, Ordering::SeqCst);
        let _ = self.doorbell.try_send(());
    }
}

fn panicked(what: &str) -> RunEvent {
    RunEvent::RunFinished {
        state: RunState::Failed,
        secs: 0.0,
        outputs: Vec::new(),
        http_log: Vec::new(),
        warnings: vec![format!("the run panicked while {what}")],
    }
}

/// Hand one event to the engine at once, or queue it behind what is already
/// waiting so order is kept. Starting or stepping a run can take a while
/// (loading its source, a slow executor); what it already said, its
/// `RunStarted` first of all, must not wait for the call to return.
fn emit(context: &LaneContext, tag: &Arc<RunTag>, outbox: &mut Outbox, event: RunEvent) {
    let mut event = event;
    // Telemetry limits first: no event carries unbounded text or logs.
    outbox.cut_bytes += event.bound_telemetry();
    let mut meta = event.meta_bytes();
    let mut media = event.media_bytes();
    if meta > EVENT_META_MAX || media > context.media_max_bytes {
        let Some(refused) = refuse_oversize(event, meta, media, context.media_max_bytes, outbox) else {
            return;
        };
        event = refused;
        meta = event.meta_bytes();
        media = event.media_bytes();
    }
    if matches!(event, RunEvent::RunFinished { .. }) {
        // The terminal event always goes through, and it says what was
        // lost; its notes are bounded with it.
        let notes = outbox.notes();
        if outbox.dropped_lifecycle > 0 {
            if let RunEvent::RunFinished { state, .. } = &mut event {
                *state = RunState::Failed;
            }
        }
        event.finish_terminal(notes);
        meta = event.meta_bytes();
    } else if !outbox.has_room(meta, media, context.media_max_bytes) {
        if event.is_lifecycle() {
            // A lifecycle event with no room: the store would never learn
            // this node's outcome, so the run ends instead, and its
            // `RunFinished` carries the count.
            outbox.dropped_lifecycle += 1;
            outbox.cancel.store(true, Ordering::SeqCst);
        } else {
            outbox.dropped_observations += 1;
        }
        return;
    }
    let bytes = meta + media;
    if !outbox.queue.is_empty() || !reserve_lane_bytes(context, bytes) {
        outbox.push(event, meta, media);
        return;
    }
    match context.events.try_send(LaneEvent { tag: tag.clone(), event, bytes }) {
        Ok(()) => {
            let _ = context.doorbell.try_send(());
        }
        // A full channel holds the run back; a closed one ends the lane at
        // its next delivery attempt.
        Err(TrySendError::Full(returned)) | Err(TrySendError::Disconnected(returned)) => {
            context.lane_bytes.fetch_sub(bytes, Ordering::AcqRel);
            outbox.push(returned.event, meta, media);
        }
    }
}

fn start_run(context: &LaneContext, start: LaneStart) -> LaneRun {
    let LaneStart { tag, input, cancel, answers } = start;
    let mut outbox = Outbox::new(cancel.clone());
    let task = catch_unwind(AssertUnwindSafe(|| {
        RunTask::start(
            input,
            context.seams.clone(),
            context.net.clone(),
            cancel,
            context.results.clone(),
            &mut |event| emit(context, &tag, &mut outbox, event),
        )
    }));
    let task = match task {
        Ok(task) => task,
        Err(_) => {
            emit(context, &tag, &mut outbox, panicked("starting"));
            None
        }
    };
    LaneRun { tag, task, answers, outbox }
}

/// The lane loop. Returns when the engine dropped the lane's command sender
/// and every run it held has finished and delivered its events, or when the
/// engine is gone.
pub(crate) fn lane_loop(context: LaneContext) {
    let _notice = ExitNotice {
        exited: context.exited.clone(),
        doorbell: context.doorbell.clone(),
    };
    let mut runs: Vec<LaneRun> = Vec::new();
    let mut closed = false;
    loop {
        // Accept new runs. The engine admits at most the lane's capacity.
        loop {
            match context.commands.try_recv() {
                Ok(start) => runs.push(start_run(&context, start)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }

        let mut progressed = false;
        for run in &mut runs {
            // Deliver in order; a full channel (or byte budget) holds this
            // run back.
            let mut delivered = false;
            while let Some((event, meta, media)) = run.outbox.pop_front() {
                let bytes = meta + media;
                if !reserve_lane_bytes(&context, bytes) {
                    run.outbox.push_front(event, meta, media);
                    break;
                }
                match context.events.try_send(LaneEvent { tag: run.tag.clone(), event, bytes }) {
                    Ok(()) => delivered = true,
                    Err(TrySendError::Full(returned)) => {
                        context.lane_bytes.fetch_sub(bytes, Ordering::AcqRel);
                        run.outbox.push_front(returned.event, meta, media);
                        break;
                    }
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
            if delivered {
                // Rung before the next step, not after the pass: a step may
                // take a while, and what is already delivered (a run's
                // `RunStarted`, say) must not wait behind it.
                let _ = context.doorbell.try_send(());
            }
            if !run.outbox.queue.is_empty() {
                continue;
            }
            let Some(task) = run.task.as_mut() else {
                continue;
            };
            for (node, value) in run.answers.try_iter().take(ANSWER_BUDGET) {
                task.answer(node, value);
            }
            let outbox = &mut run.outbox;
            let tag = &run.tag;
            let step = catch_unwind(AssertUnwindSafe(|| {
                task.step(&mut |event| emit(&context, tag, outbox, event))
            }));
            match step {
                Ok(RunStep::Progressed) => progressed = true,
                Ok(RunStep::Idle) => {}
                Ok(RunStep::Finished) => {
                    progressed = true;
                    run.task = None;
                }
                Err(_) => {
                    progressed = true;
                    run.task = None;
                    emit(&context, &run.tag, &mut run.outbox, panicked("stepping"));
                }
            }
        }
        runs.retain(|run| run.task.is_some() || !run.outbox.queue.is_empty());

        if closed && runs.is_empty() {
            return;
        }
        if progressed {
            continue;
        }
        if closed {
            std::thread::sleep(IDLE_PARK);
            continue;
        }
        // Park until a new run arrives or the interval elapses.
        match context.commands.recv_timeout(IDLE_PARK) {
            Ok(start) => runs.push(start_run(&context, start)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => closed = true,
        }
    }
}
