//! The app's sound: one small audio root, built as data on the audio
//! engine (a `Transport`, the score's `Score` node playing the slot
//! [`SLOT`], an `AudioOutput`), rendered from the platform's audio output
//! callback. The score's player is the slot's (`makepad_score_live::node`):
//! the same player Stage's audio flow plays, with its per-loop meters and
//! scopes, here on the app's own clock.
//!
//! - The UI thread builds the root at the device's rate ([`Engine::pump`])
//!   and hands it to the callback under a lock the callback only tries:
//!   a rebuild costs one silent buffer, never a wait on the audio thread.
//! - Play, stop and the tempo are the transport's controls (atomics).
//! - The composer (`LiveScore`) reads where the output is through
//!   [`EngineClock`]: the beat the last pass ended on, carried forward to
//!   now while it plays. Its tempo changes go to the transport's feed
//!   ([`TRANSPORT`]).

use makepad_audio_engine::run::{run, NoExt};
use makepad_audio_engine::schedule::{compile, AudioBudget, ControlBank, Schedule};
use makepad_audio_engine::{CHUNK, OWNER};
use makepad_flow::live::plan::OwnerSlice;
use makepad_flow::Literal;
use makepad_score_live::audition::bus;
use makepad_score_live::live::ClockSource;
use makepad_score_live::model::ScoreDoc;
use makepad_score_live::node::Slot;
use makepad_score_live::player::live_node;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The score slot the app plays (its loops' feeds are `synth/<loop>`).
pub const SLOT: &str = "synth";
/// The transport node: the composer's tempo changes go to its feed.
pub const TRANSPORT: &str = "synth_transport";
/// The score's node in the root.
const SCORE_NODE: &str = "synth_score";
/// The key the master's level is kept under in the meters.
pub const MASTER_KEY: u64 = 0;

/// The slot the app's root plays.
pub fn slot() -> Arc<Slot> {
    makepad_score_live::node::slot(SLOT)
}

/// The root the callback renders: its schedule, the transport's clock
/// index, and the last pass not yet played out.
struct Root {
    schedule: Box<Schedule<(), ()>>,
    clock: usize,
    rate: u32,
    buf_l: Box<[f32]>,
    buf_r: Box<[f32]>,
    buffered: usize,
    taken: usize,
}

/// What the callback and the UI thread share.
#[derive(Default)]
struct Shared {
    root: Mutex<Option<Root>>,
    /// The device's rate (0: no device has asked yet).
    rate: AtomicU32,
    /// Where the last pass ended (beat, f64 bits), its tempo (f64 bits),
    /// whether it played, and when (seconds on the engine's clock, bits).
    beat: AtomicU64,
    bpm: AtomicU64,
    playing: AtomicBool,
    published_at: AtomicU64,
    /// Output muted (a hidden instance): it renders, the device hears 0.
    muted: AtomicBool,
}

impl Shared {
    fn f64(v: &AtomicU64) -> f64 {
        f64::from_bits(v.load(Ordering::Acquire))
    }

    /// Render `left`/`right` (any length) from the root; silence without one.
    fn render(&self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let Ok(mut root) = self.root.try_lock() else {
            left.fill(0.0);
            right.fill(0.0);
            return;
        };
        let Some(root) = root.as_mut() else {
            left.fill(0.0);
            right.fill(0.0);
            return;
        };
        let mut at = 0;
        while at < n {
            // Whole passes always (any device block size plays the same samples).
            if root.taken == root.buffered {
                let (bl, br) = (&mut root.buf_l, &mut root.buf_r);
                run(&mut root.schedule, CHUNK, &mut |cue, l, r| {
                    if !cue {
                        bl.copy_from_slice(&l[..CHUNK]);
                        br.copy_from_slice(&r[..CHUNK]);
                    }
                }, &mut NoExt);
                let clock = root.schedule.clocks.get(root.clock).copied().unwrap_or_default();
                self.beat.store(clock.tempo.end_beat(CHUNK).to_bits(), Ordering::Release);
                self.bpm.store((clock.bpm as f64).to_bits(), Ordering::Release);
                self.playing.store(clock.playing, Ordering::Release);
                self.published_at.store(makepad_audio_engine::clock::now().to_bits(), Ordering::Release);
                root.buffered = CHUNK;
                root.taken = 0;
            }
            let k = (root.buffered - root.taken).min(n - at);
            left[at..at + k].copy_from_slice(&root.buf_l[root.taken..root.taken + k]);
            right[at..at + k].copy_from_slice(&root.buf_r[root.taken..root.taken + k]);
            root.taken += k;
            at += k;
        }
        if self.muted.load(Ordering::Relaxed) {
            left.fill(0.0);
            right.fill(0.0);
        }
    }
}

/// The composer's clock: the beat at the output, the tempo, playing.
pub struct EngineClock(Arc<Shared>);

impl ClockSource for EngineClock {
    fn now(&self) -> Option<(f64, f64, bool)> {
        let s = &self.0;
        let (beat, bpm, playing) = (Shared::f64(&s.beat), Shared::f64(&s.bpm), s.playing.load(Ordering::Acquire));
        let bpm = if bpm > 0.0 { bpm } else { 120.0 };
        let beat = if playing { beat + (makepad_audio_engine::clock::now() - Shared::f64(&s.published_at)).max(0.0) * bpm / 60.0 } else { beat };
        Some((beat, bpm, playing))
    }
}

/// The audio callback's half: fill a device buffer (stereo pairs repeat
/// on more channels; a mono device gets both sides mixed).
pub struct Output {
    shared: Arc<Shared>,
    left: Box<[f32]>,
    right: Box<[f32]>,
}

impl Output {
    pub fn fill(&mut self, rate: f64, buffer: &mut makepad_widgets::AudioBuffer) {
        let rate = rate.round() as u32;
        if self.shared.rate.load(Ordering::Relaxed) != rate {
            self.shared.rate.store(rate, Ordering::Release);
        }
        let frames = buffer.frame_count();
        let channels = buffer.channel_count();
        // Previews (the instrument switch, the keys) ride the audition bus:
        // on the main pair, or on channels 3-4 when the device has them.
        bus::set_channels(channels);
        let muted = self.shared.muted.load(Ordering::Relaxed);
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.left.len());
            let (l, r) = (&mut self.left[..n], &mut self.right[..n]);
            self.shared.render(l, r);
            if !muted {
                bus::mix_main(rate as f64, l, r);
            }
            for channel in 0..channels.min(2) {
                let out = &mut buffer.channel_mut(channel)[done..done + n];
                for (i, sample) in out.iter_mut().enumerate() {
                    *sample = match channels {
                        1 => (l[i] + r[i]) * 0.5,
                        _ if channel == 0 => l[i],
                        _ => r[i],
                    };
                }
            }
            if channels >= 4 {
                l.fill(0.0);
                r.fill(0.0);
                if !muted {
                    bus::mix_cue(rate as f64, l, r);
                }
                buffer.channel_mut(2)[done..done + n].copy_from_slice(l);
                buffer.channel_mut(3)[done..done + n].copy_from_slice(r);
            }
            for channel in (if channels >= 4 { 4 } else { 2 })..channels {
                buffer.channel_mut(channel)[done..done + n].fill(0.0);
            }
            done += n;
        }
    }
}

/// What the score's nodes are doing, read from the slot each UI turn:
/// levels and recent output per loop and FX, and the clock.
#[derive(Default)]
pub struct SynthMeters {
    inner: Mutex<MeterState>,
}

#[derive(Default)]
struct MeterState {
    beat: f64,
    bpm: f64,
    playing: bool,
    rate: f32,
    levels: HashMap<u64, (f32, f32)>,
    scopes: HashMap<u64, Vec<f32>>,
    gr: HashMap<u64, f32>,
}

impl makepad_score_player::Meters for SynthMeters {
    fn playing(&self) -> bool {
        self.inner.lock().map_or(false, |m| m.playing)
    }

    fn beat(&self) -> f64 {
        self.inner.lock().map_or(0.0, |m| m.beat)
    }

    fn bpm(&self) -> f64 {
        self.inner.lock().map_or(120.0, |m| m.bpm)
    }

    fn rate(&self) -> f32 {
        self.inner.lock().map_or(48_000.0, |m| m.rate)
    }

    fn level(&self, key: u64) -> Option<(f32, f32)> {
        self.inner.lock().ok()?.levels.get(&key).copied()
    }

    fn gr(&self, key: u64) -> Option<f32> {
        self.inner.lock().ok()?.gr.get(&key).copied()
    }

    fn node_scope(&self, key: u64, out: &mut [f32]) -> bool {
        let Ok(m) = self.inner.lock() else { return false };
        let Some(scope) = m.scopes.get(&key) else { return false };
        let n = out.len().max(1);
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = scope.get(i * scope.len() / n).copied().unwrap_or(0.0);
        }
        true
    }

    fn last_opts(&self, _key: u64) -> Option<[f32; 7]> {
        None
    }
}

/// The app's audio: the root, its controls, the meters.
pub struct Engine {
    shared: Arc<Shared>,
    /// The transport's bpm and playing controls of the root built last.
    controls: Option<(Arc<ControlBank>, u16, u16)>,
    bpm: f64,
    playing: bool,
    pub meters: Arc<SynthMeters>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine { shared: Arc::default(), controls: None, bpm: 120.0, playing: false, meters: Arc::default() }
    }
}

/// The `Score` node type known to the engine (once per process).
fn register_score_node() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        let mut registry = makepad_flow::document::Registry::with_builtins();
        let registered = makepad_audio_engine::nodes::register_nodes(&mut registry).and_then(|()| makepad_score_live::node::register(&mut registry));
        if let Err(error) = registered {
            makepad_widgets::log!("synth: the score node type: {error}");
        }
    });
}

/// The root as the owner's slice: transport, the score's node, the output.
fn root_slice() -> Option<OwnerSlice> {
    use makepad_audio_engine::nodes::descriptor;
    let num = Literal::Num;
    let transport = live_node(descriptor("Transport")?, TRANSPORT.into(), &[("bpm", num(120.0)), ("playing", num(0.0))], &[]);
    let score = live_node(&makepad_score_live::node::descriptor(), SCORE_NODE.into(), &[("score", Literal::Str(SLOT.into()))], &[("clock", vec![(TRANSPORT.to_string(), "clock")])]);
    let out = live_node(descriptor("AudioOutput")?, "out".into(), &[("enabled", num(1.0))], &[("audio", vec![(SCORE_NODE.to_string(), "audio")])]);
    Some(OwnerSlice { owner: OWNER.into(), nodes: vec![transport, score, out] })
}

impl Engine {
    /// The callback's half (one for the life of the app).
    pub fn output(&self) -> Output {
        Output { shared: self.shared.clone(), left: vec![0.0; 1024].into_boxed_slice(), right: vec![0.0; 1024].into_boxed_slice() }
    }

    /// The composer's clock.
    pub fn clock(&self) -> Arc<dyn ClockSource> {
        Arc::new(EngineClock(self.shared.clone()))
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    /// The device's rate, once one asked.
    pub fn device_rate(&self) -> Option<u32> {
        Some(self.shared.rate.load(Ordering::Acquire)).filter(|r| *r > 0)
    }

    /// Build the root at `rate` (the device's), replacing the one playing.
    pub fn build(&mut self, rate: u32) -> Result<(), String> {
        register_score_node();
        let slice = root_slice().ok_or("the audio engine has no Transport or AudioOutput")?;
        let format = (1u64 << 32) | rate as u64;
        let compiled = compile::<NoExt, ()>(&slice, 1, 1, format, None, &Arc::new(AudioBudget::default())).map_err(|e| e.message)?;
        let clock = *compiled.layout.transports.get(TRANSPORT).ok_or("the root has no transport")? as usize;
        let control = |name: &str| compiled.layout.controls.get(&format!("{TRANSPORT}.{name}")).map(|(index, _)| *index);
        let (bpm, playing) = (control("bpm").ok_or("no bpm control")?, control("playing").ok_or("no playing control")?);
        let bank = compiled.schedule.controls.clone();
        bank.set_manual(bpm, self.bpm as f32);
        bank.set_manual(playing, if self.playing { 1.0 } else { 0.0 });
        self.controls = Some((bank, bpm, playing));
        let next = Root { schedule: compiled.schedule, clock, rate, buf_l: vec![0.0; CHUNK].into_boxed_slice(), buf_r: vec![0.0; CHUNK].into_boxed_slice(), buffered: 0, taken: 0 };
        let old = self.shared.root.lock().map(|mut root| root.replace(next)).unwrap_or(None);
        // The old root (and the score node in it) is let go here, off the
        // audio thread.
        drop(old);
        Ok(())
    }

    /// The UI thread's turn: a root at the device's rate.
    pub fn pump(&mut self) {
        let Some(rate) = self.device_rate() else { return };
        let built = self.shared.root.lock().ok().and_then(|root| root.as_ref().map(|r| r.rate));
        if built != Some(rate) {
            if let Err(error) = self.build(rate) {
                makepad_widgets::log!("synth: the audio root: {error}");
            }
        }
    }

    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        if let Some((bank, _, index)) = &self.controls {
            bank.set_manual(*index, if playing { 1.0 } else { 0.0 });
        }
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    pub fn set_bpm(&mut self, bpm: f64) {
        self.bpm = bpm.clamp(20.0, 400.0);
        if let Some((bank, index, _)) = &self.controls {
            bank.set_manual(*index, self.bpm as f32);
        }
    }

    /// Render `left`/`right` as the callback does.
    #[cfg(test)]
    pub fn render(&self, left: &mut [f32], right: &mut [f32]) {
        self.shared.render(left, right);
    }

    /// The meters from the slot's player and the clock: each loop's and
    /// FX's level, gain reduction and recent output, and the master's.
    pub fn read_meters(&self, doc: &ScoreDoc) {
        let slot = slot();
        let Ok(mut meters) = self.meters.inner.lock() else { return };
        meters.beat = Shared::f64(&self.shared.beat);
        meters.bpm = Shared::f64(&self.shared.bpm);
        meters.playing = self.shared.playing.load(Ordering::Acquire);
        meters.rate = self.device_rate().unwrap_or(48_000) as f32;
        let names = doc.loops.iter().map(|l| l.name.clone()).chain(doc.fx.iter().map(|f| f.name.clone())).chain(["master".to_string()]);
        for name in names {
            let node = format!("{SLOT}/{name}");
            let key = if name == "master" { MASTER_KEY } else { makepad_score_live::key(&name) };
            let peak = slot.read(&node, "peak").unwrap_or(0.0);
            meters.levels.insert(key, (peak, peak * std::f32::consts::FRAC_1_SQRT_2));
            if let Some(gr) = slot.read(&node, "gr") {
                meters.gr.insert(key, gr);
            }
            if let Some(bins) = slot.scope(&node) {
                let scope = meters.scopes.entry(key).or_default();
                scope.clear();
                // Each bin's top and bottom, so a scope draws its envelope.
                scope.extend(bins.iter().flat_map(|b| [b[1].max(b[3]), b[0].min(b[2])]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_score_live::{LiveConfig, LiveScore};

    /// A template score played through the app's own path: the composer on
    /// the engine's clock, the slot's player inside the app's root, rendered
    /// as the output callback renders it. It sounds, and the slot's meters
    /// read the loops.
    #[test]
    fn a_template_plays_through_the_apps_root() {
        let source = makepad_score_live::templates::template("techno").expect("the techno template");
        let doc = ScoreDoc::load(source).expect("the template loads");
        let mut engine = Engine::default();
        engine.set_bpm(doc.clock.bpm);
        engine.build(48_000).expect("the root builds");
        slot().set_doc(doc.clone()).expect("the slot takes the score");
        slot().pump().expect("the slot prepares");
        let live = LiveScore::new_pumped(LiveConfig { instance: SLOT.into(), lookahead: 0.15, clock: engine.clock() }, TRANSPORT);
        live.set_source(source);
        engine.set_playing(true);
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        let mut peak = 0.0f32;
        for _ in 0..(48_000 * 4 / 512) {
            live.pump();
            slot().pump().expect("the slot pumps");
            engine.render(&mut l, &mut r);
            peak = l.iter().chain(&r).fold(peak, |m, x| m.max(x.abs()));
        }
        assert!(peak > 0.01, "the score is silent (peak {peak})");
        assert!(slot().playing() > 0, "the root's score node plays the slot's player");
        engine.read_meters(&doc);
        let master = makepad_score_player::Meters::level(&*engine.meters, MASTER_KEY).map_or(0.0, |l| l.0);
        assert!(master > 0.0, "the master's meter reads the player");
        let status = live.status();
        assert!(status.diags.is_empty(), "{:?}", status.diags);
        assert!(status.beat > 4.0, "the composer follows the engine's clock (beat {})", status.beat);
    }
}
