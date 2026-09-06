//! Artifact playback + waveform strip.
//!
//! The UI owns a transport handle and the device callback owns the playback
//! engine. Commands and atomic snapshots cross between them; neither side
//! waits on a mutex. The service emits PCM16 WAV (kokoro: mono 24kHz,
//! sa3-sfx: stereo 44.1kHz), decoded here with a minimal RIFF parser
//! (libs/asset/ai wav.rs is encode-only).

use makepad_widgets::makepad_platform::audio::AudioBuffer;
use makepad_widgets::makepad_platform::thread::{Lane, TaskHandle, TaskPool};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

#[derive(Clone)]
pub struct WavPcm {
    /// Interleaved-to-stereo frames.
    pub frames: Vec<(f32, f32)>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl WavPcm {
    pub fn seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frames.len() as f64 / self.sample_rate as f64
    }
}

/// Minimal RIFF/WAVE parse: PCM8 / PCM16 (format 1) and float32 (format 3).
/// Quake/LibreQuake SFX are often unsigned 8-bit mono at 11.025/22.05 kHz.
pub fn parse_wav(bytes: &[u8]) -> Result<WavPcm, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut data: Option<&[u8]> = None;
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body_end = (at + 8 + size).min(bytes.len());
        let body = &bytes[at + 8..body_end];
        match id {
            b"fmt " if body.len() >= 16 => {
                format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                sample_rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            }
            b"data" => data = Some(body),
            _ => {}
        }
        // Chunks are word-aligned.
        at = body_end + (size & 1);
    }
    let data = data.ok_or("wav: no data chunk")?;
    if channels == 0 || sample_rate == 0 {
        return Err("wav: no fmt chunk".into());
    }
    let ch = channels as usize;
    let mut frames = Vec::new();
    match (format, bits) {
        (1, 8) => {
            // WAV PCM8 is unsigned, 128 = silence.
            for frame in data.chunks_exact(ch) {
                let s = |i: usize| (frame[i] as f32 - 128.0) / 128.0;
                frames.push((s(0), s(ch - 1)));
            }
        }
        (1, 16) => {
            for frame in data.chunks_exact(2 * ch) {
                let s = |i: usize| {
                    i16::from_le_bytes(frame[i * 2..i * 2 + 2].try_into().unwrap()) as f32 / 32768.0
                };
                frames.push((s(0), s(ch - 1)));
            }
        }
        (3, 32) => {
            for frame in data.chunks_exact(4 * ch) {
                let s = |i: usize| f32::from_le_bytes(frame[i * 4..i * 4 + 4].try_into().unwrap());
                frames.push((s(0), s(ch - 1)));
            }
        }
        other => return Err(format!("wav: unsupported format {other:?}")),
    }
    Ok(WavPcm {
        frames,
        sample_rate,
        channels,
    })
}

// ---------------------------------------------------------------------------
// Playback mixer
// ---------------------------------------------------------------------------

/// Q32.32 source-frame cursor. The audio callback is the sole clock that
/// advances it; UI code only loads, pauses and seeks.
const FP_ONE: u64 = 1 << 32;

struct AudioSnapshot {
    cursor_fp: AtomicU64,
    playing: AtomicBool,
    ack: AtomicU64,
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        Self {
            cursor_fp: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            ack: AtomicU64::new(0),
        }
    }
}

// ---------------------------------------------------------------------------
// Separated layers ("split audio layers")
// ---------------------------------------------------------------------------

/// One separated layer, in the form the mixer consumes: stereo i16 at the
/// separation model's rate. i16 because four lanes of a six-minute track is
/// a quarter of a gigabyte as f32 and half that as i16 — and the difference
/// is inaudible under a stem the model already band-limited.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StemPcm {
    pub frames: Vec<[i16; 2]>,
    pub sample_rate: u32,
}

impl StemPcm {
    pub fn seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frames.len() as f64 / self.sample_rate as f64
    }
}

pub const STEM_LANES: usize = 4;

/// Lane order, which is `FileRole::STEMS` order — the order the side-channel
/// files are published and fetched in, so no index ever has to be remapped.
pub const STEM_LANE_NAMES: [&str; STEM_LANES] = ["drums", "bass", "vocals", "other"];

enum AudioCommand {
    InstallClip {
        serial: u64,
        clip: Arc<WavPcm>,
    },
    ClearClip {
        serial: u64,
    },
    InstallStems {
        serial: u64,
        lanes: Arc<[StemPcm; STEM_LANES]>,
    },
    ClearStems {
        serial: u64,
    },
    Play {
        serial: u64,
    },
    Pause {
        serial: u64,
    },
    Stop {
        serial: u64,
    },
    Seek {
        serial: u64,
        cursor_fp: u64,
    },
    MuteLane {
        serial: u64,
        lane: usize,
        muted: bool,
    },
}

enum RetiredAudio {
    Clip(Arc<WavPcm>),
    Stems(Arc<[StemPcm; STEM_LANES]>),
}

struct PendingDecode {
    generation: u64,
    task: TaskHandle<Result<WavPcm, String>>,
}

/// UI-thread transport handle. It owns the requested state and communicates
/// with the realtime callback exclusively through commands and atomics.
struct AudioMixer {
    commands: mpsc::Sender<AudioCommand>,
    retired: mpsc::Receiver<RetiredAudio>,
    snapshot: Arc<AudioSnapshot>,
    engine: Option<AudioEngine>,
    clip: Option<Arc<WavPcm>>,
    stems: Option<Arc<[StemPcm; STEM_LANES]>>,
    stem_generation: u64,
    muted: [bool; STEM_LANES],
    cursor_fp: u64,
    playing: bool,
    serial: u64,
    load_generation: u64,
    pending_decodes: Vec<PendingDecode>,
}

/// Realtime-owned state. Once installed in `cx.audio_output`, only the audio
/// callback touches these payloads and cursors.
pub struct AudioEngine {
    commands: mpsc::Receiver<AudioCommand>,
    retired: mpsc::Sender<RetiredAudio>,
    snapshot: Arc<AudioSnapshot>,
    clip: Option<Arc<WavPcm>>,
    stems: Option<Arc<[StemPcm; STEM_LANES]>>,
    muted: [bool; STEM_LANES],
    cursor_fp: u64,
    playing: bool,
    ack: u64,
}

impl AudioMixer {
    fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (retired_tx, retired_rx) = mpsc::channel();
        let snapshot = Arc::new(AudioSnapshot::default());
        let engine = AudioEngine {
            commands: command_rx,
            retired: retired_tx,
            snapshot: snapshot.clone(),
            clip: None,
            stems: None,
            muted: [false; STEM_LANES],
            cursor_fp: 0,
            playing: false,
            ack: 0,
        };
        Self {
            commands: command_tx,
            retired: retired_rx,
            snapshot,
            engine: Some(engine),
            clip: None,
            stems: None,
            stem_generation: u64::MAX,
            muted: [false; STEM_LANES],
            cursor_fp: 0,
            playing: false,
            serial: 0,
            load_generation: 0,
            pending_decodes: Vec::new(),
        }
    }

    fn take_engine(&mut self) -> Option<AudioEngine> {
        self.engine.take()
    }

    fn next_serial(&mut self) -> u64 {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.serial
    }

    fn send(&self, command: AudioCommand) {
        let _ = self.commands.send(command);
    }

    fn refresh_snapshot(&mut self) {
        let ack = self.snapshot.ack.load(Ordering::Acquire);
        if ack >= self.serial {
            self.cursor_fp = self.snapshot.cursor_fp.load(Ordering::Acquire);
            self.playing = self.snapshot.playing.load(Ordering::Acquire);
        }
    }

    fn pump(&mut self) {
        self.refresh_snapshot();
        for retired in self.retired.try_iter() {
            match retired {
                RetiredAudio::Clip(clip) => drop(clip),
                RetiredAudio::Stems(stems) => drop(stems),
            }
        }
        let pending_decodes = std::mem::take(&mut self.pending_decodes);
        let mut waiting = Vec::with_capacity(pending_decodes.len());
        for mut pending in pending_decodes {
            let Some(result) = pending.task.try_take() else {
                waiting.push(pending);
                continue;
            };
            match result {
                Ok(Ok(pcm)) if pending.generation == self.load_generation => {
                    self.install(pcm, pending.generation);
                }
                Ok(Err(error)) if pending.generation == self.load_generation => {
                    makepad_widgets::log!("audio decode failed: {error}");
                }
                Err(error) if pending.generation == self.load_generation => {
                    makepad_widgets::log!("audio decode task failed: {error}");
                }
                _ => {}
            }
        }
        self.pending_decodes = waiting;
    }

    fn clear_stems(&mut self) {
        self.stems = None;
        self.stem_generation = u64::MAX;
        self.muted = [false; STEM_LANES];
        let serial = self.next_serial();
        self.send(AudioCommand::ClearStems { serial });
    }

    fn clear(&mut self) {
        self.clear_stems();
        self.clip = None;
        self.cursor_fp = 0;
        self.playing = false;
        let serial = self.next_serial();
        self.send(AudioCommand::ClearClip { serial });
    }

    fn install(&mut self, pcm: WavPcm, generation: u64) -> bool {
        if self.load_generation != generation {
            return false;
        }
        if pcm.frames.is_empty() || pcm.sample_rate == 0 {
            self.clear();
            return false;
        }
        if self.stem_generation != generation {
            self.clear_stems();
        }
        let clip = Arc::new(pcm);
        self.clip = Some(clip.clone());
        self.cursor_fp = 0;
        self.playing = false;
        let serial = self.next_serial();
        self.send(AudioCommand::InstallClip { serial, clip });
        true
    }

    fn load(&mut self, pcm: WavPcm) -> bool {
        self.load_generation = self.load_generation.wrapping_add(1);
        self.install(pcm, self.load_generation)
    }

    fn set_stems(&mut self, lanes: [StemPcm; STEM_LANES], generation: u64) -> bool {
        if self.load_generation != generation {
            return false;
        }
        if lanes
            .iter()
            .any(|lane| lane.frames.is_empty() || lane.sample_rate == 0)
        {
            self.clear_stems();
            return false;
        }
        let lanes = Arc::new(lanes);
        self.stems = Some(lanes.clone());
        self.stem_generation = generation;
        self.muted = [false; STEM_LANES];
        let serial = self.next_serial();
        self.send(AudioCommand::InstallStems { serial, lanes });
        true
    }

    fn play(&mut self) {
        let Some(clip) = self.clip.as_ref() else {
            return;
        };
        let end = (clip.frames.len() as u64) << 32;
        if self.cursor_fp >= end {
            self.cursor_fp = 0;
        }
        self.playing = true;
        let serial = self.next_serial();
        self.send(AudioCommand::Play { serial });
    }

    fn pause(&mut self) {
        self.playing = false;
        let serial = self.next_serial();
        self.send(AudioCommand::Pause { serial });
    }

    fn stop(&mut self) {
        self.playing = false;
        self.cursor_fp = 0;
        let serial = self.next_serial();
        self.send(AudioCommand::Stop { serial });
    }

    fn seek_fraction(&mut self, fraction: f64) {
        let Some(clip) = self.clip.as_ref() else {
            return;
        };
        let frame = (fraction.clamp(0.0, 1.0) * clip.frames.len() as f64) as u64;
        self.cursor_fp = frame.min(clip.frames.len() as u64) << 32;
        let serial = self.next_serial();
        self.send(AudioCommand::Seek {
            serial,
            cursor_fp: self.cursor_fp,
        });
    }

    fn set_lane_muted(&mut self, lane: usize, muted: bool) {
        let Some(slot) = self.muted.get_mut(lane) else {
            return;
        };
        *slot = muted;
        let serial = self.next_serial();
        self.send(AudioCommand::MuteLane {
            serial,
            lane,
            muted,
        });
    }
}

thread_local! {
    static AUDIO_MIXER: RefCell<AudioMixer> = RefCell::new(AudioMixer::new());
}

fn with_mixer<T>(f: impl FnOnce(&mut AudioMixer) -> T) -> T {
    AUDIO_MIXER.with(|mixer| f(&mut mixer.borrow_mut()))
}

/// Move the engine into the app's one audio callback. It is intentionally
/// one-shot: there must never be a second owner of realtime state.
pub fn take_engine() -> AudioEngine {
    with_mixer(|mixer| mixer.take_engine()).expect("audio engine is installed once")
}

/// Poll background decodes and reclaim callback-retired payloads on the UI.
pub fn pump() {
    with_mixer(AudioMixer::pump);
}

/// Install four separated layers over the loaded clip. From here the
/// transport plays their SUM instead of the mixed track — which is also the
/// proof that the separation is faithful: all four unmuted must sound like
/// the original.
///
/// `generation` is the clip generation the fetch was started for (see
/// [`load_clip_async`]); a newer track having been picked meanwhile drops
/// these layers instead of playing them over the wrong song.
///
/// Refused (and the layers cleared) when a lane is empty: half a stem set is
/// a lie about what the asset carries.
pub fn set_stems(lanes: [StemPcm; STEM_LANES], generation: u64) -> bool {
    with_mixer(|mixer| mixer.set_stems(lanes, generation))
}

/// Back to the mixed track. Called whenever the clip changes, so a new
/// selection can never play the previous track's layers.
pub fn clear_stems() {
    with_mixer(AudioMixer::clear_stems);
}

/// True when the transport is playing separated layers.
pub fn stems_ready() -> bool {
    with_mixer(|mixer| mixer.stems.is_some())
}

pub fn lane_muted(lane: usize) -> bool {
    with_mixer(|mixer| mixer.muted.get(lane).copied().unwrap_or(false))
}

pub fn set_lane_muted(lane: usize, muted: bool) {
    with_mixer(|mixer| mixer.set_lane_muted(lane, muted));
}

/// Length of the installed layers, for the honest "these are this track's
/// stems" check a host wants before it draws the toggles.
pub fn stems_seconds() -> f64 {
    with_mixer(|mixer| mixer.stems.as_ref().map_or(0.0, |lanes| lanes[0].seconds()))
}

/// One lane at a fixed-point cursor, linearly interpolated — the same
/// resampling the mixed-clip path does, so switching between them cannot
/// change the pitch.
fn sample_lane(lane: &StemPcm, cursor: u64) -> (f32, f32) {
    let len = lane.frames.len();
    if len == 0 {
        return (0.0, 0.0);
    }
    let index = (cursor >> 32) as usize;
    if index >= len {
        return (0.0, 0.0);
    }
    let fraction = (cursor & (FP_ONE - 1)) as f32 / FP_ONE as f32;
    let next = (index + 1).min(len - 1);
    let sample = |value: i16| value as f32 / 32768.0;
    let (al, ar) = (sample(lane.frames[index][0]), sample(lane.frames[index][1]));
    let (bl, br) = (sample(lane.frames[next][0]), sample(lane.frames[next][1]));
    (al + (bl - al) * fraction, ar + (br - ar) * fraction)
}

/// Install one decoded WAV, paused at zero. Empty/invalid clips deliberately
/// leave the transport unavailable instead of simulating a player.
///
/// This is the FOREIGN entry point (the Create surface's viewer): it claims
/// a new clip generation, which invalidates any separated layers installed
/// for the previous one.
pub fn load(pcm: WavPcm) -> bool {
    with_mixer(|mixer| mixer.load(pcm))
}

/// Discard the loaded clip and make the transport unavailable.
pub fn clear() {
    with_mixer(AudioMixer::clear);
}

/// Take a track's bytes — WAV, MP3 or Ogg — and make the transport play them.
///
/// Decoding happens in the runtime pool: the music library is MP3s, and
/// turning six minutes of one into PCM on the frame thread is a visible
/// stall. The UI polls the task and sends the accepted result to the engine.
///
/// The transport goes unavailable immediately, because the previous track is
/// no longer what the well is showing — a stale clip left loaded is a play
/// button that plays the wrong song.
/// Returns the clip generation this request claimed, which is what a
/// side-channel fetch for the same track carries back into [`set_stems`].
pub fn load_clip_async(pool: &TaskPool, bytes: Vec<u8>) -> u64 {
    with_mixer(|mixer| {
        mixer.load_generation = mixer.load_generation.wrapping_add(1);
        let generation = mixer.load_generation;
        mixer.clear();
        match pool.submit(Lane::Heavy, move || decode_clip(&bytes)) {
            Ok(task) => mixer
                .pending_decodes
                .push(PendingDecode { generation, task }),
            Err(error) => makepad_widgets::log!("audio decode job refused: {error}"),
        }
        generation
    })
}

/// The clip generation currently claimed. A side-channel fetch records it
/// with the request and hands it back to [`set_stems`].
pub fn clip_generation() -> u64 {
    with_mixer(|mixer| mixer.load_generation)
}

/// Any container the catalog carries, in the mixer's shape. RIFF is parsed
/// here (it is a header and a memcpy); MP3 and Ogg go through the shared
/// zero-dependency decoder, the same one the preview well draws with.
pub fn decode_clip(bytes: &[u8]) -> Result<WavPcm, String> {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        return parse_wav(bytes);
    }
    let format = makepad_audio_decode::sniff(bytes).ok_or("unrecognised audio container")?;
    let audio = makepad_audio_decode::decode_audio_limited(
        bytes,
        format,
        makepad_audio_decode::Limits::default(),
    )
    .map_err(|error| format!("{error:?}"))?;
    let channels = audio.channels.max(1) as usize;
    Ok(WavPcm {
        frames: audio
            .pcm_interleaved_f32
            .chunks_exact(channels)
            .map(|frame| (frame[0], frame[channels - 1]))
            .collect(),
        sample_rate: audio.rate.max(1),
        channels: audio.channels.max(1),
    })
}

/// Start or resume. Starting from the end restarts at zero.
pub fn play() {
    with_mixer(AudioMixer::play);
}

pub fn pause() {
    with_mixer(AudioMixer::pause);
}

/// Stop returns to the start but retains the decoded clip for replay.
pub fn stop() {
    with_mixer(AudioMixer::stop);
}

pub fn is_ready() -> bool {
    with_mixer(|mixer| mixer.clip.is_some())
}

pub fn is_playing() -> bool {
    with_mixer(|mixer| {
        mixer.refresh_snapshot();
        let end = mixer
            .clip
            .as_ref()
            .map_or(0, |clip| (clip.frames.len() as u64) << 32);
        mixer.playing && mixer.cursor_fp < end
    })
}

pub fn duration_secs() -> f64 {
    with_mixer(|mixer| mixer.clip.as_ref().map_or(0.0, |clip| clip.seconds()))
}

/// Truthful device-clocked playhead, never derived from a UI timer.
pub fn playhead_secs() -> f64 {
    with_mixer(|mixer| {
        mixer.refresh_snapshot();
        mixer.clip.as_ref().map_or(0.0, |clip| {
            (mixer.cursor_fp as f64 / FP_ONE as f64) / clip.sample_rate as f64
        })
    })
}

/// Normalized playhead across the loaded clip for the waveform overlay:
/// 0.0 with no clip, monotonically 0..=1 while the device callback advances.
pub fn playhead_fraction() -> f64 {
    let duration = duration_secs();
    if duration <= 0.0 {
        return 0.0;
    }
    (playhead_secs() / duration).clamp(0.0, 1.0)
}

pub fn at_end() -> bool {
    with_mixer(|mixer| {
        mixer.refresh_snapshot();
        mixer
            .clip
            .as_ref()
            .is_some_and(|clip| mixer.cursor_fp >= (clip.frames.len() as u64) << 32)
    })
}

/// Sample-accurate fractional seek, clamped to the decoded clip.
pub fn seek_fraction(frac: f64) {
    with_mixer(|mixer| mixer.seek_fraction(frac));
}

/// Long-form threshold for the audition policy below: at/under this a voice
/// line is "short" and auditions like an SFX.
pub const ONE_SHOT_MAX_SPEECH_SECS: f64 = 30.0;

/// One-shot audition policy for a **freshly accepted** generated clip.
/// History / Library reopen must not use this — `play()` at end-of-clip
/// restarts, so a second display of a 200ms Quake/Doom shot is a loop.
///
/// Generated `audio` always auditions (the accept is the point). Short
/// `speech` lines do too. Music, imported pack `sfx` (Quake water/wind,
/// Doom DS_*), and everything else load paused; Play is explicit.
pub fn autoplay_one_shot(domain: &str, seconds: f64) -> bool {
    match domain {
        "audio" => true,
        "speech" => seconds <= ONE_SHOT_MAX_SPEECH_SECS,
        _ => false,
    }
}

/// Compact transport timestamp for the audio viewer.
pub fn format_time(secs: f64) -> String {
    let secs = secs.max(0.0);
    let minutes = (secs / 60.0).floor() as u64;
    format!("{minutes}:{:04.1}", secs - minutes as f64 * 60.0)
}

impl AudioEngine {
    fn retire(&self, retired: RetiredAudio) {
        let _ = self.retired.send(retired);
    }

    fn drain_commands(&mut self) {
        while let Ok(command) = self.commands.try_recv() {
            let serial = match command {
                AudioCommand::InstallClip { serial, clip } => {
                    if let Some(old) = self.clip.replace(clip) {
                        self.retire(RetiredAudio::Clip(old));
                    }
                    self.cursor_fp = 0;
                    self.playing = false;
                    serial
                }
                AudioCommand::ClearClip { serial } => {
                    if let Some(old) = self.clip.take() {
                        self.retire(RetiredAudio::Clip(old));
                    }
                    self.cursor_fp = 0;
                    self.playing = false;
                    serial
                }
                AudioCommand::InstallStems { serial, lanes } => {
                    if let Some(old) = self.stems.replace(lanes) {
                        self.retire(RetiredAudio::Stems(old));
                    }
                    self.muted = [false; STEM_LANES];
                    serial
                }
                AudioCommand::ClearStems { serial } => {
                    if let Some(old) = self.stems.take() {
                        self.retire(RetiredAudio::Stems(old));
                    }
                    self.muted = [false; STEM_LANES];
                    serial
                }
                AudioCommand::Play { serial } => {
                    if let Some(clip) = self.clip.as_ref() {
                        let end = (clip.frames.len() as u64) << 32;
                        if self.cursor_fp >= end {
                            self.cursor_fp = 0;
                        }
                        self.playing = true;
                    }
                    serial
                }
                AudioCommand::Pause { serial } => {
                    self.playing = false;
                    serial
                }
                AudioCommand::Stop { serial } => {
                    self.playing = false;
                    self.cursor_fp = 0;
                    serial
                }
                AudioCommand::Seek { serial, cursor_fp } => {
                    self.cursor_fp = self.clip.as_ref().map_or(0, |clip| {
                        cursor_fp.min((clip.frames.len() as u64) << 32)
                    });
                    serial
                }
                AudioCommand::MuteLane {
                    serial,
                    lane,
                    muted,
                } => {
                    if let Some(slot) = self.muted.get_mut(lane) {
                        *slot = muted;
                    }
                    serial
                }
            };
            self.ack = serial;
        }
    }

    fn publish(&self) {
        self.snapshot.cursor_fp.store(self.cursor_fp, Ordering::Relaxed);
        self.snapshot.playing.store(self.playing, Ordering::Relaxed);
        self.snapshot.ack.store(self.ack, Ordering::Release);
    }

    /// Add this transport to the app's output. Commands are drained first;
    /// no application lock or wait occurs on the realtime callback.
    pub fn mix_into(&mut self, output: &mut AudioBuffer, device_rate: f64) {
        self.drain_commands();
        if !self.playing || device_rate <= 0.0 {
            self.publish();
            return;
        }
        let Some(clip) = self.clip.as_ref() else {
            self.playing = false;
            self.publish();
            return;
        };
        let end = (clip.frames.len() as u64) << 32;
        if self.cursor_fp >= end {
            self.playing = false;
            self.publish();
            return;
        }
        let step = ((clip.sample_rate as f64 / device_rate) * FP_ONE as f64) as u64;
        if step == 0 {
            self.publish();
            return;
        }
        const GAIN: f32 = 0.9;

        if let Some(lanes) = self.stems.as_ref() {
            let stem_rate = lanes[0].sample_rate.max(1) as f64;
            let stem_step = ((stem_rate / device_rate) * FP_ONE as f64) as u64;
            let secs = (self.cursor_fp as f64 / FP_ONE as f64) / clip.sample_rate as f64;
            let mut stem_cursor = (secs * stem_rate * FP_ONE as f64) as u64;
            for frame in 0..output.frame_count() {
                if self.cursor_fp >= end {
                    self.playing = false;
                    self.cursor_fp = end;
                    break;
                }
                let (mut l, mut r) = (0.0f32, 0.0f32);
                for (index, lane) in lanes.iter().enumerate() {
                    if self.muted[index] {
                        continue;
                    }
                    let (ll, rr) = sample_lane(lane, stem_cursor);
                    l += ll;
                    r += rr;
                }
                l *= GAIN;
                r *= GAIN;
                for channel in 0..output.channel_count() {
                    output.channel_mut(channel)[frame] += if channel == 0 { l } else { r };
                }
                self.cursor_fp = self.cursor_fp.saturating_add(step);
                stem_cursor = stem_cursor.saturating_add(stem_step);
            }
            self.cursor_fp = self.cursor_fp.min(end);
            self.publish();
            return;
        }

        for frame in 0..output.frame_count() {
            if self.cursor_fp >= end {
                self.playing = false;
                self.cursor_fp = end;
                break;
            }
            let index = (self.cursor_fp >> 32) as usize;
            let fraction = (self.cursor_fp & (FP_ONE - 1)) as f32 / FP_ONE as f32;
            let next = (index + 1).min(clip.frames.len() - 1);
            let (al, ar) = clip.frames[index];
            let (bl, br) = clip.frames[next];
            let l = (al + (bl - al) * fraction) * GAIN;
            let r = (ar + (br - ar) * fraction) * GAIN;
            for channel in 0..output.channel_count() {
                output.channel_mut(channel)[frame] += if channel == 0 { l } else { r };
            }
            self.cursor_fp = self.cursor_fp.saturating_add(step);
        }
        self.cursor_fp = self.cursor_fp.min(end);
        self.publish();
    }
}

// ---------------------------------------------------------------------------
// Waveform strip: min/max column render into a BGRA pixel buffer
// (displayed via Image.set_texture, same texture path as video frames).
// ---------------------------------------------------------------------------

/// Persisted audio sidecar dimensions. SQUARE, because a card is: a wide
/// strip letterboxes into a tile with dead bands above and below, and the
/// detail you can read off it at full width is exactly what a card throws
/// away.
pub const WAVEFORM_THUMB_W: usize = 192;
pub const WAVEFORM_THUMB_H: usize = 192;

/// Encode the card picture of a track as an encoded-PNG sidecar payload.
/// Persisting this at accept/backfill time lets gallery refreshes decode a
/// small PNG instead of rereading and scanning the whole WAV.
pub fn waveform_thumbnail_png(pcm: &WavPcm) -> Option<Vec<u8>> {
    // The SAME composite the importer publishes — spectrogram with a wave
    // strip along its bottom edge — so the card in this app and the
    // thumbnail in the catalog are one picture, not two.
    let mono: Vec<f32> = pcm.frames.iter().map(|(l, r)| (l + r) * 0.5).collect();
    if let Some((rgba, _regions)) = makepad_asset_importer::spectrogram::composite_rgba(
        &mono,
        pcm.sample_rate,
        WAVEFORM_THUMB_W,
        WAVEFORM_THUMB_H,
    ) {
        return makepad_ai_hub::testpattern::encode_png_rgba(
            &rgba,
            WAVEFORM_THUMB_W,
            WAVEFORM_THUMB_H,
        )
        .ok();
    }
    let bgra = waveform_bgra(pcm, WAVEFORM_THUMB_W, WAVEFORM_THUMB_H);
    let mut rgba = Vec::with_capacity(bgra.len() * 4);
    for pixel in bgra {
        rgba.extend_from_slice(&[
            (pixel >> 16) as u8,
            (pixel >> 8) as u8,
            pixel as u8,
            (pixel >> 24) as u8,
        ]);
    }
    makepad_ai_hub::testpattern::encode_png_rgba(&rgba, WAVEFORM_THUMB_W, WAVEFORM_THUMB_H)
        .ok()
}

pub fn waveform_bgra(pcm: &WavPcm, width: usize, height: usize) -> Vec<u32> {
    const BG: u32 = 0xff10_1418;
    const FG: u32 = 0xff58_c4a0;
    const MID: u32 = 0xff2a_3238;
    let mut out = vec![BG; width * height];
    if pcm.frames.is_empty() || width == 0 || height == 0 {
        return out;
    }
    let mid_y = height / 2;
    for x in 0..width {
        out[mid_y * width + x] = MID;
    }
    let per_col = (pcm.frames.len() as f64 / width as f64).max(1.0);
    for x in 0..width {
        // A clip shorter than the strip is wide must clamp the column start
        // too, or the trailing columns slice out of range and panic.
        let start = ((x as f64 * per_col) as usize).min(pcm.frames.len() - 1);
        let end = (((x + 1) as f64 * per_col) as usize).min(pcm.frames.len());
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        for &(l, r) in &pcm.frames[start..end.max(start + 1).min(pcm.frames.len())] {
            let s = (l + r) * 0.5;
            lo = lo.min(s);
            hi = hi.max(s);
        }
        let half = (height / 2) as f32;
        let y0 = (mid_y as f32 - hi.clamp(-1.0, 1.0) * (half - 1.0)) as usize;
        let y1 = (mid_y as f32 - lo.clamp(-1.0, 1.0) * (half - 1.0)) as usize;
        for y in y0.min(height - 1)..=y1.min(height - 1) {
            out[y * width + x] = FG;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_transport() -> AudioEngine {
        AUDIO_MIXER.with(|slot| *slot.borrow_mut() = AudioMixer::new());
        take_engine()
    }

    fn transport_pcm() -> WavPcm {
        WavPcm {
            frames: vec![(0.0, 0.0), (0.4, -0.4), (0.8, -0.8), (1.0, -1.0)],
            sample_rate: 10,
            channels: 2,
        }
    }

    #[test]
    fn parses_service_style_pcm16_wav() {
        // Round-trip against the service's own encoder shape: a 100-sample
        // 24kHz mono ramp.
        let samples: Vec<f32> = (0..100).map(|i| i as f32 / 100.0 - 0.5).collect();
        let wav = makepad_ai_hub::wav::encode_wav_pcm16_mono(&samples, 24_000);
        let pcm = parse_wav(&wav).unwrap();
        assert_eq!(pcm.sample_rate, 24_000);
        assert_eq!(pcm.channels, 1);
        assert_eq!(pcm.frames.len(), 100);
        assert!((pcm.frames[50].0 - 0.0).abs() < 0.02);
        assert!((pcm.seconds() - 100.0 / 24_000.0).abs() < 1e-9);
    }

    #[test]
    fn parses_quake_style_pcm8_unsigned() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&0u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&11025u32.to_le_bytes());
        wav.extend_from_slice(&(11025u32).to_le_bytes()); // byte rate
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        let samples = [128u8, 255, 0, 128];
        wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(&samples);
        let size = (wav.len() - 8) as u32;
        wav[4..8].copy_from_slice(&size.to_le_bytes());
        let pcm = parse_wav(&wav).unwrap();
        assert_eq!(pcm.sample_rate, 11_025);
        assert_eq!(pcm.frames.len(), 4);
        assert!(pcm.frames[0].0.abs() < 0.01, "128 is silence, got {}", pcm.frames[0].0);
        assert!(pcm.frames[1].0 > 0.9);
        assert!(pcm.frames[2].0 < -0.9);
        assert!((pcm.seconds() - 4.0 / 11_025.0).abs() < 1e-9);
    }

    #[test]
    fn transport_is_device_clocked_pauseable_seekable_and_restarts_at_end() {
        let mut engine = reset_transport();
        clear();
        assert!(load(transport_pcm()));
        assert!(is_ready());
        assert!(!is_playing());
        assert_eq!(format_time(duration_secs()), "0:00.4");

        // Play alone does not advance time; only the device callback does.
        play();
        assert!(is_playing());
        assert_eq!(playhead_secs(), 0.0);
        assert_eq!(playhead_fraction(), 0.0);
        let mut output = AudioBuffer::new_with_size(2, 2);
        engine.mix_into(&mut output, 10.0);
        assert!((playhead_secs() - 0.2).abs() < 1e-9);
        // The drawn playhead tracks the same device-clocked cursor.
        assert!((playhead_fraction() - 0.5).abs() < 1e-9);
        assert_ne!(output.channel(0)[1], 0.0);

        pause();
        let paused_at = playhead_secs();
        let mut silent = AudioBuffer::new_with_size(2, 2);
        engine.mix_into(&mut silent, 10.0);
        assert_eq!(playhead_secs(), paused_at);
        assert!(silent.channel(0).iter().all(|sample| *sample == 0.0));

        seek_fraction(-1.0);
        assert_eq!(playhead_secs(), 0.0);
        assert_eq!(playhead_fraction(), 0.0);
        seek_fraction(9.0);
        assert!(at_end());
        assert_eq!(playhead_fraction(), 1.0);
        play();
        assert!(is_playing(), "play at end restarts the decoded clip");
        assert_eq!(playhead_secs(), 0.0);

        let mut to_end = AudioBuffer::new_with_size(8, 2);
        engine.mix_into(&mut to_end, 10.0);
        assert!(at_end());
        assert!(!is_playing());
        clear();
    }

    /// A constant-valued layer at the transport clip's own rate.
    fn lane(value: i16, frames: usize) -> StemPcm {
        StemPcm {
            frames: vec![[value, value]; frames],
            sample_rate: 10,
        }
    }

    #[test]
    fn layers_replace_the_mixed_track_and_mute_one_at_a_time() {
        let mut engine = reset_transport();
        clear();
        assert!(!stems_ready(), "no clip, no layers");
        assert!(load(transport_pcm()));
        assert!(!stems_ready(), "a fresh clip carries no layers until fetched");

        // FileRole::STEMS order: drums, bass, vocals, other.
        assert!(set_stems(
            [lane(1_000, 4), lane(2_000, 4), lane(4_000, 4), lane(8_000, 4)],
            clip_generation()
        ));
        assert!(stems_ready());
        assert!((stems_seconds() - 0.4).abs() < 1e-9);
        assert!(STEM_LANE_NAMES.contains(&"vocals"));

        const GAIN: f32 = 0.9;
        let expect = |sum: i32| sum as f32 / 32768.0 * GAIN;

        play();
        let mut all = AudioBuffer::new_with_size(1, 2);
        engine.mix_into(&mut all, 10.0);
        assert!(
            (all.channel(0)[0] - expect(1_000 + 2_000 + 4_000 + 8_000)).abs() < 1e-4,
            "all four layers sum: {}",
            all.channel(0)[0]
        );
        // The layers ride the SAME cursor as the mixed track, so the
        // transport, the waveform and the lyrics all still read one clock.
        assert!((playhead_secs() - 0.1).abs() < 1e-9);

        // Muting the vocals removes exactly that lane, nothing else.
        seek_fraction(0.0);
        set_lane_muted(2, true);
        assert!(lane_muted(2) && !lane_muted(0));
        let mut without_vocals = AudioBuffer::new_with_size(1, 2);
        engine.mix_into(&mut without_vocals, 10.0);
        assert!(
            (without_vocals.channel(0)[0] - expect(1_000 + 2_000 + 8_000)).abs() < 1e-4,
            "vocals muted: {}",
            without_vocals.channel(0)[0]
        );

        // All four muted is silence, not the original leaking through.
        seek_fraction(0.0);
        for index in 0..STEM_LANES {
            set_lane_muted(index, true);
        }
        let mut silent = AudioBuffer::new_with_size(1, 2);
        engine.mix_into(&mut silent, 10.0);
        assert!(silent.channel(0)[0].abs() < 1e-6, "{}", silent.channel(0)[0]);

        // Clearing the layers hands playback back to the mixed track.
        clear_stems();
        assert!(!stems_ready());
        seek_fraction(0.5);
        let mut mixed = AudioBuffer::new_with_size(1, 2);
        engine.mix_into(&mut mixed, 10.0);
        assert!(
            (mixed.channel(0)[0] - 0.8 * GAIN).abs() < 1e-4,
            "the clip's own third frame: {}",
            mixed.channel(0)[0]
        );

        // Layers belong to the clip generation they were fetched for: a
        // new track drops them, and layers offered for a generation that is
        // already gone are refused rather than played over the wrong song.
        assert!(set_stems(
            [lane(1_000, 4), lane(2_000, 4), lane(4_000, 4), lane(8_000, 4)],
            clip_generation()
        ));
        let stale = clip_generation();
        assert!(load(transport_pcm()));
        assert!(!stems_ready(), "loading another clip clears the layers");
        assert!(
            !set_stems(
                [lane(1_000, 4), lane(2_000, 4), lane(4_000, 4), lane(8_000, 4)],
                stale
            ),
            "a fetch for the previous track cannot install over this one"
        );
        assert!(!stems_ready());
        clear();
    }

    #[test]
    fn an_empty_layer_is_refused_rather_than_played_as_a_hole() {
        let _engine = reset_transport();
        clear();
        assert!(load(transport_pcm()));
        assert!(!set_stems(
            [lane(1_000, 4), lane(2_000, 4), StemPcm::default(), lane(8_000, 4)],
            clip_generation()
        ));
        assert!(!stems_ready());
        clear();
    }

    #[test]
    fn waveform_thumbnail_is_an_encoded_png_at_sidecar_dimensions() {
        let png = waveform_thumbnail_png(&transport_pcm()).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        // IHDR immediately follows the signature: width/height big-endian.
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(
            u32::from_be_bytes(png[16..20].try_into().unwrap()),
            WAVEFORM_THUMB_W as u32
        );
        assert_eq!(
            u32::from_be_bytes(png[20..24].try_into().unwrap()),
            WAVEFORM_THUMB_H as u32
        );
        // Silence still encodes (background strip), so a zero-frame WAV can
        // never wedge the backfill pump.
        assert!(waveform_thumbnail_png(&WavPcm {
            frames: Vec::new(),
            sample_rate: 44_100,
            channels: 2,
        })
        .is_some());
    }

    #[test]
    fn one_shot_policy_auditions_generated_audio_and_short_speech_only() {
        // Generated SFX (sa3/woosh/moss) run under domain "audio".
        assert!(autoplay_one_shot("audio", 1.5));
        assert!(autoplay_one_shot("audio", 45.0));
        // Speech lines audition when short; long narration loads paused.
        assert!(autoplay_one_shot("speech", 8.0));
        assert!(autoplay_one_shot("speech", ONE_SHOT_MAX_SPEECH_SECS));
        assert!(!autoplay_one_shot("speech", ONE_SHOT_MAX_SPEECH_SECS + 0.1));
        // Imported pack shots (Quake/Doom DS_*) must never auto-blast —
        // several are designed as ambients, and a History reopen would loop.
        assert!(!autoplay_one_shot("sfx", 0.3));
        assert!(!autoplay_one_shot("sfx", 8.0));
        // Music tracks never auto-blast — the scrub transport is the point.
        assert!(!autoplay_one_shot("music", 3.0));
        assert!(!autoplay_one_shot("music", 240.0));
    }

    #[test]
    fn empty_clip_is_unavailable() {
        let _engine = reset_transport();
        clear();
        assert!(!load(WavPcm {
            frames: Vec::new(),
            sample_rate: 44_100,
            channels: 2,
        }));
        assert!(!is_ready());
        assert_eq!(playhead_fraction(), 0.0);
    }
}
