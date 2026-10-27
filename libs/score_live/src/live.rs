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
use crate::model::{self, Diag};
use makepad_audio_engine::feed::{loop_feed, LoopFeed};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Where the music is: the beat playing at the output, the tempo and
/// whether it plays (Stage's transport, a game's clock).
pub trait ClockSource: Send + Sync {
    fn now(&self) -> Option<(f64, f64, bool)>;
}

pub enum Cmd {
    Source(String),
    Param(String, f32),
    Trigger(String),
    Goto { section: String, quant: f64 },
    /// Change the tempo from the next beat not yet scheduled, over `ramp`
    /// beats.
    Tempo { bpm: f64, ramp: f64 },
    /// Send the tempo changes to another transport from now on.
    Transport(String),
    /// A message from the MIDI input (see [`Composer::midi_in`]).
    Midi([u8; 3]),
    /// Play the arrangement from `beat` (song beats) at the next `quant`.
    Seek { beat: f64, quant: f64 },
    /// Repeat song beats [from, to), or play on.
    Loop(Option<(f64, f64)>),
    Quit,
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

#[derive(Clone)]
pub struct LiveConfig {
    /// The score's node in the graph: its loops' feeds are `<instance>/<loop>`.
    pub instance: String,
    /// How far ahead of the output the composer schedules, in seconds.
    pub lookahead: f64,
    pub clock: Arc<dyn ClockSource>,
}

pub struct LiveScore {
    cmd: Sender<Cmd>,
    pub status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
    /// The composer when it has no thread: the host pumps it.
    inline: Option<Mutex<Worker>>,
    instance: String,
}

/// Whether this platform can start threads (it cannot on the web without
/// shared memory): asked once, by starting one.
pub fn threads_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| std::thread::Builder::new().spawn(|| {}).map(|t| t.join().is_ok()).unwrap_or(false))
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
        LiveScore::start(config, transport, false)
    }

    /// One the host pumps ([`LiveScore::pump`]) even where threads exist.
    pub fn new_pumped(config: LiveConfig, transport: &str) -> LiveScore {
        LiveScore::start(config, transport, true)
    }

    fn start(config: LiveConfig, transport: &str, pumped: bool) -> LiveScore {
        // Its MIDI loops play to the MIDI outputs.
        makepad_audio_engine::feed::midi_out::set_live(&config.instance, true);
        let instance = config.instance.clone();
        let tempo = makepad_audio_engine::feed::tempo_feed(transport);
        let status = Arc::new(Mutex::new(Status { bpm: 120.0, sig: (4, 4), ..Status::default() }));
        let (tx, rx) = channel();
        // Made where it runs: the composer's VM never moves between threads.
        let worker = {
            let status = status.clone();
            move || Worker { config, rx, status, composer: None, source: String::new(), revision: 0, feeds: HashMap::new(), last_beat: None, dropped: 0, tempo, osc_in: None, osc_out: None, osc_pending: Vec::new(), seek_stopped: None, loop_range: None, steps: 0 }
        };
        if !threads_available() || pumped {
            return LiveScore { cmd: tx, status, thread: None, inline: Some(Mutex::new(worker())), instance };
        }
        let thread = std::thread::Builder::new().name("score-composer".into()).spawn(move || worker().run()).expect("the composer thread");
        LiveScore { cmd: tx, status, thread: Some(thread), inline: None, instance }
    }

    /// Without a composer thread: run what is due (commands, the loops'
    /// code up to the lookahead, the status). The host calls it often
    /// (every audio block or frame, at least every lookahead / 2); with a
    /// thread it does nothing.
    pub fn pump(&self) {
        if let Some(worker) = &self.inline {
            if let Ok(mut worker) = worker.lock() {
                worker.step(1);
            }
        }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.cmd.send(cmd);
    }

    pub fn set_source(&self, source: impl Into<String>) {
        self.send(Cmd::Source(source.into()));
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

    pub fn set_tempo(&self, bpm: f64, ramp: f64) {
        self.send(Cmd::Tempo { bpm, ramp });
    }

    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for LiveScore {
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        makepad_audio_engine::feed::midi_out::set_live(&self.instance, false);
    }
}

struct Worker {
    config: LiveConfig,
    rx: Receiver<Cmd>,
    status: Arc<Mutex<Status>>,
    composer: Option<Composer>,
    source: String,
    revision: u64,
    feeds: HashMap<u64, Arc<LoopFeed>>,
    last_beat: Option<f64>,
    dropped: u32,
    tempo: Arc<makepad_audio_engine::feed::TempoFeed>,
    /// Steps taken (the status goes out every few).
    steps: u32,
    /// A seek made while stopped (the next play starts there) and the loop
    /// (kept across restarts).
    seek_stopped: Option<f64>,
    loop_range: Option<(f64, f64)>,
    /// OSC: the socket listening on the Clock's `osc_in` port, the one
    /// sending, and what waits for its beat.
    osc_in: Option<(u16, std::net::UdpSocket)>,
    osc_out: Option<std::net::UdpSocket>,
    osc_pending: Vec<(f64, String, Vec<u8>)>,
}

impl Worker {
    /// The composer thread: a step whenever a command comes or every
    /// [`STEP`].
    fn run(mut self) {
        loop {
            match self.rx.recv_timeout(STEP) {
                Ok(Cmd::Quit) | Err(RecvTimeoutError::Disconnected) => break,
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
        while let Ok(cmd) = self.rx.try_recv() {
            if let Cmd::Quit = cmd {
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

    /// Start every loop over from the bar holding `beat` (the clock jumped).
    fn restart_at(&mut self, beat: f64) {
        let Ok(loaded) = model::load(&self.source) else { return };
        let params = self.composer.as_ref().map(|c| c.params.clone()).unwrap_or_default();
        let mut fresh = Composer::new(loaded);
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

    fn pump(&mut self) {
        let Some((now, bpm, playing)) = self.config.clock.now() else { return };
        if self.composer.is_none() || !playing {
            self.last_beat = None;
            return;
        }
        // The clock started, or jumped (a locate): begin on the next bar.
        match self.last_beat {
            None => self.restart_at(now),
            Some(last) if now < last - 0.5 || now > last + 16.0 => self.restart_at(now),
            _ => {}
        }
        self.last_beat = Some(now);
        let until = now + self.config.lookahead * bpm / 60.0;
        let mut events = Vec::new();
        if let Some(composer) = &mut self.composer {
            composer.pump(until, &mut events);
            for change in composer.tempo_out.drain(..) {
                self.tempo.send(change);
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

    fn command(&mut self, cmd: Cmd) {
        let now = self.config.clock.now().map_or(0.0, |(beat, bpm, _)| beat + self.config.lookahead * bpm / 60.0);
        match cmd {
            Cmd::Source(source) => self.load(source, now),
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
                self.tempo = makepad_audio_engine::feed::tempo_feed(&name);
                // The new transport starts from the tempo map as it stands.
                if let Some(composer) = &self.composer {
                    let beat = now;
                    let mut current = None;
                    for change in composer.tempo_map.iter().filter(|c| c.beat <= beat) {
                        current = Some(*change);
                    }
                    if let Some(change) = current {
                        self.tempo.send(makepad_audio_engine::feed::TempoEvent { beat, bpm: change.bpm, ramp: 0.0 });
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
                let mut events = Vec::new();
                if let Some(composer) = &mut self.composer {
                    composer.midi_in(data, beat, &mut events);
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
            Cmd::Quit => {}
        }
    }

    fn load(&mut self, source: String, now: f64) {
        self.revision += 1;
        match model::load(&source) {
            Err(diags) => {
                if let Ok(mut status) = self.status.lock() {
                    status.revision = self.revision;
                    status.diags = diags;
                }
            }
            Ok(loaded) => {
                match &mut self.composer {
                    Some(composer) => composer.reload(loaded, now),
                    None => self.composer = Some(Composer::new(loaded)),
                }
                self.feeds_for();
                self.source = source;
                if let Ok(mut status) = self.status.lock() {
                    status.revision = self.revision;
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Playing;
    impl ClockSource for Playing {
        fn now(&self) -> Option<(f64, f64, bool)> {
            Some((0.0, 120.0, true))
        }
    }

    /// Without a thread the host's pumps run the composer: the source
    /// loads, the loops' first cycles are generated and the status
    /// published.
    #[test]
    fn a_pumped_score_plays_without_a_thread() {
        let config = LiveConfig { instance: "pumped_test".into(), lookahead: 0.15, clock: Arc::new(Playing) };
        let live = LiveScore::new_pumped(config, "pumped_test_clock");
        live.set_source("use mod.score.*\nlet clock = Clock{bpm: 120}\nlet s = Synth{preset: @pluck}\nlet l = Loop{play: s beats: 1 code: fn(i) { play(@c4) sleep(1) }}\nScore{clock s l}\n");
        assert!(live.status().loops.is_empty(), "nothing runs before a pump");
        for _ in 0..3 {
            live.pump();
        }
        let status = live.status();
        assert!(status.diags.is_empty(), "{:?}", status.diags);
        assert_eq!(status.playing_revision, 1);
        assert!(status.loops.iter().any(|l| l.name == "l" && !l.notes.is_empty()), "{:?}", status.loops);
    }
}
