//! A score playing live: the composer (the Splash VM never leaves it) runs
//! each loop's code ahead of the clock and sends the events into the loops'
//! feeds, which the `Loop` nodes of the audio graph read on the audio
//! thread. Where the platform has threads it runs on its own; where it has
//! none (the web), the host calls [`LiveScore::pump`] on its own schedule.
//! Nothing in it reads a clock but the host's [`ClockSource`]. The graph itself (the lowered score, part of
//! whatever audio graph instantiates it) is the host's: Stage's audio root,
//! a game's engine, an offline render.
//!
//! A new source is loaded here; if it loads, loops change at their next
//! cycle; if it does not, the last good revision keeps playing and the
//! status carries the problems with their lines.

use crate::compose::{Composer, Played};
use crate::model::{self, CompiledShaders, Diag, LoadError};
use crate::PrepareControl;
use makepad_audio_engine::feed::{loop_feed, LoopFeed};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::mpsc::{channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where the music is: the beat playing at the output, the tempo and
/// whether it plays (Stage's transport, a game's clock).
pub trait ClockSource: Send + Sync {
    fn now(&self) -> Option<(f64, f64, bool)>;
    /// The transport's locates so far, from the same pass as `now` or an
    /// older one: a rewind waits for it to change before it believes the
    /// clock again.
    fn located(&self) -> u64;
}

pub enum Cmd {
    Source(String, crate::PrepareControl),
    Param(String, f32),
    Trigger(String),
    Goto { section: String, quant: f64 },
    /// Change the tempo from the next beat not yet scheduled, over `ramp`
    /// beats.
    Tempo { bpm: f64, ramp: f64 },
    /// Send the tempo changes to another transport from now on.
    Transport(String),
    /// A message from the MIDI input for loop code and the Clock's
    /// controllers (see [`Composer::midi_in`]); loops that play a keyboard
    /// take it on the audio thread ([`makepad_audio_engine::feed::midi_in`]).
    Midi([u8; 3]),
    /// Play the arrangement from `beat` (song beats) at the next `quant`.
    Seek { beat: f64, quant: f64 },
    /// Repeat song beats [from, to), or play on.
    Loop(Option<(f64, f64)>),
    /// Start over from beat 0 on this transport ([`LiveScore::rewind`]).
    Rewind(String),
    Quit,
}

enum WorkerCmd {
    Command(Cmd),
    Prepared,
}

struct PreparedSource {
    source: Arc<str>,
    shaders: Arc<CompiledShaders>,
    revision: u64,
    control: PrepareControl,
}

// Reserve the name before spawning. Even a third composer started while the
// second is still waiting must follow the first one's last write and cleanup.
const STARTING: u8 = 0;
const RUNNING: u8 = 1;
const CLOSING: u8 = 2;

struct Turn {
    phase: AtomicU8,
    settled: AtomicBool,
    // One unread wake suffices: the receiver always rechecks the name and phase.
    wake: SyncSender<()>,
}

#[derive(Default)]
struct NamedComposers {
    turns: VecDeque<Arc<Turn>>,
    prepares: usize,
}

impl NamedComposers {
    fn settled(&self) -> bool {
        self.turns.is_empty() || self.turns.len() == 1 && {
            let turn = &self.turns[0];
            turn.settled.load(Ordering::Acquire) && turn.phase.load(Ordering::Acquire) == RUNNING
        }
    }

    fn next_waiter(&self) -> Option<SyncSender<()>> {
        let first = self.turns.front()?;
        (self.prepares == 0 && first.phase.load(Ordering::Acquire) == STARTING).then(|| first.wake.clone())
    }
}

// An entry lasts only while a composer or a graph prepare owns the name.
static NAMES: Mutex<BTreeMap<String, NamedComposers>> = Mutex::new(BTreeMap::new());

/// A prepare admitted under a settled name. The next composer cannot replace
/// its rings while the prepare is building. A job already queued on a runner
/// takes this at execution, so an idle runner never holds up retirement.
pub(crate) struct PrepareTurn<'a> {
    name: &'a str,
    composer: Option<Arc<Turn>>,
}

pub(crate) fn name_settled(name: &str) -> bool {
    NAMES.lock().unwrap().get(name).is_none_or(NamedComposers::settled)
}

pub(crate) fn try_prepare(name: &str) -> Option<PrepareTurn<'_>> {
    let mut names = NAMES.lock().unwrap();
    let state = names.entry(name.to_owned()).or_default();
    if !state.settled() { return None; }
    state.prepares += 1;
    Some(PrepareTurn { name, composer: state.turns.front().cloned() })
}

impl PrepareTurn<'_> {
    pub(crate) fn still_current(&self) -> bool {
        let names = NAMES.lock().unwrap();
        let state = &names[self.name];
        state.settled() && match (&self.composer, state.turns.front()) {
            (Some(old), Some(current)) => Arc::ptr_eq(old, current),
            (None, None) => true,
            _ => false,
        }
    }
}

impl Drop for PrepareTurn<'_> {
    fn drop(&mut self) {
        let mut names = NAMES.lock().unwrap();
        let state = names.get_mut(self.name).unwrap();
        state.prepares -= 1;
        let next = state.next_waiter();
        if state.prepares == 0 && state.turns.is_empty() { names.remove(self.name); }
        drop(names);
        if let Some(wake) = next { let _ = wake.try_send(()); }
        crate::node::wake_pending_prepare(self.name);
    }
}

struct Session {
    name: String,
    midi_live: bool,
    turn: Arc<Turn>,
    forget_feeds: bool,
}

impl Session {
    fn reserve(name: &str, forget_feeds: bool, wake: SyncSender<()>) -> Self {
        let turn = Arc::new(Turn { wake, settled: AtomicBool::new(false), phase: AtomicU8::new(STARTING) });
        NAMES.lock().unwrap().entry(name.into()).or_default().turns.push_back(turn.clone());
        Self { name: name.into(), turn, midi_live: false, forget_feeds }
    }

    fn can_enter(&self) -> bool {
        let names = NAMES.lock().unwrap();
        let state = &names[&self.name];
        state.prepares == 0 && Arc::ptr_eq(&state.turns[0], &self.turn)
    }

    fn set_midi(&mut self, on: bool) {
        if self.midi_live != on {
            self.turn.settled.store(false, Ordering::Release);
            makepad_audio_engine::feed::midi_out::set_live(&self.name, on);
            self.midi_live = on;
            crate::node::midi_registration_changed(&self.name);
        }
        self.turn.settled.store(true, Ordering::Release);
        crate::node::wake_pending_prepare(&self.name);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Remain at the head until cleanup finishes. A skipped or unwinding
        // successor must never clear another composer's live flag.
        self.turn.settled.store(false, Ordering::Release);
        if self.midi_live {
            makepad_audio_engine::feed::midi_out::set_live(&self.name, false);
            crate::node::midi_registration_changed(&self.name);
        }
        if self.forget_feeds {
            // Games own their graph's unique name, including feeds created
            // before a prepared thread was claimed.
            makepad_audio_engine::feed::forget_instance(&self.name);
        }
        let mut names = NAMES.lock().unwrap();
        let state = names.get_mut(&self.name).unwrap();
        state.turns.retain(|t| !Arc::ptr_eq(t, &self.turn));
        let next = state.next_waiter();
        if state.turns.is_empty() && state.prepares == 0 { names.remove(&self.name); }
        drop(names);
        if let Some(wake) = next { let _ = wake.try_send(()); }
        crate::node::wake_pending_prepare(&self.name);
    }
}

struct CommandSender {
    turn: Arc<Turn>,
    commands: Sender<WorkerCmd>,
    prepared: SyncSender<PreparedSource>,
    source: Mutex<Option<PrepareControl>>,
}

impl CommandSender {
    /// Retire the previous source only after the new command has a place in the mailbox.
    fn accept(&self, control: PrepareControl, send: impl FnOnce() -> bool) -> bool {
        let mut current = self.source.lock().unwrap();
        if !send() { return false; }
        if let Some(old) = current.replace(control.clone()) {
            if !std::ptr::eq(old.word(), control.word()) { old.cancel(); }
        }
        true
    }
}

impl Drop for CommandSender {
    fn drop(&mut self) {
        if let Some(word) = self.source.get_mut().unwrap().take() { word.cancel(); }
        // Wake whichever receiver owns the worker. A launch abandoned behind
        // a still-playing predecessor must be able to retire immediately.
        if self.turn.phase.swap(CLOSING, Ordering::AcqRel) == STARTING {
            let _ = self.turn.wake.try_send(());
        } else {
            let _ = self.commands.send(WorkerCmd::Command(Cmd::Quit));
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoopView {
    pub name: String,
    pub key: u64,
    pub cycle: u64,
    /// The cycle playing or last generated: its start and length in beats.
    pub start: f64,
    pub len: f64,
    /// What it plays, as (offset beat, note or drum slot, velocity, hit).
    pub notes: Vec<(f64, f32, f32, bool)>,
    /// Each note's length in beats (0: the patch's own gate).
    pub durs: Vec<f32>,
    /// What it sang: (offset beat, syllable).
    pub lyrics: Vec<(f64, String)>,
    /// What its code logged (`log(x)`) in its last cycle.
    pub logs: Vec<String>,
    pub resting: bool,
    pub error: Option<Diag>,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    /// Sources received, and the one playing now.
    pub revision: u64,
    pub playing_revision: u64,
    /// Editor revision of the most recent prepared source received.
    pub document_revision: Option<u64>,
    /// Problems of the newest source (empty when it plays).
    pub diags: Vec<Diag>,
    pub label: String,
    pub bpm: f64,
    pub sig: (u32, u32),
    pub beat: f64,
    pub playing: bool,
    pub section: Option<String>,
    pub loops: Vec<LoopView>,
    pub recent: Vec<Played>,
    pub params: Vec<(String, f32)>,
    pub cues: Vec<(String, f64)>,
    /// Events the composer could not hand over (a full feed).
    pub dropped: u32,
    /// Where the output is in the arrangement, in beats from its start
    /// (None off it), and the loop repeating.
    pub song_beat: Option<f64>,
    pub looping: Option<(f64, f64)>,
}

impl Status {
    /// Diagnostics are displayed with the document they describe. The live
    /// composer can still be publishing an older generation's loop state.
    pub fn document_diags<'a, J>(&'a self, loads: &'a model::RevisionLoads<J>) -> &'a [Diag] {
        if !loads.diags.is_empty() { return &loads.diags; }
        if self.document_revision == loads.applied.as_ref().map(|r| r.revision) { &self.diags } else { &[] }
    }

    pub fn document_problems<J>(&self, loads: &model::RevisionLoads<J>) -> Vec<String> {
        let diags = self.document_diags(loads);
        if !diags.is_empty() { return diags.iter().map(ToString::to_string).collect(); }
        if self.document_revision != loads.applied.as_ref().map(|r| r.revision) { return Vec::new(); }
        self.loops.iter().filter_map(|l| l.error.as_ref().map(ToString::to_string)).collect()
    }
}

#[derive(Clone)]
pub struct LiveConfig {
    /// The score's node in the graph: its loops' feeds are `<instance>/<loop>`.
    pub instance: String,
    /// How far ahead of the output the composer schedules, in seconds.
    pub lookahead: f64,
    pub clock: Arc<dyn ClockSource>,
}

pub struct LiveScore {
    cmd: CommandSender,
    pub status: Arc<Mutex<Status>>,
    /// The composer when it has no thread: the host pumps it.
    inline: Option<Mutex<Option<Worker>>>,
}

/// Whether this platform can start threads (it cannot on the web without
/// shared memory): asked once, by starting one.
pub fn threads_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| std::thread::Builder::new().spawn(|| {}).is_ok())
}

/// The composer thread's step: about every 5 ms, its status every 25 ms.
const STEP: Duration = Duration::from_millis(5);
const PUBLISH_EVERY: u32 = 5;

impl LiveScore {
    /// A live score whose tempo changes go to the transport of a
    /// standalone root (`<instance>_clock`: games).
    pub fn new(config: LiveConfig) -> LiveScore {
        let transport = crate::player::transport(&config.instance);
        LiveScore::new_on(config, &transport)
    }

    /// A live score whose tempo changes (sections' `bpm`, a tempo that
    /// follows a param, `set_tempo`) go to the transport `transport`.
    pub fn new_on(config: LiveConfig, transport: &str) -> LiveScore {
        LiveScore::start(config, transport, false, false)
    }

    /// One the host pumps ([`LiveScore::pump`]) even where threads exist.
    pub fn new_pumped(config: LiveConfig, transport: &str) -> LiveScore {
        LiveScore::start(config, transport, true, false)
    }

    pub(crate) fn with_owned_feeds(config: LiveConfig) -> LiveScore {
        let transport = crate::player::transport(&config.instance);
        Self::start(config, &transport, false, true)
    }

    fn start(config: LiveConfig, transport: &str, pumped: bool, forget: bool) -> LiveScore {
        let (cmd, status, build) = worker(config, transport, forget);
        let inline = if pumped || !threads_available() {
            let mut worker = build();
            worker.activate(false);
            Some(Mutex::new(Some(worker)))
        } else {
            std::thread::Builder::new().name("score-composer".into()).spawn(move || {
                let mut worker = build();
                if worker.activate(true) { worker.run(); }
            }).expect("the composer thread");
            None
        };
        LiveScore { cmd, status, inline }
    }

    /// The score whose composer thread `live` started elsewhere.
    pub fn from_thread(live: LiveThread) -> LiveScore {
        LiveScore { cmd: live.cmd, status: live.status, inline: None }
    }

    /// Without a composer thread: run what is due (commands, the loops'
    /// code up to the lookahead, the status). The host calls it often
    /// (every audio block or frame, at least every lookahead / 2); with a
    /// thread it does nothing.
    pub fn pump(&self) {
        if let Some(inline) = &self.inline {
            let mut inline = inline.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(worker) = inline.as_mut() {
                if worker.activate(false) && !worker.step(1) { *inline = None; }
            }
        }
    }

    pub fn send(&self, cmd: Cmd) {
        match cmd {
            Cmd::Source(_, ref control) => {
                self.cmd.accept(control.clone(), || self.cmd.commands.send(WorkerCmd::Command(cmd)).is_ok());
            }
            Cmd::Quit => {
                if let Some(word) = self.cmd.source.lock().unwrap().take() { word.cancel(); }
                let _ = self.cmd.commands.send(WorkerCmd::Command(Cmd::Quit));
            }
            cmd => { let _ = self.cmd.commands.send(WorkerCmd::Command(cmd)); }
        }
    }

    pub fn set_source(&self, source: impl Into<String>, control: &crate::PrepareControl) {
        self.send(Cmd::Source(source.into(), control.clone()));
    }

    /// One prepared revision can wait for the composer. Its notification is
    /// ordered with transport commands. A full slot is retried by the editor.
    pub fn try_set_prepared(&self, source: Arc<str>, shaders: Arc<CompiledShaders>, revision: u64) -> bool {
        let control = PrepareControl::default();
        self.cmd.accept(control.clone(), || {
            if self.cmd.prepared.try_send(PreparedSource { source, shaders, revision, control }).is_err() { return false; }
            self.cmd.commands.send(WorkerCmd::Prepared).is_ok()
        })
    }

    pub fn set_param(&self, name: &str, value: f32) {
        self.send(Cmd::Param(name.into(), value));
    }

    pub fn trigger(&self, stinger: &str) {
        self.send(Cmd::Trigger(stinger.into()));
    }

    /// Jump to a section on the next bar (`quant < 0`), beat (`1`) or now (`0`).
    pub fn goto_section(&self, section: &str, quant: f64) {
        self.send(Cmd::Goto { section: section.into(), quant });
    }

    /// Send the tempo changes to `transport` from now on (Stage: its own
    /// transport while BPM follows the score, else a transport nobody
    /// reads).
    pub fn set_transport(&self, transport: &str) {
        self.send(Cmd::Transport(transport.to_string()));
    }

    /// Change the tempo from the next beat, reached over `ramp` beats.
    /// From the next `quant` after now (0: now, <0: the next bar), play
    /// the arrangement from `beat` beats from its start (whole bars): the
    /// section there from its offset. While stopped, the next play starts
    /// there.
    pub fn seek(&self, beat: f64, quant: f64) {
        self.send(Cmd::Seek { beat, quant });
    }

    /// Repeat the arrangement's beats [from, to) (whole bars), jumping on
    /// the bar grid; None plays on. A seek outside keeps it: the loop takes
    /// the playhead when it enters.
    pub fn set_loop(&self, range: Option<(f64, f64)>) {
        self.send(Cmd::Loop(range));
    }

    /// Start the score over from scratch at beat 0 of `transport`: the same
    /// composer, player and tempo a first start has (every voice and tail
    /// cut, params as written; the loop range stays). Playing, it goes on
    /// from the top; stopped, it stays stopped at 0. For a score played from
    /// its slot ([`crate::node`]).
    pub fn rewind(&self, transport: &str) {
        self.send(Cmd::Rewind(transport.to_string()));
    }

    pub fn set_tempo(&self, bpm: f64, ramp: f64) {
        self.send(Cmd::Tempo { bpm, ramp });
    }

    pub fn status(&self) -> Status {
        self.with_status(Clone::clone)
    }

    /// Read the current composer status while holding its lock.
    pub(crate) fn with_status<R>(&self, read: impl FnOnce(&Status) -> R) -> R {
        match self.status.lock() {
            Ok(status) => read(&status),
            Err(_) => read(&Status::default()),
        }
    }
}

/// A live score's composer thread, started on any thread (a host's worker) and handed to the
/// thread that drives the score ([`LiveScore::from_thread`]). Unlike [`LiveScore::new_on`]'s, it
/// waits for its first command (the source) before it reads the clock, so it can start well
/// before its music. Dropped unowned, it tells its thread to quit and does not wait for it.
pub struct LiveThread {
    cmd: CommandSender,
    status: Arc<Mutex<Status>>,
}

// A composer thread must be able to leave the thread that started it.
const _: fn() = || { fn send<T: Send>() {} send::<LiveThread>(); };

impl LiveThread {
    /// The composer thread of a score whose tempo changes go to `transport`. Where threads
    /// exist only ([`threads_available`]).
    pub fn new(config: LiveConfig, transport: &str) -> LiveThread {
        Self::start(config, transport, false)
    }

    pub(crate) fn with_owned_feeds(config: LiveConfig, transport: &str) -> LiveThread {
        Self::start(config, transport, true)
    }

    fn start(config: LiveConfig, transport: &str, forget: bool) -> LiveThread {
        let (cmd, status, build) = worker(config, transport, forget);
        std::thread::Builder::new().name("score-composer".into()).spawn(move || {
            let mut worker = build();
            if worker.activate(true) { worker.run_from_first_command(); }
        }).expect("the composer thread");
        LiveThread { cmd, status }
    }
}

/// A composer's command line and status, and the worker to build where it runs: the
/// composer's VM never moves between threads.
fn worker(config: LiveConfig, transport: &str, forget: bool) -> (CommandSender, Arc<Mutex<Status>>, impl FnOnce() -> Worker + Send) {
    let (commands, rx) = channel();
    let (wake, admission) = sync_channel(1);
    let session = Session::reserve(&config.instance, forget, wake);
    let turn = session.turn.clone();
    let transport = transport.to_string();
    let status = Arc::new(Mutex::new(Status { bpm: 120.0, sig: (4, 4), ..Status::default() }));
    let (prepared, sources) = sync_channel(1);
    let shared = status.clone();
    let worker = move || Worker { config, rx, admission, first: None, sources, status: shared, composer: None, playing_source: None, revision: 0, feeds: HashMap::new(), last_beat: None, dropped: 0, session, transport, tempo: None, osc_in: None, osc_out: None, osc_pending: Vec::new(), seek_stopped: None, loop_range: None, steps: 0, locating: None, from_top: false };
    (CommandSender { turn, commands, prepared, source: Mutex::default() }, status, worker)
}

struct PlayingSource {
    text: Arc<str>,
    shaders: Arc<CompiledShaders>,
    control: PrepareControl,
}

struct Worker {
    rx: Receiver<WorkerCmd>,
    admission: Receiver<()>,
    first: Option<WorkerCmd>,
    sources: Receiver<PreparedSource>,
    status: Arc<Mutex<Status>>,
    composer: Option<Composer>,
    playing_source: Option<PlayingSource>,
    revision: u64,
    feeds: HashMap<u64, Arc<LoopFeed>>,
    last_beat: Option<f64>,
    dropped: u32,
    transport: String,
    tempo: Option<Arc<makepad_audio_engine::feed::TempoFeed>>,
    /// Steps taken (the status goes out every few).
    steps: u32,
    /// A seek made while stopped (the next play starts there) and the loop
    /// (kept across restarts).
    seek_stopped: Option<f64>,
    loop_range: Option<(f64, f64)>,
    /// A rewind's locate is on its way: the transport's count before it.
    /// Until it changes the clock still reads the old run.
    locating: Option<u64>,
    /// The composer stands at beat 0 from a rewind: the next start goes on
    /// from there, not from the next bar.
    from_top: bool,
    /// OSC: the socket listening on the Clock's `osc_in` port, the one
    /// sending, and what waits for its beat.
    osc_in: Option<(u16, std::net::UdpSocket)>,
    osc_out: Option<std::net::UdpSocket>,
    osc_pending: Vec<(f64, String, Vec<u8>)>,
    // Drop the VM and its feed handles before publishing retirement. The
    // host clock is released last, after the name is available again.
    session: Session,
    config: LiveConfig,
}

impl Worker {
    fn activate(&mut self, wait: bool) -> bool {
        if self.tempo.is_some() { return true; }
        while !self.session.can_enter() {
            if !wait || self.session.turn.phase.load(Ordering::Acquire) == CLOSING { return false; }
            if self.admission.recv().is_err() { return false; }
        }
        if self.session.turn.phase.compare_exchange(STARTING, RUNNING, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return false;
        }
        // A source queued during retirement can fail. Resolve it before
        // publishing any MIDI registration for its graphs to observe.
        self.first = self.rx.try_recv().ok();
        if !matches!(self.first, Some(WorkerCmd::Prepared | WorkerCmd::Command(Cmd::Source(..) | Cmd::Quit))) {
            self.session.set_midi(true);
        }
        self.tempo = Some(makepad_audio_engine::feed::tempo_feed(&self.transport));
        true
    }

    /// [`Self::run`] once the first command came (a [`LiveThread`]'s source): until then
    /// nothing reads the clock.
    fn run_from_first_command(mut self) {
        match self.first.take().or_else(|| self.rx.recv().ok()) {
            Some(WorkerCmd::Command(Cmd::Quit)) | None => return,
            Some(cmd) => self.command(cmd),
        }
        if self.step(PUBLISH_EVERY) {
            self.run();
        }
    }

    /// The composer thread: a step whenever a command comes or every
    /// [`STEP`].
    fn run(mut self) {
        loop {
            match self.first.take().map(Ok).unwrap_or_else(|| self.rx.recv_timeout(STEP)) {
                Ok(WorkerCmd::Command(Cmd::Quit)) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => self.command(cmd),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if !self.step(PUBLISH_EVERY) {
                break;
            }
        }
    }

    /// The commands waiting, the loops' code up to the lookahead, OSC, and
    /// the status every `publish_every` steps. False once told to quit.
    fn step(&mut self, publish_every: u32) -> bool {
        while let Some(cmd) = self.first.take().or_else(|| self.rx.try_recv().ok()) {
            if let WorkerCmd::Command(Cmd::Quit) = cmd {
                return false;
            }
            self.command(cmd);
        }
        self.pump();
        self.osc();
        self.steps = self.steps.wrapping_add(1);
        if self.steps % publish_every.max(1) == 0 {
            self.publish();
        }
        true
    }

    /// OSC in (the Clock's `osc_in` port) into the composer, and what loop
    /// code sent out when its beat comes.
    fn osc(&mut self) {
        if !crate::osc::available() {
            return;
        }
        let Some(composer) = &mut self.composer else { return };
        let wanted = composer.doc.clock.osc_in;
        if self.osc_in.as_ref().map(|(p, _)| *p) != wanted {
            self.osc_in = wanted.and_then(|port| match std::net::UdpSocket::bind(("0.0.0.0", port)) {
                Ok(socket) if socket.set_nonblocking(true).is_ok() => Some((port, socket)),
                Ok(_) => None,
                Err(error) => {
                    eprintln!("score OSC: cannot listen on port {port}: {error}");
                    None
                }
            });
        }
        let clock = self.config.clock.now();
        let beat = clock.map_or(0.0, |(beat, ..)| beat);
        if let Some((_, socket)) = &self.osc_in {
            let mut buf = [0u8; 2048];
            for _ in 0..256 {
                let Ok((n, _)) = socket.recv_from(&mut buf) else { break };
                for (addr, args) in crate::osc::decode(&buf[..n]) {
                    composer.osc_in(&addr, args, beat);
                }
            }
            for (fx, event) in composer.automation_out.drain(..) {
                makepad_audio_engine::feed::automation_feed(&format!("{}/{fx}", self.config.instance)).send(event);
            }
        }
        self.osc_pending.append(&mut composer.osc_out);
        let playing = clock.is_some_and(|(.., playing)| playing);
        if !playing {
            self.osc_pending.clear();
            return;
        }
        if self.osc_pending.iter().any(|(at, ..)| *at <= beat) && self.osc_out.is_none() {
            self.osc_out = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok();
        }
        let Some(socket) = &self.osc_out else { return };
        self.osc_pending.retain(|(at, target, packet)| {
            if *at > beat {
                return true;
            }
            let _ = socket.send_to(packet, target.as_str());
            false
        });
    }

    fn feeds_for(&mut self) {
        let Some(composer) = &self.composer else { return };
        for name in composer.doc.feed_names() {
            let key = crate::key(&name);
            self.feeds.entry(key).or_insert_with(|| loop_feed(&format!("{}/{name}", self.config.instance)));
        }
    }

    fn reevaluate(&self) -> Option<model::Loaded> {
        let source = self.playing_source.as_ref()?;
        match source.shaders.evaluate_controlled(&source.text, &source.control) {
            Ok(loaded) => Some(loaded),
            Err(LoadError::Cancelled) => None,
            Err(error) => {
                let diags = error.into_diagnostics();
                if let Ok(mut status) = self.status.lock() { status.diags = diags; }
                None
            }
        }
    }

    /// Start every loop over from the bar holding `beat` (the clock jumped).
    fn restart_at(&mut self, beat: f64) {
        let Some(loaded) = self.reevaluate() else { return };
        let params = self.composer.as_ref().map(|c| c.params.clone()).unwrap_or_default();
        let mut fresh = Composer::new(loaded.into_evaluated());
        for (name, value) in params {
            fresh.set_param(&name, value);
        }
        let bar = fresh.doc.beats_per_bar();
        let at = (beat / bar).ceil() * bar;
        for state in &mut fresh.loops {
            state.next = at;
        }
        fresh.set_loop(self.loop_range);
        if let Some(song) = self.seek_stopped.take() {
            fresh.seek(song, at, 0.0);
        }
        self.composer = Some(fresh);
    }

    /// The first start again: a new composer and player, the first events
    /// in the player's feeds before the transport goes back, so the top
    /// sounds on time. Only this thread sends, so nothing of the old run
    /// reaches the new feeds, and the locate follows the old tempo changes
    /// in theirs.
    fn rewind(&mut self, transport: &str) {
        self.seek_stopped = None;
        self.osc_pending.clear();
        self.dropped = 0;
        self.last_beat = None;
        self.from_top = true;
        self.locating = Some(self.config.clock.located());
        let clock = makepad_audio_engine::feed::tempo_feed(transport);
        let locate = |bpm| makepad_audio_engine::feed::TempoEvent { beat: 0.0, bpm, ramp: 0.0, locate: true };
        let Some(loaded) = self.reevaluate() else {
            clock.send(locate(0.0));
            return;
        };
        let mut composer = Composer::new(loaded.into_evaluated());
        composer.set_loop(self.loop_range);
        let bpm = crate::render::transport_bpm(&composer.doc);
        let slot = crate::node::slot(&self.config.instance);
        let player = slot.prepare_fresh().unwrap_or_else(|diags| {
            eprintln!("score: rewind: {}", diags.first().map_or("", |d| d.message.as_str()));
            None
        });
        let playing = self.config.clock.now().is_some_and(|(.., playing)| playing);
        let mut events = Vec::new();
        if playing {
            composer.pump(self.config.lookahead * bpm / 60.0, &mut events);
        }
        for (fx, event) in composer.automation_out.drain(..) {
            makepad_audio_engine::feed::automation_feed(&format!("{}/{fx}", self.config.instance)).send(event);
        }
        let tempo: Vec<_> = composer.tempo_out.drain(..).collect();
        self.composer = Some(composer);
        self.feeds_for();
        for event in events {
            match self.feeds.get(&event.loop_key) {
                Some(feed) if feed.send(event) => {}
                _ => self.dropped += 1,
            }
        }
        clock.send(locate(bpm));
        if let Some(player) = player {
            slot.post_fresh(player);
        }
        for change in tempo {
            self.tempo.as_ref().unwrap().send(change);
        }
    }

    fn pump(&mut self) {
        if let Some(before) = self.locating {
            if self.config.clock.located() == before {
                return;
            }
            self.locating = None;
        }
        let Some((now, bpm, playing)) = self.config.clock.now() else { return };
        if self.composer.is_none() || !playing {
            self.last_beat = None;
            return;
        }
        // The clock started, or jumped (a locate): begin on the next bar.
        match self.last_beat {
            None if self.from_top && self.seek_stopped.is_none() => {}
            None => self.restart_at(now),
            Some(last) if now < last - 0.5 || now > last + 16.0 => self.restart_at(now),
            _ => {}
        }
        self.from_top = false;
        self.last_beat = Some(now);
        let until = now + self.config.lookahead * bpm / 60.0;
        let mut events = Vec::new();
        if let Some(composer) = &mut self.composer {
            composer.pump(until, &mut events);
            for change in composer.tempo_out.drain(..) {
                self.tempo.as_ref().unwrap().send(change);
            }
            for (fx, event) in composer.automation_out.drain(..) {
                makepad_audio_engine::feed::automation_feed(&format!("{}/{fx}", self.config.instance)).send(event);
            }
        }
        for event in events {
            match self.feeds.get(&event.loop_key) {
                Some(feed) if feed.send(event) => {}
                _ => self.dropped += 1,
            }
        }
    }

    fn command(&mut self, cmd: WorkerCmd) {
        let locating = self.locating.is_some();
        let now = self.config.clock.now().map_or(0.0, |(beat, bpm, _)| if locating { 0.0 } else { beat } + self.config.lookahead * bpm / 60.0);
        let cmd = match cmd {
            WorkerCmd::Command(cmd) => cmd,
            WorkerCmd::Prepared => {
                if let Ok(prepared) = self.sources.try_recv() {
                    let loaded = prepared.shaders.evaluate_controlled(&prepared.source, &prepared.control);
                    self.load(prepared.source, Some(prepared.shaders), Some(prepared.revision), prepared.control, now, loaded);
                }
                return;
            }
        };
        match cmd {
            Cmd::Source(source, control) => {
                let loaded = model::load_controlled(&source, &control);
                self.load(source.into(), None, None, control, now, loaded);
            }
            Cmd::Param(name, value) => {
                if let Some(composer) = &mut self.composer {
                    composer.set_param(&name, value);
                }
            }
            Cmd::Trigger(name) => {
                if let Some(composer) = &mut self.composer {
                    let _ = composer.trigger(&name, now);
                }
            }
            Cmd::Goto { section, quant } => {
                if let Some(composer) = &mut self.composer {
                    let _ = composer.goto_section(&section, now, quant);
                }
            }
            Cmd::Tempo { bpm, ramp } => {
                if let Some(composer) = &mut self.composer {
                    composer.set_tempo(bpm, ramp);
                }
            }
            Cmd::Transport(name) => {
                self.tempo = Some(makepad_audio_engine::feed::tempo_feed(&name));
                // The new transport starts from the tempo map as it stands.
                if let Some(composer) = &self.composer {
                    let beat = now;
                    let mut current = None;
                    for change in composer.tempo_map.iter().filter(|c| c.beat <= beat) {
                        current = Some(*change);
                    }
                    if let Some(change) = current {
                        self.tempo.as_ref().unwrap().send(makepad_audio_engine::feed::TempoEvent { beat, bpm: change.bpm, ramp: 0.0, locate: false });
                    }
                }
            }
            Cmd::Seek { beat, quant } => {
                let playing = self.config.clock.now().is_some_and(|(.., playing)| playing);
                match &mut self.composer {
                    Some(composer) if playing && self.last_beat.is_some() => {
                        composer.seek(beat, now, quant);
                    }
                    _ => self.seek_stopped = Some(beat),
                }
            }
            Cmd::Loop(range) => {
                self.loop_range = range;
                if let Some(composer) = &mut self.composer {
                    composer.set_loop(range);
                }
            }
            Cmd::Midi(data) => {
                // Played now: stamped with the beat playing, not ahead.
                let beat = self.config.clock.now().map_or(0.0, |(beat, ..)| beat);
                if let Some(composer) = &mut self.composer {
                    composer.midi_in(data, beat);
                    for (fx, event) in composer.automation_out.drain(..) {
                        makepad_audio_engine::feed::automation_feed(&format!("{}/{fx}", self.config.instance)).send(event);
                    }
                }
            }
            Cmd::Rewind(transport) => self.rewind(&transport),
            Cmd::Quit => {}
        }
    }

    fn load(&mut self, source: Arc<str>, shaders: Option<Arc<CompiledShaders>>, revision: Option<u64>, control: PrepareControl, now: f64, loaded: Result<model::Loaded, LoadError>) {
        let result = if control.is_cancelled() { Err(LoadError::Cancelled) } else { loaded };
        match result {
            Err(LoadError::Cancelled) => {
                if self.composer.is_none() { self.session.set_midi(false); }
            }
            Err(error) => {
                let diags = error.into_diagnostics();
                self.revision += 1;
                // The last good score stays playable, including a rewind after this refusal.
                // The sender now owns this command's word for its next evaluation.
                if let Some(source) = &mut self.playing_source { source.control = control; }
                if self.composer.is_none() { self.session.set_midi(false); }
                if let Ok(mut status) = self.status.lock() {
                    status.revision = self.revision;
                    status.document_revision = revision;
                    status.diags = diags;
                }
            }
            Ok(loaded) => {
                self.revision += 1;
                self.session.set_midi(true);
                self.playing_source = Some(PlayingSource {
                    text: source, shaders: shaders.unwrap_or_else(|| CompiledShaders::from_doc(&loaded.doc)), control,
                });
                match &mut self.composer {
                    Some(composer) => composer.reload(loaded.into_evaluated(), now),
                    None => self.composer = Some(Composer::new(loaded.into_evaluated())),
                }
                self.feeds_for();
                if let Ok(mut status) = self.status.lock() {
                    status.revision = self.revision;
                    status.document_revision = revision;
                    status.playing_revision = self.revision;
                    status.diags.clear();
                }
            }
        }
    }

    fn publish(&mut self) {
        let clock = self.config.clock.now();
        let Ok(mut status) = self.status.lock() else { return };
        if let Some((beat, bpm, playing)) = clock {
            status.beat = beat;
            status.bpm = bpm;
            status.playing = playing;
        }
        status.dropped = self.dropped;
        let Some(composer) = &mut self.composer else { return };
        status.label = composer.doc.label.clone();
        status.sig = composer.doc.clock.sig;
        status.section = composer.section_at(status.beat);
        status.song_beat = if status.playing { composer.song_beat(status.beat) } else { self.seek_stopped };
        status.looping = composer.loop_range;
        status.params = composer.params.clone();
        status.loops = composer.loops.iter().map(|s| LoopView {
            name: s.name.clone(), key: s.key, cycle: s.cycle, start: s.last_start, len: s.last_len, notes: s.last_notes.clone(), durs: s.last_durs.clone(), lyrics: s.last_lyrics.iter().map(|y| (y.beat, y.text.clone())).collect(), logs: s.last_logs.clone(), resting: s.resting, error: s.error.clone(),
        }).collect();
        let keep = composer.history.len().saturating_sub(512);
        status.recent = composer.history.iter().skip(keep).cloned().collect();
        status.cues = composer.cues.iter().cloned().collect();
    }
}

/// Tests installing the process-wide log tap hold this until they remove it.
#[cfg(test)]
pub(crate) static TEST_LOG_TAP: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    struct Playing;
    impl ClockSource for Playing {
        fn now(&self) -> Option<(f64, f64, bool)> {
            Some((0.0, 120.0, true))
        }
        fn located(&self) -> u64 {
            0
        }
    }

    #[test]
    fn accepted_sources_interrupt_rewinds_and_retire() {
        use makepad_script::makepad_error_log::{set_log_tap, LogLevel};
        const WAIT: Duration = Duration::from_secs(5);
        const NAME: &str = "rewind_document_cancel";
        const SOURCE: &str = "use mod.score.*\nmod.std.log(\"composer_eval_entry\")\nlet n = 0\nfor i in 0..2048 { n += 1 }\nmod.std.log(\"composer_eval_end\")\nlet c = Clock{}\nScore{c}";
        static EVENTS: Mutex<Option<Sender<(bool, Sender<()>)>>> = Mutex::new(None);
        fn tap(message: &str, _: LogLevel) {
            let entry = message.contains("composer_eval_entry");
            if !entry && !message.contains("composer_eval_end") { return; }
            let (release, hold) = channel();
            let events = EVENTS.lock().unwrap().clone();
            if events.is_some_and(|tx| tx.send((entry, release)).is_ok()) && entry {
                let _ = hold.recv_timeout(WAIT);
            }
        }
        struct Tap;
        impl Drop for Tap {
            fn drop(&mut self) { set_log_tap(None); EVENTS.lock().unwrap().take(); }
        }
        struct Clock(Sender<()>);
        impl ClockSource for Clock {
            fn now(&self) -> Option<(f64, f64, bool)> { Some((0.0, 120.0, false)) }
            fn located(&self) -> u64 { 0 }
        }
        impl Drop for Clock {
            fn drop(&mut self) { let _ = self.0.send(()); }
        }
        let doc = model::ScoreDoc::test_load(SOURCE).unwrap();
        let shaders = CompiledShaders::from_doc(&doc);
        let (tx, events) = channel();
        let _owner = TEST_LOG_TAP.lock().unwrap_or_else(|e| e.into_inner());
        *EVENTS.lock().unwrap() = Some(tx);
        set_log_tap(Some(tap));
        let _tap = Tap;
        let event = |entry, property| {
            let (actual, release) = events.recv_timeout(WAIT).expect(property);
            assert_eq!(actual, entry, "{property}");
            release
        };
        let (end, retired) = channel();
        let live = LiveScore::new_on(LiveConfig { instance: NAME.into(), lookahead: 0.15, clock: Arc::new(Clock(end)) }, NAME);
        assert!(live.try_set_prepared(SOURCE.into(), shaders.clone(), 1));
        event(true, "the prepared source runs on the worker").send(()).unwrap();
        event(false, "an uninterrupted evaluation finishes");

        live.rewind(NAME);
        let rewind = event(true, "the rewind is inside its own document evaluation");
        let playing = live.cmd.source.lock().unwrap().clone().unwrap();
        let mut drafts = model::RevisionLoads::<()>::default();
        drafts.submit(2, "let =", drop);
        drafts.submit(3, "", drop);
        assert!(!playing.is_cancelled(), "typing and superseding drafts leaves the running rewind alone");
        let audition = SOURCE.replace("Score{c}", "Score{label: \"audition\" c}");
        assert!(live.try_set_prepared(audition.into(), shaders, 1), "an audition is accepted at the same revision");
        assert!(playing.is_cancelled(), "accepting the audition interrupts the running rewind");
        rewind.send(()).unwrap();
        event(true, "the audition enters before the obsolete evaluation can finish").send(()).unwrap();
        event(false, "the audition finishes under its own word");
        live.rewind(NAME);
        let hold = event(true, "the audition is re-evaluating when the composer closes");
        drop(live);
        hold.send(()).unwrap();
        retired.recv_timeout(WAIT).expect("the composer retires after its interrupted evaluation");
        assert!(events.try_recv().is_err(), "the closed VM never reaches the end of its score code");
        assert!(name_settled(NAME), "RETIRE released the composer's name");
    }

    /// Without a thread the host's pumps run the composer: the source
    /// loads, the loops' first cycles are generated and the status
    /// published.
    #[test]
    fn a_pumped_score_plays_without_a_thread() {
        let config = LiveConfig { instance: "pumped_test".into(), lookahead: 0.15, clock: Arc::new(Playing) };
        let live = LiveScore::new_pumped(config, "pumped_test_clock");
        let good = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet l = Loop{play: s beats: 1 code: fn(i) { play(@c4) sleep(1) }}\nScore{clock s l}\n";
        live.set_source(good, &crate::PrepareControl::default());
        assert!(live.status().loops.is_empty(), "nothing runs before a pump");
        for _ in 0..3 {
            live.pump();
        }
        let status = live.status();
        assert!(status.diags.is_empty(), "{:?}", status.diags);
        assert_eq!(status.playing_revision, 1);
        assert!(status.loops.iter().any(|l| l.name == "l" && !l.notes.is_empty()), "{:?}", status.loops);

        crate::tests::with_refused_score_concat(|source, line| {
            live.set_source(source, &crate::PrepareControl::default());
            live.pump();
            let refused = live.status();
            assert_eq!(refused.playing_revision, status.playing_revision, "the last good score is still applied");
            assert!(refused.playing && makepad_audio_engine::feed::midi_out::is_live("pumped_test/l"),
                "the playing loop keeps its live MIDI feed after a refused edit");
            let [diag] = refused.diags.as_slice() else { panic!("expected one memory diagnostic: {:?}", refused.diags) };
            assert_eq!((diag.line, &diag.message), (line, &format!("Evaluation ran out of memory at line {line}")),
                "the composer reports the physical refusal where the edited score grew");
        });
        live.set_source(good.replace("Score{", "Score{label: \"Next\" "), &crate::PrepareControl::default());
        live.pump();
        let next = live.status();
        assert!(next.playing_revision > status.playing_revision && next.label == "Next" && next.diags.is_empty(),
            "the next finite edit becomes the playing revision and clears the refusal");

        if threads_available() {
            struct Unclaimed { _alive: Sender<()> }
            impl ClockSource for Unclaimed {
                fn now(&self) -> Option<(f64, f64, bool)> { panic!("an unclaimed launch never reads its clock") }
                fn located(&self) -> u64 { unreachable!() }
            }
            let (alive, gone) = channel();
            let waiting = LiveThread::new(LiveConfig {
                instance: "pumped_test".into(), lookahead: 0.15, clock: Arc::new(Unclaimed { _alive: alive }),
            }, "pumped_test_clock");
            std::thread::spawn(move || {
                // The second send only fits once the worker is actually waiting for its name.
                waiting.cmd.turn.wake.send(()).unwrap();
                waiting.cmd.turn.wake.send(()).unwrap();
                drop(waiting);
            });
            assert_eq!(gone.recv_timeout(Duration::from_secs(10)), Err(RecvTimeoutError::Disconnected),
                "abandoning a waiting launch releases it while its predecessor is still playing");
            assert!(name_settled("pumped_test") && makepad_audio_engine::feed::midi_out::is_live("pumped_test/l"),
                "an abandoned launch leaves the existing composer live and its slot available");
        }
    }
}
