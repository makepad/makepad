//! A score as a game's adaptive music: one instrument on the game's synth
//! bus (`makepad_audio_synth::SynthBus`, in its MUSIC group, one instrument
//! slot), driven from the game through a handle.
//!
//! - [`music`] prepares the score's graph and returns the two halves: the
//!   [`MusicInstrument`] the game adds to its bus, and the [`GameMusic`]
//!   handle it keeps.
//! - The handle's commands (`set_param`, `trigger`, `goto_section`,
//!   `set_source`) never touch the audio thread: the composer (the Splash
//!   VM) runs on its own thread and hands events to the graph through
//!   lock-free feeds, ahead of the clock.
//! - `fade_to`, `park` and `resume` are atomics the instrument reads: a
//!   parked score fades out, stops its transport and costs nothing.
//! - Beats, bars, section changes and the score's cues come back through
//!   [`GameMusic::poll`], in order, each with the beat it happened on.
//! - [`Clocking::Headless`] keeps the composer in the handle: a server, a
//!   test or an offline bake calls [`GameMusic::advance`] before rendering
//!   each block and drives the music at any pace (a virtual clock), with
//!   the same result sample for sample.
//!
//! The instrument's `render` never allocates, locks or frees.

use crate::compose::Composer;
use crate::live::{ClockSource, Cmd, LiveConfig, LiveScore};
use crate::model::{self, Diag};
use crate::player::{prepare, prepare_slice, transport, ClockMode, Player};
use crate::render::transport_bpm;
use makepad_audio_synth::instrument::Instrument;
use makepad_flow::live::plan::OwnerSlice;
use makepad_flow::live::spsc::{ring, Consumer, Producer};
use makepad_audio_engine::feed::{loop_feed, LoopFeed};
use makepad_audio_engine::schedule::{migrate_state, ClockChunk, Layout};
use makepad_audio_engine::CHUNK;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// How far ahead of the output the composer schedules (seconds): more than
/// a game's audio buffer plus the composer's wake-up.
pub const LOOKAHEAD: f64 = 0.15;
/// Events the handle has not polled yet.
const EVENTS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clocking {
    /// The game's audio thread plays it; the composer runs on a thread.
    Realtime,
    /// Whoever calls `render` is the clock (servers, tests, bakes).
    Headless,
}

/// What the music did, on the beat it did it.
#[derive(Clone, Debug, PartialEq)]
pub enum MusicEvent {
    /// A beat started (every beat; `bar` counts from 0, `beat_in_bar` too).
    Beat { beat: u64, bar: u64, beat_in_bar: u32 },
    /// A section started (the arrangement, or `goto_section`).
    Section { beat: f64, name: String },
    /// A `cue("name")` in loop code came due.
    Cue { beat: f64, name: String },
}

/// What the instrument shares with the handle.
struct Shared {
    /// The beat at the output (f64 bits), the tempo (f64 bits), playing.
    beat: AtomicU64,
    bpm: AtomicU64,
    playing: AtomicBool,
    /// Gain target and fade length (f32 bits), parked.
    gain: AtomicU32,
    fade: AtomicU32,
    parked: AtomicBool,
    /// Beats per bar (f64 bits).
    bar: AtomicU64,
    /// The composer has scheduled the start: the transport may run (a
    /// realtime score starts on beat 0 with its first notes in place).
    ready: AtomicBool,
    clock_reads: AtomicU32,
}

impl Shared {
    fn beat(&self) -> f64 {
        f64::from_bits(self.beat.load(Ordering::Acquire))
    }
}

struct Clock(Arc<Shared>);

impl ClockSource for Clock {
    fn now(&self) -> Option<(f64, f64, bool)> {
        let s = &self.0;
        if !s.ready.load(Ordering::Acquire) {
            // Armed at beat 0: the composer's first pass reads the clock,
            // composes from 0, and reads it again; then the music starts.
            if s.clock_reads.fetch_add(1, Ordering::AcqRel) >= 1 {
                s.ready.store(true, Ordering::Release);
            }
            return Some((0.0, f64::from_bits(s.bpm.load(Ordering::Acquire)), true));
        }
        Some((s.beat(), f64::from_bits(s.bpm.load(Ordering::Acquire)), s.playing.load(Ordering::Acquire) && !s.parked.load(Ordering::Acquire)))
    }
}

/// A new graph for the instrument, and the old one coming back to drop.
enum Swap {
    Next(Box<Player>),
}

/// The game's handle on its music.
pub struct GameMusic {
    shared: Arc<Shared>,
    live: Option<LiveScore>,
    /// Headless: the composer, run by `advance`.
    inline: Option<Box<Inline>>,
    events: Consumer<MusicEvent>,
    swaps: Producer<Swap>,
    retired: Consumer<Box<Player>>,
    name: String,
    /// What the playing graph was prepared from.
    slice: OwnerSlice,
    layout: Arc<Layout>,
    generation: u64,
    rate: f32,
    /// Realtime: the section last announced and the last cue's beat.
    section: Option<String>,
    cued: f64,
}

/// The score's instrument: add it to the bus (`SynthBus::add_instrument`)
/// and put it in the MUSIC group (`set_instrument_group(key, Group::Music)`).
pub struct MusicInstrument {
    instance: Box<Player>,
    shared: Arc<Shared>,
    events: Producer<MusicEvent>,
    swaps: Consumer<Swap>,
    retired: Producer<Box<Player>>,
    playing_index: Option<u16>,
    rate: f32,
    gain: f32,
    parked: bool,
    last_beat: Option<u64>,
    /// The last pass's output, `taken` of its `buffered` frames played.
    buffer_l: Box<[f32]>,
    buffer_r: Box<[f32]>,
    buffered: usize,
    taken: usize,
    /// The clock of the buffered pass.
    pass: ClockChunk,
    started: bool,
}

/// Headless: the composer, the loops' feeds and the transport's.
struct Inline {
    composer: Composer,
    feeds: Vec<(u64, Arc<LoopFeed>)>,
    tempo: Arc<makepad_audio_engine::feed::TempoFeed>,
}

/// Prepare `source` as a game's music at the bus's `rate`.
pub fn music(source: &str, rate: f32, clocking: Clocking) -> Result<(GameMusic, MusicInstrument), Vec<Diag>> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let loaded = model::load(source)?;
    let name = format!("music_{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let instance = prepare(&loaded.doc, rate, &name, ClockMode::Own, None, 1)?;
    let shared = Arc::new(Shared {
        beat: AtomicU64::new(0f64.to_bits()),
        bpm: AtomicU64::new(transport_bpm(&loaded.doc).to_bits()),
        playing: AtomicBool::new(false),
        gain: AtomicU32::new(1f32.to_bits()),
        fade: AtomicU32::new(0f32.to_bits()),
        parked: AtomicBool::new(false),
        bar: AtomicU64::new(loaded.doc.beats_per_bar().to_bits()),
        ready: AtomicBool::new(clocking == Clocking::Headless),
        clock_reads: AtomicU32::new(0),
    });
    let slice = instance.slice.clone();
    let (events_in, events) = ring(EVENTS);
    let (swaps, swaps_out) = ring(2);
    let (retired_in, retired) = ring(4);
    let (live, inline) = match clocking {
        Clocking::Realtime => {
            let live = LiveScore::new(LiveConfig { instance: name.clone(), lookahead: LOOKAHEAD, clock: Arc::new(Clock(shared.clone())) });
            live.set_source(source);
            (Some(live), None)
        }
        Clocking::Headless => {
            let feeds = instance.feeds.clone();
            let tempo = makepad_audio_engine::feed::tempo_feed(&transport(&name));
            (None, Some(Box::new(Inline { composer: Composer::new(loaded), feeds, tempo })))
        }
    };
    let layout = instance.layout.clone();
    let playing_index = layout.controls.get(&format!("{}.playing", transport(&name))).map(|c| c.0);
    if let (Clocking::Realtime, Some(index)) = (clocking, playing_index) {
        instance.schedule.controls.set_manual(index, 0.0);
    }
    let handle = GameMusic { shared: shared.clone(), live, inline, events, swaps, retired, name, slice, layout, generation: 1, rate, section: None, cued: f64::NEG_INFINITY };
    let instrument = MusicInstrument {
        instance: Box::new(instance), shared, events: events_in, swaps: swaps_out, retired: retired_in, playing_index,
        rate, gain: 1.0, parked: false, last_beat: None,
        buffer_l: vec![0.0; CHUNK].into_boxed_slice(), buffer_r: vec![0.0; CHUNK].into_boxed_slice(), buffered: 0, taken: 0, pass: ClockChunk::default(), started: false,
    };
    Ok((handle, instrument))
}

impl GameMusic {
    fn command(&mut self, cmd: Cmd) {
        if let Some(live) = &self.live {
            live.send(cmd);
            return;
        }
        let now = self.shared.beat();
        let Some(inline) = &mut self.inline else { return };
        let composer = &mut inline.composer;
        match cmd {
            Cmd::Source(source) => {
                if let Ok(loaded) = model::load(&source) {
                    composer.reload(loaded, now);
                    for name in composer.doc.feed_names() {
                        let key = crate::key(&name);
                        if !inline.feeds.iter().any(|(k, _)| *k == key) {
                            inline.feeds.push((key, loop_feed(&format!("{}/{name}", self.name))));
                        }
                    }
                }
            }
            Cmd::Param(name, value) => {
                composer.set_param(&name, value);
            }
            Cmd::Trigger(name) => {
                let _ = composer.trigger(&name, now);
            }
            Cmd::Goto { section, quant } => {
                let _ = composer.goto_section(&section, now, quant);
            }
            Cmd::Tempo { bpm, ramp } => composer.set_tempo(bpm, ramp),
            Cmd::Midi(data) => {
                let mut events = Vec::new();
                composer.midi_in(data, now, &mut events);
                for event in events {
                    if let Some((_, feed)) = inline.feeds.iter().find(|(key, _)| *key == event.loop_key) {
                        let _ = feed.send(event);
                    }
                }
            }
            Cmd::Seek { beat, quant } => {
                composer.seek(beat, now, quant);
            }
            Cmd::Loop(range) => composer.set_loop(range),
            Cmd::Transport(_) | Cmd::Quit => {}
        }
    }

    /// Headless: run the composer for the next `frames` frames. Call it
    /// before rendering them (with any block size).
    pub fn advance(&mut self, frames: usize) {
        let Some(inline) = &mut self.inline else { return };
        // As far as the fastest tempo could reach in these frames.
        let until = self.shared.beat() + (frames + CHUNK) as f64 * 400.0 / 60.0 / self.rate as f64 + 1e-9;
        let mut events = Vec::new();
        inline.composer.pump(until, &mut events);
        inline.composer.osc_out.clear();
        inline.composer.history.clear();
        for change in inline.composer.tempo_out.drain(..) {
            inline.tempo.send(change);
        }
        for (fx, event) in inline.composer.automation_out.drain(..) {
            makepad_audio_engine::feed::automation_feed(&format!("{}/{fx}", self.name)).send(event);
        }
        for event in events {
            if let Some((_, feed)) = inline.feeds.iter().find(|(key, _)| *key == event.loop_key) {
                let _ = feed.send(event);
            }
        }
    }

    /// A Clock param (`Clock{params: {intensity: 0.5}}`), read by loop code
    /// through `param(@intensity)` and by loops that `follow` it.
    pub fn set_param(&mut self, name: &str, value: f32) {
        self.command(Cmd::Param(name.into(), value));
    }

    /// Change the tempo from the next beat, reached over `ramp` beats
    /// (0: at once). Sample-exact: the music's beats and the beat events
    /// follow it.
    pub fn set_tempo(&mut self, bpm: f64, ramp: f64) {
        self.command(Cmd::Tempo { bpm, ramp });
    }

    /// Play a `Stinger` on the next beat.
    pub fn trigger(&mut self, stinger: &str) {
        self.command(Cmd::Trigger(stinger.into()));
    }

    /// Jump to a section: on the next bar (`quant < 0`), the next beat (1)
    /// or now (0).
    pub fn goto_section(&mut self, section: &str, quant: f64) {
        self.command(Cmd::Goto { section: section.into(), quant });
    }

    /// Change the score. Loops change at their next cycle; when its
    /// instruments or effects changed, a new graph takes over at the next
    /// block, staying loops keeping their voices. A problem leaves the
    /// music playing as it was.
    pub fn set_source(&mut self, source: &str) -> Result<(), Vec<Diag>> {
        let loaded = model::load(source)?;
        let slice = crate::player::slice(&loaded.doc, &self.name, ClockMode::Own);
        if slice != self.slice {
            self.generation += 1;
            let next = prepare_slice(&loaded.doc, slice.clone(), self.rate, &self.name, ClockMode::Own, Some(&self.layout), self.generation)?;
            self.layout = next.layout.clone();
            if self.swaps.push(Swap::Next(Box::new(next))).is_err() {
                return Err(vec![Diag { line: 1, col: 1, node: None, message: "the music is still taking the last change".into() }]);
            }
            self.slice = slice;
        }
        self.shared.bar.store(loaded.doc.beats_per_bar().to_bits(), Ordering::Release);
        self.command(Cmd::Source(source.into()));
        Ok(())
    }

    /// Glide the music's level to `gain` over `secs`.
    pub fn fade_to(&self, gain: f32, secs: f32) {
        self.shared.fade.store(secs.max(0.0).to_bits(), Ordering::Release);
        self.shared.gain.store(gain.clamp(0.0, 4.0).to_bits(), Ordering::Release);
    }

    /// Fade out over `secs` and stop: a parked score costs nothing.
    pub fn park(&self, secs: f32) {
        self.fade_to(0.0, secs);
        self.shared.parked.store(true, Ordering::Release);
    }

    /// Start again (from the next bar), fading in over `secs`.
    pub fn resume(&self, secs: f32) {
        self.shared.parked.store(false, Ordering::Release);
        self.fade_to(1.0, secs);
    }

    /// The beat at the output, and whether it plays.
    pub fn beat(&self) -> (f64, bool) {
        (self.shared.beat(), self.shared.playing.load(Ordering::Acquire) && !self.shared.parked.load(Ordering::Acquire))
    }

    /// Everything that happened since the last poll, in order (call it
    /// every game frame). Also frees replaced graphs here, off the audio
    /// thread.
    pub fn poll(&mut self) -> Vec<MusicEvent> {
        while self.retired.pop().is_some() {}
        let mut out = Vec::new();
        while let Some(event) = self.events.pop() {
            out.push(event);
        }
        // Sections and cues are the composer's: they come due by beat.
        let now = self.shared.beat();
        let (section, cues): (Option<String>, Vec<(String, f64)>) = match (&self.live, &mut self.inline) {
            (Some(live), _) => {
                let status = live.status();
                (status.section, status.cues)
            }
            (_, Some(inline)) => (inline.composer.section_at(now), inline.composer.cues.iter().cloned().collect()),
            _ => (None, Vec::new()),
        };
        if section != self.section && self.beat().1 {
            if let Some(name) = &section {
                out.push(MusicEvent::Section { beat: now, name: name.clone() });
            }
            self.section = section;
        }
        let mut last = self.cued;
        for (name, beat) in cues {
            if beat > self.cued && beat <= now {
                out.push(MusicEvent::Cue { beat, name });
                last = last.max(beat);
            }
        }
        self.cued = last;
        out
    }

    /// The composer's view (loops, section, problems), realtime mode.
    pub fn status(&self) -> Option<crate::live::Status> {
        self.live.as_ref().map(|live| live.status())
    }
}

impl Drop for GameMusic {
    /// The music's feeds leave the registry with its handle (the
    /// instrument keeps its rings until the bus drops it).
    fn drop(&mut self) {
        // The composer thread first, so nothing sends to them again.
        self.live = None;
        makepad_audio_engine::feed::forget_instance(&self.name);
    }
}

impl MusicInstrument {
    fn take_swap(&mut self) {
        let Some(Swap::Next(mut next)) = self.swaps.pop() else { return };
        if next.schedule.base == Some(self.instance.schedule.generation) {
            for (from, to) in next.schedule.migrate.iter() {
                migrate_state(&mut self.instance.schedule.states[*from as usize], &mut next.schedule.states[*to as usize]);
            }
        }
        self.playing_index = next.layout.controls.get(&format!("{}.playing", transport(&next.name))).map(|c| c.0);
        if let (true, Some(index)) = (self.parked || !self.started, self.playing_index) {
            next.schedule.controls.set_manual(index, 0.0);
        }
        let old = std::mem::replace(&mut self.instance, next);
        if let Err(old) = self.retired.push(old) {
            // The handle has not polled: keep it rather than free it here.
            std::mem::forget(old);
        }
    }

    /// The beats that started in this block.
    fn announce(&mut self, beat0: f64, beat1: f64) {
        let per_bar = f64::from_bits(self.shared.bar.load(Ordering::Acquire)).max(1.0);
        let first = beat0.max(0.0).ceil() as u64;
        let mut b = first;
        while (b as f64) < beat1 {
            if self.last_beat.is_none_or(|last| b > last) {
                let bar = (b as f64 / per_bar).floor() as u64;
                let _ = self.events.push(MusicEvent::Beat { beat: b, bar, beat_in_bar: (b as f64 - bar as f64 * per_bar).round() as u32 });
                self.last_beat = Some(b);
            }
            b += 1;
        }
    }
}

impl Instrument for MusicInstrument {
    fn name(&self) -> &str {
        "score"
    }

    fn sample_rate(&self) -> f32 {
        self.rate
    }

    /// Notes from the game are not the score's: a score is played by its
    /// handle.
    fn note_on(&mut self, _note: u8, _velocity: u8) {}

    fn note_off(&mut self, _note: u8) {}

    /// Parks the music (a short fade out): the bus retiring it.
    fn all_notes_off(&mut self) {
        self.shared.fade.store(0.05f32.to_bits(), Ordering::Release);
        self.shared.gain.store(0f32.to_bits(), Ordering::Release);
        self.shared.parked.store(true, Ordering::Release);
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len().min(out_r.len());
        out_l.fill(0.0);
        out_r.fill(0.0);
        self.take_swap();
        let target = f32::from_bits(self.shared.gain.load(Ordering::Acquire));
        let fade = f32::from_bits(self.shared.fade.load(Ordering::Acquire));
        let parked = self.shared.parked.load(Ordering::Acquire);
        // Parked and faded out: stop the transport and skip the graph.
        if parked && self.gain <= 0.0 {
            if !self.parked {
                if let Some(index) = self.playing_index {
                    self.instance.schedule.controls.set_manual(index, 0.0);
                }
                self.parked = true;
                self.shared.playing.store(false, Ordering::Release);
                // One pass lets the loops release their voices.
                self.instance.run(CHUNK.min(n.max(1)), &mut |_, _, _| {});
            }
            return;
        }
        if !self.started && self.shared.ready.load(Ordering::Acquire) {
            if let Some(index) = self.playing_index {
                self.instance.schedule.controls.set_manual(index, 1.0);
            }
            self.started = true;
        }
        if self.parked && !parked {
            if let Some(index) = self.playing_index {
                self.instance.schedule.controls.set_manual(index, 1.0);
            }
            self.parked = false;
        }
        let step = if fade > 0.0 { 1.0 / (fade * self.rate) } else { f32::INFINITY };
        let mut at = 0;
        while at < n {
            // The graph always runs whole passes (so any host block size
            // plays the same samples); what a block does not take waits.
            if self.buffered == self.taken {
                let (bl, br) = (&mut self.buffer_l, &mut self.buffer_r);
                self.instance.run(CHUNK, &mut |cue, a, b| {
                    if !cue {
                        bl.copy_from_slice(&a[..CHUNK]);
                        br.copy_from_slice(&b[..CHUNK]);
                    }
                });
                let clock = self.instance.schedule.clocks.first().copied().unwrap_or_default();
                let beat1 = clock.start + clock.per_frame * CHUNK as f64;
                self.pass = clock;
                self.shared.beat.store(beat1.to_bits(), Ordering::Release);
                self.shared.bpm.store((clock.bpm as f64).to_bits(), Ordering::Release);
                self.shared.playing.store(clock.playing, Ordering::Release);
                self.buffered = CHUNK;
                self.taken = 0;
            }
            let k = (self.buffered - self.taken).min(n - at);
            // Beats are announced as they reach the output.
            if self.pass.playing {
                let tempo = self.pass.tempo;
                self.announce(tempo.beat_at(self.taken as f64), tempo.beat_at((self.taken + k) as f64));
            }
            for i in 0..k {
                self.gain = if self.gain < target { (self.gain + step).min(target) } else { (self.gain - step).max(target) };
                out_l[at + i] = self.buffer_l[self.taken + i] * self.gain;
                out_r[at + i] = self.buffer_r[self.taken + i] * self.gain;
            }
            self.taken += k;
            at += k;
        }
    }

    /// Parked and faded out, it needs no render until `resume`.
    fn is_active(&self) -> bool {
        !self.parked || !self.shared.parked.load(Ordering::Acquire)
    }
}
