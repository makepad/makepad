//! `Score`: a score as one node of any audio flow graph. The node plays
//! the score its slot holds (`Score{ score: "score" clock: transport.out(@clock) }`),
//! following the clock wired into it, and outputs the master.
//!
//! - The host gives a slot its score ([`Slot::set_doc`], [`Slot::set_source`]);
//!   the slot prepares a player off the host's thread through its runner,
//!   against the one playing, and hands it to the node through a one-place
//!   mailbox. Without a runner it prepares on the calling thread, outside
//!   the host lock (tests, offline renders and hosts without a pool). The node
//!   takes it at its next pass: staying loops keep their voices and feeds
//!   (state migration, as a revision of any graph).
//! - Loop feeds are `<slot>/<loop>`, so a composer (`LiveScore`) drives a
//!   slot's score as it drives any player of that name.
//! - The audio thread never allocates, locks or frees: replaced players go
//!   back through a retire bin the host empties ([`Slot::pump`]).
//! - What the node's player measures (its loops' and FX's peaks, their
//!   streams) the host reads from the slot ([`Slot::read`], [`Slot::scope`]);
//!   held controls go to it with [`Slot::set_control`].

use crate::model::{Diag, ScoreDoc};
use crate::player::{prepare, slice as score_slice, ClockMode, Meters, Player};
use makepad_audio_engine::nested::NestedNode;
use makepad_audio_engine::nodes::{input, node, output, AUDIO, CLOCK, LEVEL};
use makepad_audio_engine::schedule::{ClockChunk, ControlBank, Layout};
use makepad_flow::document::{ParamDescriptor, ParamType, Registry, TypeDescriptor};
use makepad_flow::live::owner::PrepareControl;
use makepad_flow::live::plan::{LiveNode, OwnerSlice, PlanError};
use makepad_flow::Literal;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The node type.
pub const TYPE: &str = "Score";
/// The slot a `Score` node without a `score` setting plays.
pub const DEFAULT_SLOT: &str = "score";
/// Players retired by the audio thread the host has not freed yet.
const RETIRED: usize = 8;

/// Run a slot's prepare job off the host's thread, or report a submit error.
pub type Runner = Arc<dyn Fn(Box<dyn FnOnce() + Send>) -> Result<(), String> + Send + Sync>;

/// Register `Score` in a flow registry (after the audio types and nodes,
/// `makepad_audio_engine::nodes::register_nodes`) and its player factory
/// with the engine.
pub fn register(registry: &mut Registry) -> Result<(), String> {
    registry.register(descriptor()).map_err(|error| error.to_string())?;
    makepad_audio_engine::nested::register(TYPE, factory);
    Ok(())
}

/// `Score`'s descriptor.
pub fn descriptor() -> TypeDescriptor {
    node(
        TYPE,
        "A score played as one node: the score its slot holds (the host gives the slot its score), following the clock wired in; its output is the score's master.",
        vec![ParamDescriptor::authored("score", ParamType::String, Literal::Str(DEFAULT_SLOT.into()), "The score slot it plays (the host fills it).")],
        vec![input("clock", CLOCK, None)],
        vec![output("audio", AUDIO), output("peak", LEVEL)],
    )
}

/// The slot `name` (made on first use; slots live as long as the process).
pub fn slot(name: &str) -> Arc<Slot> {
    let mut slots = slots().lock().unwrap_or_else(|e| e.into_inner());
    slots.entry(name.to_string()).or_insert_with(|| Arc::new(Slot::new(name))).clone()
}

fn slots() -> &'static Mutex<HashMap<String, Arc<Slot>>> {
    static SLOTS: OnceLock<Mutex<HashMap<String, Arc<Slot>>>> = OnceLock::new();
    SLOTS.get_or_init(Mutex::default)
}

/// A player remembers whether its MIDI loops were live at prepare time.
/// Also repair an existing offline player if its composer starts afterwards.
/// Invalidate before the composer publishes its settled MIDI registration.
pub(crate) fn midi_registration_changed(name: &str) {
    let slot = slots().lock().unwrap_or_else(|e| e.into_inner()).get(name).cloned();
    if let Some(slot) = slot {
        let mut host = slot.lock();
        if host.uses_midi && (host.posted.is_some() || host.in_flight) {
            host.invalidate();
        }
    }
}

/// Ask the owner to dispatch work that was pending when the name settled.
pub(crate) fn wake_pending_prepare(name: &str) {
    let slot = slots().lock().unwrap_or_else(|e| e.into_inner()).get(name).cloned();
    let Some(slot) = slot else { return };
    let runner = {
        let host = slot.lock();
        if !slot.can_start_prepare(&host) { return; }
        host.runner.clone()
    };
    if let Some(runner) = runner {
        if let Err(error) = runner(Box::new(move || slot.kick())) {
            eprintln!("score: prepare: {error}");
        }
    }
}

/// What the audio thread and the host share.
struct Shared {
    /// Latency of the newest published player, shared with its parent layouts.
    latency: Arc<AtomicU32>,
    observer: PrepareControl,
    /// The next player to play (the newest prepared), or null.
    next: AtomicPtr<Player>,
    /// Players the audio thread let go, for the host to free.
    retired: [AtomicPtr<Player>; RETIRED],
    /// The generation playing (0: none).
    playing: AtomicU64,
    /// A node runs without a player (it was not carried over): the host
    /// prepares one at its next pump.
    wanted: AtomicBool,
}

/// What a host reads of a player it prepared.
struct Watch {
    generation: u64,
    layout: Arc<Layout>,
    controls: Arc<ControlBank>,
    meters: Arc<Meters>,
    params: Arc<PlayingControls>,
}

/// Native effect controls in one playing generation, indexed by score path.
pub struct PlayingControls {
    pub generation: u64,
    params: HashMap<String, ControlReader>,
}

#[derive(Clone)]
pub struct ControlReader {
    bank: Arc<ControlBank>,
    index: u16,
    pub driver: Option<Arc<str>>,
}

impl ControlReader {
    pub fn value(&self) -> f32 { self.bank.get(self.index) }
}

impl PlayingControls {
    pub fn get(&self, path: &str) -> Option<&ControlReader> { self.params.get(path) }

    pub fn driven(&self) -> impl Iterator<Item = (&str, &ControlReader)> {
        self.params.iter().filter(|(_, reader)| reader.driver.is_some()).map(|(path, reader)| (path.as_str(), reader))
    }

    fn new(doc: &ScoreDoc, player: &Player) -> Self {
        let mut params = HashMap::new();
        for fx in doc.fx.iter().filter(|fx| fx.shader.is_none()) {
            for (i, &(param, ..)) in fx.kind.params().iter().enumerate() {
                let Some(index) = player.control(&fx.name, param) else { continue };
                let driver = param_driver(fx, doc, i, param);
                params.insert(format!("{}.{param}", fx.name), ControlReader {
                    bank: player.schedule.controls.clone(), index, driver: driver.map(Into::into),
                });
            }
        }
        Self { generation: player.schedule.generation, params }
    }
}

fn param_driver(fx: &crate::model::FxDef, doc: &ScoreDoc, i: usize, param: &str) -> Option<String> {
    fx.sides.iter().find(|(field, sources)| field == param && !sources.is_empty())
        .map(|(_, sources)| sources.ports().map(|(n, p)| format!("{n}.{p}")).collect::<Vec<_>>().join(", "))
        .or_else(|| fx.follow.iter().find(|(slot, _)| *slot == i).map(|(_, follow)| format!("automation ({}.{})", doc.clock.name, follow.param)))
}

/// A new document can bind a field before the playing generation changes.
/// Editors check this on writes so a pending binding survives a gesture.
pub fn authored_driver(doc: &ScoreDoc, node: &str, field: &str) -> Option<String> {
    let fx = doc.fx.iter().find(|fx| fx.name == node && fx.shader.is_none())?;
    param_driver(fx, doc, fx.kind.param_index(field)?, field)
}

fn clock_bindings(doc: &ScoreDoc) -> impl Iterator<Item = (&str, usize, &str, &str)> {
    doc.fx.iter().flat_map(move |fx| fx.follow.iter().map(move |(i, follow)| (fx.name.as_str(), *i, doc.clock.name.as_str(), follow.param.as_str())))
}

type SlotDoc = (Arc<ScoreDoc>, Arc<OwnerSlice>);

struct PreparedFrom {
    doc: SlotDoc,
    resources: u64,
}

struct Host {
    doc: Option<SlotDoc>,
    /// The device rate the slot's node runs at (from its last prepare).
    rate: Option<u32>,
    generation: u64,
    /// Generations reserved by ordinary and fresh prepares, even on failure.
    reserved: u64,
    /// The layout of the newest player prepared: the base of the next.
    base: Option<Arc<Layout>>,
    /// A score is waiting for the mailbox to empty.
    pending: bool,
    /// What the newest player was prepared from (a score that makes the
    /// same graph needs none: its composer changes what it plays).
    posted: Option<PreparedFrom>,
    resources: u64,
    uses_midi: bool,
    rewinding: bool,
    /// The players prepared and not yet replaced twice: the playing one is
    /// among them.
    watches: Vec<Watch>,
    /// The latest wanted prepare, including a rewind waiting to post.
    control: Option<PrepareControl>,
    /// One ordinary prepare is out, either through the runner or inline.
    in_flight: bool,
    runner: Option<Runner>,
    /// Prepare errors waiting for the host's next turn.
    diags: Vec<Diag>,
}

impl Host {
    fn supersede(&mut self) {
        if let Some(control) = self.control.take() { control.cancel(); }
        self.diags.clear();
    }

    fn edit(&mut self) {
        if self.rewinding { self.diags.clear(); } else { self.supersede(); }
    }

    fn invalidate(&mut self) {
        self.edit();
        self.resources += 1;
        self.pending = true;
    }

    fn matches(&self, from: &PreparedFrom) -> bool {
        self.resources == from.resources && self.doc.as_ref().is_some_and(|(doc, slice)| {
            Arc::ptr_eq(doc, &from.doc.0) && Arc::ptr_eq(slice, &from.doc.1)
                || slice == &from.doc.1 && clock_bindings(doc).eq(clock_bindings(&from.doc.0))
        })
    }

    fn needs_prepare(&self) -> bool {
        self.posted.as_ref().is_none_or(|posted| !self.matches(posted))
    }

    fn begin(&mut self, rewind: bool, observer: &PrepareControl) -> (u64, PrepareControl) {
        self.supersede();
        self.rewinding = rewind;
        self.reserved += 1;
        let control = observer.renewed();
        self.control = Some(control.clone());
        self.pending = false;
        (self.reserved, control)
    }

    fn wants(&self, generation: u64, control: &PrepareControl) -> bool {
        !control.is_cancelled() && self.reserved == generation
    }
}

/// A rewind keeps its player and observations private until the slot admits it.
pub(crate) struct FreshPlayer {
    from: PreparedFrom,
    player: Player,
    watch: Option<Watch>,
    control: PrepareControl,
}

/// A score the `Score` nodes named after it play.
pub struct Slot {
    name: String,
    shared: Arc<Shared>,
    host: Mutex<Host>,
}

fn prepare_hosted(doc: &ScoreDoc, slice: OwnerSlice, rate: u32, name: &str, base: Option<&Layout>, generation: u64, control: &PrepareControl) -> Result<(Player, Option<Watch>), crate::LoadError> {
    let player = prepare(doc, slice, rate as f32, name, ClockMode::Host, base, generation, control)?;
    if player.schedule.latency > u64::from(u32::MAX) {
        return Err(vec![Diag { line: 1, col: 1, node: Some(name.into()), message: "nested score latency exceeds the 32-bit frame range".into() }].into());
    }
    let watch = player.meters.as_ref().map(|meters| Watch {
        generation, layout: player.layout.clone(), controls: player.schedule.controls.clone(),
        meters: meters.clone(), params: Arc::new(PlayingControls::new(doc, &player)),
    });
    Ok((player, watch))
}

impl Slot {
    fn new(name: &str) -> Slot {
        Slot {
            name: name.to_string(),
            shared: Arc::new(Shared {
                latency: Arc::new(AtomicU32::new(0)),
                observer: PrepareControl::default(),
                next: AtomicPtr::new(std::ptr::null_mut()),
                retired: std::array::from_fn(|_| AtomicPtr::new(std::ptr::null_mut())),
                playing: AtomicU64::new(0),
                wanted: AtomicBool::new(false),
            }),
            host: Mutex::new(Host { doc: None, rate: None, generation: 0, reserved: 0, base: None, pending: false, posted: None, resources: 0, uses_midi: false, rewinding: false, watches: Vec::new(), control: None, in_flight: false, runner: None, diags: Vec::new() }),
        }
    }

    /// True during shader initialization for any prepare of this slot.
    pub fn initialising(&self) -> bool {
        self.shared.observer.executing()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Host> {
        self.host.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Submit prepares through `runner`, always outside the host lock.
    /// A name settling wakes only slots with a runner. Without one, a prepare
    /// deferred during retirement runs inline on the host's next [`Self::pump`].
    pub fn set_runner(&self, runner: Runner) {
        let old = self.lock().runner.replace(runner);
        drop(old);
    }

    /// Play `source` (loaded here: its problems come back).
    pub fn set_source(&self, source: &str, control: &PrepareControl) -> Result<(), crate::LoadError> {
        Ok(self.set_doc(ScoreDoc::load_controlled(source, control)?)?)
    }

    /// Play `doc`: when its graph differs from the one playing, its player
    /// is prepared through the runner (inline without one), once the node
    /// has taken the last one. Register its samples, singers and files first
    /// (`render::register_*`); after a later arrival, [`Slot::refresh`].
    pub fn set_doc(&self, doc: ScoreDoc) -> Result<(), Vec<Diag>> {
        let slice = score_slice(&doc, &self.name, ClockMode::Host);
        self.set_slice(doc, slice)
    }

    /// Keep the submitted graph with its document so a refresh prepares the same graph.
    pub(crate) fn set_slice(&self, doc: ScoreDoc, slice: OwnerSlice) -> Result<(), Vec<Diag>> {
        let uses_midi = !doc.streams.is_empty() || doc.insts.iter().any(|i| matches!(i.kind, crate::model::InstKind::Midi { .. }));
        let doc = Arc::new(doc);
        let slice = Arc::new(slice);
        let old = {
            let mut host = self.lock();
            host.edit();
            host.uses_midi = uses_midi;
            let old = host.doc.replace((doc, slice));
            host.pending = host.rewinding || host.needs_prepare();
            old
        };
        drop(old);
        self.kick();
        self.take_diags()
    }

    /// Prepare the score again (samples, singers or files it plays arrived:
    /// its instruments look them up when prepared).
    pub fn refresh(&self) -> Result<(), Vec<Diag>> {
        self.lock().invalidate();
        self.kick();
        self.take_diags()
    }

    /// The host's turn (every UI frame): frees what the audio thread
    /// retired and prepares what is due.
    pub fn pump(&self) -> Result<(), Vec<Diag>> {
        for retired in &self.shared.retired {
            let player = retired.swap(std::ptr::null_mut(), Ordering::AcqRel);
            if !player.is_null() {
                // SAFETY: made by `Box::into_raw` in `retire`; swapped out once.
                drop(unsafe { Box::from_raw(player) });
            }
        }
        let mut host = self.lock();
        let playing = self.shared.playing.load(Ordering::Acquire);
        let report = if let Some(at) = host.watches.iter().position(|w| w.generation == playing) {
            host.watches.drain(..at);
            host.watches[0].layout.take_report()
        } else { None };
        // A queued replacement will be checked when it starts playing. A
        // second pump must not queue another job against the same old layout.
        let nested_changed = playing != 0 && playing == host.generation && (host.rewinding || !host.in_flight && host.control.is_none())
            && host.base.as_ref().is_some_and(|layout| layout.nested_changed());
        let lost_player = self.shared.wanted.swap(false, Ordering::AcqRel) && host.base.is_some() && self.shared.next.load(Ordering::Acquire).is_null();
        if lost_player {
            // The node lost its player (its host graph changed without
            // carrying it over): a fresh one.
            host.base = None;
        }
        if nested_changed || lost_player { host.invalidate(); }
        drop(host);
        if let Some(line) = report { eprintln!("{line}"); }
        self.kick();
        self.take_diags()
    }

    /// A player for the slot's score that carries nothing over from the one
    /// playing: its nodes start as new and its loops' feeds are new rings
    /// (what waited in the old ones is gone). Fill its feeds, then
    /// [`Slot::post_fresh`] it. None before a node asked for a rate or the
    /// slot has a score.
    pub(crate) fn prepare_fresh(&self) -> Result<Option<FreshPlayer>, Vec<Diag>> {
        let (from, rate, generation, control, was_pending) = {
            let mut host = self.lock();
            let (Some(doc), Some(rate)) = (host.doc.clone(), host.rate) else { return Ok(None) };
            let was_pending = host.pending || host.in_flight;
            let (generation, control) = host.begin(true, &self.shared.observer);
            (PreparedFrom { doc, resources: host.resources }, rate, generation, control, was_pending)
        };
        let (doc, slice) = &from.doc;
        let result = prepare_hosted(doc, (**slice).clone(), rate, &self.name, None, generation, &control);
        let mut host = self.lock();
        if !host.wants(generation, &control) {
            drop(host);
            drop(result);
            self.kick();
            return Ok(None);
        }
        match result {
            Ok((player, watch)) => Ok(Some(FreshPlayer { from, player, watch, control })),
            Err(crate::LoadError::Cancelled) => Ok(None),
            Err(crate::LoadError::Problems(diags)) => {
                host.control = None;
                host.rewinding = false;
                if !host.matches(&from) {
                    host.pending = host.needs_prepare();
                    drop(host);
                    self.kick();
                    Ok(None)
                } else {
                    host.pending |= was_pending;
                    Err(diags)
                }
            }
        }
    }

    /// Publish only if this rewind still owns the requested generation.
    pub(crate) fn post_fresh(&self, fresh: FreshPlayer) {
        let generation = fresh.player.schedule.generation;
        self.finish(self.lock(), generation, &fresh.control, fresh.from, Ok((fresh.player, fresh.watch)));
    }

    /// Check the identity and publish in the same critical section for both prepare paths.
    fn finish(&self, mut host: std::sync::MutexGuard<'_, Host>, generation: u64, control: &PrepareControl, from: PreparedFrom,
        result: Result<(Player, Option<Watch>), crate::LoadError>) {
        if host.wants(generation, control) {
            host.control = None;
            host.rewinding = false;
            let retired = match result {
                Ok((player, watch)) => Some(self.post(&mut host, player, watch, from)),
                Err(crate::LoadError::Problems(diags)) => { host.diags = diags; None }
                Err(crate::LoadError::Cancelled) => None,
            };
            drop(host);
            drop(retired);
        } else {
            drop(host);
            drop(result);
        }
        self.kick();
    }

    // A posted rewind must reach the node before an edit can build on it.
    fn can_start_prepare(&self, host: &Host) -> bool {
        host.pending && !host.in_flight && host.control.is_none()
            && self.shared.next.load(Ordering::Acquire).is_null()
    }

    /// Prepare the pending score when the mailbox is empty (so the base is
    /// the player the node plays).
    fn kick(&self) {
        let (from, rate, base, generation, control, runner) = {
            let mut host = self.lock();
            if !self.can_start_prepare(&host) {
                return;
            }
            let (Some(doc), Some(rate)) = (host.doc.clone(), host.rate) else { return };
            if !crate::live::name_settled(&self.name) { return; }
            let (generation, control) = host.begin(false, &self.shared.observer);
            host.in_flight = true;
            (PreparedFrom { doc, resources: host.resources }, rate, host.base.clone(), generation, control, host.runner.clone())
        };
        let name = self.name.clone();
        let worker_control = control.clone();
        let job: Box<dyn FnOnce() + Send> = Box::new(move || {
            let slot = slot(&name);
            let prepared = crate::live::try_prepare(&name).map(|turn| {
                let (doc, slice) = &from.doc;
                let result = prepare_hosted(doc, (**slice).clone(), rate, &name, base.as_deref(), generation, &worker_control);
                (turn, result)
            });
            let mut host = slot.lock();
            host.in_flight = false;
            match prepared {
                Some((turn, result)) if turn.still_current() => {
                    slot.finish(host, generation, &worker_control, from, result);
                }
                stale => {
                    if host.wants(generation, &worker_control) {
                        host.control = None;
                        host.pending = true;
                    }
                    drop(host);
                    drop(stale);
                    // If settle saw in_flight, we see the settled name here.
                    // If it settles later, it sees pending with in_flight cleared.
                    if crate::live::name_settled(&name) { slot.kick(); }
                }
            }
        });
        match runner {
            Some(runner) => {
                if let Err(error) = runner(job) {
                    let mut host = self.lock();
                    host.in_flight = false;
                    if host.wants(generation, &control) {
                        host.control = None;
                        host.pending = true;
                        drop(host);
                        eprintln!("score: prepare: {error}");
                    } else {
                        drop(host);
                        self.kick();
                    }
                }
            }
            None => job(),
        }
    }

    /// Inline errors return in the call that prepared; runner errors are
    /// collected by a later set_doc, refresh or pump.
    fn take_diags(&self) -> Result<(), Vec<Diag>> {
        let diags = std::mem::take(&mut self.lock().diags);
        if diags.is_empty() { Ok(()) } else { Err(diags) }
    }

    /// Put `player` in the mailbox; free the one replaced after unlocking.
    fn post(&self, host: &mut Host, player: Player, watch: Option<Watch>, from: PreparedFrom) -> (Option<Box<Player>>, Option<PreparedFrom>) {
        host.generation = player.schedule.generation;
        host.watches.extend(watch);
        let old_doc = host.posted.replace(from);
        host.pending = host.needs_prepare();
        host.base = Some(player.layout.clone());
        self.shared.latency.store(player.schedule.latency as u32, Ordering::Release);
        let old = self.shared.next.swap(Box::into_raw(Box::new(player)), Ordering::AcqRel);
        let old_player = if !old.is_null() {
            // SAFETY: made by `Box::into_raw` here and never taken.
            Some(unsafe { Box::from_raw(old) })
        } else { None };
        (old_player, old_doc)
    }

    /// A node of this slot is prepared at `rate`: a player for it now,
    /// unless one at that rate plays or waits (the node carries it over).
    fn attach(&self, rate: u32) {
        let mut host = self.lock();
        if host.rate == Some(rate) && (host.base.is_some() || host.control.is_some()) {
            return;
        }
        if host.rate != Some(rate) {
            self.shared.latency.store(0, Ordering::Release);
        }
        host.rewinding = false;
        host.invalidate();
        host.rate = Some(rate);
        host.base = None;
        // A score that does not prepare leaves the node silent; the host
        // collects its diagnostics at the next pump.
        let old = self.shared.next.swap(std::ptr::null_mut(), Ordering::AcqRel);
        drop(host);
        if !old.is_null() {
            // SAFETY: as in `post`.
            drop(unsafe { Box::from_raw(old) });
        }
        self.kick();
    }

    /// The generation the node plays (0: none yet).
    pub fn playing(&self) -> u64 {
        self.shared.playing.load(Ordering::Acquire)
    }

    fn with_watch<T>(&self, read: impl FnOnce(&Watch) -> Option<T>) -> Option<T> {
        let playing = self.playing();
        let host = self.lock();
        host.watches.iter().find(|w| w.generation == playing).and_then(read)
    }

    /// The playing generation's layout gives its total latency and each
    /// node's own latency and aligned input frame.
    pub fn playing_layout(&self) -> Option<Arc<Layout>> {
        self.with_watch(|watch| Some(watch.layout.clone()))
    }

    /// The playing player's probe `node.port` (`node` its own id,
    /// `<slot>/<name>`): a loop's or an FX's `peak`, a Comp's `gr`.
    pub fn read(&self, node: &str, port: &str) -> Option<f32> {
        self.with_watch(|w| w.layout.probes.get(&format!("{node}.{port}")).map(|index| w.meters.probe(*index)))
    }

    /// The playing generation's probe reader of shader node `node`
    /// (`<slot>/<name>`): resolve it once per generation, then drain it
    /// each frame without allocating.
    pub fn probe_reader(&self, node: &str) -> Option<Arc<makepad_audio_engine::schedule::ProbeReader>> {
        self.with_watch(|watch| watch.layout.probe_taps.get(node).cloned())
    }

    /// Drain a playing shader node's records, with their names and cumulative fold count.
    pub fn probes(&self, node: &str) -> Option<(Vec<String>, Vec<makepad_score_player::shader::ProbeRecord>, u64)> {
        let tap = self.probe_reader(node)?;
        let mut records = Vec::new();
        let folded = tap.drain(|record| records.push(record));
        Some((tap.names.clone(), records, folded))
    }

    /// The playing player's stream `node.audio` over its last published
    /// pass: per bin (left min, left max, right min, right max).
    pub fn scope(&self, node: &str) -> Option<Vec<[f32; 4]>> {
        self.with_watch(|w| w.layout.audio_probes.get(&format!("{node}.audio")).map(|span| w.meters.scope(*span)))
    }

    /// The playing player's generation and the scope span of each of
    /// `nodes` (`<slot>/<name>`): resolved once per player, then read with
    /// [`Slot::scope_read`] without allocating.
    pub fn scope_spans(&self, nodes: &[String]) -> Option<(u64, Vec<Option<u16>>)> {
        let playing = self.playing();
        self.with_watch(|w| Some((playing, nodes.iter().map(|n| w.layout.audio_probes.get(&format!("{n}.audio")).copied()).collect())))
    }

    /// Span `span`'s last published pass into `out` (per bin: left min,
    /// left max, right min, right max) while generation `generation`
    /// plays; false once another plays (resolve the spans again).
    pub fn scope_read(&self, generation: u64, span: u16, out: &mut [[f32; 4]]) -> bool {
        if self.playing() != generation {
            return false;
        }
        self.with_watch(|w| {
            w.meters.scope_into(span, out);
            Some(())
        }).is_some()
    }

    /// Resolve once on a generation change. Subsequent value reads only load the bank atomic.
    pub fn playing_controls(&self) -> Option<Arc<PlayingControls>> {
        self.with_watch(|w| Some(w.params.clone()))
    }

    /// Hold the playing player's control `node.param` at `value` (until
    /// the next score plays); false when it has none.
    pub fn set_control(&self, node: &str, param: &str, value: f32) -> bool {
        self.with_watch(|w| w.layout.controls.get(&format!("{node}.{param}")).map(|(index, _)| w.controls.set_manual(*index, value))).is_some()
    }
}

/// The node's state on the audio thread.
struct ScoreNode {
    shared: Arc<Shared>,
    player: Option<Box<Player>>,
    rate: u32,
    key: u64,
}

fn factory(node: &LiveNode, rate: u32, _generation: u64, _base: Option<&Layout>) -> Result<Box<dyn NestedNode>, PlanError> {
    let name = match node.param("score") {
        Some(Literal::Str(name) | Literal::Id(name)) if !name.is_empty() => name.clone(),
        _ => return Err(PlanError::at(&node.id, Some("score"), "a score slot's name")),
    };
    let slot = slot(&name);
    slot.attach(rate);
    Ok(Box::new(ScoreNode { shared: slot.shared.clone(), player: None, rate, key: crate::key(&name) }))
}

impl ScoreNode {
    fn retire(&self, player: Box<Player>) {
        let raw = Box::into_raw(player);
        for bin in &self.shared.retired {
            if bin.compare_exchange(std::ptr::null_mut(), raw, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                return;
            }
        }
        // Every bin is full (the host has not pumped): it stays allocated
        // rather than be freed here.
    }

    /// The newest prepared player, if one waits: it takes over, carrying
    /// the staying nodes' state over from the one playing.
    fn take_next(&mut self) {
        let raw = self.shared.next.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if raw.is_null() {
            return;
        }
        // SAFETY: made by `Box::into_raw` in `Slot::post`; swapped out once.
        let mut next = unsafe { Box::from_raw(raw) };
        if next.schedule.rate != self.rate {
            self.retire(next);
            return;
        }
        if let Some(current) = &mut self.player {
            if next.schedule.base == Some(current.schedule.generation) {
                next.schedule.take_over(&mut current.schedule);
            } else {
                // Nothing carries over: its notes on MIDI outputs end here.
                current.release_all();
            }
        }
        self.shared.playing.store(next.schedule.generation, Ordering::Release);
        if let Some(old) = self.player.replace(next) {
            self.retire(old);
        }
    }
}

impl NestedNode for ScoreNode {
    fn latency(&self) -> Arc<AtomicU32> {
        self.shared.latency.clone()
    }

    fn key(&self) -> u64 {
        self.key
    }

    fn run(&mut self, clock: &ClockChunk, left: &mut [f32], right: &mut [f32]) {
        self.take_next();
        match &mut self.player {
            Some(player) => player.run_on(clock, left, right),
            None => {
                left.fill(0.0);
                right.fill(0.0);
                self.shared.wanted.store(true, Ordering::Release);
            }
        }
    }

    fn running(&self) -> bool {
        !self.shared.next.load(Ordering::Acquire).is_null() || self.player.as_ref().is_some_and(|player| player.busy())
    }
}

impl Drop for ScoreNode {
    fn drop(&mut self) {
        // Dropped with its host's schedule, off the audio thread: the
        // player it holds goes with it. A slot's next node prepares anew.
        if self.player.is_some() {
            self.shared.playing.store(0, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod retirement_tests {
    use super::*;
    use crate::live::{ClockSource, LiveConfig, LiveScore, LiveThread};
    use makepad_audio_engine::feed::{midi_out, tempo_feed};
    use makepad_audio_engine::schedule::NodeState;
    use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(10);

    struct Clock {
        located: SyncSender<()>,
        release: Mutex<Option<Receiver<()>>>,
        retired: Sender<()>,
    }

    impl ClockSource for Clock {
        fn now(&self) -> Option<(f64, f64, bool)> { Some((0.0, 120.0, false)) }
        fn located(&self) -> u64 {
            let _ = self.located.try_send(());
            if let Some(release) = self.release.lock().unwrap().take() {
                release.recv_timeout(TIMEOUT).expect("the dropping thread can release the old step");
            }
            0
        }
    }

    impl Drop for Clock {
        fn drop(&mut self) { let _ = self.retired.send(()); }
    }

    fn config(name: &str, release: Option<Receiver<()>>) -> (LiveConfig, Receiver<()>, Receiver<()>) {
        let (located, step) = sync_channel(2);
        let (retired, end) = channel();
        let clock = Arc::new(Clock { located, release: Mutex::new(release), retired });
        (LiveConfig { instance: name.into(), lookahead: 0.15, clock }, step, end)
    }

    #[test]
    fn retiring_composers_order_feeds_and_hosted_midi_prepares() {
        const RATE: u32 = 8_000;
        let source = "use mod.score.*\nlet c = Clock{bpm: 120}\nlet m = Instrument{kind: @midi channel: 1}\nlet l = Loop{play: m beats: 1 notes: [[0 60 0.25 1]]}\nScore{c m l}";
        let doc = ScoreDoc::test_load(source).expect("a checked MIDI loop");
        for how in ["document", "attach", "rewind", "abandoned", "invalid", "runner"] {
            let name = format!("retire_{how}");
            let transport = crate::player::transport(&name);
            let slot = slot(&name);
            let (release, hold) = channel();
            let (old_config, entered, retired) = config(&name, Some(hold));
            let old = LiveScore::with_owned_feeds(old_config);
            let old_feed = tempo_feed(&transport);
            let mut old_ring = old_feed.fresh();
            old.rewind(&transport);
            entered.recv_timeout(TIMEOUT).expect("old composer is inside its final rewind");

            let runner_jobs = (how == "runner").then(|| {
                let (submit, jobs) = channel();
                slot.set_runner(Arc::new(move |job| submit.send(job).map_err(|e| e.to_string())));
                jobs
            });
            if how != "attach" { slot.attach(RATE); }
            if matches!(how, "attach" | "rewind") { slot.set_doc(doc.clone()).unwrap(); }
            let mut node = ScoreNode { shared: slot.shared.clone(), player: None, rate: RATE, key: crate::key(&name) };
            node.take_next();
            let before = slot.lock().reserved;
            drop(old);
            let (next_config, locates, next_retired) = config(&name, None);
            let next_thread = LiveThread::new(next_config, &transport);
            let next = if how == "abandoned" {
                drop(next_thread); // the launch never gets claimed or registers a feed
                None
            } else {
                let next = LiveScore::from_thread(next_thread);
                if how == "rewind" { next.set_source(source, &crate::PrepareControl::default()); }
                if how == "invalid" { next.set_source("let =", &crate::PrepareControl::default()); }
                next.rewind(&transport);
                Some(next)
            };
            match how {
                "attach" => slot.attach(RATE),
                "rewind" => {} // only the composer's rewind may post the fresh player
                _ => slot.set_doc(doc.clone()).unwrap(),
            }
            slot.pump().unwrap();
            assert_eq!(slot.lock().reserved, before, "{how}: no prepare began during retirement");
            if let Some(jobs) = &runner_jobs {
                assert!(matches!(jobs.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)), "the owner has no job while the old composer is held");
            }
            assert!(old_ring.pop().is_none(), "{how}: drop returned before the old step wrote");
            if how == "document" {
                let (other, steps, gone) = config("retire_independent", None);
                let other = LiveScore::new(other);
                other.rewind("retire_independent_clock");
                steps.recv_timeout(TIMEOUT).expect("a different name can write during retirement");
                drop(other);
                gone.recv_timeout(TIMEOUT).expect("the independent composer finished");
            }

            release.send(()).expect("drop returned with the old composer still held inside its step");
            retired.recv_timeout(TIMEOUT).expect("old worker cleaned up its feeds");
            assert!(old_ring.pop().is_some_and(|e| e.locate), "{how}: the old worker finished its last write");
            if next.is_some() {
                // The first located() is before the new locate write. The next
                // is the pump checking its acknowledgement, after post_fresh.
                for _ in 0..2 { locates.recv_timeout(TIMEOUT).expect("replacement completed its rewind"); }
                let new_feed = tempo_feed(&transport);
                assert!(!Arc::ptr_eq(&old_feed, &new_feed), "{how}: old cleanup preceded new registration");
                assert!(new_feed.fresh().pop().is_some_and(|e| e.locate), "{how}: the replacement wrote its own ring");
                assert!(old_ring.pop().is_none(), "{how}: the old ring got only the old composer's locate");
            } else {
                next_retired.recv_timeout(TIMEOUT).expect("the unclaimed replacement also retired");
            }
            if let Some(jobs) = runner_jobs {
                // Both composers have signalled above. Let the owner run its queue,
                // including jobs enqueued by a wake, without another host pump.
                assert!(slot.shared.next.load(Ordering::Acquire).is_null(), "the settling composer left the prepare to its owner");
                let (done, finished) = channel();
                let owner = std::thread::spawn(move || {
                    for job in jobs.try_iter() {
                        job();
                        done.send(()).unwrap();
                    }
                });
                finished.recv_timeout(TIMEOUT).expect("settle must wake the runner without a host pump");
                owner.join().unwrap();
                drop(slot.lock().runner.take());
            } else if how != "rewind" { slot.pump().unwrap(); }
            node.take_next();
            let player = node.player.as_ref().expect("the pending slot published a player");
            let live = !matches!(how, "abandoned" | "invalid");
            assert!(player.schedule.generation > before, "{how}: a fresh generation reached the node");
            assert_eq!(player.schedule.states.iter().find_map(|s| match s {
                NodeState::Loop(l) => Some(l.midi_live), _ => None,
            }), Some(live), "{how}: the player captured the settled MIDI registration");
            assert_eq!(midi_out::is_live(&format!("{name}/l")), live, "{how}: final MIDI registration");
            if let Some(next) = next {
                drop(next);
                next_retired.recv_timeout(TIMEOUT).expect("the replacement retired");
            }
        }
    }
}
