//! The equalizer and our own volume. Two sounds go through them:
//!
//! * the music system's player (Spotify) keeps decoding and streaming, and
//!   while connected its sound is tapped (`makepad-audio-route`). The route
//!   only monitors (the player plays by itself; the tap feeds the pictures
//!   and meters) until the equalizer is on: then it processes,
//!   the player's own path silenced and its sound shaped here and played
//!   by this app instead, crossfaded both ways;
//! * local files always play through them (`player.rs`); with the
//!   equalizer off their bands and output level sit flat.
//!
//! Our volume is only on local files: a routed player keeps its own volume
//! (the slider still drives it), so routing never changes how loud it is.
//!
//! The knobs are atomics shared with the audio threads: the UI turns them
//! directly, and a route opened again (a new output device, the player
//! restarted) picks up the same knobs. The route itself is opened and
//! dropped on the **audio** worker, since opening talks to Core Audio and
//! can raise the system's permission dialog. Dropping it gives the player
//! its own path to the speakers back. The settings are kept on the disk by
//! the disk worker (`eq.txt`).

use crate::worker::{send as send_message, Message};
use crate::geq::{self, GraphicEq, GraphicEqHandle, BANDS};
use makepad_audio_route::{default_output, Analyzer, AnalyzerHandle, FnProcessor, Gain, GainHandle, Limiter, Mute, Processor, Route, RouteConfig, Source};
use makepad_widgets::log;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The output level's range: the graphic EQ's preamp (0 dB = unity). The
/// output never boosts more than +6 dB.
pub const GAIN_MIN_DB: f32 = -24.0;
pub const GAIN_MAX_DB: f32 = 6.0;
/// How long the safety limiter keeps running after the knobs stop
/// boosting, seconds: the bands glide back over about 0.1 s.
const LIMITER_HOLD_SECS: f64 = 0.5;
/// Mono samples a visualizer ring holds: about 0.7 s at 48 kHz.
const RING: usize = 32_768;
/// While on and not routed, the player is looked for again this often.
const RETRY: Duration = Duration::from_secs(5);
/// While routed, the default output is checked this often.
const CHECK: Duration = Duration::from_secs(2);
/// While routed, whether the route should be processing is looked at this
/// often.
const POLL: Duration = Duration::from_millis(100);

/// What the equalizer is set to. Saved on the disk as `eq.txt`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqSettings {
    pub on: bool,
    /// The graphic EQ's sliders (`geq::BAND_HZ`), dB.
    pub bands: [f32; BANDS],
    /// The preamp, dB.
    pub gain_db: f32,
    /// AUTO: the preamp follows the curve so its loudest point sits at 0 dB.
    pub auto: bool,
    /// The EQ panel is open (a drawer or popover where the layout has no
    /// `eq` slot).
    pub open: bool,
    /// Our volume, 0..1 as the slider shows it: what the now-playing bar
    /// sets while the sound runs through this app (see
    /// `Model::own_volume`), never the music system's.
    pub volume: f32,
    /// The Spotify audio connection (the tap and route, `connection.rs`)
    /// is wanted: on by default, off only once the person disconnected.
    /// Independent of `on`, which only bypasses the equalizer.
    pub connect: bool,
    /// macOS: the connection's explainer was shown once (the first use
    /// asks through it, never with a silent prompt).
    pub explained: bool,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self { on: false, bands: [0.0; BANDS], gain_db: 0.0, auto: false, open: false, volume: 1.0, connect: true, explained: false }
    }
}

impl EqSettings {
    pub fn serialize(&self) -> String {
        let bands: Vec<String> = self.bands.iter().map(|b| format!("{b}")).collect();
        format!("on {}\nbands {}\ngain {}\nauto {}\nopen {}\nvolume {}\nconnect {}\nexplained {}\n", self.on as u8, bands.join(" "), self.gain_db, self.auto as u8, self.open as u8, self.volume, self.connect as u8, self.explained as u8)
    }

    /// Settings read back, each within its range; unknown lines are skipped.
    /// The three-band tone control this replaced (`low`, `mid`, `high`)
    /// reads back as the bands round its old centres. Settings from before
    /// the connection was its own switch (no `connect` line) had it on
    /// while the equalizer was on.
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        let mut connect = None;
        let clamp = |v: f32| v.clamp(-geq::RANGE_DB, geq::RANGE_DB);
        for line in text.lines().take(16) {
            let mut parts = line.split_whitespace();
            if line.starts_with("bands ") {
                let values: Vec<f32> = parts.skip(1).filter_map(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).collect();
                if values.len() == BANDS {
                    for (slot, v) in settings.bands.iter_mut().zip(values) {
                        *slot = clamp(v);
                    }
                }
                continue;
            }
            let (Some(key), Some(value)) = (parts.next(), parts.next()) else { continue };
            let Ok(value) = value.parse::<f32>() else { continue };
            if !value.is_finite() {
                continue;
            }
            match key {
                "on" => settings.on = value != 0.0,
                "low" => settings.bands[..3].fill(clamp(value)),
                "mid" => settings.bands[4..7].fill(clamp(value)),
                "high" => settings.bands[8..].fill(clamp(value)),
                "gain" => settings.gain_db = value.clamp(GAIN_MIN_DB, GAIN_MAX_DB),
                "auto" => settings.auto = value != 0.0,
                "open" => settings.open = value != 0.0,
                "volume" => settings.volume = value.clamp(0.0, 1.0),
                "connect" => connect = Some(value != 0.0),
                "explained" => settings.explained = value != 0.0,
                _ => {}
            }
        }
        settings.connect = connect.unwrap_or(true);
        settings
    }
}

/// The knobs the UI turns and the rings the visualizer reads, shared with
/// every route the worker opens and with local playback.
#[derive(Clone)]
pub struct Knobs {
    /// The graphic EQ's filter gains (`geq`).
    pub eq: GraphicEqHandle,
    pub gain: GainHandle,
    /// Our volume: last in the local chain, after the output level. The
    /// route has none: the player's own volume already set its level.
    pub volume: GainHandle,
    /// The bands and the output level boost somewhere, so the safety
    /// limiter runs. Flat or cutting, nothing touches the sound.
    pub boost: Arc<AtomicBool>,
    /// The equalizer is on: what makes the route process.
    pub eq_on: Arc<AtomicBool>,
    /// What the route played.
    pub route_sound: AnalyzerHandle,
    /// What local playback played.
    pub local_sound: AnalyzerHandle,
}

impl Knobs {
    pub fn new() -> Self {
        let eq = GraphicEqHandle::new();
        let (_, gain) = Gain::new(1.0);
        let (_, volume) = Gain::new(1.0);
        let boost = Arc::new(AtomicBool::new(false));
        let eq_on = Arc::new(AtomicBool::new(false));
        let (_, route_sound) = Analyzer::new(Box::new(makepad_audio_route::Passthrough), RING);
        let (_, local_sound) = Analyzer::new(Box::new(makepad_audio_route::Passthrough), RING);
        Self { eq, gain, volume, boost, eq_on, route_sound, local_sound }
    }

    /// Turn every knob to what `settings` means: the bands and the output
    /// level as set while the equalizer is on, flat while it is off (local
    /// files still play through them); our volume either way.
    pub fn apply(&self, settings: &EqSettings) {
        let flat = EqSettings::default();
        let shaping = if settings.on { settings } else { &flat };
        let gains = geq::solve(&shaping.bands);
        self.eq.set(&gains);
        self.gain.set(db_to_linear(shaping.gain_db));
        self.volume.set(volume_to_linear(settings.volume));
        self.eq_on.store(settings.on, Ordering::Relaxed);
        let peak = (0..=190).map(|i| geq::response_db(&gains, geq::axis_hz(i as f32 / 190.0))).fold(0.0f32, f32::max);
        self.boost.store(shaping.gain_db.clamp(GAIN_MIN_DB, GAIN_MAX_DB) + peak > 0.0, Ordering::Relaxed);
    }

    /// The processor a sound runs: the bands and the output level, the tap
    /// its visualizer reads, our volume (local files only), then a safety
    /// limiter, all on these knobs. The tap sits before the volume so the
    /// picture of the sound does not shrink with how loud it is played.
    ///
    /// **Unity when flat.** With the bands and the output level at 0 dB and
    /// the volume full, every stage is skipped and the sound leaves sample
    /// for sample as it came: no preamp cut for headroom, no limiter
    /// bending the peaks of a loud master. The limiter runs only while the
    /// knobs boost somewhere ([`Knobs::boost`]), and a moment longer, while
    /// the bands glide back.
    fn chain(&self, sound: &AnalyzerHandle, own_volume: bool) -> Box<dyn Processor> {
        let mut eq = GraphicEq::with_handle(&self.eq);
        let mut gain = Gain::with_handle(&self.gain);
        let shaped = FnProcessor(move |frames: &mut [f32], info: &makepad_audio_route::FrameInfo| {
            eq.process(frames, info);
            gain.process(frames, info);
        });
        let mut tapped = Analyzer::with_handle(Box::new(shaped), sound);
        let mut volume = own_volume.then(|| Gain::with_handle(&self.volume));
        let mut limiter = Limiter::default();
        let boost = Arc::clone(&self.boost);
        // Frames the limiter keeps running after the boost ends.
        let mut hold = 0usize;
        // `AMP_SIM_NO_SILENT_BLOCKS=1` (tests): a silent block reaches
        // nothing, as when macOS's tap stops calling back while the player
        // is silent (paused, or playing on another device).
        let skip_silence = std::env::var_os("AMP_SIM_NO_SILENT_BLOCKS").is_some();
        Box::new(FnProcessor(move |frames: &mut [f32], info: &makepad_audio_route::FrameInfo| {
            if skip_silence && frames.iter().all(|s| *s == 0.0) {
                return;
            }
            tapped.process(frames, info);
            if let Some(volume) = &mut volume {
                volume.process(frames, info);
            }
            let block = frames.len() / (info.channels as usize).max(1);
            if boost.load(Ordering::Relaxed) {
                hold = (info.sample_rate * LIMITER_HOLD_SECS) as usize;
            }
            if hold > 0 {
                limiter.process(frames, info);
                hold = hold.saturating_sub(block);
            }
        }))
    }

    /// The routed player's chain: its level is the player's own volume, so
    /// no volume of ours on top (the slider drives the player's volume
    /// while routed, as when it is not).
    pub fn route_chain(&self) -> Box<dyn Processor> {
        self.chain(&self.route_sound, false)
    }

    /// Whether the routed player's sound should be ours to play: only while
    /// the equalizer changes it. Otherwise the route monitors
    /// and the player plays by itself.
    pub fn processing(&self) -> bool {
        self.eq_on.load(Ordering::Relaxed)
    }

    pub fn local_chain(&self) -> Box<dyn Processor> {
        self.chain(&self.local_sound, true)
    }
}

/// A slider position as our volume's gain: cubic, so loudness follows the
/// slider as heard (half way is -18 dB, a quarter -36 dB) and the bottom is
/// silent.
pub fn volume_to_linear(volume: f32) -> f32 {
    volume.clamp(0.0, 1.0).powi(3)
}

pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db.clamp(GAIN_MIN_DB, GAIN_MAX_DB) / 20.0)
}

/// Where the route stands, for the panel and the status line.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioState {
    /// The equalizer is off: the player plays by itself.
    Off,
    /// On, but the player has no sound to tap yet (not running, or never
    /// played since it started). Tried again every few seconds.
    Waiting,
    /// The player's sound runs through the equalizer to `device`.
    Routed { device: String, sample_rate: f64 },
    /// The route could not open; the equalizer is off again.
    Failed(String),
}

pub enum AudioCommand {
    /// The Spotify audio connection on (open the route) or off (drop it).
    Enable(bool),
    /// On, and opened again now even when it is open: macOS allowed it
    /// since, or the person pressed Connect.
    Reopen,
}

/// The audio worker: opens, watches and drops the route as asked.
pub fn run_audio(player: &'static str, knobs: Knobs, rx: Receiver<AudioCommand>, tx: SyncSender<Message>) {
    // A route the app did not close (it crashed while processing) may have
    // left the player's own level changed (Windows ducks its session).
    if let Some(dir) = crate::worker::data_dir() {
        makepad_audio_route::restore_after_crash(&dir);
    }
    let mut enabled = false;
    let mut route: Option<(Route, String)> = None;
    let mut state = AudioState::Off;
    let mut next_try = Instant::now();
    let mut last_check = Instant::now();
    // What the open route was last asked to be (`Route::set_processing`).
    let mut asked_processing: Option<bool> = None;
    loop {
        let wait = match (&route, enabled) {
            (Some(_), _) => Some(POLL),
            (None, true) => Some(next_try.saturating_duration_since(Instant::now())),
            (None, false) => None,
        };
        let received = match wait {
            Some(wait) => rx.recv_timeout(wait),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let mut asked = false;
        match received {
            Ok(first) => {
                // A burst of switches ends where the last one left it; a
                // reopen in it drops the route first.
                let mut reopen = false;
                let mut take = |command: AudioCommand| match command {
                    AudioCommand::Enable(on) => enabled = on,
                    AudioCommand::Reopen => {
                        enabled = true;
                        reopen = true;
                    }
                };
                take(first);
                while let Ok(command) = rx.try_recv() {
                    take(command);
                }
                if reopen && route.take().is_some() {
                    log!("amp: reopening the audio connection");
                }
                asked = true;
                next_try = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // The UI showed the switch at once; whatever this turn finds (the
        // same failure again too) goes back to it.
        if asked {
            state = AudioState::Off;
        }

        // Processing only while the equalizer is on; asked once
        // per change (a refusal is logged, not retried every poll).
        if let Some((opened, _)) = &route {
            let processing = knobs.processing();
            if asked_processing != Some(processing) {
                asked_processing = Some(processing);
                match opened.set_processing(processing) {
                    Ok(()) => log!("amp: the route {}", if processing { "processes (the player's own path silenced)" } else { "monitors (the player plays by itself)" }),
                    Err(error) => log!("amp: cannot switch the route's processing: {error}"),
                }
            }
        }
        // The output device the route plays to changed: open it again there.
        let check = last_check.elapsed() >= CHECK;
        if check {
            last_check = Instant::now();
        }
        if let (Some((_, uid)), true) = (&route, check) {
            if enabled && default_output().map(|device| device.uid != *uid).unwrap_or(false) {
                log!("amp: the output device changed; routing again");
                route = None;
                next_try = Instant::now();
            }
        }
        if !enabled && route.is_some() {
            route = None;
            log!("amp: equalizer off; the player plays by itself");
        }
        let wanted = if enabled {
            if route.is_none() && Instant::now() >= next_try {
                match open(player, &knobs) {
                    Ok((opened, device, uid)) => {
                        log!("amp: routing {player} to {device} at {:.0} Hz, {} ch", opened.sample_rate(), opened.channels());
                        let sample_rate = opened.sample_rate();
                        asked_processing = Some(opened.processing());
                        route = Some((opened, uid));
                        AudioState::Routed { device, sample_rate }
                    }
                    Err(makepad_audio_route::Error::NotFound) => {
                        next_try = Instant::now() + RETRY;
                        AudioState::Waiting
                    }
                    Err(error) => {
                        log!("amp: cannot route {player}: {error}");
                        enabled = false;
                        AudioState::Failed(failure(&error))
                    }
                }
            } else {
                state.clone()
            }
        } else {
            match &state {
                // A failure stays on screen until the equalizer is turned on again.
                AudioState::Failed(_) => state.clone(),
                _ => AudioState::Off,
            }
        };
        if wanted != state {
            state = wanted;
            send_message(&tx, Message::Audio(state.clone()));
        }
    }
}

fn open(player: &str, knobs: &Knobs) -> Result<(Route, String, String), makepad_audio_route::Error> {
    let output = default_output()?;
    let route = Route::open(
        RouteConfig {
            sources: vec![Source::BundleId(player.to_string())],
            mute: Mute::ReplaceWhileRouted,
            output: Some(output.uid.clone()),
            processing: knobs.processing(),
            state_dir: crate::worker::data_dir(),
        },
        knobs.route_chain(),
    )?;
    Ok((route, output.name, output.uid))
}

fn failure(error: &makepad_audio_route::Error) -> String {
    match error {
        makepad_audio_route::Error::Unsupported if cfg!(target_os = "macos") => "The equalizer needs macOS 14.2 or later".to_string(),
        makepad_audio_route::Error::Unsupported if cfg!(target_os = "windows") => "The equalizer needs Windows 10 version 2004 (build 19041) or later".to_string(),
        makepad_audio_route::Error::Unsupported => "The equalizer cannot tap another app's sound on this system yet".to_string(),
        other if cfg!(target_os = "macos") => format!("The equalizer could not tap the player's sound ({other}). Allow Makepad Amp under System Settings › Privacy & Security › Screen & System Audio Recording, then turn it on again."),
        other => format!("The equalizer could not tap the player's sound ({other}). Turn it on again to retry."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_back_and_stay_in_range() {
        let settings = EqSettings { on: true, bands: [3.5, -2.0, 0.0, 1.0, 0.0, 0.0, -4.5, 0.0, 6.0, 12.0], gain_db: -6.0, auto: true, open: true, volume: 0.5, connect: false, explained: true };
        assert_eq!(EqSettings::parse(&settings.serialize()), settings);
        let wild = EqSettings::parse("on 1\nlow 99\nmid -99\ngain 40\nhigh nan\nvolume 7\nbogus 3\n");
        let r = geq::RANGE_DB;
        assert_eq!(wild, EqSettings { on: true, bands: [r, r, r, 0.0, -r, -r, -r, 0.0, 0.0, 0.0], gain_db: GAIN_MAX_DB, auto: false, open: false, volume: 1.0, connect: true, explained: false });
        assert_eq!(EqSettings::parse(""), EqSettings::default());
    }

    /// The audio connection is its own saved switch, on unless the person
    /// disconnected it, whatever the equalizer.
    #[test]
    fn the_connection_reads_back_apart_from_the_equalizer() {
        assert!(EqSettings::parse("on 1\n").connect);
        assert!(EqSettings::parse("on 0\n").connect);
        assert!(EqSettings::default().connect && !EqSettings::default().explained);
        let off_but_connected = EqSettings { on: false, connect: true, ..EqSettings::default() };
        assert_eq!(EqSettings::parse(&off_but_connected.serialize()), off_but_connected);
        assert!(!EqSettings::parse("on 1\nconnect 0\n").connect);
    }

    #[test]
    fn output_level_never_boosts_past_its_limit() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(6.0) - 1.995).abs() < 0.01);
        assert_eq!(db_to_linear(20.0), db_to_linear(GAIN_MAX_DB));
    }

    /// Our volume: full is unity, the bottom silent, cubic in between (a
    /// quarter of the slider is still heard: -36 dB, not near silence).
    #[test]
    fn our_volume_is_cubic() {
        assert_eq!(volume_to_linear(1.0), 1.0);
        assert_eq!(volume_to_linear(0.0), 0.0);
        assert_eq!(volume_to_linear(-0.5), 0.0);
        assert_eq!(volume_to_linear(1.5), 1.0);
        let db = |v: f32| 20.0 * volume_to_linear(v).log10();
        assert!((db(0.5) + 18.06).abs() < 0.01);
        assert!((db(0.25) + 36.12).abs() < 0.01);
        assert!(db(0.24) > -38.0);
    }

    /// With the equalizer off the bands and the output level sit flat (local
    /// files play through them), while our volume still holds.
    #[test]
    fn off_is_flat_but_keeps_our_volume() {
        let knobs = Knobs::new();
        let settings = EqSettings { on: false, bands: [6.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], gain_db: -12.0, auto: false, open: false, volume: 0.5, connect: false, explained: false };
        knobs.apply(&settings);
        assert_eq!(knobs.gain.get(), 1.0);
        assert!((knobs.volume.get() - volume_to_linear(0.5)).abs() < 1e-6);
        knobs.apply(&EqSettings { on: true, ..settings });
        assert!((knobs.gain.get() - db_to_linear(-12.0)).abs() < 1e-6);
    }
}

#[cfg(test)]
#[path = "levels_test.rs"]
mod levels;
