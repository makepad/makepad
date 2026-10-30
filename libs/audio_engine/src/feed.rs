//! How a loop's events reach its node: the composer (on its own thread)
//! sends beat-stamped events into a ring the `Loop` node's state reads on
//! the audio thread; a transport's tempo changes reach it the same way. A
//! feed is named by the node's qualified id; the ring's reading end lives in
//! the node's state (it moves with it across revisions), the writing end
//! here, for whoever composes. Whoever instantiates a graph under a name
//! forgets its feeds when done ([`forget_instance`]).

use makepad_flow::live::spsc::{ring, Consumer, Producer};
use makepad_score_player::Event;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Events a loop may have in flight between the composer and its node.
pub const FEED_EVENTS: usize = 4096;
/// Tempo changes a transport may have waiting.
pub const TEMPO_EVENTS: usize = 64;
/// The tempo range a transport plays (a performance's fermata can hold a
/// beat for seconds).
pub const TEMPO_MIN: f64 = 4.0;
pub const TEMPO_MAX: f64 = 400.0;

/// A tempo change: from `beat`, `bpm`, reached over `ramp` beats (0: at
/// once; a ramp moves in sixteenth-beat steps).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TempoEvent {
    pub beat: f64,
    pub bpm: f64,
    pub ramp: f64,
}

/// Seconds from beat 0 to `beat` on a transport that starts at `bpm` and
/// takes `changes` (in beat order): exactly the steps the transport makes,
/// so a timeline's beat and bar marks fall where the audio's do.
pub fn tempo_seconds(bpm: f64, changes: &[TempoEvent], beat: f64) -> f64 {
    let (mut tempo, mut at, mut seconds) = (bpm, 0.0f64, 0.0f64);
    let mut ramp: Option<(f64, f64, f64, u32, u32)> = None;
    let mut next_event = 0;
    loop {
        let ramp_next = ramp.and_then(|(start, _, _, steps, next)| (next < steps).then_some(start + next as f64 / 16.0));
        let event_next = changes.get(next_event).map(|e| e.beat);
        let next = match (ramp_next, event_next) {
            (Some(r), Some(e)) => r.min(e),
            (Some(r), None) => r,
            (None, Some(e)) => e,
            (None, None) => break,
        };
        if next >= beat {
            break;
        }
        let next = next.max(at);
        seconds += (next - at) * 60.0 / tempo;
        at = next;
        if event_next.is_some_and(|e| e <= next) {
            let event = changes[next_event];
            next_event += 1;
            if event.ramp > 0.0 {
                let steps = (event.ramp * 16.0).round().max(1.0) as u32;
                ramp = Some((event.beat, tempo, event.bpm, steps, 0));
            } else {
                ramp = None;
                tempo = event.bpm;
            }
        }
        if let Some((start, from, to, steps, step)) = ramp {
            if start + step as f64 / 16.0 <= next + 1e-12 && step < steps {
                tempo = from + (to - from) * (step + 1) as f64 / steps as f64;
                ramp = if step + 1 < steps { Some((start, from, to, steps, step + 1)) } else { None };
            }
        }
        tempo = tempo.clamp(TEMPO_MIN, TEMPO_MAX);
    }
    seconds + (beat - at).max(0.0) * 60.0 / tempo
}

/// An FX setting moving to `value` from `beat`, over `ramp` beats (0: at
/// once): a score's automation (a param it follows, a section's settings).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AutomationEvent {
    pub beat: f64,
    pub param: u8,
    pub value: f32,
    pub ramp: f32,
}

/// Automation an FX may have waiting.
pub const AUTOMATION_EVENTS: usize = 64;

pub struct Feed<T> {
    inner: Mutex<Inner<T>>,
    capacity: usize,
}

struct Inner<T> {
    producer: Option<Producer<T>>,
    /// Events sent before a node read this feed (a loop the composer knows
    /// before its graph revision is prepared); they go in first.
    backlog: Vec<T>,
}

pub type LoopFeed = Feed<Event>;
pub type PadFeed = Feed<PadHit>;

/// Hits a `Pads` node may have waiting.
pub const PAD_EVENTS: usize = 256;

/// A hit on one of a `Pads` node's pads: its sample from the start at
/// `gain`, or (`stop`) every voice of that pad let go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PadHit {
    pub pad: u32,
    pub gain: f32,
    pub stop: bool,
}
pub type TempoFeed = Feed<TempoEvent>;
pub type AutomationFeed = Feed<AutomationEvent>;

impl<T: Send> Feed<T> {
    fn new(capacity: usize) -> Feed<T> {
        Feed { inner: Mutex::new(Inner { producer: None, backlog: Vec::new() }), capacity }
    }

    /// A new ring: its reading end for a node that starts fresh; the
    /// writing end replaces the previous one here, and what waited goes in.
    pub fn fresh(&self) -> Consumer<T> {
        let (mut producer, consumer) = ring(self.capacity);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for event in inner.backlog.drain(..) {
            let _ = producer.push(event);
        }
        inner.producer = Some(producer);
        consumer
    }

    /// Send an event; false when the ring is full.
    pub fn send(&self, event: T) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let inner = &mut *inner;
        match inner.producer.as_mut() {
            Some(producer) => producer.push(event).is_ok(),
            None if inner.backlog.len() < self.capacity => {
                inner.backlog.push(event);
                true
            }
            None => false,
        }
    }
}

/// Settings of a loop's instrument held live by an editor (a control under
/// a finger): a shader instrument's params by name, a recipe synth's layer
/// keys (`cutoff`, `q`, `a`, …). The host writes them; the `Loop` node
/// reads them on the audio thread before its next pass when the version
/// moved (a lock it only tries: a pass that finds it taken reads on the
/// next), and sets them on its instrument without a new revision: no
/// prepare, no compile. Cleared once the source the editor wrote plays.
#[derive(Default)]
pub struct LiveParams {
    version: std::sync::atomic::AtomicU32,
    values: Mutex<Vec<(String, f32)>>,
}

/// Values one instrument may hold live at once.
pub const LIVE_PARAMS: usize = 64;

impl LiveParams {
    /// Hold `name` at `value` (the host; never the audio thread).
    pub fn set(&self, name: &str, value: f32) {
        let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
        match values.iter().position(|(n, _)| n == name) {
            Some(at) => values[at].1 = value,
            None if values.len() < LIVE_PARAMS => {
                let len = values.len();
                if values.capacity() < LIVE_PARAMS {
                    values.reserve_exact(LIVE_PARAMS - len);
                }
                values.push((name.to_string(), value));
            }
            None => return,
        }
        self.version.fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    /// Let every value go: the instrument plays its authored settings.
    pub fn clear(&self) {
        let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
        if !values.is_empty() {
            values.clear();
            self.version.fetch_add(1, std::sync::atomic::Ordering::Release);
        }
    }

    pub fn version(&self) -> u32 {
        self.version.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Whether anything is held.
    pub fn holding(&self) -> bool {
        !self.values.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    /// The values held, for the audio thread: false (and nothing read) when
    /// the host holds the lock right now. Never allocates.
    pub fn try_read(&self, mut f: impl FnMut(&str, f32)) -> bool {
        let Ok(values) = self.values.try_lock() else { return false };
        for (name, value) in values.iter() {
            f(name, *value);
        }
        true
    }
}

struct Registry {
    loops: HashMap<String, Arc<LoopFeed>>,
    tempos: HashMap<String, Arc<TempoFeed>>,
    fxs: HashMap<String, Arc<AutomationFeed>>,
    live: HashMap<String, Arc<LiveParams>>,
    pads: HashMap<String, Arc<PadFeed>>,
}

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    static FEEDS: OnceLock<Mutex<Registry>> = OnceLock::new();
    FEEDS.get_or_init(|| Mutex::new(Registry { loops: HashMap::new(), tempos: HashMap::new(), fxs: HashMap::new(), live: HashMap::new(), pads: HashMap::new() })).lock().unwrap_or_else(|e| e.into_inner())
}

/// The live settings of the `Loop` node with this qualified id (made on
/// first use). Never called on the audio thread.
pub fn live_params(node: &str) -> Arc<LiveParams> {
    registry().live.entry(node.to_string()).or_default().clone()
}

/// The feed of the `Loop` node with this qualified id (made on first use).
/// Never called on the audio thread.
pub fn loop_feed(node: &str) -> Arc<LoopFeed> {
    registry().loops.entry(node.to_string()).or_insert_with(|| Arc::new(Feed::new(FEED_EVENTS))).clone()
}

/// The hits of the `Pads` node with this qualified id (made on first use).
/// Never called on the audio thread.
pub fn pad_feed(node: &str) -> Arc<PadFeed> {
    registry().pads.entry(node.to_string()).or_insert_with(|| Arc::new(Feed::new(PAD_EVENTS))).clone()
}

/// The tempo feed of the `Transport` node with this qualified id.
pub fn tempo_feed(node: &str) -> Arc<TempoFeed> {
    registry().tempos.entry(node.to_string()).or_insert_with(|| Arc::new(Feed::new(TEMPO_EVENTS))).clone()
}

/// The automation feed of the FX node with this qualified id.
pub fn automation_feed(node: &str) -> Arc<AutomationFeed> {
    registry().fxs.entry(node.to_string()).or_insert_with(|| Arc::new(Feed::new(AUTOMATION_EVENTS))).clone()
}

/// Forget every feed of the graph instantiated as `name`: its loops
/// (`<name>/<loop>`) and its transport (`<name>_clock`). Nodes still
/// playing keep their rings until their graph is dropped.
pub fn forget_instance(name: &str) {
    let prefix = format!("{name}/");
    let clock = format!("{name}_clock");
    let mut registry = registry();
    registry.loops.retain(|key, _| !key.starts_with(&prefix));
    registry.tempos.retain(|key, _| *key != clock && !key.starts_with(&prefix));
    registry.fxs.retain(|key, _| !key.starts_with(&prefix));
    registry.live.retain(|key, _| !key.starts_with(&prefix));
    registry.pads.retain(|key, _| !key.starts_with(&prefix));
    drop(registry);
    midi_in::forget_instance(name);
    streams::forget_instance(name);
}

/// How many feeds exist (loops, tempos): what a leak test watches.
pub fn registry_len() -> (usize, usize) {
    let registry = registry();
    (registry.loops.len(), registry.tempos.len())
}

/// The key a loop's voices are seeded by: the hash of its own name (the
/// last segment of its qualified id), so a score sounds the same wherever
/// its graph is instantiated (Stage's root, an offline render, a game).
pub fn key_of(node: &str) -> u64 {
    let name = node.rsplit('/').next().unwrap_or(node);
    name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)).max(1)
}

/// Decoded samples a graph's samplers and kit slots play, by name (a
/// library asset's `id@rev`). The host registers them before it prepares
/// a graph that names them; preparing looks them up (never the audio
/// thread). A name nobody registered plays silence.
pub mod samples {
    use makepad_score_player::Sample;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    fn table() -> &'static Mutex<HashMap<String, Arc<Sample>>> {
        static SAMPLES: OnceLock<Mutex<HashMap<String, Arc<Sample>>>> = OnceLock::new();
        SAMPLES.get_or_init(Default::default)
    }

    pub fn register(name: &str, sample: Arc<Sample>) {
        table().lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_string(), sample);
    }

    pub fn get(name: &str) -> Option<Arc<Sample>> {
        table().lock().unwrap_or_else(|e| e.into_inner()).get(name).cloned()
    }

    /// Forget a sample (graphs already prepared keep theirs).
    pub fn forget(name: &str) {
        table().lock().unwrap_or_else(|e| e.into_inner()).remove(name);
    }
}

/// Singer profiles a graph's `Voice` loops sing with, by name (a library
/// asset's `id@rev`), beside the built-in voices. Registered and looked up
/// like `samples`; a name nobody registered sings with the built-in alto
/// until it arrives.
pub mod voices {
    use makepad_score_player::sung::VoiceProfile;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    fn table() -> &'static Mutex<HashMap<String, Arc<VoiceProfile>>> {
        static VOICES: OnceLock<Mutex<HashMap<String, Arc<VoiceProfile>>>> = OnceLock::new();
        VOICES.get_or_init(Default::default)
    }

    pub fn register(name: &str, profile: Arc<VoiceProfile>) {
        table().lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_string(), profile);
    }

    pub fn get(name: &str) -> Option<Arc<VoiceProfile>> {
        table().lock().unwrap_or_else(|e| e.into_inner()).get(name).cloned()
    }

    /// Forget a profile (graphs already prepared keep theirs).
    pub fn forget(name: &str) {
        table().lock().unwrap_or_else(|e| e.into_inner()).remove(name);
    }
}

/// Files a graph's native instruments load (a SoundFont's bytes), by name
/// (a library asset's `id@rev`); registered and looked up like `samples`.
pub mod files {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    fn table() -> &'static Mutex<HashMap<String, Arc<[u8]>>> {
        static FILES: OnceLock<Mutex<HashMap<String, Arc<[u8]>>>> = OnceLock::new();
        FILES.get_or_init(Default::default)
    }

    pub fn register(name: &str, bytes: Arc<[u8]>) {
        table().lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_string(), bytes);
    }

    pub fn get(name: &str) -> Option<Arc<[u8]>> {
        table().lock().unwrap_or_else(|e| e.into_inner()).get(name).cloned()
    }

    pub fn forget(name: &str) {
        table().lock().unwrap_or_else(|e| e.into_inner()).remove(name);
    }
}

/// MIDI the graph's MIDI loops send, for the host to put on its outputs:
/// the audio thread stamps each message with when it is due (seconds on
/// [`midi_out::now`]'s clock) and queues it without blocking; the host
/// takes what is due and sends it.
pub mod midi_out {
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};

    /// Messages that may wait.
    pub const CAPACITY: usize = 4096;

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct MidiOut {
        pub at: f64,
        /// The output port's name hash ([`port_hash`]); 0 is every output.
        pub port: u64,
        pub data: [u8; 3],
    }

    fn queue() -> &'static Mutex<VecDeque<MidiOut>> {
        static QUEUE: OnceLock<Mutex<VecDeque<MidiOut>>> = OnceLock::new();
        QUEUE.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAPACITY)))
    }

    /// Make the queue and start the clock (off the audio thread, when a
    /// graph with a MIDI loop is planned).
    pub fn init() {
        let _ = queue();
        let _ = crate::clock::now();
    }

    fn live() -> &'static Mutex<Vec<String>> {
        static LIVE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
        LIVE.get_or_init(Default::default)
    }

    /// Whether the graph instantiated as `instance` plays live: its MIDI
    /// loops to the outputs and its `MidiIn` loops from the inputs
    /// ([`super::midi_in`]). The live score does; offline renders and
    /// previews do not (set before the graph is prepared).
    pub fn set_live(instance: &str, on: bool) {
        let mut live = live().lock().unwrap_or_else(|e| e.into_inner());
        live.retain(|name| name != instance);
        if on {
            live.push(instance.to_string());
        }
    }

    /// Whether the MIDI loop `node` (a qualified id) sends.
    pub fn is_live(node: &str) -> bool {
        live().lock().unwrap_or_else(|e| e.into_inner()).iter().any(|name| node.strip_prefix(name.as_str()).is_some_and(|rest| rest.starts_with('/')))
    }

    /// Seconds on the queue's clock ([`crate::clock`]).
    pub fn now() -> f64 {
        crate::clock::now()
    }

    /// The hash a port name is sent under ("" is every output).
    pub fn port_hash(name: &str) -> u64 {
        if name.is_empty() {
            return 0;
        }
        name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)) | 1
    }

    /// From the audio thread: queue `f`'s messages unless the host holds
    /// the queue this instant (then false: try again next chunk). Never
    /// blocks or allocates.
    pub fn try_send(f: impl FnOnce(&mut dyn FnMut(MidiOut))) -> bool {
        let Ok(mut q) = queue().try_lock() else { return false };
        f(&mut |m| {
            if q.len() < CAPACITY {
                q.push_back(m);
            }
        });
        true
    }

    /// The messages due by `now` (seconds), in time order.
    pub fn take_due(now: f64, out: &mut Vec<MidiOut>) {
        let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
        // Loops queue in their own order: pick every due one.
        let mut i = 0;
        while i < q.len() {
            if q[i].at <= now {
                if let Some(m) = q.remove(i) {
                    out.push(m);
                }
            } else {
                i += 1;
            }
        }
        out.sort_by(|a, b| a.at.total_cmp(&b.at));
    }
}

/// MIDI in: what keyboards and controllers play, for the loops a score's
/// `MidiIn` streams feed. The host hands every message from its inputs to
/// [`midi_in::push`] with when it arrived (seconds on [`crate::clock`]);
/// each loop fed by a `MidiIn` has its own ring, filled with the messages
/// its device and channel take, which its node reads on the audio thread
/// and plays on their sample (a fixed short delay keeps their spacing).
/// The host also names the ports ([`midi_in::set_ports`]: a device plugged
/// in later is matched by name as soon as it is listed) and reads what came
/// in for its displays ([`midi_in::activity`]).
pub mod midi_in {
    use makepad_flow::live::spsc::{ring, Consumer, Producer};
    use std::sync::{Mutex, OnceLock};

    /// Messages a loop may have waiting.
    pub const CAPACITY: usize = 1024;
    /// Messages the activity log keeps.
    pub const LOG: usize = 64;

    /// One message: when it arrived (seconds on the engine's clock; minus
    /// infinity plays it at the start of the next pass), the port it came
    /// in on and its bytes.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct MidiMsg {
        pub at: f64,
        pub port: u64,
        pub data: [u8; 3],
    }

    struct Sub {
        node: String,
        /// The device it takes: "" or "any" for every input, else part of
        /// a port's name (any case).
        device: String,
        /// The channel it takes (1..16), 0 for every one.
        channel: u8,
        producer: Producer<MidiMsg>,
    }

    #[derive(Default)]
    struct Hub {
        subs: Vec<Sub>,
        /// The input ports: (id, name).
        ports: Vec<(u64, String)>,
        log: std::collections::VecDeque<MidiMsg>,
        dropped: u64,
    }

    fn hub() -> std::sync::MutexGuard<'static, Hub> {
        static HUB: OnceLock<Mutex<Hub>> = OnceLock::new();
        HUB.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a port named `name` is a device `device` names.
    pub fn matches(device: &str, name: &str) -> bool {
        let device = device.trim();
        device.is_empty() || device.eq_ignore_ascii_case("any") || name.to_ascii_lowercase().contains(&device.to_ascii_lowercase())
    }

    /// A new ring for the loop node `node` (a qualified id), taking the
    /// messages of `device` on `channel` (0: all); it replaces the node's
    /// last one. Off the audio thread (a graph being prepared).
    pub fn fresh(node: &str, device: &str, channel: u8) -> Consumer<MidiMsg> {
        let (producer, consumer) = ring(CAPACITY);
        let mut hub = hub();
        hub.subs.retain(|s| s.node != node);
        hub.subs.push(Sub { node: node.to_string(), device: device.to_string(), channel, producer });
        consumer
    }

    /// The input ports as the host lists them (id, name); a hot-plugged
    /// device is in the next list.
    pub fn set_ports(ports: Vec<(u64, String)>) {
        hub().ports = ports;
    }

    /// The input ports' names.
    pub fn ports() -> Vec<String> {
        hub().ports.iter().map(|(_, n)| n.clone()).collect()
    }

    /// The name of port `id` ("" when it is not listed).
    pub fn port_name(id: u64) -> String {
        hub().ports.iter().find(|(p, _)| *p == id).map(|(_, n)| n.clone()).unwrap_or_default()
    }

    static STAMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// How many messages came in so far (a display redraws when it moves).
    pub fn stamp() -> u64 {
        STAMP.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A message from an input (the host's thread; never the audio thread).
    pub fn push(msg: MidiMsg) {
        STAMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut hub = hub();
        let hub = &mut *hub;
        let name = hub.ports.iter().find(|(p, _)| *p == msg.port).map(|(_, n)| n.as_str()).unwrap_or("");
        let channel = msg.data[0] & 15;
        let system = msg.data[0] >= 0xf0;
        for sub in hub.subs.iter_mut() {
            if system || !matches(&sub.device, name) || (sub.channel != 0 && sub.channel - 1 != channel) {
                continue;
            }
            if sub.producer.push(msg).is_err() {
                hub.dropped += 1;
            }
        }
        if !system {
            if hub.log.len() >= LOG {
                hub.log.pop_front();
            }
            hub.log.push_back(msg);
        }
    }

    /// Messages dropped because a loop's ring was full.
    pub fn dropped() -> u64 {
        hub().dropped
    }

    /// What a `MidiIn` of `device` on `channel` (0: all) took lately: when
    /// its last message came (seconds on the engine's clock; minus infinity
    /// for never), the keys down now by what came in (lowest first), and
    /// its last note-ons (note, velocity), newest last.
    pub fn activity(device: &str, channel: u8) -> Activity {
        let hub = hub();
        let mut out = Activity { last: f64::NEG_INFINITY, down: Vec::new(), recent: Vec::new() };
        let mut down = [false; 128];
        for m in hub.log.iter() {
            let name = hub.ports.iter().find(|(p, _)| *p == m.port).map(|(_, n)| n.as_str()).unwrap_or("");
            if !matches(device, name) || (channel != 0 && channel - 1 != m.data[0] & 15) {
                continue;
            }
            out.last = out.last.max(m.at);
            let (status, note, vel) = (m.data[0] & 0xf0, m.data[1] as usize & 127, m.data[2]);
            if status == 0x90 && vel > 0 {
                down[note] = true;
                out.recent.push((note as u8, vel));
            } else if status == 0x80 || status == 0x90 {
                down[note] = false;
            }
        }
        let keep = out.recent.len().saturating_sub(8);
        out.recent.drain(..keep);
        out.down = (0..128u8).filter(|n| down[*n as usize]).collect();
        out
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Activity {
        pub last: f64,
        pub down: Vec<u8>,
        pub recent: Vec<(u8, u8)>,
    }

    /// Forget the rings of the graph instantiated as `name`.
    pub fn forget_instance(name: &str) {
        let prefix = format!("{name}/");
        hub().subs.retain(|s| !s.node.starts_with(&prefix));
    }
}

/// What a loop's note stream plays, as its node publishes it from the
/// audio thread for displays (a lens's lights and step view, a tracker's
/// live channel): the keys held, the notes sounding, the arp's pattern as
/// built and its step, the last notes started. Atomics only: the audio
/// thread stores, a display loads.
pub mod streams {
    use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering::Relaxed};
    use std::sync::{Arc, Mutex, OnceLock};

    /// Steps of an arp's pattern a view shows.
    pub const STEPS: usize = 64;

    pub struct StreamView {
        held: [AtomicU64; 2],
        sounding: [AtomicU64; 2],
        steps: [AtomicU8; STEPS],
        nsteps: AtomicU32,
        step: AtomicU32,
        recent: [AtomicU32; 8],
        recent_beats: [AtomicU64; 8],
        nrecent: AtomicU32,
        started: AtomicU64,
    }

    impl Default for StreamView {
        fn default() -> Self {
            StreamView {
                held: Default::default(), sounding: Default::default(), steps: std::array::from_fn(|_| AtomicU8::new(0)), nsteps: AtomicU32::new(0),
                step: AtomicU32::new(0), recent: Default::default(), recent_beats: Default::default(), nrecent: AtomicU32::new(0), started: AtomicU64::new(0),
            }
        }
    }

    /// One snapshot of a stream.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Snapshot {
        pub held: Vec<u8>,
        pub sounding: Vec<u8>,
        pub steps: Vec<u8>,
        pub step: usize,
        /// The last notes started (beat, note, velocity), newest last.
        pub recent: Vec<(f64, u8, u8)>,
        /// Notes started so far (a display sees one start by its change).
        pub started: u64,
    }

    fn notes(set: &[AtomicU64; 2]) -> Vec<u8> {
        let words = [set[0].load(Relaxed), set[1].load(Relaxed)];
        (0..128u8).filter(|n| words[(*n >> 6) as usize] & (1 << (n & 63)) != 0).collect()
    }

    impl StreamView {
        /// From the audio thread. No allocation.
        pub fn store(&self, held: [u64; 2], sounding: [u64; 2], steps: Option<(&[u8], usize)>, recent: &[(f64, u8, u8)], started: u64) {
            for i in 0..2 {
                self.held[i].store(held[i], Relaxed);
                self.sounding[i].store(sounding[i], Relaxed);
            }
            let (list, step) = steps.unwrap_or((&[], 0));
            let n = list.len().min(STEPS);
            for (slot, note) in self.steps.iter().zip(&list[..n]) {
                slot.store(*note, Relaxed);
            }
            self.nsteps.store(n as u32, Relaxed);
            self.step.store(step as u32, Relaxed);
            let r = recent.len().min(8);
            for (k, (beat, note, vel)) in recent[recent.len() - r..].iter().enumerate() {
                self.recent[k].store((*note as u32) << 8 | *vel as u32, Relaxed);
                self.recent_beats[k].store(beat.to_bits(), Relaxed);
            }
            self.nrecent.store(r as u32, Relaxed);
            self.started.store(started, Relaxed);
        }

        pub fn snapshot(&self) -> Snapshot {
            let n = (self.nsteps.load(Relaxed) as usize).min(STEPS);
            let r = (self.nrecent.load(Relaxed) as usize).min(8);
            Snapshot {
                held: notes(&self.held),
                sounding: notes(&self.sounding),
                steps: self.steps[..n].iter().map(|s| s.load(Relaxed)).collect(),
                step: self.step.load(Relaxed) as usize,
                recent: (0..r).map(|k| {
                    let v = self.recent[k].load(Relaxed);
                    (f64::from_bits(self.recent_beats[k].load(Relaxed)), (v >> 8) as u8, v as u8)
                }).collect(),
                started: self.started.load(Relaxed),
            }
        }
    }

    fn table() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, Arc<StreamView>>> {
        static TABLE: OnceLock<Mutex<std::collections::HashMap<String, Arc<StreamView>>>> = OnceLock::new();
        TABLE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The view of loop node `node` (a qualified id; made on first use).
    /// Never called on the audio thread.
    pub fn view(node: &str) -> Arc<StreamView> {
        table().entry(node.to_string()).or_default().clone()
    }

    pub fn forget_instance(name: &str) {
        let prefix = format!("{name}/");
        table().retain(|k, _| !k.starts_with(&prefix));
    }
}

/// Live audio in (a microphone, a line in): the input device's thread
/// pushes stereo frames into a ring the score's input loop reads on the
/// audio thread. One reader at a time: a graph that plans an input loop
/// takes a fresh ring ([`audio_in::fresh`]).
pub mod audio_in {
    use makepad_flow::live::spsc::{ring, Consumer, Producer};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};

    /// Frames the ring holds (about a third of a second at 48 kHz).
    pub const FRAMES: usize = 16_384;
    /// Frames a reader lets wait before it skips ahead (latency bound).
    pub const KEEP: usize = 1_024;

    fn slot() -> &'static Mutex<Option<Producer<[f32; 2]>>> {
        static SLOT: OnceLock<Mutex<Option<Producer<[f32; 2]>>>> = OnceLock::new();
        SLOT.get_or_init(Default::default)
    }

    static RATE: AtomicU64 = AtomicU64::new(0);

    /// A new ring for the reader (off the audio thread); the input's frames
    /// go there from now on.
    pub fn fresh() -> Consumer<[f32; 2]> {
        let (producer, consumer) = ring(FRAMES);
        *slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(producer);
        consumer
    }

    /// From the input device's thread: its frames (mono: `right` is `left`)
    /// at `rate`. Never blocks: while a graph takes a fresh ring, a buffer
    /// may be skipped.
    pub fn push(rate: f64, left: &[f32], right: &[f32]) {
        RATE.store(rate.to_bits(), Ordering::Relaxed);
        let Ok(mut slot) = slot().try_lock() else { return };
        let Some(producer) = slot.as_mut() else { return };
        for (l, r) in left.iter().zip(right) {
            if producer.push([*l, *r]).is_err() {
                break;
            }
        }
    }

    /// The input's sample rate (0 before it delivered).
    pub fn rate() -> f64 {
        f64::from_bits(RATE.load(Ordering::Relaxed))
    }

    /// Read `left.len()` frames (silence where the input has none); a
    /// reader far behind skips ahead to [`KEEP`] frames of latency.
    pub fn read(consumer: &mut Consumer<[f32; 2]>, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        while consumer.len() > KEEP + n {
            consumer.pop();
        }
        for i in 0..n {
            let [l, r] = consumer.pop().unwrap_or([0.0, 0.0]);
            left[i] = l;
            right[i] = r;
        }
    }
}
