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
    static SLOTS: OnceLock<Mutex<HashMap<String, Arc<Slot>>>> = OnceLock::new();
    let mut slots = SLOTS.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner());
    slots.entry(name.to_string()).or_insert_with(|| Arc::new(Slot::new(name))).clone()
}

/// What the audio thread and the host share.
struct Shared {
    /// Latency of the newest published player, shared with its parent layouts.
    latency: Arc<AtomicU32>,
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

    fn begin(&mut self, rewind: bool) -> (u64, PrepareControl) {
        self.supersede();
        self.rewinding = rewind;
        self.reserved += 1;
        let control = PrepareControl::default();
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
                next: AtomicPtr::new(std::ptr::null_mut()),
                retired: std::array::from_fn(|_| AtomicPtr::new(std::ptr::null_mut())),
                playing: AtomicU64::new(0),
                wanted: AtomicBool::new(false),
            }),
            host: Mutex::new(Host { doc: None, rate: None, generation: 0, reserved: 0, base: None, pending: false, posted: None, resources: 0, rewinding: false, watches: Vec::new(), control: None, in_flight: false, runner: None, diags: Vec::new() }),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Host> {
        self.host.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Submit prepares through `runner`, always outside the host lock.
    /// Without a runner, tests, offline renders and hosts without a pool
    /// use the same snapshot/prepare/post path on the calling thread.
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
        let doc = Arc::new(doc);
        let slice = Arc::new(slice);
        let old = {
            let mut host = self.lock();
            host.edit();
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
            let (generation, control) = host.begin(true);
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

    /// Prepare the pending score when the mailbox is empty (so the base is
    /// the player the node plays).
    fn kick(&self) {
        let (from, rate, base, generation, control, runner) = {
            let mut host = self.lock();
            if !host.pending || host.in_flight || host.control.is_some() || !self.shared.next.load(Ordering::Acquire).is_null() {
                return;
            }
            let (Some(doc), Some(rate)) = (host.doc.clone(), host.rate) else { return };
            let (generation, control) = host.begin(false);
            host.in_flight = true;
            (PreparedFrom { doc, resources: host.resources }, rate, host.base.clone(), generation, control, host.runner.clone())
        };
        let name = self.name.clone();
        let worker_control = control.clone();
        let job: Box<dyn FnOnce() + Send> = Box::new(move || {
            let (doc, slice) = &from.doc;
            let result = prepare_hosted(doc, (**slice).clone(), rate, &name, base.as_deref(), generation, &worker_control);
            let slot = slot(&name);
            let mut host = slot.lock();
            host.in_flight = false;
            slot.finish(host, generation, &worker_control, from, result);
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
