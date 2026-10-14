//! The Talk screen: a conversation between two people, or live captions of
//! the room, and tap-to-toggle dictation for the main screen.
//!
//! Everything arrives through [`Ears`] and leaves through [`Mouth`], so the
//! hub's Whisper and Kokoro can be swapped for the phone's own models without
//! touching this file. A child module of `view` (it reads the view's state).

use super::*;
use crate::voice::{Capture, Cut, EarEvent, Ears, HubEars, HubMouth, Listening, Mouth, MouthEvent, Player, Segmenter, VoiceInfo, Voices};
use crate::prompt::{live_system, quick_max_tokens, quick_system};
use makepad_widgets::makepad_platform::audio::{AudioDeviceId, AudioDevicesEvent};
use std::collections::{HashMap, VecDeque};

/// Newest bubbles kept on screen and in memory; older ones live in the history.
pub(super) const MAX_BUBBLES: usize = 20;
/// A pause this long ends an utterance; speech is cut at the longest this many seconds.
const PAUSE_SECS: f32 = 0.6;
const MAX_UTTERANCE_SECS: f32 = 12.0;
/// A partial that stood still this long gets a provisional translation (live mode).
const PROVISIONAL_AFTER: f64 = 0.9;
const TICK_SECS: f64 = 0.05;
/// An open microphone stream that delivered nothing for this long is reported.
const MIC_SILENT_SECS: f64 = 3.0;

/// The microphone's state: closed until a record press, asked for once.
#[derive(Default, Debug)]
pub(super) struct MicState {
    pub open: bool,
    asked: bool,
}

impl MicState {
    /// True the first time only: the permission prompt belongs to the first record press.
    pub fn request_permission(&mut self) -> bool {
        !std::mem::replace(&mut self.asked, true)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TalkMode {
    /// Two people taking turns: each turn detected, translated, spoken.
    Conversation,
    /// Captions of the room: translated as you go, not spoken.
    Live,
}

#[derive(Clone, Debug)]
pub(super) struct Bubble {
    pub from: Lang,
    pub source: String,
    pub target: String,
    pub time: String,
    /// When the speaker stopped, the text arrived, the translation arrived (latency log).
    t_end: Option<std::time::Instant>,
    t_heard: Option<std::time::Instant>,
    t_translated: Option<std::time::Instant>,
    t_audio: Option<std::time::Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum JobKind {
    Final(usize),
    Provisional,
}

pub(super) struct Talk {
    pub open: bool,
    /// Seconds of speech in the turn being transcribed, and the samples fed to
    /// the recogniser in this dictation.
    pub(super) heard_secs: f32,
    dict_samples: usize,
    /// When microphone audio (or the test clip standing in for it) last arrived.
    heard_at: f64,
    /// The mic look last applied: lit (capturing) or not.
    mic_lit: Option<bool>,
    pub mode: TalkMode,
    pub listening: bool,
    /// Dictation (tap the mic to start, tap again to stop) into the source pane (main screen).
    pub dictating: bool,
    pub auto_speak: bool,
    /// A speak press whose audio is not all played yet.
    pub speak_requested: bool,
    ears: Option<Box<dyn Ears>>,
    mouth: Option<Box<dyn Mouth>>,
    capture: Capture,
    player: Player,
    seg: Segmenter,
    tick: Option<Timer>,
    audio_installed: bool,
    pub mic: MicState,
    mic_permission: Option<PermissionStatus>,
    /// The microphone to record from: the system's default input, as the last
    /// device list named it (None until a list arrived, or when it has none).
    input_device: Option<AudioDeviceId>,
    input_name: String,
    /// The device list said the default input failed to open.
    input_failed: bool,
    /// A device list has arrived (before it, no input is not an error yet).
    devices_known: bool,
    /// The device the open stream was asked for, when, and whether audio came since.
    mic_device: Option<AudioDeviceId>,
    mic_opened_at: f64,
    mic_heard: bool,
    /// Why the microphone gives nothing, shown where the recording was started.
    mic_error: Option<String>,
    ears_note: String,
    mouth_note: String,
    /// The voices the mouth offers, once it said so.
    pub voices: Vec<VoiceInfo>,
    /// When the last utterance ended, its text arrived, its translation arrived
    /// (for the per-turn latency log).
    ends: VecDeque<std::time::Instant>,
    /// How much of the running translation (bytes of the cleaned text) has been
    /// handed to the voice already, so speech starts at the first clause.
    spoken_upto: usize,
    pub bubbles: Vec<Bubble>,
    partial: String,
    partial_since: f64,
    provisional: String,
    provisional_for: String,
    pub(super) in_utterance: bool,
    /// Recognition of the utterance so far was started at a short pause; speech
    /// that resumes discards it and reads the whole utterance again.
    speculating: bool,
    /// Utterances handed to the recogniser whose transcript has not come back.
    pub(super) finals_pending: usize,
    /// The utterance's audio so far (bounded), kept to redo a speculative reading.
    utt_audio: Vec<f32>,
    pending: VecDeque<usize>,
    engine: Option<Engine>,
    pub(super) job: Option<(u64, JobKind)>,
    next_job: u64,
    /// Speech requests in flight: id -> what the audio is for.
    say: HashMap<u64, SayFor>,
    /// Utterances whose speech already arrived as chunks: their whole-take audio is a duplicate.
    streamed: std::collections::HashSet<u64>,
    inject: Inject,
    level: f32,
    /// Chunks of dictation that were louder than the room (a held mic with
    /// nothing said must not be sent: Whisper invents text for silence).
    voiced_chunks: usize,
    /// The last stretch of quiet before the first words of a dictation, kept so
    /// the first syllable is not clipped; nothing is sent to Whisper until then.
    dictation_preroll: Vec<f32>,
    pub status: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SayFor {
    /// Speech to play; the bubble it belongs to (for the latency log).
    Play(Option<usize>),
    /// A synthetic utterance fed back as microphone audio (development hook).
    Inject(usize),
}

/// `MAKEPAD_TRANSLATOR_INJECT="zh:你好;en:Hello"`: instead of the microphone,
/// the Talk screen is fed these utterances, spoken by the hub's own voices and
/// played into the same path as real audio. For checking voice on machines with
/// no microphone and without anyone talking.
#[derive(Default)]
struct Inject {
    script: Vec<(Lang, String)>,
    started: bool,
    clips: HashMap<usize, Vec<f32>>,
    next: usize,
    current: Vec<f32>,
    pos: usize,
    quiet_left: usize,
    last_tick: Option<std::time::Instant>,
}

#[cfg(target_os = "android")]
fn inject_property() -> Option<String> {
    use std::ffi::{c_char, c_int};
    extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }
    let mut value = [0 as c_char; 92];
    let len = unsafe { __system_property_get(c"debug.makepad.inject".as_ptr(), value.as_mut_ptr()) };
    (len > 0).then(|| value[..len as usize].iter().map(|c| *c as u8 as char).collect())
}

#[cfg(not(target_os = "android"))]
fn inject_property() -> Option<String> {
    None
}

impl Inject {
    fn from_env() -> Inject {
        let mut script = Vec::new();
        // On a phone the hook comes from `adb shell setprop debug.makepad.inject
        // "en:..."` (apps get no environment there; 91 characters at most).
        if let Some(spec) = std::env::var("MAKEPAD_TRANSLATOR_INJECT").ok().or_else(inject_property) {
            for part in spec.split(';') {
                if let Some((code, text)) = part.split_once(':') {
                    let lang = if code.trim() == "zh" { Lang::Zh } else { Lang::En };
                    if !text.trim().is_empty() {
                        script.push((lang, text.trim().to_string()));
                    }
                }
            }
        }
        Inject { script, ..Inject::default() }
    }
}

impl Default for Talk {
    fn default() -> Self {
        Talk {
            open: false,
            heard_secs: 0.0,
            dict_samples: 0,
            heard_at: 0.0,
            mic_lit: None,
            mode: TalkMode::Conversation,
            listening: false,
            dictating: false,
            auto_speak: true,
            speak_requested: false,
            ears: None,
            mouth: None,
            capture: Capture::default(),
            player: Player::default(),
            seg: Segmenter::new(PAUSE_SECS, MAX_UTTERANCE_SECS),
            tick: None,
            audio_installed: false,
            mic: MicState::default(),
            mic_permission: None,
            input_device: None,
            input_name: String::new(),
            input_failed: false,
            devices_known: false,
            mic_device: None,
            mic_opened_at: 0.0,
            mic_heard: false,
            mic_error: None,
            ears_note: String::new(),
            mouth_note: String::new(),
            voices: Vec::new(),
            ends: VecDeque::new(),
            spoken_upto: 0,
            bubbles: Vec::new(),
            partial: String::new(),
            partial_since: 0.0,
            provisional: String::new(),
            provisional_for: String::new(),
            in_utterance: false,
            speculating: false,
            finals_pending: 0,
            utt_audio: Vec::new(),
            pending: VecDeque::new(),
            engine: None,
            job: None,
            next_job: 1 << 40,
            say: HashMap::new(),
            streamed: Default::default(),
            inject: Inject::from_env(),
            level: 0.0,
            voiced_chunks: 0,
            dictation_preroll: Vec::new(),
            status: String::new(),
        }
    }
}

/// Why a wanted microphone gives nothing, if it does: no input device at all,
/// the default one failed to open, or an open stream silent for too long.
fn mic_problem(talk: &Talk, now: f64) -> Option<String> {
    let wanted = (talk.listening || talk.dictating) && talk.inject.script.is_empty();
    if !wanted || !talk.devices_known || talk.mic_heard {
        None
    } else if talk.input_device.is_none() {
        Some("No microphone found: connect one, or choose one in the system's sound settings.".to_string())
    } else if talk.input_failed {
        Some("The microphone could not be opened: check the system's sound settings.".to_string())
    } else if talk.mic.open && now - talk.mic_opened_at > MIC_SILENT_SECS {
        Some("No sound from the microphone: it could not be opened. Check the system's sound settings.".to_string())
    } else {
        None
    }
}

/// `whisper (whisper-large-v3-turbo) on https://10.0.0.149:8790` -> `whisper on 10.0.0.149`.
fn short_note(note: &str) -> String {
    let engine = note.split(' ').next().unwrap_or(note);
    match note.split(" on ").nth(1) {
        Some(node) => format!("{engine} on {}", node.trim_start_matches("https://").trim_start_matches("http://").split(':').next().unwrap_or(node)),
        None => engine.to_string(),
    }
}

/// The end (byte index just past the mark) of the first full clause of `text`
/// after `from`, or `None` while there is none yet. A clause ends at a sentence
/// mark or a comma; very short ones wait for the next so the voice is not asked
/// for a word at a time.
fn clause_end(text: &str, from: usize, lang: Lang) -> Option<usize> {
    let rest = text.get(from..)?;
    let min = if lang == Lang::Zh { 4 } else { 14 };
    let mut chars = 0;
    for (i, c) in rest.char_indices() {
        chars += 1;
        let mark = matches!(c, '，' | '。' | '！' | '？' | '；' | '：' | ',' | '.' | '!' | '?' | ';' | ':' | '\n');
        // A decimal point or a time (3.5, 10:30) is not a clause end.
        let numeric = matches!(c, '.' | ':' | ',') && rest[i + c.len_utf8()..].chars().next().is_some_and(|n| n.is_ascii_digit());
        if mark && !numeric && chars >= min {
            return Some(from + i + c.len_utf8());
        }
    }
    None
}

fn clock(cx: &Cx) -> String {
    let secs = Cx::time_now().max(0.0) as i64 + script_local_utc_offset_secs() as i64;
    let _ = cx;
    format!("{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60)
}

use makepad_widgets::makepad_platform::script::timer::script_local_utc_offset_secs;

impl TranslatorView {
    // ------------------------------------------------------------ plumbing

    fn voice_wake() -> Arc<dyn Fn() + Send + Sync> {
        Arc::new(|| SignalToUI::set_ui_signal())
    }

    /// Start the models and the audio callbacks, once (first use).
    fn ensure_voice(&mut self, cx: &mut Cx) {
        // On the phone the listening model (Whisper) is loaded only while the
        // microphone is in use; everywhere else it is set up with the rest.
        if !self.on_device() {
            self.ensure_ears();
        }
        if self.talk.mouth.is_none() {
            crate::ondevice::speech_in_process(self.on_device());
            self.talk.mouth = Some(Box::new(HubMouth::start(Self::voice_wake(), self.chosen_voices())));
        }
        if self.talk.engine.is_none() {
            self.talk.engine = Some(Engine::start(Self::voice_wake()));
        }
        if !self.talk.audio_installed {
            self.talk.audio_installed = true;
            let capture = self.talk.capture.clone();
            cx.audio_input(0, move |info, buffer| {
                let channels: Vec<&[f32]> = (0..buffer.channel_count()).map(|c| buffer.channel(c)).collect();
                capture.push_planar(info.sample_rate, &channels);
            });
            let player = self.talk.player.clone();
            cx.audio_output(0, move |info, buffer| {
                let channels = buffer.channel_count();
                let frames = buffer.frame_count();
                let mut inter = vec![0.0f32; frames * channels];
                player.fill(info.sample_rate, channels, &mut inter);
                for c in 0..channels {
                    let out = buffer.channel_mut(c);
                    for f in 0..frames {
                        out[f] = inter[f * channels + c];
                    }
                }
            });
        }
        if self.talk.tick.is_none() {
            self.talk.tick = Some(cx.start_interval(TICK_SECS));
        }
    }

    /// Open the microphone stream. Only ever called from a user's record press
    /// (the dictation mic, the Talk mic, Live); the permission is asked here, not at start.
    fn mic_on(&mut self, cx: &mut Cx) {
        if self.talk.inject.script.is_empty() {
            if self.talk.mic.request_permission() {
                cx.request_permission(Permission::AudioInput);
            }
            if let Some(device) = self.talk.input_device {
                cx.use_audio_inputs(&[device]);
                self.talk.mic.open = true;
                if self.talk.mic_device != Some(device) {
                    self.talk.mic_device = Some(device);
                    self.talk.mic_opened_at = Cx::time_now();
                    self.talk.mic_heard = false;
                    log!("translator: microphone opening: {}", self.talk.input_name);
                }
            }
        }
        self.check_mic(cx);
    }

    /// Close the microphone stream completely (not just stop processing).
    fn mic_off(&mut self, cx: &mut Cx) {
        cx.use_audio_inputs(&[]);
        self.talk.mic.open = false;
        self.talk.mic_device = None;
        self.talk.capture.take();
        self.check_mic(cx);
    }

    fn listening_status(&self) -> String {
        if self.talk.mode == TalkMode::Conversation {
            "Listening. Take turns; each turn is translated and spoken.".to_string()
        } else {
            "Listening. Captions appear as people talk.".to_string()
        }
    }

    /// Says so, where the recording was started, when the microphone gives
    /// nothing: no input device at all, the default one failed to open, or a
    /// stream that opened but has not delivered any audio for a few seconds.
    /// The error goes away as soon as audio arrives.
    fn check_mic(&mut self, cx: &mut Cx) {
        let error = mic_problem(&self.talk, Cx::time_now());
        if error == self.talk.mic_error {
            return;
        }
        let old = std::mem::replace(&mut self.talk.mic_error, error.clone());
        match &error {
            Some(why) => {
                log!("translator: microphone: {why} (device: {})", self.talk.input_name);
                if self.talk.dictating {
                    self.set_status(cx, why, true);
                } else {
                    self.talk.status = why.clone();
                }
            }
            None => {
                if self.talk.dictating {
                    self.set_status(cx, "Recording… tap the microphone to translate.", false);
                } else if old.is_some_and(|old| old == self.talk.status) {
                    self.talk.status = if self.talk.listening { self.listening_status() } else { String::new() };
                }
            }
        }
        self.refresh_talk(cx);
        self.sync_activity(cx);
    }

    /// The microphone stream is open right now (the recording indicator).
    pub fn mic_open(&self) -> bool {
        self.talk.mic.open
    }

    /// The microphone is wanted but gives nothing (the reason is on screen).
    pub(super) fn mic_failed(&self) -> bool {
        self.talk.mic_error.is_some()
    }

    /// Hub / On-device switched: the listening and speaking sessions end and
    /// start again where the models now run (the next use, or now while listening).
    pub(super) fn drop_speech_sessions(&mut self) {
        if let Some(e) = self.talk.ears.take() {
            e.cancel();
        }
        if let Some(m) = self.talk.mouth.take() {
            m.cancel();
        }
        self.talk.in_utterance = false;
        self.talk.partial.clear();
        self.talk.ears_note.clear();
        if self.talk.listening || self.talk.dictating {
            self.ensure_ears();
        }
        if self.talk.audio_installed {
            crate::ondevice::speech_in_process(self.on_device());
            self.talk.mouth = Some(Box::new(HubMouth::start(Self::voice_wake(), self.chosen_voices())));
        }
    }

    /// Background, pause or shutdown: every way of recording ends and the mic closes.
    pub(super) fn stop_all_recording(&mut self, cx: &mut Cx) {
        if self.talk.listening {
            self.set_listening(cx, false);
        }
        if self.talk.dictating {
            self.end_dictation(cx);
        }
        self.mic_off(cx);
        self.sync_chrome(cx);
    }

    fn on_device(&self) -> bool {
        self.saved.model == crate::ondevice::MODEL
    }

    /// Create the listening session (loads Whisper in-process on the phone).
    fn ensure_ears(&mut self) {
        if self.talk.ears.is_none() {
            self.talk.ears_note = if self.on_device() { "Loading the listening model…" } else { "Connecting to the hub's Whisper…" }.to_string();
            log!("translator: listening session started ({})", if self.on_device() { "on this phone" } else { "hub" });
            crate::ondevice::speech_in_process(self.on_device());
            self.talk.ears = Some(Box::new(HubEars::start(Self::voice_wake(), self.listening_for_mode())));
        }
    }

    /// On the phone: drop the listening model once nobody is talking and the last
    /// transcript is in, freeing its memory for the translator and the voice.
    fn release_ears_if_idle(&mut self) {
        if self.on_device() && !self.talk.listening && !self.talk.dictating && !self.talk.in_utterance && self.talk.finals_pending == 0 {
            if let Some(e) = self.talk.ears.take() {
                e.cancel();
                log!("translator: listening model released");
            }
            self.talk.ears_note.clear();
            self.talk.partial.clear();
        }
    }

    fn listening_for_mode(&self) -> Listening {
        if self.talk.mode == TalkMode::Live {
            Listening::WithPartials
        } else {
            Listening::FinalOnly
        }
    }

    /// Conversation reads each turn once; Live also reads it while it is spoken.
    /// The recognition session is set up for one or the other, so switching starts a new one.
    fn restart_ears(&mut self) {
        if self.talk.ears.is_some() {
            if let Some(old) = self.talk.ears.take() {
                old.cancel();
            }
            crate::ondevice::speech_in_process(self.on_device());
            self.talk.ears = Some(Box::new(HubEars::start(Self::voice_wake(), self.listening_for_mode())));
            self.talk.ears_note = "Connecting to the hub's Whisper…".to_string();
            self.talk.in_utterance = false;
            self.talk.seg.reset();
            self.talk.partial.clear();
        }
    }

    // ------------------------------------------------------------- screens

    /// Speech is requested (synthesis running) or playing: the Speaking state starts
    /// at the press, not at the first sound.
    /// A heard turn is still being recognised or translated.
    pub(super) fn talk_working(&self) -> bool {
        self.talk.finals_pending > 0 || self.talk.job.is_some()
    }

    pub(super) fn talk_speaking(&self) -> bool {
        self.talk.speak_requested || self.talk.player.is_playing()
    }

    /// Playback position of the current speech: (seconds played, seconds queued so far).
    pub fn speech_progress(&self) -> (f64, f64) {
        self.talk.player.progress()
    }

    /// The output card's play button: speaks `text` at once, or stops speech instantly
    /// (synthesis and playback) when it is already speaking.
    pub fn toggle_speaking(&mut self, cx: &mut Cx, text: &str, lang: Lang) {
        if self.talk_speaking() {
            self.stop_speaking();
        } else if !text.trim().is_empty() {
            self.ensure_voice(cx);
            self.talk.speak_requested = true;
            self.speak(text, lang);
        }
        self.sync_chrome(cx);
    }

    pub fn stop_speaking(&mut self) {
        if let Some(m) = &self.talk.mouth {
            m.cancel();
        }
        self.talk.say.retain(|_, s| !matches!(s, SayFor::Play(_)));
        self.talk.streamed.clear();
        self.talk.player.stop();
        self.talk.speak_requested = false;
    }

    /// The Listen screen: live captions of whatever is said, listening at once.
    pub(super) fn open_listen(&mut self, cx: &mut Cx) {
        if self.talk.mode != TalkMode::Live {
            self.talk.mode = TalkMode::Live;
            self.restart_ears();
        }
        self.talk.player.stop();
        // Opening the Listen screen never records: the mic opens on the record tap.
        self.open_talk(cx);
        self.talk.status = "Tap the microphone to start listening.".to_string();
    }

    pub(super) fn open_talk(&mut self, cx: &mut Cx) {
        cx.set_key_focus(Area::Empty);
        cx.hide_text_ime();
        self.close_camera_if_open(cx);
        self.talk.open = true;
        self.talk.listening = false;
        self.talk.status = "Tap the microphone, then talk. Each turn is translated and spoken.".to_string();
        self.ensure_voice(cx);
        self.sync_chrome(cx);
        self.refresh_talk(cx);
    }

    pub(super) fn close_talk(&mut self, cx: &mut Cx) {
        self.set_listening(cx, false);
        self.talk.open = false;
        // Dictation on the Translate screen reads final turns only.
        self.talk.mode = TalkMode::Conversation;
        if let Some(m) = &self.talk.mouth {
            m.cancel();
        }
        if let Some(e) = &self.talk.ears {
            e.cancel();
        }
        self.talk.player.stop();
        self.sync_chrome(cx);
    }

    pub(super) fn close_camera_if_open(&mut self, cx: &mut Cx) {
        if self.cam.open {
            self.close_camera(cx);
        }
    }

    pub(super) fn set_listening(&mut self, cx: &mut Cx, on: bool) {
        if self.talk.listening == on {
            return;
        }
        self.talk.listening = on;
        self.talk.seg.reset();
        if on {
            self.ensure_voice(cx);
            self.ensure_ears();
            self.talk.status = self.listening_status();
            self.mic_on(cx);
            self.start_injection(cx);
        } else {
            self.mic_off(cx);
            if self.talk.in_utterance {
                if let Some(e) = &self.talk.ears {
                    e.end_utterance();
                    self.talk.finals_pending += 1;
                }
                self.talk.in_utterance = false;
            }
            self.talk.status = "Paused.".to_string();
            // A turn still being recognised keeps the model until its transcript is in.
            self.release_ears_if_idle();
        }
        self.refresh_talk(cx);
    }

    fn start_injection(&mut self, _cx: &mut Cx) {
        if self.talk.inject.script.is_empty() || self.talk.inject.started {
            return;
        }
        self.talk.inject.started = true;
        let script = self.talk.inject.script.clone();
        if let Some(m) = &self.talk.mouth {
            for (i, (lang, text)) in script.iter().enumerate() {
                let id = m.say(text, *lang, false);
                self.talk.say.insert(id, SayFor::Inject(i));
            }
        }
        log!("translator: injecting {} synthetic utterance(s) as microphone audio", script.len());
    }

    // --------------------------------------------------------------- events

    pub(super) fn on_talk_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::AudioDevices(devices) = event {
            self.on_audio_devices(cx, devices);
        }
        if let Event::PermissionResult(r) = event {
            if r.permission == Permission::AudioInput {
                self.talk.mic_permission = Some(r.status);
                if !matches!(r.status, PermissionStatus::Granted) && (self.talk.open || self.talk.dictating) {
                    self.talk.status = "The microphone needs permission: allow it in the phone's app settings.".to_string();
                    self.refresh_talk(cx);
                }
            }
        }
        let tick = self.talk.tick.as_ref().is_some_and(|t| t.is_event(event).is_some());
        let signalled = matches!(event, Event::Signal);
        if tick || signalled {
            self.pump_voice(cx);
        }
    }

    fn on_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        // The development injection hook (no microphone, hidden machines) never opens a
        // loudspeaker: speech is still rendered and timed, only not played.
        if self.talk.inject.script.is_empty() {
            cx.use_audio_outputs(&devices.default_output());
        } else {
            cx.use_audio_outputs(&[]);
        }
        // The default input, followed as it changes: the next list names the new one
        // and the stream moves to it (on Linux the "default" pcm follows the sound server).
        let input = devices.default_input().into_iter().next();
        let desc = input.and_then(|id| devices.descs.iter().find(|d| d.device_id == id));
        self.talk.input_device = input;
        self.talk.input_name = desc.map(|d| d.name.clone()).unwrap_or_default();
        self.talk.input_failed = desc.is_some_and(|d| d.has_failed);
        self.talk.devices_known = true;
        if self.talk.listening || self.talk.dictating {
            self.mic_on(cx);
        }
    }

    /// Everything that happens between two frames of the Talk screen.
    fn pump_voice(&mut self, cx: &mut Cx) {
        if self.talk.ears.is_none() && self.talk.mouth.is_none() && self.talk.engine.is_none() {
            return;
        }
        self.talk.player.expire();
        // Ears.
        let events = self.talk.ears.as_ref().map(|e| e.poll()).unwrap_or_default();
        let mut changed = !events.is_empty();
        for event in events {
            self.on_ear_event(cx, event);
        }
        if changed {
            self.release_ears_if_idle();
        }
        // Mouth.
        let events = self.talk.mouth.as_ref().map(|m| m.poll()).unwrap_or_default();
        changed |= !events.is_empty();
        for event in events {
            self.on_mouth_event(cx, event);
        }
        // Translation results.
        let updates = self.talk.engine.as_ref().map(|e| e.poll()).unwrap_or_default();
        changed |= !updates.is_empty();
        for update in updates {
            self.on_talk_update(cx, update);
        }
        // Audio in.
        let speaking_back = self.talk.player.is_playing();
        let mut audio = self.talk.capture.take();
        if !audio.is_empty() && !self.talk.mic_heard && self.talk.mic.open {
            self.talk.mic_heard = true;
            log!("translator: microphone delivers audio after {:.0} ms ({})", (Cx::time_now() - self.talk.mic_opened_at) * 1000.0, self.talk.input_name);
            self.check_mic(cx);
        }
        self.feed_injection(&mut audio);
        if !audio.is_empty() {
            self.talk.heard_at = Cx::time_now();
            self.talk.level = (audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32).sqrt();
            if self.talk.dictating {
                if self.talk.level > 0.012 {
                    self.talk.voiced_chunks += 1;
                }
                if self.talk.voiced_chunks == 0 {
                    // Still waiting for the first words: keep 0.4 s of room, send nothing.
                    self.talk.dictation_preroll.extend_from_slice(&audio);
                    let keep = crate::voice::SAMPLE_RATE as usize * 2 / 5;
                    if self.talk.dictation_preroll.len() > keep {
                        let drop = self.talk.dictation_preroll.len() - keep;
                        self.talk.dictation_preroll.drain(..drop);
                    }
                } else {
                    let mut chunk = std::mem::take(&mut self.talk.dictation_preroll);
                    chunk.extend_from_slice(&audio);
                    self.talk.dict_samples += chunk.len();
                    if let Some(e) = &self.talk.ears {
                        e.feed(chunk);
                    }
                }
            } else if self.talk.listening {
                if speaking_back {
                    // Half duplex: the phone does not listen to its own voice.
                    self.talk.seg.reset();
                } else {
                    for cut in self.talk.seg.push(&audio) {
                        match cut {
                            Cut::Begin => {
                                self.talk.in_utterance = true;
                                self.talk.speculating = false;
                                self.talk.utt_audio.clear();
                                self.talk.partial.clear();
                            }
                            Cut::Audio(a) => {
                                self.talk.utt_audio.extend_from_slice(&a);
                                if let Some(e) = &self.talk.ears {
                                    if self.talk.speculating {
                                        // The speaker went on: forget the early reading, read all again.
                                        self.talk.speculating = false;
                                        self.talk.finals_pending = self.talk.finals_pending.saturating_sub(1);
                                        e.cancel();
                                        e.feed(self.talk.utt_audio.clone());
                                        self.talk.ends.pop_back();
                                        log!("translator: speculative reading discarded (speech resumed)");
                                    } else {
                                        e.feed(a);
                                    }
                                }
                            }
                            Cut::Maybe => {
                                if self.talk.mode == TalkMode::Conversation && !self.talk.speculating {
                                    self.talk.speculating = true;
                                    // The silence began half a pause ago: turn latency counts from there.
                                    let began = std::time::Instant::now() - std::time::Duration::from_millis((PAUSE_SECS * 500.0) as u64);
                                    self.talk.ends.push_back(began);
                                    if let Some(e) = &self.talk.ears {
                                        e.end_utterance();
                                        self.talk.finals_pending += 1;
                                    }
                                }
                            }
                            Cut::End => {
                                self.talk.in_utterance = false;
                                if !self.talk.speculating {
                                    self.talk.ends.push_back(std::time::Instant::now() - std::time::Duration::from_millis((PAUSE_SECS * 1000.0) as u64));
                                }
                                while self.talk.ends.len() > 8 {
                                    self.talk.ends.pop_front();
                                }
                                if !self.talk.speculating {
                                    self.talk.heard_secs = self.talk.utt_audio.len() as f32 / crate::voice::SAMPLE_RATE as f32;
                                    if let Some(e) = &self.talk.ears {
                                        e.end_utterance();
                                        self.talk.finals_pending += 1;
                                    }
                                }
                                self.talk.speculating = false;
                            }
                        }
                    }
                }
            }
            changed = true;
        }
        // Live: a partial that stood still gets a provisional translation.
        if self.talk.listening && self.talk.mode == TalkMode::Live && !self.talk.partial.is_empty() {
            let now = Cx::time_now();
            if now - self.talk.partial_since > PROVISIONAL_AFTER
                && self.talk.job.is_none()
                && self.talk.pending.is_empty()
                && self.talk.provisional_for != self.talk.partial
            {
                self.submit_provisional();
            }
        }
        if changed && self.talk.open {
            self.refresh_talk(cx);
        }
        self.check_mic(cx);
        self.sync_mics(cx);
    }

    /// Every microphone button: bright red while the microphone really captures
    /// (audio arrived in the last moments), gray otherwise.
    pub(super) fn sync_mics(&mut self, cx: &mut Cx) {
        let wanted = self.talk.listening || self.talk.dictating;
        let lit = wanted && Cx::time_now() - self.talk.heard_at < 0.4;
        if self.talk.mic_lit == Some(lit) {
            return;
        }
        self.talk.mic_lit = Some(lit);
        for path in [ids!(source_card.head.dictate), ids!(target_card.head.dictate_other), ids!(talk.talk_ctrl.talk_mic)] {
            let mut mic = self.view.widget(cx, path);
            if lit {
                script_apply_eval!(cx, mic, {
                    draw_icon +: {color: #ffffff}
                    draw_bg +: {color: mod.translator.mic_live color_hover: mod.translator.mic_live color_focus: mod.translator.mic_live color_down: mod.translator.mic_live}
                    draw_text +: {color: #ffffff color_hover: #ffffff color_focus: #ffffff color_down: #ffffff}
                });
            } else {
                script_apply_eval!(cx, mic, {
                    draw_icon +: {color: mod.translator.ink_dim}
                    draw_bg +: {color: mod.translator.surface_2 color_hover: mod.translator.surface_2 color_focus: mod.translator.surface_2 color_down: mod.translator.line}
                    draw_text +: {color: mod.translator.ink_dim color_hover: mod.translator.ink_dim color_focus: mod.translator.ink_dim color_down: mod.translator.ink}
                });
            }
        }
        self.view.redraw(cx);
    }

    fn feed_injection(&mut self, audio: &mut Vec<f32>) {
        let inj = &mut self.talk.inject;
        if inj.script.is_empty() || !self.talk.listening {
            return;
        }
        let now = std::time::Instant::now();
        let elapsed = inj.last_tick.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
        inj.last_tick = Some(now);
        let mut want = (elapsed.min(0.5) * crate::voice::SAMPLE_RATE as f64) as usize;
        // Not while the app is speaking, like a real room.
        if self.talk.player.is_playing() {
            return;
        }
        while want > 0 {
            if inj.pos < inj.current.len() {
                let n = want.min(inj.current.len() - inj.pos);
                audio.extend_from_slice(&inj.current[inj.pos..inj.pos + n]);
                inj.pos += n;
                want -= n;
                if inj.pos >= inj.current.len() {
                    inj.quiet_left = crate::voice::SAMPLE_RATE as usize * 2;
                }
            } else if inj.quiet_left > 0 {
                let n = want.min(inj.quiet_left);
                audio.extend(std::iter::repeat(0.0).take(n));
                inj.quiet_left -= n;
                want -= n;
            } else if let Some(clip) = inj.clips.remove(&inj.next) {
                inj.next += 1;
                inj.current = clip;
                inj.pos = 0;
            } else {
                // The next clip is still being made: room silence.
                audio.extend(std::iter::repeat(0.0).take(want));
                want = 0;
            }
        }
    }

    fn on_ear_event(&mut self, cx: &mut Cx, event: EarEvent) {
        match event {
            EarEvent::Ready(note) => {
                self.talk.ears_note = format!("hearing: {}", short_note(&note));
                log!("translator: ears ready: {}", note);
            }
            EarEvent::Failed(why) => {
                self.talk.ears_note = String::new();
                self.talk.status = format!("Speech recognition is not available: {why}");
                log!("translator: ears failed: {}", why);
                if !self.talk.open {
                    self.set_status(cx, &self.talk.status.clone(), true);
                }
                // The next listen tries again.
                self.talk.ears = None;
            }
            EarEvent::Error(why) => {
                self.talk.finals_pending = self.talk.finals_pending.saturating_sub(1);
                self.talk.status = format!("Could not recognise that: {why}");
            }
            EarEvent::Partial(text) => {
                if text != self.talk.partial {
                    self.talk.partial = text.clone();
                    self.talk.partial_since = Cx::time_now();
                    if self.talk.dictating && self.talk.voiced_chunks >= 2 {
                        self.source_text = text.clone();
                        self.view.text_input(cx, ids!(source_card.input)).set_text(cx, &text);
                        self.sync_chrome(cx);
                    }
                }
            }
            EarEvent::Final { text, lang } => {
                self.talk.finals_pending = self.talk.finals_pending.saturating_sub(1);
                self.talk.partial.clear();
                self.talk.provisional.clear();
                self.talk.provisional_for.clear();
                let text = text.trim().to_string();
                if text.chars().count() < 2 {
                    return;
                }
                let text = collapse_repeats(&text);
                if self.talk.dictating {
                    log!("translator: dictated {} chars ({} voiced chunks)", text.chars().count(), self.talk.voiced_chunks);
                    self.talk.dictating = false;
                    self.mic_off(cx);
                    self.source_text = text.clone();
                    self.pick = None;
                    self.view.text_input(cx, ids!(source_card.input)).set_text(cx, &text);
                    self.sync_chrome(cx);
                    self.speak_after_translate = self.talk.auto_speak;
                    self.start_translate(cx);
                    return;
                }
                let from = lang.or_else(|| detect(&text)).unwrap_or(Lang::En);
                log!("translator: heard {} chars of {}", text.chars().count(), from.code());
                let t_end = self.talk.ends.pop_front();
                self.talk.bubbles.push(Bubble { from, source: text, target: String::new(), time: clock(cx), t_end, t_heard: Some(std::time::Instant::now()), t_translated: None, t_audio: None });
                if self.talk.bubbles.len() > MAX_BUBBLES {
                    self.talk.bubbles.remove(0);
                    // The queue indexes the list: shift it with the removal.
                    self.talk.pending = self.talk.pending.iter().filter_map(|i| i.checked_sub(1)).collect();
                    if let Some((id, JobKind::Final(i))) = self.talk.job {
                        self.talk.job = i.checked_sub(1).map(|i| (id, JobKind::Final(i)));
                    }
                }
                self.talk.pending.push_back(self.talk.bubbles.len() - 1);
                self.next_talk_job();
            }
        }
    }

    fn on_mouth_event(&mut self, cx: &mut Cx, event: MouthEvent) {
        match event {
            MouthEvent::Ready(note, voices) => {
                self.talk.mouth_note = format!("speaking: {}", short_note(&note));
                self.talk.voices = voices;
                self.sync_voice_buttons(cx);
                log!("translator: mouth ready: {}", note);
            }
            MouthEvent::Failed(why) => {
                self.talk.speak_requested = false;
                self.talk.mouth_note = String::new();
                self.talk.status = format!("Speech is not available: {why}");
                log!("translator: mouth failed: {}", why);
                // Said where it was asked for (the Translate screen too), and the
                // next press tries again (a hub found since, a network back).
                if !self.talk.open {
                    self.set_status(cx, &self.talk.status.clone(), true);
                }
                self.talk.mouth = None;
            }
            MouthEvent::Error { id, message } => {
                self.talk.say.remove(&id);
                self.talk.speak_requested = false;
                self.talk.status = format!("Could not speak that: {message}");
                if !self.talk.open {
                    self.set_status(cx, &self.talk.status.clone(), true);
                }
            }
            MouthEvent::Chunk { id, samples, rate } => {
                if let Some(SayFor::Play(bubble)) = self.talk.say.get(&id).copied() {
                    let since = bubble.and_then(|i| self.talk.bubbles.get(i)).and_then(|b| b.t_heard).map_or(0, |t| t.elapsed().as_millis());
                    log!("translator: audio chunk of {} samples {} ms after the text was heard", samples.len(), since);
                    self.talk.streamed.insert(id);
                    self.first_sound(bubble);
                    self.talk.player.enqueue(samples, rate);
                }
            }
            MouthEvent::Audio { id, samples, rate } => match self.talk.say.remove(&id) {
                Some(SayFor::Inject(i)) => {
                    let to16 = makepad_ai_hub::resample::resample_channel(&samples, rate, crate::voice::SAMPLE_RATE);
                    log!("translator: injected clip {} ready ({} samples at 16 kHz)", i, to16.len());
                    self.talk.inject.clips.insert(i, to16);
                }
                Some(SayFor::Play(bubble)) => {
                    self.talk.speak_requested = false;
                    // Already played chunk by chunk: this is the whole take again.
                    if !self.talk.streamed.remove(&id) {
                        self.first_sound(bubble);
                        self.talk.player.enqueue(samples, rate);
                    }
                }
                None => {}
            },
        }
        let _ = cx;
    }

    /// The first sound of a turn: the latency log line.
    fn first_sound(&mut self, bubble: Option<usize>) {
        if let Some(b) = bubble.and_then(|i| self.talk.bubbles.get_mut(i)) {
            if b.t_audio.is_none() {
                let now = std::time::Instant::now();
                b.t_audio = Some(now);
                if let Some(heard) = b.t_heard {
                    let ms = |a: std::time::Instant, b: std::time::Instant| b.duration_since(a).as_millis();
                    let recognise = b.t_end.map_or(String::from("?"), |e| ms(e, heard).to_string());
                    let total = b.t_end.map_or(String::from("?"), |e| ms(e, now).to_string());
                    log!(
                        "translator: turn latency: recognise {} ms, translate to the first sound {} ms, speech starts {} ms after the pause was detected (counted from the start of the silence)",
                        recognise, ms(heard, now), total
                    );
                }
            }
        }
    }

    // ---------------------------------------------------------- translating

    /// Start the next waiting utterance's translation, if the engine is free.
    fn next_talk_job(&mut self) {
        if self.talk.job.is_some() {
            return;
        }
        let Some(&index) = self.talk.pending.front() else { return };
        let Some(b) = self.talk.bubbles.get(index) else {
            self.talk.pending.pop_front();
            return self.next_talk_job();
        };
        let (from, source) = (b.from, b.source.clone());
        self.talk.next_job += 1;
        let id = self.talk.next_job;
        self.talk.job = Some((id, JobKind::Final(index)));
        self.talk.spoken_upto = 0;
        let max_tokens = Some(quick_max_tokens(&source));
        let job = Job { id, system: if self.talk.mode == TalkMode::Live { live_system(&self.style()) } else { quick_system(from, from.other(), &self.style()) }, text: source, settings: self.hub_settings(), max_tokens, image: None };
        if let Some(e) = &self.talk.engine {
            e.submit(job);
        }
    }

    fn submit_provisional(&mut self) {
        let text = self.talk.partial.clone();
        let Some(from) = detect(&text) else { return };
        self.talk.next_job += 1;
        let id = self.talk.next_job;
        self.talk.job = Some((id, JobKind::Provisional));
        self.talk.provisional_for = text.clone();
        let max_tokens = Some(quick_max_tokens(&text));
        let job = Job { id, system: if self.talk.mode == TalkMode::Live { live_system(&self.style()) } else { quick_system(from, from.other(), &self.style()) }, text, settings: self.hub_settings(), max_tokens, image: None };
        if let Some(e) = &self.talk.engine {
            e.submit(job);
        }
    }

    fn on_talk_update(&mut self, cx: &mut Cx, update: Update) {
        if let Update::HubMoved { addr } = update {
            self.hub_moved(cx, addr);
            return;
        }
        let Some((job_id, kind)) = self.talk.job else { return };
        match update {
            Update::Done { id, text } if id == job_id => {
                self.talk.job = None;
                let text = clean(&text);
                match kind {
                    JobKind::Provisional => self.talk.provisional = text,
                    JobKind::Final(index) => {
                        self.talk.pending.pop_front();
                        if let Some(b) = self.talk.bubbles.get_mut(index) {
                            b.t_translated = Some(std::time::Instant::now());
                            b.target = text.clone();
                            let (from, source) = (b.from, b.source.clone());
                            self.saved.remember(from.code(), from.other().code(), &source, &text, "");
                            self.persist(cx);
                            self.rebuild_rows(cx);
                            if self.talk.mode == TalkMode::Conversation && self.talk.auto_speak {
                                // Whatever the clauses spoken so far did not cover.
                                let rest = text.get(self.talk.spoken_upto..).unwrap_or("").trim().to_string();
                                if !rest.is_empty() {
                                    self.speak_for(&rest, from.other(), Some(index));
                                }
                            }
                        }
                    }
                }
                self.next_talk_job();
            }
            Update::Partial { id, text } if id == job_id && looping(&clean(&text)).is_some() => {
                // The model repeats itself: stop it and keep one copy.
                let kept = looping(&clean(&text)).unwrap_or_default();
                log!("translator: translation looped ({} chars), stopped at one copy", text.chars().count());
                if let Some(e) = &self.talk.engine {
                    e.cancel(id + 1);
                }
                self.on_talk_update(cx, Update::Done { id, text: kept });
            }
            Update::Partial { id, text } if id == job_id => {
                // The translation as it grows: shown at once, and spoken a clause at a time.
                if let Some(b) = self.talk.bubbles.iter().rev().find(|b| b.t_heard.is_some()) {
                    log!("translator: translation partial {:?} of {} chars {} ms after the text was heard", text.chars().take(90).collect::<String>(), text.chars().count(), b.t_heard.map_or(0, |t| t.elapsed().as_millis()));
                }
                if let JobKind::Final(index) = kind {
                    let text = clean(&text);
                    if !text.is_empty() {
                        let speak = self.talk.mode == TalkMode::Conversation && self.talk.auto_speak;
                        let to = self.talk.bubbles.get(index).map(|b| b.from.other());
                        if let Some(b) = self.talk.bubbles.get_mut(index) {
                            b.target = text.clone();
                        }
                        if let (true, Some(to)) = (speak, to) {
                            while let Some(end) = clause_end(&text, self.talk.spoken_upto, to) {
                                let clause = text[self.talk.spoken_upto..end].trim().to_string();
                                self.talk.spoken_upto = end;
                                if !clause.is_empty() {
                                    self.speak_for(&clause, to, Some(index));
                                }
                            }
                        }
                        self.refresh_talk(cx);
                    }
                }
            }
            Update::Failed { id, error } if id == job_id => {
                self.talk.job = None;
                if let JobKind::Final(_) = kind {
                    self.talk.pending.pop_front();
                }
                self.talk.status = error;
                self.next_talk_job();
            }
            _ => {}
        }
    }

    /// Say `text` aloud in `lang` (the Talk screen, the speak button).
    pub(super) fn speak(&mut self, text: &str, lang: Lang) {
        self.speak_for(text, lang, None);
    }

    fn speak_for(&mut self, text: &str, lang: Lang, bubble: Option<usize>) {
        // A voice that failed is set up again (the hub may be reachable now).
        if self.talk.mouth.is_none() && self.talk.audio_installed {
            crate::ondevice::speech_in_process(self.on_device());
            self.talk.mouth = Some(Box::new(HubMouth::start(Self::voice_wake(), self.chosen_voices())));
        }
        if let Some(m) = &self.talk.mouth {
            let id = m.say(text, lang, true);
            self.talk.say.insert(id, SayFor::Play(bubble));
            let since = bubble.and_then(|i| self.talk.bubbles.get(i)).and_then(|b| b.t_heard).map_or(0, |t| t.elapsed().as_millis());
            log!("translator: clause sent to the voice {} ms after the text was heard ({} chars)", since, text.chars().count());
        }
    }

    // --------------------------------------------------------------- voices

    pub(super) fn chosen_voices(&self) -> Voices {
        let d = Voices::default();
        Voices {
            en: self.saved.voice_en.clone().filter(|v| !v.is_empty()).unwrap_or(d.en),
            zh: self.saved.voice_zh.clone().filter(|v| !v.is_empty()).unwrap_or(d.zh),
        }
    }

    /// The voices offered for `lang`: those the engine tags with it, else all.
    fn voices_for(&self, lang: Lang) -> Vec<VoiceInfo> {
        let tagged: Vec<VoiceInfo> = self.talk.voices.iter().filter(|v| v.lang == Some(lang)).cloned().collect();
        if tagged.is_empty() && lang == Lang::Zh && !self.talk.voices.is_empty() {
            // A hub that lists only its English catalogue still serves the Mandarin
            // packs it has installed (zf_001.. are the v1.1-zh voices). Hubs that list
            // Chinese voices are used as listed.
            return (1..=8).map(|i| VoiceInfo { id: format!("zf_00{i}"), name: format!("zf_00{i}"), lang: Some(Lang::Zh) }).collect();
        }
        tagged
    }

    pub(super) fn sync_voice_buttons(&mut self, cx: &mut Cx) {
        let chosen = self.chosen_voices();
        for (lang, id, path) in [(Lang::En, chosen.en, ids!(settings.voice_en_row.name)), (Lang::Zh, chosen.zh, ids!(settings.voice_zh_row.name))] {
            let name = self.voices_for(lang).iter().find(|v| v.id == id).map(|v| v.name.clone()).unwrap_or_else(|| id.clone());
            let text = format!("{}: {}", lang.name(), name);
            self.view.button(cx, path).set_text(cx, &text);
        }
    }

    /// ‹ and ›: the previous or next voice of that language; tapping its name says
    /// the sample again. Each change says a short sample.
    fn step_voice(&mut self, cx: &mut Cx, lang: Lang, delta: isize) {
        let list = self.voices_for(lang);
        if list.is_empty() {
            self.set_pair_status(cx, "The hub has not listed its voices yet (open Talk or speak once).");
            return;
        }
        let chosen = self.chosen_voices();
        let current = if lang == Lang::En { chosen.en } else { chosen.zh };
        let n = list.len() as isize;
        let at = match list.iter().position(|v| v.id == current) {
            Some(i) => ((i as isize + delta).rem_euclid(n)) as usize,
            None => 0,
        };
        let pick = list[at].id.clone();
        if lang == Lang::En {
            self.saved.voice_en = Some(pick);
        } else {
            self.saved.voice_zh = Some(pick);
        }
        self.persist(cx);
        if let Some(m) = &self.talk.mouth {
            m.set_voices(self.chosen_voices());
        }
        self.sync_voice_buttons(cx);
        let sample = if lang == Lang::En { "Hello, how can I help you?" } else { "你好，有什么可以帮你的吗？" };
        self.speak(sample, lang);
    }

    // ------------------------------------------------------------- dictation

    /// A box's mic tapped on the main screen: talk, tap again, translate.
    pub(super) fn begin_dictation(&mut self, cx: &mut Cx) {
        self.ensure_voice(cx);
        self.ensure_ears();
        self.talk.dictating = true;
        self.talk.voiced_chunks = 0;
        self.talk.dict_samples = 0;
        self.talk.dictation_preroll.clear();
        self.talk.partial.clear();
        self.talk.seg.reset();
        self.mic_on(cx);
        self.set_status(cx, "Recording… tap the microphone to translate.", false);
        if !self.talk.inject.script.is_empty() {
            self.talk.listening = true;
            self.start_injection(cx);
        }
    }

    pub(super) fn end_dictation(&mut self, cx: &mut Cx) {
        if !self.talk.dictating {
            return;
        }
        // Whatever was said is read now; the turn ends when the final arrives.
        self.talk.listening = false;
        self.mic_off(cx);
        if self.talk.voiced_chunks < 4 {
            self.talk.dictating = false;
            if let Some(e) = &self.talk.ears {
                e.cancel();
            }
            self.talk.partial.clear();
            self.set_status(cx, "Tap the microphone, then talk.", false);
            return;
        }
        if let Some(e) = &self.talk.ears {
            self.talk.heard_secs = self.talk.dict_samples as f32 / crate::voice::SAMPLE_RATE as f32;
            e.end_utterance();
            self.talk.finals_pending += 1;
        }
        self.set_status(cx, "Recognising…", false);
    }

    // ----------------------------------------------------------- the screen

    /// The live screen: English only, oldest first, the newest line apart and in
    /// focus at the bottom. English speech is shown as it is.
    fn refresh_live_main(&mut self, cx: &mut Cx) {
        let mut lines: Vec<String> = self
            .talk
            .bubbles
            .iter()
            .map(|b| if b.from == Lang::En { b.source.clone() } else if b.target.is_empty() { "…".to_string() } else { b.target.clone() })
            .collect();
        if self.talk.listening && !self.talk.partial.is_empty() {
            let prov = if self.talk.provisional_for == self.talk.partial { self.talk.provisional.clone() } else { String::new() };
            let now = if detect(&self.talk.partial) == Some(Lang::Zh) { prov } else { self.talk.partial.clone() };
            lines.push(if now.is_empty() { "…".to_string() } else { now });
        }
        let newest = lines.pop().unwrap_or_default();
        self.view.label(cx, ids!(talk.live_scroll.live_old)).set_text(cx, &lines.join("\n\n"));
        let shown = if newest.is_empty() && self.talk.listening && self.talk.mic_error.is_none() { "Listening…".to_string() } else { newest };
        self.view.label(cx, ids!(talk.live_scroll.live_new)).set_text(cx, &shown);
        self.view.redraw(cx);
    }

    pub(super) fn refresh_talk(&mut self, cx: &mut Cx) {
        if !self.talk.open {
            return;
        }
        let status = if self.talk.status.is_empty() { String::new() } else { self.talk.status.clone() };
        let notes = [self.talk.ears_note.as_str(), self.talk.mouth_note.as_str()]
            .iter()
            .filter(|n| !n.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("  ·  ");
        let line = if notes.is_empty() { status } else { format!("{status}\n{notes}") };
        self.view.label(cx, ids!(talk.talk_status)).set_text(cx, &line);
        // Hints stay off; the line speaks only for a problem or a confirmation.
        let low = self.talk.status.to_lowercase();
        let speaks = ["permission", "not available", "fail", "error", "unreachable", "copied", "could not", "no microphone", "no sound"].iter().any(|w| low.contains(w));
        self.view.label(cx, ids!(talk.talk_status)).set_visible(cx, speaks);

        // Mic state.
        let mic = self.view.button(cx, ids!(talk.talk_ctrl.talk_mic));
        mic.set_text(cx, "");
        let mut mic_w = self.view.widget(cx, ids!(talk.talk_ctrl.talk_mic));
        // A clicked button is in its focus state: every state gets the colour, or the one the
        // button is in (focus after a click) draws the theme's grey.
        // Its colour follows the capture state (sync_mics).
        let _ = &mut mic_w;
        self.view.label(cx, ids!(talk.talk_ctrl.mic_hint)).set_text(cx, if self.talk.listening && self.talk.mic_error.is_some() { "Tap to stop" } else if self.talk.listening { "Listening — tap to pause" } else { "Tap to listen" });
        // Level meter.
        // Room noise sits near 0.005, speech near 0.05..0.2.
        let width = (self.talk.level * 10.0).clamp(0.0, 1.0) as f64 * 82.0 + 6.0;
        let mut meter = self.view.widget(cx, ids!(talk.talk_ctrl.level_box.level));
        script_apply_eval!(cx, meter, { width: #(width) });
        let listening = self.talk.mic.open && self.talk.mic_error.is_none();
        self.view.label(cx, ids!(talk.talk_bar.listen_title)).set_text(cx, if listening { "Listening" } else if self.talk.mic_error.is_some() { "Microphone unavailable" } else { "Not listening" });
        // The recording dot shows only while the microphone stream is open and working.
        self.view.widget(cx, ids!(talk.talk_bar.listen_dot)).set_visible(cx, listening);
        self.refresh_live_main(cx);
        self.view.redraw(cx);
    }

    pub(super) fn handle_talk_actions(&mut self, cx: &mut Cx, actions: &Actions) {

        if self.view.button(cx, ids!(talk.talk_ctrl.talk_mic)).clicked(actions) {
            let on = !self.talk.listening;
            self.set_listening(cx, on);
        }
        if self.view.button(cx, ids!(talk.talk_bar.talk_copy)).clicked(actions) {
            let text = self
                .talk
                .bubbles
                .iter()
                .map(|b| format!("[{}] {}\n{}", b.time, b.source, b.target))
                .collect::<Vec<_>>()
                .join("\n\n");
            cx.copy_to_clipboard(&text);
            self.talk.status = "Transcript copied.".to_string();
            self.refresh_talk(cx);
        }
        if self.view.button(cx, ids!(talk.talk_bar.talk_clear)).clicked(actions) {
            self.talk.bubbles.clear();
            self.talk.pending.clear();
            self.refresh_talk(cx);
        }
        for (lang, row) in [(Lang::En, live_id!(voice_en_row)), (Lang::Zh, live_id!(voice_zh_row))] {
            for (button, delta) in [(live_id!(prev), -1isize), (live_id!(name), 0), (live_id!(next), 1)] {
                if self.view.button(cx, &[live_id!(settings), row, button]).clicked(actions) {
                    self.ensure_voice(cx);
                    self.step_voice(cx, lang, delta);
                }
            }
        }
        // Either box's mic is a switch: tap to record (the mic opens; the language is detected
        // from the speech, the direction follows it, the translation is read aloud), tap again
        // to stop (the mic closes).
        for path in [ids!(source_card.head.dictate), ids!(target_card.head.dictate_other)] {
            if self.view.button(cx, path).clicked(actions) {
                if self.talk.dictating {
                    self.end_dictation(cx);
                } else {
                    self.begin_dictation(cx);
                }
            }
        }
        // The output card's big play button: speak the translation, or stop it.
        if self.view.button(cx, ids!(target_card.play_row.play)).clicked(actions)
            || self.view.button(cx, ids!(target_card.play_row.stop_play)).clicked(actions)
        {
            let text = self.view.text_input(cx, ids!(target_card.output)).text();
            let to = self.effective_from().other();
            self.toggle_speaking(cx, &text, to);
        }
    }
}

/// A reading caught in a loop repeats one sentence over and over: keep each
/// sentence once when it follows itself.
pub(super) fn collapse_repeats(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let end = rest.char_indices().find(|(_, c)| matches!(c, '.' | '?' | '!' | '。' | '？' | '！')).map_or(rest.len(), |(i, c)| i + c.len_utf8());
        let sentence = rest[..end].trim();
        if !sentence.is_empty() && out.last().map_or(true, |last| !last.eq_ignore_ascii_case(sentence)) {
            out.push(sentence);
        }
        rest = rest[end..].trim_start();
    }
    // A block of sentences read again and again: keep the block once.
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    if let Some(period) = (1..=out.len() / 2).find(|&p| (p..out.len()).all(|i| same(out[i], out[i - p]))) {
        out.truncate(period);
    }
    let joiner = if text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) { "" } else { " " };
    collapse_word_loop(&out.join(joiner))
}

/// "Hello Hello Hello Hello": the same words three or more times over, with no
/// sentence marks between them -- kept once.
fn collapse_word_loop(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let n = words.len();
    let same = |a: &str, b: &str| a.trim_matches(|c: char| !c.is_alphanumeric()).eq_ignore_ascii_case(b.trim_matches(|c: char| !c.is_alphanumeric()));
    match (1..=n / 3).find(|&p| (p..n).all(|i| same(words[i], words[i - p]))) {
        Some(p) => words[..p].join(" "),
        None => text.to_string(),
    }
}

/// A reply caught in a loop (the same block over and over): the one copy to keep.
pub(super) fn looping(text: &str) -> Option<String> {
    let kept = collapse_repeats(text);
    (text.chars().count() > 24 && kept.chars().count() * 2 < text.chars().count()).then_some(kept)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_looped_reading_collapses_to_one_sentence() {
        let looped = "Good evening. Could you recommend a good restaurant nearby? Good evening. Could you recommend a good restaurant nearby? Good evening.";
        assert_eq!(super::collapse_repeats(looped), "Good evening. Could you recommend a good restaurant nearby?");
        assert_eq!(super::collapse_repeats("你好。你好。谢谢。"), "你好。谢谢。");
        assert_eq!(super::collapse_repeats("One. Two. One."), "One. Two. One.");
        assert_eq!(super::collapse_repeats("Hello Hello Hello Hello"), "Hello");
        assert_eq!(super::collapse_repeats("no no"), "no no");
        assert_eq!(super::looping("机场的火车。机场的火车。机场的火车。机场的火车。机场的火车。").as_deref(), Some("机场的火车。"));
        assert_eq!(super::looping("The train leaves from platform three."), None);
    }

    use super::*;

    #[test]
    fn the_microphone_starts_closed_and_asks_permission_once() {
        let mut mic = MicState::default();
        assert!(!mic.open);
        assert!(mic.request_permission());
        assert!(!mic.request_permission());
    }

    #[test]
    fn nothing_opens_the_microphone_before_a_record_press() {
        let talk = Talk::default();
        assert!(!talk.mic.open && !talk.listening && !talk.dictating);
    }

    /// Listening with a microphone that gives nothing says why, and only then.
    #[test]
    fn a_microphone_that_gives_nothing_says_why() {
        let mut talk = Talk::default();
        talk.inject.script.clear();
        talk.listening = true;
        // No device list yet: not an error (it is on its way).
        assert_eq!(mic_problem(&talk, 0.0), None);
        talk.devices_known = true;
        assert!(mic_problem(&talk, 0.0).unwrap().starts_with("No microphone found"));
        talk.input_device = Some(AudioDeviceId::default());
        talk.input_failed = true;
        assert!(mic_problem(&talk, 0.0).unwrap().contains("could not be opened"));
        talk.input_failed = false;
        talk.mic.open = true;
        talk.mic_opened_at = 10.0;
        assert_eq!(mic_problem(&talk, 10.0 + MIC_SILENT_SECS - 0.5), None);
        assert!(mic_problem(&talk, 10.0 + MIC_SILENT_SECS + 0.5).unwrap().starts_with("No sound"));
        talk.mic_heard = true;
        assert_eq!(mic_problem(&talk, 100.0), None);
        talk.mic_heard = false;
        talk.listening = false;
        assert_eq!(mic_problem(&talk, 100.0), None);
    }

    #[test]
    fn clauses_end_at_marks_but_not_in_numbers_or_after_a_word() {
        let zh = "晚上好。能推荐一家附近的好餐厅吗？";
        let first = clause_end(zh, 0, Lang::Zh).unwrap();
        assert_eq!(&zh[..first], "晚上好。");
        let second = clause_end(zh, first, Lang::Zh).unwrap();
        assert_eq!(&zh[first..second], "能推荐一家附近的好餐厅吗？");
        assert_eq!(clause_end("能推荐一家附近的", 0, Lang::Zh), None);
        // a short English opening waits for the next mark
        let en = "Hi, could you tell me the time? It is 3.5 hours.";
        let e1 = clause_end(en, 0, Lang::En).unwrap();
        assert_eq!(&en[..e1], "Hi, could you tell me the time?");
        let e2 = clause_end(en, e1, Lang::En).unwrap();
        assert_eq!(&en[e1..e2].trim(), &"It is 3.5 hours.");
    }
}
