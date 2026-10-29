//! `Score`: a score as one node of any audio flow graph. The node plays
//! the score its slot holds (`Score{ score: "score" clock: transport.out(@clock) }`),
//! following the clock wired into it, and outputs the master.
//!
//! - The host gives a slot its score ([`Slot::set_doc`], [`Slot::set_source`]);
//!   the slot prepares a player for it off the audio thread, against the one
//!   playing, and hands it to the node through a one-place mailbox. The node
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
use crate::player::{prepare_slice, slice as score_slice, ClockMode, Meters, Player};
use makepad_audio_engine::nested::NestedNode;
use makepad_audio_engine::nodes::{input, node, output, AUDIO, CLOCK, LEVEL};
use makepad_audio_engine::schedule::{migrate_state, ClockChunk, ControlBank, Layout};
use makepad_flow::document::{ParamDescriptor, ParamType, Registry, TypeDescriptor};
use makepad_flow::live::plan::{LiveNode, OwnerSlice, PlanError};
use makepad_flow::Literal;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The node type.
pub const TYPE: &str = "Score";
/// The slot a `Score` node without a `score` setting plays.
pub const DEFAULT_SLOT: &str = "score";
/// Players retired by the audio thread the host has not freed yet.
const RETIRED: usize = 8;

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
}

struct Host {
    doc: Option<ScoreDoc>,
    /// The device rate the slot's node runs at (from its last prepare).
    rate: Option<u32>,
    generation: u64,
    /// The layout of the newest player prepared: the base of the next.
    base: Option<Arc<Layout>>,
    /// A score is waiting for the mailbox to empty.
    pending: bool,
    /// What the newest player was prepared from (a score that makes the
    /// same graph needs none: its composer changes what it plays).
    slice: Option<OwnerSlice>,
    /// The players prepared and not yet replaced twice: the playing one is
    /// among them.
    watches: Vec<Watch>,
}

/// A score the `Score` nodes named after it play.
pub struct Slot {
    name: String,
    shared: Arc<Shared>,
    host: Mutex<Host>,
}

impl Slot {
    fn new(name: &str) -> Slot {
        Slot {
            name: name.to_string(),
            shared: Arc::new(Shared {
                next: AtomicPtr::new(std::ptr::null_mut()),
                retired: std::array::from_fn(|_| AtomicPtr::new(std::ptr::null_mut())),
                playing: AtomicU64::new(0),
                wanted: AtomicBool::new(false),
            }),
            host: Mutex::new(Host { doc: None, rate: None, generation: 0, base: None, pending: false, slice: None, watches: Vec::new() }),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Host> {
        self.host.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Play `source` (loaded here: its problems come back).
    pub fn set_source(&self, source: &str) -> Result<(), Vec<Diag>> {
        self.set_doc(ScoreDoc::load(source)?)
    }

    /// Play `doc`: when its graph differs from the one playing, its player
    /// is prepared now (or at the next pump, when the node has not taken the
    /// last one yet). Register its samples, singers and files first
    /// (`render::register_*`); after a later arrival, [`Slot::refresh`].
    pub fn set_doc(&self, doc: ScoreDoc) -> Result<(), Vec<Diag>> {
        let mut host = self.lock();
        let same = host.slice.as_ref().is_some_and(|slice| *slice == score_slice(&doc, &self.name, ClockMode::Host));
        host.doc = Some(doc);
        host.pending |= !same;
        self.prepare_due(&mut host)
    }

    /// Prepare the score again (samples, singers or files it plays arrived:
    /// its instruments look them up when prepared).
    pub fn refresh(&self) -> Result<(), Vec<Diag>> {
        let mut host = self.lock();
        host.pending = true;
        self.prepare_due(&mut host)
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
        if let Some(at) = host.watches.iter().position(|w| w.generation == playing) {
            host.watches.drain(..at);
        }
        if self.shared.wanted.swap(false, Ordering::AcqRel) && self.shared.next.load(Ordering::Acquire).is_null() {
            // The node lost its player (its host graph changed without
            // carrying it over): a fresh one.
            host.base = None;
            host.pending = true;
        }
        self.prepare_due(&mut host)
    }

    /// Prepare the pending score when the mailbox is empty (so the base is
    /// the player the node plays).
    fn prepare_due(&self, host: &mut Host) -> Result<(), Vec<Diag>> {
        if !host.pending || !self.shared.next.load(Ordering::Acquire).is_null() {
            return Ok(());
        }
        let (Some(doc), Some(rate)) = (host.doc.as_ref(), host.rate) else { return Ok(()) };
        host.pending = false;
        let generation = host.generation + 1;
        let slice = score_slice(doc, &self.name, ClockMode::Host);
        let player = prepare_slice(doc, slice.clone(), rate as f32, &self.name, ClockMode::Host, host.base.as_deref(), generation)?;
        host.generation = generation;
        host.slice = Some(slice);
        self.post(host, player);
        Ok(())
    }

    /// Put `player` in the mailbox (one there not taken is freed here).
    fn post(&self, host: &mut Host, player: Player) {
        host.base = Some(player.layout.clone());
        if let Some(meters) = &player.meters {
            host.watches.push(Watch { generation: player.schedule.generation, layout: player.layout.clone(), controls: player.schedule.controls.clone(), meters: meters.clone() });
        }
        let old = self.shared.next.swap(Box::into_raw(Box::new(player)), Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: made by `Box::into_raw` here and never taken.
            drop(unsafe { Box::from_raw(old) });
        }
    }

    /// A node of this slot is prepared at `rate`: a player for it now,
    /// unless one at that rate plays or waits (the node carries it over).
    fn attach(&self, rate: u32) {
        let mut host = self.lock();
        if host.rate == Some(rate) && host.base.is_some() {
            return;
        }
        host.rate = Some(rate);
        host.base = None;
        host.pending = true;
        // A score that does not prepare leaves the node silent; the problem
        // came back from `set_doc` when it was given.
        if !self.shared.next.load(Ordering::Acquire).is_null() {
            let old = self.shared.next.swap(std::ptr::null_mut(), Ordering::AcqRel);
            if !old.is_null() {
                // SAFETY: as in `post`.
                drop(unsafe { Box::from_raw(old) });
            }
        }
        let _ = self.prepare_due(&mut host);
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

    /// The playing player's probe `node.port` (`node` its own id,
    /// `<slot>/<name>`): a loop's or an FX's `peak`, a Comp's `gr`.
    pub fn read(&self, node: &str, port: &str) -> Option<f32> {
        self.with_watch(|w| w.layout.probes.get(&format!("{node}.{port}")).map(|index| w.meters.probe(*index)))
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
                for (from, to) in next.schedule.migrate.iter() {
                    migrate_state(&mut current.schedule.states[*from as usize], &mut next.schedule.states[*to as usize]);
                }
            }
        }
        self.shared.playing.store(next.schedule.generation, Ordering::Release);
        if let Some(old) = self.player.replace(next) {
            self.retire(old);
        }
    }
}

impl NestedNode for ScoreNode {
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
