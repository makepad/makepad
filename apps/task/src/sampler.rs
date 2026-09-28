//! The sampler worker: one long-lived thread that owns the OS backend and
//! the on-disk journal. It takes commands over a bounded channel (the UI
//! `try_send`s and retries), publishes `Arc`-shared samples and detail over a
//! bounded channel (the UI `try_recv`s on `Event::Signal`), and never blocks
//! the UI: a full outbox is retried next tick, never awaited.
//!
//! Start-up loads the journal in bounded batches that are streamed to the UI
//! between ticks, so a day of history does not stall sampling or flood one
//! frame. Termination re-verifies the process identity here, right before
//! the signal.
//!
//! Beyond the basic sample the worker records richer reads (see `supp.rs`)
//! for the inspected process — every detail read it already makes — and,
//! in the background, for pinned processes: memory ledger, counters and
//! thread CPU every [`PIN_EVERY_MS`], the descriptor list every
//! [`PIN_FILES_EVERY_MS`], never the address-space walk. At most
//! [`PIN_READS_PER_TICK`] pinned reads run per tick, oldest due first, and
//! they stop for the tick once [`PIN_BUDGET_SECS`] is spent, so 32 pins
//! stretch their cadence rather than the tick; the expected spacing that
//! gap detection uses stretches with them. A pinned process that is also
//! inspected is read once, at the inspector's cadence. Unpinning, or the
//! process leaving the process list, stops its extra reads.

use crate::backend::{self, ProcDetail, ProcKey, Want};
use crate::history::Sample;
use crate::persist::{Journal, PinRecord, SuppJournal};
use crate::supp::{Recorder, SuppEvent};
use std::collections::HashMap;
use makepad_widgets::log;
use makepad_widgets::makepad_platform::thread::{
    CancellationToken, SignalToUI, SpawnError, TaskHandle, ThreadOptions, ThreadSpawner,
};
use makepad_widgets::Cx;
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;

/// Floor on the tick period, whatever the UI asks for.
const MIN_INTERVAL_SECS: f64 = 0.1;
/// How long the wait can go without re-reading commands, so switching from
/// 10 s down to 0.5 s is felt immediately instead of after the current wait.
const WAIT_SLICE_SECS: f64 = 0.05;
/// Detail for the inspected process is refreshed at most this often.
const DETAIL_EVERY_MS: u64 = 1_000;
/// The mapped-file walk is the expensive part; it runs this often.
const LIBRARIES_EVERY_MS: u64 = 10_000;
const COMMAND_CAPACITY: usize = 16;
const MESSAGE_CAPACITY: usize = 32;
/// Messages the worker holds while the UI channel is full. Samples past this
/// are dropped from the UI's copy; the journal keeps at most one a second.
const OUTBOX_CAPACITY: usize = 64;
/// Commands the UI keeps while the command channel is full.
const PENDING_CAPACITY: usize = 8;
/// A worker that overran its interval still yields this long before the next
/// sample, so a slow backend cannot pin a core.
const OVERRUN_YIELD_SECS: f64 = 0.02;
/// Background reads of a pinned process: metrics and threads, descriptors.
pub const PIN_EVERY_MS: u64 = 2_000;
pub const PIN_FILES_EVERY_MS: u64 = 10_000;
pub const PIN_READS_PER_TICK: usize = 4;
pub const PIN_BUDGET_SECS: f64 = 0.025;
/// Supplemental events one message carries while the journal is streamed,
/// and the most the outbox holds for a UI that stopped reading.
const SUPP_LOAD_EVENTS: usize = 20_000;
const SUPP_OUTBOX_BYTES: usize = 16 * 1024 * 1024;
/// One supplemental message carries at most about this much, so the
/// channel's 32 messages stay bounded too.
const SUPP_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
/// The inspected process' thread list follows the sampling tick, so its
/// per-thread CPU covers the same intervals as the process', but never
/// faster than this.
const THREADS_EVERY_MIN_MS: u64 = 250;

/// UI → worker.
#[derive(Clone, Debug)]
pub enum Command {
    Interval(u64),
    /// Which process the inspector wants detail for; `None` stops collecting.
    Inspect(Option<ProcKey>),
    Terminate { key: ProcKey, force: bool },
    SavePins(Vec<PinRecord>),
    /// The table's columns, as `GridColumns::serialize` writes them.
    SaveColumns(String),
}

/// Worker → UI.
#[derive(Debug)]
pub enum Message {
    Sample(Arc<Sample>),
    /// A batch read from disk, oldest first.
    Loaded(Vec<Arc<Sample>>),
    Pins(Vec<PinRecord>),
    /// The table's saved columns.
    Columns(String),
    Detail(Arc<ProcDetail>),
    /// Supplemental events, recorded now or read back from the sidecar.
    Supp(Vec<SuppEvent>),
    Terminated { key: ProcKey, force: bool, result: Result<(), String> },
    Status(String),
}

/// The UI's handle: bounded, non-blocking both ways.
pub struct Worker {
    commands: SyncSender<Command>,
    messages: Receiver<Message>,
    /// Commands the channel had no room for, resent on the next poll.
    pending: VecDeque<Command>,
    _task: TaskHandle<()>,
}

impl Worker {
    pub fn spawn(spawner: &ThreadSpawner, initial_interval_ms: u64) -> Result<Self, SpawnError> {
        let (commands, command_rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (message_tx, messages) = mpsc::sync_channel(MESSAGE_CAPACITY);
        let task = spawner.spawn_worker(
            ThreadOptions { name: Some("task-sampler".into()), ..Default::default() },
            move || run(command_rx, message_tx, initial_interval_ms),
        )?;
        Ok(Self { commands, messages, pending: VecDeque::new(), _task: task })
    }

    /// Queue a command; it goes out now if the channel has room, else on a
    /// later `poll`. A newer command of the same kind replaces an older one
    /// still waiting (a repeated End press is one request, not a queue), and
    /// the queue is bounded: `false` means it was full and the command was
    /// not taken.
    pub fn send(&mut self, command: Command) -> bool {
        match &command {
            Command::Interval(_) => self.pending.retain(|c| !matches!(c, Command::Interval(_))),
            Command::Inspect(_) => self.pending.retain(|c| !matches!(c, Command::Inspect(_))),
            Command::SavePins(_) => self.pending.retain(|c| !matches!(c, Command::SavePins(_))),
            Command::SaveColumns(_) => self.pending.retain(|c| !matches!(c, Command::SaveColumns(_))),
            Command::Terminate { .. } => self.pending.retain(|c| !matches!(c, Command::Terminate { .. })),
        }
        if self.pending.len() >= PENDING_CAPACITY {
            self.flush();
            if self.pending.len() >= PENDING_CAPACITY {
                return false;
            }
        }
        self.pending.push_back(command);
        self.flush();
        true
    }

    fn flush(&mut self) {
        while let Some(command) = self.pending.pop_front() {
            match self.commands.try_send(command) {
                Ok(()) => {}
                Err(TrySendError::Full(command)) => {
                    self.pending.push_front(command);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.pending.clear();
                    break;
                }
            }
        }
    }

    /// Everything the worker sent since the last poll. Also retries queued
    /// commands, so a full channel needs no timer of its own.
    pub fn poll(&mut self) -> Vec<Message> {
        self.flush();
        let mut out = Vec::new();
        while let Ok(message) = self.messages.try_recv() {
            out.push(message);
        }
        out
    }
}

/// The worker's outbox: messages wait here when the UI channel is full.
struct Outbox {
    tx: SyncSender<Message>,
    queue: VecDeque<Message>,
    dropped: u64,
    /// Supplemental events were dropped for a UI that stopped reading: a
    /// checkpoint of the open descriptor records follows.
    supp_dropped: bool,
}

impl Outbox {
    fn push(&mut self, message: Message) {
        // Only the newest detail and status matter to a UI that fell behind.
        match &message {
            Message::Detail(_) => self.queue.retain(|m| !matches!(m, Message::Detail(_))),
            Message::Status(_) => self.queue.retain(|m| !matches!(m, Message::Status(_))),
            _ => {}
        }
        // Supplemental events join one waiting message instead of queueing
        // many; a UI that stopped reading loses the oldest (the sidecar
        // keeps them).
        let message = match message {
            Message::Supp(events) => match self.queue.iter_mut().rev().find_map(|m| if let Message::Supp(queued) = m { Some(queued) } else { None }) {
                Some(queued) => {
                    queued.extend(events);
                    let mut bytes: usize = queued.iter().map(|e| e.approx_bytes()).sum();
                    if bytes > SUPP_OUTBOX_BYTES {
                        let mut excess = 0;
                        while bytes > SUPP_OUTBOX_BYTES / 2 && excess < queued.len() {
                            bytes -= queued[excess].approx_bytes();
                            excess += 1;
                        }
                        queued.drain(..excess);
                        self.supp_dropped = true;
                        log!("task: UI not reading; {excess} supplemental events left to the sidecar only");
                    }
                    self.flush();
                    return;
                }
                None => {
                    // A message alone over its bound keeps its newest events
                    // (the sidecar has the rest) and a checkpoint follows.
                    let mut events = events;
                    let mut bytes: usize = events.iter().map(|e| e.approx_bytes()).sum();
                    if bytes > SUPP_MESSAGE_BYTES {
                        let mut excess = 0;
                        while bytes > SUPP_MESSAGE_BYTES && excess < events.len() {
                            bytes -= events[excess].approx_bytes();
                            excess += 1;
                        }
                        events.drain(..excess);
                        self.supp_dropped = true;
                        log!("task: {excess} supplemental events over one message's bound left to the sidecar only");
                    }
                    Message::Supp(events)
                }
            },
            other => other,
        };
        self.queue.push_back(message);
        while self.queue.len() > OUTBOX_CAPACITY {
            // Oldest sample first; never a Terminated/Status the UI acts on.
            if let Some(at) = self.queue.iter().position(|m| matches!(m, Message::Sample(_))) {
                self.queue.remove(at);
                self.dropped += 1;
            } else {
                break;
            }
        }
        self.flush();
    }

    /// True while the UI is still connected.
    fn flush(&mut self) -> bool {
        let mut sent = false;
        while let Some(message) = self.queue.pop_front() {
            match self.tx.try_send(message) {
                Ok(()) => sent = true,
                Err(TrySendError::Full(message)) => {
                    self.queue.push_front(message);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => return false,
            }
        }
        // Wake the UI only when there is something for it.
        if sent {
            SignalToUI::set_ui_signal();
        }
        true
    }
}

fn run(commands: Receiver<Command>, tx: SyncSender<Message>, initial_interval_ms: u64) {
    let mut backend = backend::new_backend();
    log!("task: sampling via {}", backend.name());
    let mut outbox = Outbox { tx, queue: VecDeque::new(), dropped: 0, supp_dropped: false };
    let wait = CancellationToken::new();
    let mut interval_ms = initial_interval_ms;

    // The journal: ours, or somebody else's, or nowhere to put it.
    let mut journal = match Journal::open() {
        Ok(journal) => {
            log!("task: history journal at {}", journal.dir().display());
            Some(journal)
        }
        Err(error) => {
            log!("task: history journal unavailable: {error}; recording in memory only");
            outbox.push(Message::Status(format!("history: {error}; in memory only")));
            None
        }
    };
    // Loading is bounded by the history budget (known after the first
    // sample reports the machine's RAM) and streamed a batch at a time while
    // the outbox has room, between samples.
    let mut load_batches: VecDeque<Vec<Arc<Sample>>> = VecDeque::new();
    let mut supp_load: VecDeque<Vec<SuppEvent>> = VecDeque::new();
    let mut loaded = journal.is_none();
    let mut pins: Vec<ProcKey> = Vec::new();
    if let Some(journal) = &journal {
        let records = journal.load_pins();
        pins = records.iter().map(|pin| pin.key).take(32).collect();
        if !records.is_empty() {
            outbox.push(Message::Pins(records));
        }
        if let Some(columns) = journal.load_columns() {
            outbox.push(Message::Columns(columns));
        }
    }
    let mut supp_journal: Option<SuppJournal> = journal.as_ref().map(|journal| journal.supp());
    // One recording session per worker start: lines never join across it.
    let session = backend::now_ms();
    let mut recorder = Recorder::new(session);
    // Pinned process → (next metrics read, next descriptor read), ms.
    let mut pin_due: HashMap<ProcKey, (u64, u64)> = HashMap::new();

    let mut inspected: Option<ProcKey> = None;
    let mut last_detail_ms: u64 = 0;
    let mut last_threads_ms: u64 = 0;
    let mut last_libraries_ms: u64 = 0;
    let mut previous_time_ms: u64 = 0;
    let mut last_dropped_report: u64 = 0;

    loop {
        let started = Cx::monotonic_now();
        let time_ms = backend::now_ms();
        let span_ms = if previous_time_ms == 0 { 0 } else { time_ms.saturating_sub(previous_time_ms).min(u32::MAX as u64) as u32 };
        previous_time_ms = time_ms;
        let snapshot = backend.sample();
        let cost_us = ((Cx::monotonic_now() - started) * 1e6).clamp(0.0, u32::MAX as f64) as u32;
        let mut sample = Sample::from_snapshot(snapshot, time_ms, interval_ms as u32, span_ms, session);
        sample.cost_us = cost_us;
        let sample = Arc::new(sample);
        if let Some(journal) = &mut journal {
            journal.append(&sample);
            if journal.maintain(time_ms) {
                let mib = journal.disk_bytes() as f64 / (1024.0 * 1024.0);
                log!("task: journal {mib:.1} MiB on disk");
            }
        }
        if let Some(supp) = &mut supp_journal {
            supp.maintain(time_ms);
        }
        // A tracked process missing from the list is checked directly: gone
        // (or its pid reused) is an exit, and its descriptors closed with it;
        // an answer the OS will not give stops the extra reads without
        // closing anything.
        for key in recorder.tracked() {
            if sample.process(key).is_none() {
                match backend.is_alive(key) {
                    Some(false) => recorder.exited(key, time_ms),
                    Some(true) => {}
                    None => recorder.stop(key, time_ms),
                }
            }
        }
        // The first live frame goes out before the journal is parsed.
        outbox.push(Message::Sample(sample.clone()));
        if !loaded {
            loaded = true;
            if let Some(journal) = &journal {
                let budget = crate::history::budget_for_ram(sample.system.mem_total);
                let (batches, mut status) = journal.load(time_ms, budget);
                load_batches.extend(batches);
                if let Some(supp) = &supp_journal {
                    let (batches, supp_status) = supp.load(time_ms, crate::supp::budget_for_ram(sample.system.mem_total));
                    status.push_str("; ");
                    status.push_str(&supp_status);
                    supp_load.extend(batches);
                }
                outbox.push(Message::Status(status));
            }
        }
        if outbox.dropped > last_dropped_report {
            last_dropped_report = outbox.dropped;
            log!("task: UI channel full, {} samples omitted from the live view (the journal keeps at most one a second)", outbox.dropped);
        }

        // A new sidecar segment starts with the descriptor records still
        // open, so it never depends on an older segment.
        if let Some(supp) = &mut supp_journal {
            supp.begin_tick(time_ms, || recorder.checkpoint());
        }
        // The inspected process: its thread list (with the process' CPU
        // time) on the sampling tick, so per-thread CPU covers the same
        // intervals as the process'; descriptors, the memory ledger and the
        // UI's detail every second; mapped files every ten.
        let detail_every = DETAIL_EVERY_MS.max(interval_ms);
        let threads_every = interval_ms.max(THREADS_EVERY_MIN_MS);
        if let Some(key) = inspected {
            let full = time_ms.saturating_sub(last_detail_ms) >= detail_every;
            if full || time_ms.saturating_sub(last_threads_ms) >= threads_every {
                let libraries = full && time_ms.saturating_sub(last_libraries_ms) >= LIBRARIES_EVERY_MS;
                let want = Want { threads: true, files: full, libraries };
                let detail = backend.detail(key, want);
                recorder.observe(&detail, want, threads_every, detail_every);
                last_threads_ms = time_ms;
                if full {
                    last_detail_ms = time_ms;
                    if libraries {
                        last_libraries_ms = time_ms;
                    }
                    outbox.push(Message::Detail(Arc::new(detail)));
                }
            }
        }

        // Pinned processes in the background: oldest due first, bounded in
        // count and time per tick.
        let alive: Vec<ProcKey> = pins.iter().copied().filter(|key| Some(*key) != inspected && sample.process(*key).is_some()).collect();
        if !alive.is_empty() {
            let ticks_per_round = alive.len().div_ceil(PIN_READS_PER_TICK) as u64;
            let expected = PIN_EVERY_MS.max(interval_ms).max(ticks_per_round * interval_ms.max(100));
            let files_expected = PIN_FILES_EVERY_MS.max(expected);
            let mut due: Vec<(u64, ProcKey)> = alive
                .iter()
                .filter_map(|key| {
                    let (next, _) = pin_due.get(key).copied().unwrap_or((0, 0));
                    (next <= time_ms).then_some((next, *key))
                })
                .collect();
            due.sort();
            let budget_from = Cx::monotonic_now();
            for (_, key) in due.into_iter().take(PIN_READS_PER_TICK) {
                if Cx::monotonic_now() - budget_from > PIN_BUDGET_SECS {
                    break;
                }
                let (_, files_due) = pin_due.get(&key).copied().unwrap_or((0, 0));
                let want = Want { threads: true, files: files_due <= time_ms, libraries: false };
                let detail = backend.detail(key, want);
                recorder.observe(&detail, want, expected, files_expected);
                let files_next = if want.files { time_ms + PIN_FILES_EVERY_MS } else { files_due };
                pin_due.insert(key, (time_ms + PIN_EVERY_MS.max(interval_ms), files_next));
            }
        }
        send_supp(&mut recorder, &mut supp_journal, &mut outbox, time_ms);

        // Wait out the interval, reading commands as they come.
        let collected_at = Cx::monotonic_now();
        let mut yielded = false;
        loop {
            loop {
                match commands.try_recv() {
                    Ok(Command::Interval(next)) => interval_ms = next,
                    Ok(Command::Inspect(key)) => {
                        if key != inspected {
                            // Extra reads of the previous selection end,
                            // unless it is pinned.
                            if let Some(previous) = inspected {
                                if !pins.contains(&previous) {
                                    recorder.stop(previous, backend::now_ms());
                                }
                            }
                            inspected = key;
                            last_detail_ms = 0;
                            last_libraries_ms = 0;
                            // First detail right away: the inspector opened.
                            if let Some(key) = key {
                                let want = Want { threads: true, files: true, libraries: true };
                                let detail = backend.detail(key, want);
                                let every = DETAIL_EVERY_MS.max(interval_ms);
                                recorder.observe(&detail, want, every, every);
                                last_detail_ms = backend::now_ms();
                                last_libraries_ms = last_detail_ms;
                                outbox.push(Message::Detail(Arc::new(detail)));
                            }
                            send_supp(&mut recorder, &mut supp_journal, &mut outbox, backend::now_ms());
                        }
                    }
                    Ok(Command::Terminate { key, force }) => {
                        let result = backend::terminate(backend.as_mut(), key, force);
                        match &result {
                            Ok(()) => log!("task: {} -> pid {} (start {})", if force { "SIGKILL/terminate" } else { "SIGTERM" }, key.pid, key.start),
                            Err(error) => log!("task: refused to signal pid {}: {error}", key.pid),
                        }
                        outbox.push(Message::Terminated { key, force, result });
                    }
                    Ok(Command::SaveColumns(text)) => {
                        if let Some(journal) = &journal {
                            journal.save_columns(&text);
                        }
                    }
                    Ok(Command::SavePins(records)) => {
                        if let Some(journal) = &journal {
                            journal.save_pins(&records);
                        }
                        let next: Vec<ProcKey> = records.iter().map(|pin| pin.key).take(32).collect();
                        let now = backend::now_ms();
                        for key in &pins {
                            if !next.contains(key) && Some(*key) != inspected {
                                recorder.stop(*key, now);
                            }
                        }
                        pins = next;
                        pin_due.retain(|key, _| pins.contains(key));
                        send_supp(&mut recorder, &mut supp_journal, &mut outbox, now);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        // The UI dropped its handle: the app is going away.
                        if let Some(supp) = supp_journal.take() {
                            supp.close();
                        }
                        if let Some(journal) = journal.take() {
                            journal.close();
                        }
                        return;
                    }
                }
            }
            if !outbox.flush() {
                if let Some(supp) = supp_journal.take() {
                    supp.close();
                }
                if let Some(journal) = journal.take() {
                    journal.close();
                }
                return;
            }
            // Stream the journal while the UI keeps up, newest batch first,
            // one per wait slice whatever the sampling interval; then the
            // sidecar, oldest first (a close must follow its open).
            if outbox.queue.is_empty() {
                if let Some(batch) = load_batches.pop_back() {
                    outbox.push(Message::Loaded(batch));
                } else if !supp_load.is_empty() {
                    let mut events = Vec::new();
                    let mut bytes = 0usize;
                    while let Some(batch) = supp_load.pop_front() {
                        bytes += batch.iter().map(|e| e.approx_bytes()).sum::<usize>();
                        events.extend(batch);
                        if events.len() >= SUPP_LOAD_EVENTS || bytes >= SUPP_MESSAGE_BYTES / 2 {
                            break;
                        }
                    }
                    outbox.push(Message::Supp(events));
                }
            }
            let target = (interval_ms as f64 / 1000.0).max(MIN_INTERVAL_SECS);
            let now = Cx::monotonic_now();
            if now - started >= target {
                // Start-to-start cadence normally; a genuine overrun still
                // yields briefly instead of sampling back to back.
                if !yielded && now - collected_at < OVERRUN_YIELD_SECS {
                    yielded = true;
                    let _ = wait.wait_until(now + OVERRUN_YIELD_SECS);
                    continue;
                }
                break;
            }
            let deadline = (started + target).min(now + WAIT_SLICE_SECS);
            let _ = wait.wait_until(deadline);
        }
        if let Some(journal) = &mut journal {
            journal.flush();
        }
        if let Some(supp) = &mut supp_journal {
            supp.flush();
        }
    }
}

/// Hand what the recorder produced to the sidecar and the UI. After the
/// UI lost events, the still-open descriptor records go again first, so
/// later observations and closes find their record.
fn send_supp(recorder: &mut Recorder, journal: &mut Option<SuppJournal>, outbox: &mut Outbox, time_ms: u64) {
    if outbox.supp_dropped {
        outbox.supp_dropped = false;
        let checkpoint = recorder.checkpoint();
        if !checkpoint.is_empty() {
            outbox.push(Message::Supp(checkpoint));
        }
    }
    let events = recorder.take();
    if events.is_empty() {
        return;
    }
    if let Some(journal) = journal {
        journal.append(&events, time_ms, || recorder.checkpoint());
    }
    outbox.push(Message::Supp(events));
}
