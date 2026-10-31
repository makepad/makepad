//! Voice: listening, speaking and the model-agnostic interfaces they sit behind.
//!
//! * [`Ears`] turns 16 kHz mono audio into text (partials while someone is
//!   still talking, a final per utterance, the spoken language). [`HubEars`]
//!   is Whisper on a hub node; the on-device Whisper is another `Ears`.
//! * [`Mouth`] turns text into audio. [`HubMouth`] is Kokoro on a hub node.
//! * [`Segmenter`] cuts a continuous microphone stream into utterances on
//!   pauses, with a maximum length.
//! * [`Player`] plays what a `Mouth` made.
//!
//! The screens only know these; switching from the hub to the phone's own
//! models is a new implementation, not a UI change.

use crate::lang::Lang;
use makepad_ai_hub::speech::{
    SayOptions, SpeechAudio, SpeechReach, SttConfig, SttEngine, SttEvent, SttSession, TtsConfig, TtsEngine, TtsEvent, TtsSession,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub const SAMPLE_RATE: u32 = 16_000;

// ------------------------------------------------------------------ ears

fn makepad_widgets_log(secs: f64) {
    makepad_widgets::log!("translator: whisper job took {:.0} ms", secs * 1000.0);
}

#[derive(Clone, Debug, PartialEq)]
pub enum EarEvent {
    /// Which engine and where; once.
    Ready(String),
    /// The running hypothesis of the utterance being spoken.
    Partial(String),
    /// A finished utterance and the language it was spoken in.
    Final { text: String, lang: Option<Lang> },
    /// The engine is not there (a reason a person can read). Nothing else follows.
    Failed(String),
    /// One utterance failed; listening goes on.
    Error(String),
}

pub trait Ears: Send {
    fn feed(&self, samples_16k: Vec<f32>);
    fn end_utterance(&self);
    /// Drop what has been fed and not finished.
    fn cancel(&self);
    fn poll(&self) -> Vec<EarEvent>;
}

pub struct HubEars {
    session: SttSession,
}

/// How the hub's Whisper is used: a conversation turn needs only the final
/// reading (a partial in flight would delay it); live captions want partials.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Listening {
    FinalOnly,
    WithPartials,
}

impl HubEars {
    /// Whisper on a hub node, strictly (no silent fallback to the OS recognizer),
    /// language detected from the audio.
    pub fn start(wake: Arc<dyn Fn() + Send + Sync>, listening: Listening) -> HubEars {
        let config = SttConfig {
            engine: SttEngine::Whisper,
            reach: SpeechReach::Lan,
            language: "auto".to_string(),
            partial_results: listening == Listening::WithPartials,
            partial_interval_ms: 1200,
            timestamps: false,
            // A turn is at most 12 s of speech (MAX_UTTERANCE_SECS): a cap on the
            // reading stops a decoder caught in a repetition loop (seen on the
            // phone: one 4 s sentence read back 19 times).
            max_tokens: 160,
            single_segment: true,
            wake: Some(wake),
            ..SttConfig::default()
        };
        HubEars { session: SttSession::start(config) }
    }
}

impl Ears for HubEars {
    fn feed(&self, samples_16k: Vec<f32>) {
        self.session.feed(samples_16k);
    }

    fn end_utterance(&self) {
        self.session.end_utterance();
    }

    fn cancel(&self) {
        self.session.cancel();
    }

    fn poll(&self) -> Vec<EarEvent> {
        let mut out = Vec::new();
        for event in self.session.poll() {
            match event {
                SttEvent::Ready(info) => out.push(EarEvent::Ready(match info.remote {
                    Some(node) => format!("{} on {node}", info.engine),
                    None => info.engine,
                })),
                SttEvent::Partial(text) => out.push(EarEvent::Partial(text)),
                SttEvent::Final { transcript, secs, .. } => {
                    makepad_widgets_log(secs);
                    let text: String = transcript.segments.iter().map(|s| s.text.trim()).collect::<Vec<_>>().join(" ");
                    let lang = transcript.language.as_deref().and_then(lang_of_code);
                    out.push(EarEvent::Final { text, lang });
                }
                SttEvent::Failed(why) => out.push(EarEvent::Failed(why)),
                SttEvent::Error { message, .. } => out.push(EarEvent::Error(message)),
                SttEvent::Loading { .. } | SttEvent::Level(_) | SttEvent::ListenEnded => {}
            }
        }
        out
    }
}

/// `zh`, `zh-CN`, `yue` -> Chinese; `en`, `en-US` -> English; anything else
/// (Whisper knows 99 languages) is `None`, and the text decides.
pub fn lang_of_code(code: &str) -> Option<Lang> {
    match code.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase().as_str() {
        "zh" | "yue" => Some(Lang::Zh),
        "en" => Some(Lang::En),
        _ => None,
    }
}

// ----------------------------------------------------------------- mouth

#[derive(Clone, Debug)]
pub enum MouthEvent {
    /// Engine note, and the voices it offers as (id, name, language code).
    Ready(String, Vec<VoiceInfo>),
    Audio { id: u64, samples: Vec<f32>, rate: u32 },
    /// One clause of a streamed utterance, in order, before the whole-take `Audio`
    /// (which then must not be played again).
    Chunk { id: u64, samples: Vec<f32>, rate: u32 },
    Failed(String),
    Error { id: u64, message: String },
}

/// One voice a mouth can speak with.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub lang: Option<Lang>,
}

pub trait Mouth: Send {
    /// Which voice speaks each language from now on.
    fn set_voices(&self, voices: Voices);
    /// Render `text` in `lang`; the audio arrives as a [`MouthEvent::Audio`] with this id.
    /// `stream`: ask for the speech clause by clause (`MouthEvent::Chunk`) so it
    /// can start before the whole take is rendered.
    fn say(&self, text: &str, lang: Lang, stream: bool) -> u64;
    fn cancel(&self);
    fn poll(&self) -> Vec<MouthEvent>;
}

/// The voices the hub speaks with: Kokoro's own, one per language.
#[derive(Clone, Debug, PartialEq)]
pub struct Voices {
    pub en: String,
    pub zh: String,
}

impl Default for Voices {
    fn default() -> Self {
        Voices { en: "af_heart".to_string(), zh: "zf_001".to_string() }
    }
}

pub struct HubMouth {
    session: TtsSession,
    voices: Arc<Mutex<Voices>>,
}

impl HubMouth {
    pub fn start(wake: Arc<dyn Fn() + Send + Sync>, voices: Voices) -> HubMouth {
        let config = TtsConfig {
            engine: TtsEngine::Kokoro,
            reach: SpeechReach::Lan,
            wake: Some(wake),
            ..TtsConfig::default()
        };
        HubMouth { session: TtsSession::start(config), voices: Arc::new(Mutex::new(voices)) }
    }
}

impl Mouth for HubMouth {
    fn say(&self, text: &str, lang: Lang, stream: bool) -> u64 {
        let voices = self.voices.lock().map(|v| v.clone()).unwrap_or_default();
        let (voice, language) = match lang {
            Lang::En => (voices.en, "en-US"),
            Lang::Zh => (voices.zh, "zh-CN"),
        };
        self.session.say_with(text, SayOptions { voice: Some(voice), language: Some(language.to_string()), stream: Some(stream), ..SayOptions::default() })
    }

    fn set_voices(&self, voices: Voices) {
        if let Ok(mut v) = self.voices.lock() {
            *v = voices;
        }
    }

    fn cancel(&self) {
        self.session.cancel();
    }

    fn poll(&self) -> Vec<MouthEvent> {
        let mut out = Vec::new();
        for event in self.session.poll() {
            match event {
                TtsEvent::Ready(info) => {
                    let note = match &info.remote {
                        Some(node) => format!("{} on {node}", info.engine),
                        None => info.engine.clone(),
                    };
                    let voices = info
                        .voices
                        .iter()
                        .map(|v| VoiceInfo { id: v.id.clone(), name: v.name.clone(), lang: lang_of_code(&v.language) })
                        .collect();
                    out.push(MouthEvent::Ready(note, voices));
                }
                TtsEvent::Audio { utterance, audio: SpeechAudio { samples, sample_rate }, .. } => {
                    out.push(MouthEvent::Audio { id: utterance, samples, rate: sample_rate })
                }
                TtsEvent::AudioChunk { utterance, audio: SpeechAudio { samples, sample_rate }, .. } => {
                    out.push(MouthEvent::Chunk { id: utterance, samples, rate: sample_rate })
                }
                TtsEvent::Failed(why) => out.push(MouthEvent::Failed(why)),
                TtsEvent::Error { utterance, message } => out.push(MouthEvent::Error { id: utterance, message }),
                TtsEvent::Loading { .. } => {}
            }
        }
        out
    }
}

// ------------------------------------------------------------- segmenter

/// What the segmenter decided about a stretch of audio.
#[derive(Clone, Debug, PartialEq)]
pub enum Cut {
    /// Speech started (the pre-roll is the first `Audio`).
    Begin,
    Audio(Vec<f32>),
    /// A short pause (about half the end-of-turn pause): the utterance may be over.
    /// Recognition can start now; `Audio` after it means the speaker went on.
    Maybe,
    /// The utterance is over (a pause, or the length limit).
    End,
}

/// Cuts a continuous 16 kHz stream into utterances: speech starts when the
/// energy rises clearly above the room's noise floor, and ends after a pause
/// or at `max_secs`. 20 ms frames, a short pre-roll so the first syllable
/// survives, a noise floor that follows the room.
pub struct Segmenter {
    frame: Vec<f32>,
    preroll: VecDeque<Vec<f32>>,
    speaking: bool,
    loud_run: usize,
    quiet_run: usize,
    spoken_frames: usize,
    /// Quiet frames since the last loud one: not sent until speech resumes, so a
    /// pause costs the recogniser nothing (and no partial is read over silence).
    held: Vec<Vec<f32>>,
    noise: f32,
    pause_frames: usize,
    maybe_frames: usize,
    max_frames: usize,
}

const FRAME: usize = 320;
const PREROLL_FRAMES: usize = 15;
const MIN_LEVEL: f32 = 0.012;

impl Segmenter {
    pub fn new(pause_secs: f32, max_secs: f32) -> Segmenter {
        Segmenter {
            frame: Vec::new(),
            preroll: VecDeque::new(),
            speaking: false,
            loud_run: 0,
            quiet_run: 0,
            spoken_frames: 0,
            held: Vec::new(),
            noise: 0.004,
            pause_frames: (pause_secs * 50.0).round().max(1.0) as usize,
            maybe_frames: (pause_secs * 25.0).round().max(1.0) as usize,
            max_frames: (max_secs * 50.0).round().max(10.0) as usize,
        }
    }

    pub fn reset(&mut self) {
        self.frame.clear();
        self.preroll.clear();
        self.speaking = false;
        self.loud_run = 0;
        self.quiet_run = 0;
        self.spoken_frames = 0;
        self.held.clear();
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<Cut> {
        let mut cuts = Vec::new();
        self.frame.extend_from_slice(samples);
        while self.frame.len() >= FRAME {
            let frame: Vec<f32> = self.frame.drain(..FRAME).collect();
            self.step(frame, &mut cuts);
        }
        cuts
    }

    fn step(&mut self, frame: Vec<f32>, cuts: &mut Vec<Cut>) {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        let threshold = (self.noise * 3.5).max(MIN_LEVEL);
        let loud = rms > threshold;
        if !self.speaking {
            // The floor follows quiet frames only.
            if !loud {
                self.noise += (rms.max(0.0005) - self.noise) * 0.05;
            }
            self.loud_run = if loud { self.loud_run + 1 } else { 0 };
            self.preroll.push_back(frame);
            while self.preroll.len() > PREROLL_FRAMES {
                self.preroll.pop_front();
            }
            if self.loud_run >= 3 {
                self.speaking = true;
                self.quiet_run = 0;
                self.spoken_frames = self.preroll.len();
                cuts.push(Cut::Begin);
                cuts.push(Cut::Audio(self.preroll.drain(..).flatten().collect()));
            }
            return;
        }
        self.quiet_run = if loud { 0 } else { self.quiet_run + 1 };
        self.spoken_frames += 1;
        if loud {
            // Speech again: the held pause goes first, so the audio stays continuous.
            for h in self.held.drain(..) {
                cuts.push(Cut::Audio(h));
            }
            cuts.push(Cut::Audio(frame));
        } else {
            self.held.push(frame);
        }
        if !loud && self.quiet_run == self.maybe_frames && self.quiet_run < self.pause_frames {
            // A short natural tail goes with the speculative reading.
            let tail = self.held.len().min(6);
            for h in self.held.drain(..tail) {
                cuts.push(Cut::Audio(h));
            }
            cuts.push(Cut::Maybe);
        }
        if self.quiet_run >= self.pause_frames || self.spoken_frames >= self.max_frames {
            if self.quiet_run < self.maybe_frames {
                let tail = self.held.len().min(6);
                for h in self.held.drain(..tail) {
                    cuts.push(Cut::Audio(h));
                }
            }
            self.held.clear();
            cuts.push(Cut::End);
            self.speaking = false;
            self.loud_run = 0;
            self.quiet_run = 0;
            self.spoken_frames = 0;
        }
    }
}

// --------------------------------------------------------------- capture

/// Mono 16 kHz audio from a device callback: any rate, any channel count in.
#[derive(Clone, Default)]
pub struct Capture {
    queue: Arc<Mutex<Vec<f32>>>,
}

impl Capture {
    /// Called on the audio thread with interleaved or planar `channels` of `rate` Hz.
    pub fn push_planar(&self, rate: f64, channels: &[&[f32]]) {
        let Some(first) = channels.first() else { return };
        let n = first.len();
        if n == 0 || rate <= 1.0 {
            return;
        }
        let mono: Vec<f32> = (0..n).map(|i| channels.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / channels.len() as f32).collect();
        let out = if (rate - SAMPLE_RATE as f64).abs() < 1.0 {
            mono
        } else {
            makepad_ai_hub::resample::resample_channel(&mono, rate.round() as u32, SAMPLE_RATE)
        };
        if let Ok(mut q) = self.queue.try_lock() {
            q.extend_from_slice(&out);
            // Bounded: a UI that stopped draining must not grow without limit.
            let cap = SAMPLE_RATE as usize * 30;
            if q.len() > cap {
                let drop = q.len() - cap;
                q.drain(..drop);
            }
        }
    }

    pub fn take(&self) -> Vec<f32> {
        self.queue.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
    }
}

// ---------------------------------------------------------------- player

struct Clip {
    samples: Vec<f32>,
    rate: f64,
    pos: f64,
}

/// What the speaker plays: clips queued by the app, mixed into the device
/// callback at the device's own rate.
#[derive(Clone)]
pub struct Player {
    clips: Arc<Mutex<VecDeque<Clip>>>,
    played: Arc<Mutex<u64>>,
    /// Seconds of speech played / queued since playback last started (the inline progress line).
    progress: Arc<Mutex<(f64, f64)>>,
    /// When the queued speech should be over, by its length: the app mutes the
    /// microphone until then (half duplex), also where no output device exists.
    busy_until: Arc<Mutex<std::time::Instant>>,
}

impl Default for Player {
    fn default() -> Self {
        Player { clips: Default::default(), played: Default::default(), progress: Default::default(), busy_until: Arc::new(Mutex::new(std::time::Instant::now())) }
    }
}

impl Player {
    pub fn enqueue(&self, samples: Vec<f32>, rate: u32) {
        if samples.is_empty() {
            return;
        }
        let secs = samples.len() as f64 / rate.max(1) as f64;
        let fresh = !self.is_playing();
        if let Ok(mut p) = self.progress.lock() {
            if fresh {
                *p = (0.0, 0.0);
            }
            p.1 += secs;
        }
        if let Ok(mut c) = self.clips.lock() {
            c.push_back(Clip { samples, rate: rate.max(1) as f64, pos: 0.0 });
        }
        if let Ok(mut b) = self.busy_until.lock() {
            let from = (*b).max(std::time::Instant::now());
            *b = from + std::time::Duration::from_secs_f64(secs + 0.4);
        }
    }

    /// Forget clips whose time is up (no device ever drained them).
    pub fn expire(&self) {
        if !self.is_playing() {
            if let Ok(mut c) = self.clips.lock() {
                c.clear();
            }
        }
    }

    /// (seconds played, seconds queued so far) of the current speech.
    pub fn progress(&self) -> (f64, f64) {
        self.progress.lock().map(|p| *p).unwrap_or((0.0, 0.0))
    }

    pub fn stop(&self) {
        if let Ok(mut c) = self.clips.lock() {
            c.clear();
        }
        if let Ok(mut p) = self.progress.lock() {
            *p = (0.0, 0.0);
        }
        if let Ok(mut b) = self.busy_until.lock() {
            *b = std::time::Instant::now();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.busy_until.lock().map(|b| std::time::Instant::now() < *b).unwrap_or(false)
    }

    /// Total samples handed to the device so far (a test hook).
    #[cfg(test)]
    pub fn samples_played(&self) -> u64 {
        self.played.lock().map(|p| *p).unwrap_or(0)
    }

    /// Fill one device buffer: `out` is `frames * channels`, interleaved.
    pub fn fill(&self, device_rate: f64, channels: usize, out: &mut [f32]) {
        out.iter_mut().for_each(|s| *s = 0.0);
        let Ok(mut clips) = self.clips.try_lock() else { return };
        let frames = out.len() / channels.max(1);
        let mut frame = 0;
        while frame < frames {
            let Some(clip) = clips.front_mut() else { break };
            let step = clip.rate / device_rate.max(1.0);
            while frame < frames {
                let i = clip.pos as usize;
                if i + 1 >= clip.samples.len() {
                    clip.pos = clip.samples.len() as f64;
                    break;
                }
                let t = (clip.pos - i as f64) as f32;
                let s = clip.samples[i] * (1.0 - t) + clip.samples[i + 1] * t;
                for c in 0..channels {
                    out[frame * channels + c] = s;
                }
                clip.pos += step;
                frame += 1;
            }
            if clip.pos as usize + 1 >= clip.samples.len() {
                clips.pop_front();
            }
        }
        if let Ok(mut p) = self.played.lock() {
            *p += frame as u64;
        }
        if frame > 0 {
            if let Ok(mut p) = self.progress.lock() {
                p.0 += frame as f64 / device_rate.max(1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_reports_progress_and_stop_resets_it() {
        let p = Player::default();
        p.enqueue(vec![0.1; 16000], 16000);
        p.enqueue(vec![0.1; 16000], 16000);
        assert_eq!(p.progress(), (0.0, 2.0));
        let mut out = vec![0.0f32; 8000];
        p.fill(16000.0, 1, &mut out);
        assert!((p.progress().0 - 0.5).abs() < 0.01);
        p.stop();
        assert_eq!(p.progress(), (0.0, 0.0));
        assert!(!p.is_playing());
    }

    fn tone(secs: f32, amp: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE as f32) as usize).map(|i| (i as f32 * 0.07).sin() * amp).collect()
    }

    fn silence(secs: f32) -> Vec<f32> {
        // faint room noise
        (0..(secs * SAMPLE_RATE as f32) as usize).map(|i| ((i * 7919 % 13) as f32 - 6.0) * 0.0003).collect()
    }

    fn run(seg: &mut Segmenter, audio: &[f32]) -> Vec<Cut> {
        audio.chunks(1600).flat_map(|c| seg.push(c)).collect()
    }

    fn count(cuts: &[Cut], what: &Cut) -> usize {
        cuts.iter().filter(|c| std::mem::discriminant(*c) == std::mem::discriminant(what)).count()
    }

    #[test]
    fn two_phrases_with_a_pause_are_two_utterances() {
        let mut seg = Segmenter::new(0.6, 12.0);
        let mut audio = silence(1.0);
        audio.extend(tone(1.5, 0.2));
        audio.extend(silence(1.2));
        audio.extend(tone(1.0, 0.2));
        audio.extend(silence(1.2));
        let cuts = run(&mut seg, &audio);
        assert_eq!(count(&cuts, &Cut::Begin), 2);
        assert_eq!(count(&cuts, &Cut::End), 2);
        assert_eq!(count(&cuts, &Cut::Maybe), 2);
        // the audio handed over covers the phrases (with pre-roll and the pause tail)
        let samples: usize = cuts.iter().map(|c| if let Cut::Audio(a) = c { a.len() } else { 0 }).sum();
        // two phrases (2.5 s) plus pre-roll and a short tail, not the 0.6 s pauses
        assert!(samples > SAMPLE_RATE as usize * 2 && samples < SAMPLE_RATE as usize * 4, "{samples}");
    }

    #[test]
    fn a_long_speech_is_cut_at_the_limit() {
        let mut seg = Segmenter::new(0.6, 3.0);
        let cuts = run(&mut seg, &tone(7.0, 0.2));
        assert!(count(&cuts, &Cut::End) >= 2, "{cuts:?}", cuts = count(&cuts, &Cut::End));
    }

    #[test]
    fn room_noise_alone_never_starts_an_utterance() {
        let mut seg = Segmenter::new(0.6, 12.0);
        assert_eq!(count(&run(&mut seg, &silence(5.0)), &Cut::Begin), 0);
    }

    #[test]
    fn language_codes() {
        assert_eq!(lang_of_code("zh"), Some(Lang::Zh));
        assert_eq!(lang_of_code("yue"), Some(Lang::Zh));
        assert_eq!(lang_of_code("en-US"), Some(Lang::En));
        assert_eq!(lang_of_code("fr"), None);
    }

    #[test]
    fn the_player_resamples_and_drains() {
        let p = Player::default();
        p.enqueue(vec![0.5; 1600], 16_000);
        assert!(p.is_playing());
        let mut out = vec![0.0f32; 4800 * 2]; // 100 ms at 48 kHz stereo
        p.fill(48_000.0, 2, &mut out);
        assert!(out[100] > 0.4 && out[101] > 0.4);
        p.stop();
        assert!(!p.is_playing());
        assert!(p.samples_played() >= 4700);
    }

    #[test]
    fn capture_downmixes_and_resamples() {
        let c = Capture::default();
        let l = vec![0.2f32; 4800];
        let r = vec![0.4f32; 4800];
        c.push_planar(48_000.0, &[&l, &r]);
        let got = c.take();
        assert!((got.len() as i64 - 1600).abs() < 8);
        assert!((got[100] - 0.3).abs() < 0.01);
    }
}
