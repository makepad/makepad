//! Audio-clock-master playback bridge. UI gestures are converted to
//! `score_play` messages; the audio callback owns the realtime engine and a
//! piano voice or installed Audio Unit. The SoundFont sampler serves the metronome
//! click.
//!
//! The UI builds each engine (for a plan and the device's rate) and sends it,
//! and every message, down one ordered ring; the callback adopts an engine
//! between blocks and hands the one it replaced back on a second ring, so it
//! never allocates or frees. A command the ring has no room for is held here,
//! reported, and sent first on a later frame: nothing the user did is lost.

use crate::sound::{audio_units, SoundSettings};
use makepad_audio_unit_host::{Instrument, MidiMessage};
use makepad_widgets_core::makepad_platform::thread::{Class, TaskHandle, TaskPool};
use makepad_score::model::{EventKind, Rational, Score};
use makepad_score_play::{
    Articulation as PlayArticulation, AtomicAudioClock, AudioClockSnapshot, AudioMessage,
    AudioMessageKind, AuditionController, ClockQuality, CountInSpec, DisplayPosition, EventBatch,
    Meter, NoteInput, PartMixer, PerformancePlan, PlanInput, PlaybackEngine, RenderContext,
    ScheduleOptions, Scheduler, ScrubConfig, ScrubController, ScrubHit, ScrubOutcome,
    SynthBackend, SynthEvent, SynthEventKind, SynthEventTiming, TempoMap, TransportLoop,
};
use crate::sound::ScoreEngine;
use makepad_piano_model::{
    fx::{soft_clip, Limiter, Perspective, Reverb, ReverbPreset},
    learned::PianoEngine,
    Piano,
    PianoEvent, PianoPreset, TimedEvent as PianoTimedEvent, Voicing,
};
use makepad_soundfont::{metronome_click, NoSamples, Sampler, SamplerEvent, TimedEvent};
use makepad_widgets_core::*;
use std::{
    ops::Range,
    ptr,
    collections::VecDeque,
    sync::{
        atomic::{AtomicI64, AtomicPtr, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

const MESSAGE_CAPACITY: usize = 512;
const PENDING_CAPACITY: usize = 256;
const PART_CAPACITY: usize = 32;
const VOICE_CAPACITY: usize = 96;
const EVENT_CAPACITY: usize = 64;
const SCRATCH_FRAMES: usize = 2048;
/// A callback's worth of frames for an instrument's warm-up.
const WARM_UP_FRAMES: usize = 512;
const PLAN_RATE: u32 = 48_000;
/// Peak-meter fall-back, per frame. Full scale to silence in about a third of
/// a second — slow enough to read, fast enough to follow a phrase.
const PEAK_FALL_PER_FRAME: f32 = 1.0 / 16_384.0;
/// How long a replaced instrument keeps sounding while it fades out. Swapping
/// the piano mid-phrase would otherwise cut every ringing string dead on one
/// sample, which is a click; ~40 ms of fade is inaudible.
const INSTRUMENT_FADE_FRAMES: u32 = 2048;

/// The room every instrument is heard in. One part of [`SoundSettings`]; it
/// reaches the audio thread through the same shared cell as everything else
/// there — plain atomics, no lock, no allocation, applied at the top of a
/// render block — and is played by the one [`Room`] on the instrument bus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoomSettings {
    pub preset: ReverbPreset,
    /// Dry/wet amount, 0.0 = dry.
    pub mix: f32,
    pub perspective: Perspective,
}

impl RoomSettings {
    pub const MIX_MAX: f32 = 1.0;
}

impl Default for RoomSettings {
    fn default() -> Self {
        Self {
            preset: ReverbPreset::Studio,
            mix: 0.25,
            perspective: Perspective::Player,
        }
    }
}

pub const REVERB_PRESETS: [(ReverbPreset, &str); 5] = [
    (ReverbPreset::PracticeRoom, "Practice"),
    (ReverbPreset::Studio, "Studio"),
    (ReverbPreset::SmallHall, "Small hall"),
    (ReverbPreset::ConcertHall, "Concert hall"),
    (ReverbPreset::Cathedral, "Cathedral"),
];

pub fn reverb_preset_label(preset: ReverbPreset) -> &'static str {
    REVERB_PRESETS
        .iter()
        .find(|(candidate, _)| *candidate == preset)
        .map_or("Room", |(_, label)| *label)
}

fn preset_index(preset: ReverbPreset) -> u32 {
    REVERB_PRESETS
        .iter()
        .position(|(candidate, _)| *candidate == preset)
        .unwrap_or(1) as u32
}

fn preset_from_index(index: u32) -> ReverbPreset {
    REVERB_PRESETS
        .get(index as usize)
        .map_or(ReverbPreset::Studio, |(preset, _)| *preset)
}

/// The one room of the Sound panel: a reverb on the instrument bus, after
/// whatever instrument is playing (and the one fading out under it), driven
/// by the panel's room and Reverb amount. Every instrument of ours is built
/// dry ([`build_engine`]), so this is the only room anything is heard in.
///
/// Its delay lines are allocated here, on the UI thread, with the backend;
/// on the audio thread a room change only recomputes eight decay gains, and
/// a block only runs the network — or nothing, once its tail has died away
/// and no sound comes in.
struct Room {
    reverb: Reverb,
    /// The tail's level on top of the dry instrument; 0 bypasses the room.
    mix: f32,
    /// Spans in a row with nothing above [`ROOM_SILENCE`] coming out.
    quiet_spans: u32,
    /// Frames left of the fade that takes away the tail of a place the
    /// transport left (0: none), and its length.
    fade_left: u32,
    fade_len: u32,
}

/// A room put out this little (-180 dBFS) for [`ROOM_QUIET_SPANS`] spans in
/// a row is emptied and sleeps until sound comes in again, so a tail never
/// decays into denormals and an idle score costs the callback nothing.
const ROOM_SILENCE: f32 = 1.0e-9;
const ROOM_QUIET_SPANS: u32 = 32;
/// How long the room takes to fade out the tail of a place the transport
/// jumped away from, s: long enough not to click, short enough that the old
/// place's tail never smears over the new one.
const ROOM_FADE_SECS: f32 = 0.03;

impl Room {
    fn new(sample_rate: f32) -> Self {
        Self {
            reverb: Reverb::new(sample_rate),
            mix: 0.0,
            quiet_spans: ROOM_QUIET_SPANS,
            fade_left: 0,
            fade_len: ((ROOM_FADE_SECS * sample_rate) as u32).max(1),
        }
    }

    fn apply(&mut self, room: &RoomSettings) {
        self.reverb.set_preset(room.preset);
        let mix = if room.mix.is_finite() { room.mix.clamp(0.0, RoomSettings::MIX_MAX) } else { 0.0 };
        // A bypassed room stops listening; empty it so turning it back up
        // never replays a tail from before.
        if mix == 0.0 && self.mix > 0.0 {
            self.clear();
        }
        self.mix = mix;
    }

    /// Silence, at once.
    fn clear(&mut self) {
        self.reverb.reset();
        self.quiet_spans = ROOM_QUIET_SPANS;
        self.fade_left = 0;
    }

    /// The transport jumped: the tail of the place it left fades out over
    /// [`ROOM_FADE_SECS`] and the room is emptied after, so nothing is cut.
    /// (A stop needs nothing: the tail rings out the way it would in a hall.)
    fn fade_out(&mut self) {
        if self.mix == 0.0 || self.quiet_spans >= ROOM_QUIET_SPANS {
            self.clear();
        } else if self.fade_left == 0 {
            self.fade_left = self.fade_len;
        }
    }

    /// The instrument plus its tail, in place. With the room bypassed the
    /// instrument passes untouched. The knee after it is the bus's.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.mix == 0.0 {
            return;
        }
        let peak = |left: &[f32], right: &[f32]| left.iter().chain(right).fold(0.0_f32, |peak, x| peak.max(x.abs()));
        if self.quiet_spans >= ROOM_QUIET_SPANS && !(peak(left, right) >= ROOM_SILENCE) {
            return;
        }
        let step = std::f32::consts::PI / self.fade_len as f32;
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            let (wet_left, wet_right) = self.reverb.process(*left, *right);
            let mut mix = self.mix;
            if self.fade_left > 0 {
                // A raised cosine from the full tail to none.
                self.fade_left -= 1;
                mix *= 0.5 - 0.5 * (step * self.fade_left as f32).cos();
                if self.fade_left == 0 {
                    self.reverb.reset();
                }
            }
            *left += mix * wet_left;
            *right += mix * wet_right;
        }
        if peak(left, right) >= ROOM_SILENCE {
            self.quiet_spans = 0;
        } else {
            self.quiet_spans += 1;
            if self.quiet_spans == ROOM_QUIET_SPANS {
                self.clear();
            }
        }
    }
}

/// Everything that shapes the piano, as plain atomics. UI writes, audio thread
/// reads. `revision` is the only thing the audio side polls, so a sound nobody
/// is editing costs one relaxed load per block.
///
/// This is the whole binding to `makepad_piano_model`'s control surface: the
/// six voicing amounts, the output EQ and trim, and the room. Every one of
/// them is documented safe to set between `process()` calls, so applying them
/// at the top of a render block allocates nothing and locks nothing.
#[derive(Debug, Default)]
struct SharedSound {
    revision: AtomicU32,
    // Room.
    preset: AtomicU32,
    mix_bits: AtomicU32,
    audience: AtomicU32,
    early_bits: AtomicU32,
    // Voicing: the runtime mechanism mix.
    body_tap: AtomicU32,
    knock: AtomicU32,
    roughness: AtomicU32,
    phantoms: AtomicU32,
    attack_noise: AtomicU32,
    attack_body: AtomicU32,
    sympathetic: AtomicU32,
    // Output EQ and trim.
    shelf_db: AtomicU32,
    shelf_hz: AtomicU32,
    bell_hz: AtomicU32,
    bell_db: AtomicU32,
    bell_q: AtomicU32,
    tone_bass: AtomicU32,
    tone_treble: AtomicU32,
    master: AtomicU32,
}

impl SharedSound {
    fn publish(&self, sound: SoundSettings) {
        let store = |cell: &AtomicU32, value: f32| cell.store(value.to_bits(), Ordering::Relaxed);
        self.preset.store(preset_index(sound.room.preset), Ordering::Relaxed);
        store(&self.mix_bits, sound.room.mix.clamp(0.0, RoomSettings::MIX_MAX));
        self.audience.store(
            u32::from(matches!(sound.room.perspective, Perspective::Audience)),
            Ordering::Relaxed,
        );
        store(&self.early_bits, sound.early_reflections);
        store(&self.body_tap, sound.voicing.body_tap);
        store(&self.knock, sound.voicing.knock);
        store(&self.roughness, sound.voicing.roughness);
        store(&self.phantoms, sound.voicing.phantoms);
        store(&self.attack_noise, sound.voicing.attack_noise);
        store(&self.attack_body, sound.voicing.attack_body);
        store(&self.sympathetic, sound.voicing.sympathetic);
        store(&self.shelf_db, sound.eq_shelf_db);
        store(&self.shelf_hz, sound.eq_shelf_hz);
        store(&self.bell_hz, sound.eq_bell_hz);
        store(&self.bell_db, sound.eq_bell_db);
        store(&self.bell_q, sound.eq_bell_q);
        store(&self.tone_bass, sound.tone_bass_db);
        store(&self.tone_treble, sound.tone_treble_db);
        store(&self.master, sound.master_gain);
        // Release last: the audio thread acquires this and then reads the rest.
        self.revision.fetch_add(1, Ordering::Release);
    }

    fn read(&self) -> SoundSettings {
        let load = |cell: &AtomicU32| f32::from_bits(cell.load(Ordering::Relaxed));
        SoundSettings {
            engine: ScoreEngine::Physical,
            // The preset index is a UI label; the audio side only ever needs
            // the values it produced, which are all published above.
            preset: crate::sound::default_preset_index(ScoreEngine::Physical),
            voicing: Voicing {
                body_tap: load(&self.body_tap),
                knock: load(&self.knock),
                roughness: load(&self.roughness),
                phantoms: load(&self.phantoms),
                attack_noise: load(&self.attack_noise),
                attack_body: load(&self.attack_body),
                sympathetic: load(&self.sympathetic),
            },
            eq_shelf_db: load(&self.shelf_db),
            eq_shelf_hz: load(&self.shelf_hz),
            eq_bell_hz: load(&self.bell_hz),
            eq_bell_db: load(&self.bell_db),
            eq_bell_q: load(&self.bell_q),
            tone_bass_db: load(&self.tone_bass),
            tone_treble_db: load(&self.tone_treble),
            master_gain: load(&self.master),
            room: RoomSettings {
                preset: preset_from_index(self.preset.load(Ordering::Relaxed)),
                mix: load(&self.mix_bits),
                perspective: if self.audience.load(Ordering::Relaxed) == 0 {
                    Perspective::Player
                } else {
                    Perspective::Audience
                },
            },
            early_reflections: load(&self.early_bits),
        }
    }
}

/// A voice is constructed away from the callback and has one owner at a time.
enum Voice {
    Piano(PianoEngine),
    AudioUnit(AuVoice),
    Waveguide(makepad_piano_waveguide::WaveguidePiano),
}

/// The same event for the waveguide grand, which has its own event type.
fn waveguide_event(event: PianoTimedEvent) -> makepad_piano_waveguide::TimedEvent {
    use makepad_piano_waveguide::PianoEvent as W;
    makepad_piano_waveguide::TimedEvent {
        offset: event.offset,
        event: match event.event {
            PianoEvent::NoteOn { key, velocity } => W::NoteOn { key, velocity },
            PianoEvent::NoteOff { key } => W::NoteOff { key },
            PianoEvent::Sustain { value } => W::Sustain { value },
            PianoEvent::SoftPedal { on } => W::SoftPedal { on },
            PianoEvent::Sostenuto { on } => W::Sostenuto { on },
            PianoEvent::AllSoundOff => W::AllSoundOff,
        },
    }
}

struct AuVoice {
    unit: Instrument,
    gain: f32,
}

/// Channel zero, with MIDI's velocity-zero note-off convention preserved.
fn midi_for(event: PianoEvent) -> [Option<MidiMessage>; 3] {
    let message = match event {
        PianoEvent::NoteOn { key, velocity } => MidiMessage::note_on(0, key, velocity),
        PianoEvent::NoteOff { key } => MidiMessage::note_off(0, key),
        PianoEvent::Sustain { value } => MidiMessage::control(0, 64, (value.clamp(0.0, 1.0) * 127.0).round() as u8),
        PianoEvent::SoftPedal { on } => MidiMessage::control(0, 67, if on { 127 } else { 0 }),
        PianoEvent::Sostenuto { on } => MidiMessage::control(0, 66, if on { 127 } else { 0 }),
        PianoEvent::AllSoundOff => return [
            Some(MidiMessage::control(0, 64, 0)),
            Some(MidiMessage::control(0, 120, 0)),
            Some(MidiMessage::control(0, 123, 0)),
        ],
    };
    [Some(message), None, None]
}

impl Voice {
    fn process(&mut self, events: &[PianoTimedEvent], left: &mut [f32], right: &mut [f32]) {
        match self {
            Self::Piano(piano) => piano.process(events, left, right),
            Self::Waveguide(piano) => {
                // Converted on the stack, 64 at a time; a block with more
                // events than that takes the rest at its end.
                let silent = makepad_piano_waveguide::TimedEvent { offset: 0, event: makepad_piano_waveguide::PianoEvent::Sustain { value: 0.0 } };
                let mut converted = [silent; 64];
                let (first, rest) = events.split_at(events.len().min(converted.len()));
                for (to, from) in converted.iter_mut().zip(first) {
                    *to = waveguide_event(*from);
                }
                piano.process(&converted[..first.len()], left, right);
                for chunk in rest.chunks(converted.len()) {
                    for (to, from) in converted.iter_mut().zip(chunk) {
                        *to = waveguide_event(*from);
                    }
                    piano.process(&converted[..chunk.len()], &mut [], &mut []);
                }
            }
            Self::AudioUnit(voice) => {
                for event in events {
                    for message in midi_for(event.event).into_iter().flatten() {
                        let _ = voice.unit.midi(message, event.offset);
                    }
                }
                if voice.unit.render(left, right).is_err() {
                    left.fill(0.0);
                    right.fill(0.0);
                }
                for sample in left.iter_mut().chain(right.iter_mut()) {
                    *sample *= voice.gain;
                }
            }
        }
        let trim = self.trim();
        if trim != 1.0 {
            for sample in left.iter_mut().chain(right.iter_mut()) {
                *sample *= trim;
            }
        }
    }

    /// Plays a mezzo-forte note with the pedal down through one callback's
    /// worth of frames and silences it again, on the thread that built the
    /// instrument: the audio thread's first callback then runs code that has
    /// run before. Under a JIT a path compiles on its first run, and a fresh
    /// waveguide grand's first callback took 106 % of its 11.6 ms budget
    /// (every later one at most 41 %); warmed up, it takes 34 %. Our own
    /// engines only: an Audio Unit
    /// is handed over as it loaded.
    fn warm_up(&mut self) {
        if matches!(self, Self::AudioUnit(_)) {
            return;
        }
        let (mut left, mut right) = (vec![0.0f32; WARM_UP_FRAMES], vec![0.0f32; WARM_UP_FRAMES]);
        let events = [
            PianoTimedEvent { offset: 0, event: PianoEvent::Sustain { value: 1.0 } },
            PianoTimedEvent { offset: 0, event: PianoEvent::NoteOn { key: 60, velocity: 64 } },
        ];
        self.process(&events, &mut left, &mut right);
        match self {
            Self::Waveguide(piano) => piano.reset(),
            Self::Piano(piano) => piano.reset(),
            Self::AudioUnit(_) => {}
        }
    }

    /// [`ScoreEngine::trim`] of the engine this voice plays: its level
    /// beside the others', after its own limiter.
    fn trim(&self) -> f32 {
        match self {
            Self::Waveguide(_) => ScoreEngine::Waveguide.trim(),
            Self::AudioUnit(_) => ScoreEngine::AudioUnit.trim(),
            Self::Piano(PianoEngine::Learned(_)) => ScoreEngine::Learned.trim(),
            Self::Piano(PianoEngine::Physical(_)) => ScoreEngine::Physical.trim(),
        }
    }

    /// The instrument's own controls. The room is not one of them: every
    /// voice plays dry into the bus's [`Room`]. AU voicing and EQ belong to
    /// the plug-in; only the output trim is ours.
    fn apply_sound(&mut self, sound: &SoundSettings) {
        match self {
            Self::AudioUnit(voice) => voice.gain = sound.master_gain,
            // Brightness is the same treble shelf.
            Self::Waveguide(piano) => {
                piano.set_brightness(sound.eq_shelf_db, sound.eq_shelf_hz);
                piano.set_master_gain(sound.master_gain);
            }
            Self::Piano(piano) => {
                // The listening position places the modal piano's image and
                // its lid reflections; the room around it is the bus's.
                piano.set_perspective(sound.room.perspective);
                piano.set_early_reflection_level(sound.early_reflections);
                piano.set_voicing(sound.voicing);
                piano.set_eq_shelf(sound.eq_shelf_db, sound.eq_shelf_hz);
                piano.set_eq_bell(sound.eq_bell_hz, sound.eq_bell_db, sound.eq_bell_q);
                piano.set_tone(sound.tone_bass_db, sound.tone_treble_db);
                piano.set_master_gain(sound.master_gain);
            }
        }
    }
}

fn build_audio_unit(index: usize, rate: f32) -> Result<Box<Voice>, String> {
    let entry = audio_units().get(index).ok_or("Audio Unit is no longer installed")?;
    Instrument::new(&entry.info.description, rate as f64, SCRATCH_FRAMES as u32)
        .map(|unit| Box::new(Voice::AudioUnit(AuVoice { unit, gain: 1.0 })))
        .map_err(|error| error.to_string())
}

#[derive(Debug, PartialEq, Eq)]
pub enum InstrumentLoad {
    Loaded(usize),
    Failed(usize, String),
    /// One of our own instruments, built on the pool, is now sounding.
    Built(ScoreEngine, usize),
    /// Its build did not finish; the instrument before it still sounds.
    Unbuilt(ScoreEngine, usize, String),
}

struct AudioUnitLoad {
    request: u64,
    index: usize,
    /// `None` for an Audio Unit, the engine for an instrument of ours.
    built: Option<ScoreEngine>,
    task: TaskHandle<Result<Box<Voice>, String>>,
}

/// Build the instrument the application asked for.
///
/// Hybrid is the one that is not simply a `PianoEngine::new`: it is the
/// physical instrument with [`crate::hybrid`]'s baked per-partial targets
/// applied across all 88 keys before it ever renders a block. That costs
/// 3.6 ms, which is why it belongs here on the UI thread alongside the
/// design rebuilds rather than anywhere near the callback.
///
/// Each is built DRY: the engines ship with rooms of their own, and here
/// those are switched off for good, because the room is the bus's [`Room`]
/// and an instrument with a second room inside would be heard in two.
fn build_engine(engine: ScoreEngine, rate: f32, preset: &PianoPreset) -> Box<Voice> {
    let mut voice = construct_engine(engine, rate, preset);
    voice.warm_up();
    Box::new(voice)
}

/// [`build_engine`] without the warm-up.
fn construct_engine(engine: ScoreEngine, rate: f32, preset: &PianoPreset) -> Voice {
    if engine == ScoreEngine::Waveguide {
        let mut piano = makepad_piano_waveguide::WaveguidePiano::new(rate);
        piano.set_room_mix(0.0);
        Voice::Waveguide(piano)
    } else {
        let mut piano = match engine {
            ScoreEngine::Hybrid => {
                let mut piano = Piano::new_with_preset(rate, preset);
                crate::hybrid::apply_targets(&mut piano);
                PianoEngine::Physical(Box::new(piano))
            }
            other => PianoEngine::new(other.kind(), rate, preset),
        };
        piano.set_reverb_mix(0.0);
        Voice::Piano(piano)
    }
}

/// Handing a rebuilt instrument to the audio thread without allocating or
/// freeing on it.
///
/// Two things travel this way, and they are the same thing to the audio
/// thread: a preset with a construction-time `design` override is a different
/// instrument, and a different ENGINE is a different instrument too. Both
/// allocate to build — `Piano::new_with_preset` builds 88 key designs and
/// their modal banks, `LearnedPiano::new` parses the network and precomputes
/// all 88 key designs. The UI builds those voices; the task pool builds Audio
/// Units. Ownership passes through `incoming`; the audio thread
/// takes it, and passes the instrument it replaced back through `retired` for
/// the UI thread to dispose (Audio Units go to the pool). Neither slot ever
/// holds more than one instrument:
/// the audio side refuses to take a new one while it still owes the old one
/// back, and the UI side refuses to offer one until the previous handoff has
/// completed. Nothing is allocated, freed, or waited on inside the callback.
#[derive(Debug, Default)]
struct InstrumentHandoff {
    incoming: AtomicPtr<Voice>,
    retired: AtomicPtr<Voice>,
    /// UI-owned count of accepted offers not yet reclaimed or withdrawn.
    outstanding: AtomicU32,
}

// A constructed voice can move from the UI or a worker to the audio thread.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Voice>();
};

impl InstrumentHandoff {
    /// UI thread: offer a freshly built instrument. Handed back when the
    /// previous swap has not finished, so the caller can simply try again on
    /// the next frame with whatever the user has landed on by then.
    fn offer(&self, piano: Box<Voice>) -> Option<Box<Voice>> {
        if !self.retired.load(Ordering::Acquire).is_null() {
            return Some(piano);
        }
        let raw = Box::into_raw(piano);
        match self.incoming.compare_exchange(
            ptr::null_mut(),
            raw,
            Ordering::Release,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                self.outstanding.fetch_add(1, Ordering::Relaxed);
                None
            }
            // Safety: the exchange failed, so the pointer was never published
            // and this thread still owns it exclusively.
            Err(_) => Some(unsafe { Box::from_raw(raw) }),
        }
    }

    /// UI thread: take back the instrument the audio thread replaced, so it is
    /// dropped here rather than in the callback.
    fn reclaim(&self) -> Option<Box<Voice>> {
        let raw = self.retired.swap(ptr::null_mut(), Ordering::Acquire);
        // Safety: the audio thread published this pointer with Release and
        // never touches it again.
        (!raw.is_null()).then(|| {
            self.outstanding.fetch_sub(1, Ordering::Relaxed);
            unsafe { Box::from_raw(raw) }
        })
    }

    /// UI thread: revoke a superseded offer if audio has not adopted it yet.
    fn withdraw(&self) -> Option<Box<Voice>> {
        let raw = self.incoming.swap(ptr::null_mut(), Ordering::Acquire);
        (!raw.is_null()).then(|| {
            self.outstanding.fetch_sub(1, Ordering::Relaxed);
            // Safety: the atomic swap gave us exclusive ownership.
            unsafe { Box::from_raw(raw) }
        })
    }

    /// Audio thread: adopt an offered instrument, but only while the previous
    /// one has already been handed back.
    fn take(&self) -> Option<Box<Voice>> {
        if !self.retired.load(Ordering::Relaxed).is_null() {
            return None;
        }
        let raw = self.incoming.swap(ptr::null_mut(), Ordering::Acquire);
        // Safety: the UI thread published this pointer with Release and gave
        // up its own copy in the same operation.
        (!raw.is_null()).then(|| unsafe { Box::from_raw(raw) })
    }

    /// Audio thread: give a replaced instrument back to be dropped. Never
    /// frees anything here — the slot is guaranteed empty because `take` only
    /// hands an instrument over while it is.
    fn retire(&self, piano: Box<Voice>) {
        let previous = self.retired.swap(Box::into_raw(piano), Ordering::Release);
        debug_assert!(previous.is_null(), "the retired slot was not empty");
    }
}

impl Drop for InstrumentHandoff {
    fn drop(&mut self) {
        for slot in [&self.incoming, &self.retired] {
            let raw = slot.swap(ptr::null_mut(), Ordering::Acquire);
            if !raw.is_null() {
                // Safety: nothing else can reach these pointers any more.
                drop(unsafe { Box::from_raw(raw) });
            }
        }
    }
}

type Engine = PlaybackEngine<SamplerBackend, PENDING_CAPACITY, PART_CAPACITY>;

/// What the callback renders with: an engine, the plan it plays and the
/// stream generation it reports. Built on the UI thread and handed over whole.
struct Playing {
    engine: Engine,
    plan: Arc<PerformancePlan>,
    generation: u32,
}

/// The UI's commands to the callback, in the order they were given.
enum ToAudio {
    /// Render with this from the next block on; the one it replaces goes back.
    Playing(Box<Playing>),
    Message(AudioMessage),
}

pub struct PlaybackBridge {
    commands: makepad_lock_free::Producer<ToAudio>,
    /// Engines the callback replaced, dropped here.
    retired: makepad_lock_free::Consumer<Box<Playing>>,
    /// The callback's state until `install_audio_output` moves it there.
    audio: Option<AudioSide>,
    /// Commands the ring had no room for, sent first, in order, as it drains.
    held: VecDeque<ToAudio>,
    /// The current plan has not been sent with an engine yet.
    plan_unsent: bool,
    generation: u32,
    clock: Arc<AtomicAudioClock>,
    mixer: Arc<PartMixer<PART_CAPACITY>>,
    plan: Arc<PerformancePlan>,
    device_sample: Arc<AtomicU64>,
    device_rate_bits: Arc<AtomicU32>,
    host_offset_ns: Arc<AtomicI64>,
    audition: AuditionController<16>,
    scrub: ScrubController<16>,
    sequence: u32,
    next_metronome_quarter: Option<f64>,
    sound: Arc<SharedSound>,
    instrument: Arc<InstrumentHandoff>,
    /// A rebuilt instrument waiting for the audio thread to have room for it.
    pending_instrument: Option<Box<Voice>>,
    pending_audio_unit: Option<usize>,
    pending_built: Option<(ScoreEngine, usize)>,
    pool: Option<TaskPool>,
    load_request: u64,
    loads: Vec<AudioUnitLoad>,
    load_outcome: Option<InstrumentLoad>,
    /// Kept for retry if the pool refuses a disposal during shutdown.
    deferred_disposals: Vec<Box<Voice>>,
    /// What the instrument is actually putting out, written by the audio
    /// thread once per rendered span. One relaxed store; read for display.
    peak: Arc<AtomicU32>,
}

impl PlaybackBridge {
    pub fn new(score: &Score, bpm: f64, count_in: bool) -> Self {
        let (commands, audio_commands) = makepad_lock_free::ring(MESSAGE_CAPACITY);
        // Each command retires at most one engine, and one more is rendering:
        // the callback's push back can never find this ring full.
        let (audio_retired, retired) = makepad_lock_free::ring(MESSAGE_CAPACITY + 1);
        let clock = Arc::new(AtomicAudioClock::new());
        let mixer = Arc::new(PartMixer::new());
        let device_sample = Arc::new(AtomicU64::new(0));
        // Zero until a callback reports the device's rate.
        let device_rate_bits = Arc::new(AtomicU32::new(0));
        let host_offset_ns = Arc::new(AtomicI64::new(0));
        Self {
            commands,
            retired,
            audio: Some(AudioSide {
                commands: audio_commands,
                retired: audio_retired,
                playing: None,
                refused: None,
                clock: clock.clone(),
                mixer: mixer.clone(),
                device_sample: device_sample.clone(),
                device_rate_bits: device_rate_bits.clone(),
                host_offset_ns: host_offset_ns.clone(),
                fallback_device_sample: 0,
                // Agent/test instances must never make sound on the user's machine.
                muted: std::env::var_os("MAKEPAD_SCORE_MUTE").is_some(),
                #[cfg(test)]
                received: 0,
            }),
            held: VecDeque::new(),
            plan_unsent: true,
            generation: 1,
            clock,
            mixer,
            plan: Arc::new(compile_plan(score, bpm, count_in)),
            device_sample,
            device_rate_bits,
            host_offset_ns,
            audition: AuditionController::new(),
            scrub: ScrubController::new(ScrubConfig::for_sample_rate(PLAN_RATE)),
            sequence: 1_000_000,
            next_metronome_quarter: None,
            sound: {
                let sound = Arc::new(SharedSound::default());
                sound.publish(SoundSettings::default());
                sound
            },
            instrument: Arc::new(InstrumentHandoff::default()),
            pending_instrument: None,
            pending_audio_unit: None,
            pending_built: None,
            pool: None,
            load_request: 0,
            loads: Vec::new(),
            load_outcome: None,
            deferred_disposals: Vec::new(),
            peak: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Hand the whole sound — instrument voicing, EQ, trim and room — to the
    /// audio thread. Lock-free by construction: the synth itself is owned by
    /// the callback and is never touched here.
    pub fn set_sound(&self, sound: SoundSettings) {
        self.sound.publish(sound);
    }

    /// Build the instrument a preset describes and hand it over.
    ///
    /// Only for presets whose `needs_rebuild()` is true: those change
    /// construction-time design (felt, scale, radiation, board) and cannot be
    /// reached with setters. Building costs well under a millisecond and
    /// happens here, on the UI thread; the audio thread only ever swaps a
    /// pointer and fades the instrument it replaced out. Publish the sound
    /// itself with [`Self::set_sound`] as usual — the new instrument adopts it
    /// on the block it arrives, so a slider the user had moved off the preset
    /// survives the swap.
    /// Publish a clock snapshot as if the audio thread had, so the rules the
    /// transport gates — hover audition, the page follow, the Play/Pause
    /// label — are testable without an audio device.
    #[cfg(test)]
    pub(crate) fn publish_clock_for_test(&self, snapshot: makepad_score_play::AudioClockSnapshot) {
        self.clock.publish(snapshot);
    }

    /// The audio device's sample rate, once a callback has reported one.
    /// `None` before the first block: an instrument built against a guessed
    /// rate is the wrong piano, so construction waits for the real number.
    pub fn device_rate(&self) -> Option<f32> {
        let rate = f32::from_bits(self.device_rate_bits.load(Ordering::Relaxed));
        (rate.is_finite() && rate >= 8_000.0).then_some(rate)
    }

    /// Build a fresh instrument for this engine and preset and hand it over.
    ///
    /// Changing engine goes through exactly the same path as changing to a
    /// preset that needs a rebuild: built here on the UI thread, adopted by
    /// the audio thread, and crossfaded over `INSTRUMENT_FADE_FRAMES` against
    /// the one it replaces — so switching engine mid-phrase is a dissolve,
    /// not a cut, and never a panic.
    pub fn rebuild_instrument(&mut self, engine: ScoreEngine, preset: &PianoPreset) {
        self.begin_instrument_request();
        let rate = self.device_rate().unwrap_or(PLAN_RATE as f32).clamp(8_000.0, 192_000.0);
        self.pending_instrument = Some(build_engine(engine, rate, preset));
        let _ = self.service_instrument();
    }

    fn begin_instrument_request(&mut self) {
        self.load_request = self.load_request.wrapping_add(1);
        self.load_outcome = None;
        self.pending_audio_unit = None;
        self.pending_built = None;
        if let Some(old) = self.pending_instrument.take() {
            self.dispose_voice(old);
        }
        if let Some(old) = self.instrument.withdraw() {
            self.dispose_voice(old);
        }
    }

    /// Plug-in construction can take seconds. Poll the pool's result on later
    /// frames; retain older handles so their results are disposed off the UI.
    pub fn rebuild_audio_unit(&mut self, index: usize) {
        self.begin_instrument_request();
        let rate = self.device_rate().unwrap_or(PLAN_RATE as f32);
        if let Some(pool) = &self.pool {
            match pool.submit(Class::Interactive, move || build_audio_unit(index, rate)) {
                Ok(task) => self.loads.push(AudioUnitLoad { request: self.load_request, index, built: None, task }),
                Err(error) => self.load_outcome = Some(InstrumentLoad::Failed(index, error.to_string())),
            }
        } else {
            // Tests and callers before audio installation have no runtime pool.
            self.accept_audio_unit(index, build_audio_unit(index, rate));
        }
    }

    /// Build one of our instruments whose construction is long (the
    /// waveguide grand designs 88 keys) on the task pool. The instrument
    /// that sounds now keeps sounding until the new one is ready; then the
    /// usual handoff crossfades to it. `service` reports the outcome.
    pub fn rebuild_instrument_on_pool(&mut self, engine: ScoreEngine, index: usize, preset: &'static PianoPreset) {
        self.begin_instrument_request();
        let rate = self.device_rate().unwrap_or(PLAN_RATE as f32).clamp(8_000.0, 192_000.0);
        if let Some(pool) = &self.pool {
            match pool.submit(Class::Interactive, move || Ok(build_engine(engine, rate, preset))) {
                Ok(task) => self.loads.push(AudioUnitLoad { request: self.load_request, index, built: Some(engine), task }),
                Err(error) => self.load_outcome = Some(InstrumentLoad::Unbuilt(engine, index, error.to_string())),
            }
        } else {
            // Tests and callers before audio installation have no runtime pool.
            self.pending_instrument = Some(build_engine(engine, rate, preset));
            self.pending_built = Some((engine, index));
        }
    }

    fn accept_audio_unit(&mut self, index: usize, result: Result<Box<Voice>, String>) {
        match result {
            Ok(voice) => {
                self.pending_instrument = Some(voice);
                self.pending_audio_unit = Some(index);
            }
            Err(error) => self.load_outcome = Some(InstrumentLoad::Failed(index, error)),
        }
    }

    fn dispose_voice(&mut self, voice: Box<Voice>) {
        let Some(pool) = &self.pool else { return drop(voice) };
        if matches!(&*voice, Voice::Piano(_) | Voice::Waveguide(_)) {
            return drop(voice);
        }
        // The mutex protects only an ownership transfer. Neither thread holds
        // it while unloading the plug-in, and a refused submit retains ownership.
        let pending = Arc::new(Mutex::new(Some(voice)));
        let work = pending.clone();
        match pool.submit(Class::Interactive, move || {
            let voice = work.lock().unwrap_or_else(|p| p.into_inner()).take();
            drop(voice);
        }) {
            Ok(task) => task.detach(),
            Err(error) => {
                log!("score audio unit disposal deferred: {error}");
                if let Some(voice) = pending.lock().unwrap_or_else(|p| p.into_inner()).take() {
                    self.deferred_disposals.push(voice);
                }
            }
        }
    }

    /// Reclaim replaced voices and poll construction without waiting. Only the
    /// latest request may reach the callback or report an outcome to the UI.
    pub fn service_instrument(&mut self) -> Option<InstrumentLoad> {
        let deferred = std::mem::take(&mut self.deferred_disposals);
        for voice in deferred {
            self.dispose_voice(voice);
        }
        if let Some(old) = self.instrument.reclaim() {
            self.dispose_voice(old);
        }
        let mut at = 0;
        while at < self.loads.len() {
            let Some(result) = self.loads[at].task.try_take() else {
                at += 1;
                continue;
            };
            let load = self.loads.swap_remove(at);
            let result = result.map_err(|error| error.to_string()).and_then(|result| result);
            if load.request == self.load_request {
                match (load.built, result) {
                    (None, result) => self.accept_audio_unit(load.index, result),
                    (Some(engine), Ok(voice)) => {
                        self.pending_instrument = Some(voice);
                        self.pending_built = Some((engine, load.index));
                    }
                    (Some(engine), Err(error)) => self.load_outcome = Some(InstrumentLoad::Unbuilt(engine, load.index, error)),
                }
            } else if let Ok(voice) = result {
                self.dispose_voice(voice);
            }
        }
        if let Some(voice) = self.pending_instrument.take() {
            self.pending_instrument = self.instrument.offer(voice);
            if self.pending_instrument.is_none() {
                if let Some(index) = self.pending_audio_unit.take() {
                    self.load_outcome = Some(InstrumentLoad::Loaded(index));
                }
                if let Some((engine, index)) = self.pending_built.take() {
                    self.load_outcome = Some(InstrumentLoad::Built(engine, index));
                }
            }
        }
        self.load_outcome.take()
    }

    /// A new plan goes to the callback with a fresh engine (the transport
    /// stops, as a new score should), ahead of anything still held.
    pub fn rebuild_plan(&mut self, score: &Score, bpm: f64, count_in: bool) {
        self.plan = Arc::new(compile_plan(score, bpm, count_in));
        self.plan_unsent = true;
        self.flush();
    }

    /// Once a frame: the instrument handoff, then the commands still held and
    /// the engines the callback gave back.
    pub fn service(&mut self) -> Option<InstrumentLoad> {
        let outcome = self.service_instrument();
        self.flush();
        outcome
    }

    /// Commands are waiting for room in the ring: keep frames coming so they
    /// go as soon as the callback drains it.
    /// Instrument loads also need frames until they report, and a handoff
    /// until the callback has given the replaced instrument back; the handoff
    /// only once a device runs, since nothing takes it before then.
    pub fn holds_commands(&self) -> bool {
        let handoff = self.pending_instrument.is_some() || self.instrument.outstanding.load(Ordering::Relaxed) != 0;
        (!self.held.is_empty() && !self.plan_unsent)
            || !self.loads.is_empty()
            || self.load_outcome.is_some()
            || (handoff && self.device_rate().is_some())
    }

    /// Send what is held, in order, as far as the ring has room. An engine for
    /// an unsent plan is built here once a callback has reported the device's
    /// rate and goes ahead of everything held: no message reaches the callback
    /// before it has an engine to give it to.
    fn flush(&mut self) {
        while let Some(old) = self.retired.pop() {
            drop(old);
        }
        if self.plan_unsent {
            let Some(rate) = self.device_rate() else { return };
            self.generation = self.generation.wrapping_add(1).max(1);
            let engine = Engine::new(SamplerBackend::new(
                rate,
                self.sound.clone(),
                self.instrument.clone(),
                self.peak.clone(),
            ));
            // An engine built for an older plan and never sent is superseded.
            self.held.retain(|command| !matches!(command, ToAudio::Playing(_)));
            self.held.push_front(ToAudio::Playing(Box::new(Playing {
                engine,
                plan: self.plan.clone(),
                generation: self.generation,
            })));
            self.plan_unsent = false;
        }
        while let Some(command) = self.held.pop_front() {
            if let Err(command) = self.commands.push(command) {
                self.held.push_front(command);
                return;
            }
        }
    }

    /// Queue a command behind anything held. The first one that finds the
    /// ring full is reported; it and the rest go on a later frame.
    fn send(&mut self, command: ToAudio) {
        let was_held = self.held.len();
        self.held.push_back(command);
        self.flush();
        if was_held == 0 && !self.held.is_empty() && !self.plan_unsent {
            log!(
                "score playback: the audio queue is full ({} commands); later ones are held and sent in order as the callback drains it",
                self.commands.capacity()
            );
        }
    }

    pub fn plan(&self) -> &PerformancePlan {
        &self.plan
    }

    /// How long the piece is, in quarters. The scrub bar's range: a fixed one
    /// puts the marker at a fraction of the wrong whole, which is a playhead
    /// that disagrees with the page.
    pub fn end_quarter(&self) -> f64 {
        let end = self
            .plan
            .tempo_map()
            .sample_to_quarter(self.plan.end_sample());
        if end.is_finite() && end > 1.0 {
            end
        } else {
            1.0
        }
    }

    pub fn clock_snapshot(&self) -> AudioClockSnapshot {
        self.clock.read()
    }

    pub fn display_position(&self) -> DisplayPosition {
        let ui_ns = (Cx::time_now().max(0.0) * 1_000_000_000.0) as i128;
        let host_ns = (ui_ns + i128::from(self.host_offset_ns.load(Ordering::Relaxed)))
            .clamp(0, i128::from(u64::MAX)) as u64;
        self.clock.read().estimate(&self.plan, host_ns)
    }

    pub fn current_device_sample(&self) -> u64 {
        self.device_sample.load(Ordering::Acquire)
    }

    pub fn play(&mut self) {
        self.push(AudioMessageKind::Play);
    }

    pub fn pause(&mut self) {
        self.push(AudioMessageKind::Pause);
    }

    pub fn stop(&mut self) {
        self.push(AudioMessageKind::Stop);
    }

    pub fn seek_quarter(&mut self, quarter: f64) {
        self.push(AudioMessageKind::Seek {
            timeline_sample: self.plan.score_quarter_to_sample(quarter.max(0.0)),
        });
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        let base = self.plan.tempo_map().initial_bpm().max(1.0);
        self.push(AudioMessageKind::SetTempoScale {
            scale: (bpm / base).clamp(0.05, 8.0),
        });
    }

    pub fn set_loop(&mut self, start_quarter: f64, end_quarter: f64, enabled: bool) {
        if let Some(range) = TransportLoop::from_score_range(&self.plan, start_quarter, end_quarter) {
            self.push(AudioMessageKind::SetLoop { range, enabled });
        }
    }

    pub fn set_part_mix(&self, part: usize, gain: f32, pan: f32, mute: bool, solo: bool) {
        let _ = self.mixer.set(
            part,
            makepad_score_play::PartMix {
                gain,
                pan,
                mute,
                solo,
            },
        );
    }

    pub fn audition(&mut self, part: u16, pitches: &[u8]) {
        let at = self.current_device_sample().saturating_add(64);
        if let Ok(batch) = self.audition.audition_default(at, part, pitches) {
            self.push_batch(batch);
        }
    }

    pub fn release_audition(&mut self) {
        let at = self.current_device_sample().saturating_add(64);
        let batch = self.audition.release(at, 240);
        self.push_batch(batch);
    }

    pub fn scrub(&mut self, token: u64, part: u16, pitches: &[u8], speed: f32) {
        let at = self.current_device_sample().saturating_add(32);
        if let ScrubOutcome::Triggered(batch) = self.scrub.update(
            at,
            ScrubHit {
                token,
                part,
                pitches,
                cursor_units_per_second: speed,
            },
        ) {
            self.push_batch(batch);
        }
    }

    /// Keep a two-quarter lookahead of metronome clicks in the realtime
    /// queue. UI cadence only fills the queue; the click itself is stamped in
    /// device samples and therefore lands on the audio clock.
    pub fn service_metronome(&mut self, enabled: bool, bpm: f64) {
        let snapshot = self.clock.read();
        if !enabled || snapshot.state != makepad_score_play::PlaybackState::Playing {
            self.next_metronome_quarter = None;
            return;
        }
        let display = self.display_position();
        let current = display.score_quarter.max(0.0);
        let mut next = self
            .next_metronome_quarter
            .filter(|next| *next >= current - 0.05)
            .unwrap_or_else(|| current.ceil());
        let rate = f32::from_bits(self.device_rate_bits.load(Ordering::Relaxed))
            .clamp(8_000.0, 384_000.0) as f64;
        while next <= current + 2.0 {
            let delta_frames = ((next - current).max(0.0) * 60.0 / bpm.max(1.0) * rate).round() as u64;
            let beat = next.round() as i64;
            self.push_at(
                self.current_device_sample().saturating_add(delta_frames),
                AudioMessageKind::Synth(SynthEvent {
                    source: makepad_score_play::EventSource::Metronome,
                    kind: SynthEventKind::Click {
                        level: if beat.rem_euclid(4) == 0 {
                            makepad_score_play::ClickLevel::Bar
                        } else {
                            makepad_score_play::ClickLevel::Beat
                        },
                    },
                }),
            );
            next += 1.0;
        }
        self.next_metronome_quarter = Some(next);
    }

    fn push(&mut self, kind: AudioMessageKind) {
        self.push_at(self.current_device_sample().saturating_add(32), kind);
    }

    fn push_at(&mut self, at_device_sample: u64, kind: AudioMessageKind) {
        let message = AudioMessage {
            at_device_sample,
            sequence: self.sequence,
            kind,
        };
        self.sequence = self.sequence.wrapping_add(1).max(1_000_000);
        self.send(ToAudio::Message(message));
    }

    fn push_batch(&mut self, batch: EventBatch) {
        for message in batch.as_slice().iter().copied() {
            self.send(ToAudio::Message(message));
        }
    }

    /// Installs callback zero, once. Plans and engines reach it through the
    /// command ring (see `flush`).
    pub fn install_audio_output(&mut self, cx: &mut Cx) {
        self.pool = Some(cx.task_pool());
        let Some(mut audio) = self.audio.take() else { return };
        cx.audio_output(0, move |info, output| audio.render(info, output));
    }
}

/// The audio callback's state. It renders with the engine the UI sent last,
/// and allocates and frees nothing: engines come built and go back whole.
struct AudioSide {
    commands: makepad_lock_free::Consumer<ToAudio>,
    retired: makepad_lock_free::Producer<Box<Playing>>,
    playing: Option<Box<Playing>>,
    /// A message the engine's pending queue had no room for; it goes first on
    /// the next block, ahead of the ring.
    refused: Option<AudioMessage>,
    clock: Arc<AtomicAudioClock>,
    mixer: Arc<PartMixer<PART_CAPACITY>>,
    device_sample: Arc<AtomicU64>,
    device_rate_bits: Arc<AtomicU32>,
    host_offset_ns: Arc<AtomicI64>,
    fallback_device_sample: u64,
    muted: bool,
    /// Messages handed to an engine (tests count what arrived).
    #[cfg(test)]
    received: u64,
}

impl AudioSide {
    /// Adopt what the UI sent, in order, while the engine has room for it.
    fn take_commands(&mut self) {
        loop {
            if let Some(message) = self.refused.take() {
                let Some(playing) = self.playing.as_mut() else { return };
                if let Err(message) = playing.engine.queue(message) {
                    self.refused = Some(message);
                    return;
                }
                #[cfg(test)]
                {
                    self.received += 1;
                }
            }
            let Some(command) = self.commands.pop() else { return };
            match command {
                ToAudio::Playing(mut next) => {
                    if let Some(old) = &mut self.playing {
                        next.engine.backend_mut().carry_instrument_from(old.engine.backend_mut());
                    }
                    if let Some(old) = self.playing.replace(next) {
                        if let Err(old) = self.retired.push(old) {
                            // Unreachable: the retire ring holds more than the
                            // command ring can ever retire. Never free here.
                            debug_assert!(false, "the retire ring was full");
                            std::mem::forget(old);
                        }
                    }
                }
                ToAudio::Message(message) => {
                    // `flush` sends an engine before any message.
                    debug_assert!(self.playing.is_some(), "a message came before an engine");
                    self.refused = Some(message);
                }
            }
        }
    }

    fn render(&mut self, info: AudioInfo, output: &mut AudioBuffer) {
        output.zero();
        let rate = info.sample_rate.clamp(8_000.0, 384_000.0) as f32;
        self.device_rate_bits.store(rate.to_bits(), Ordering::Relaxed);
        self.take_commands();
        let frames = output.frame_count();
        let channel_count = output.channel_count();
        if frames == 0 || channel_count == 0 {
            return;
        }
        let first_device_sample = info
            .time
            .filter(|time| time.sample_time.is_finite() && time.sample_time >= 0.0)
            .map(|time| time.sample_time.round() as u64)
            .unwrap_or(self.fallback_device_sample);
        self.fallback_device_sample = first_device_sample.saturating_add(frames as u64);
        self.device_sample.store(self.fallback_device_sample, Ordering::Release);
        let ui_ns = (Cx::time_now().max(0.0) * 1_000_000_000.0) as u64;
        let presentation_host_ns = info.time.map_or(ui_ns, |time| time.host_time);
        let offset = i128::from(presentation_host_ns) - i128::from(ui_ns);
        self.host_offset_ns.store(
            offset.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
            Ordering::Relaxed,
        );
        // Silence until the UI's first engine arrives (it needs this rate).
        let Some(playing) = self.playing.as_mut() else { return };
        let Playing { engine, plan, generation } = &mut **playing;
        let context = RenderContext {
            first_device_sample,
            device_sample_rate: info.sample_rate.clamp(8_000.0, 384_000.0) as u32,
            first_presentation_host_ns: presentation_host_ns,
            frames,
            output_latency_frames: 0,
            stream_generation: *generation,
            clock_quality: if info.time.is_some() {
                ClockQuality::Exact
            } else {
                ClockQuality::Estimated
            },
        };
        if channel_count == 1 {
            let channel = &mut output.data[..frames];
            let mut channels: [&mut [f32]; 1] = [channel];
            let _ = engine.render(context, &mut channels, plan, &self.mixer, &self.clock);
        } else {
            let (left, remaining) = output.data.split_at_mut(frames);
            let right = &mut remaining[..frames];
            let mut channels: [&mut [f32]; 2] = [left, right];
            let _ = engine.render(context, &mut channels, plan, &self.mixer, &self.clock);
        }
        // A muted instance renders normally — so the transport clock, cursor
        // and engine state stay verifiable — and then discards the samples.
        if self.muted {
            output.zero();
        }
    }
}

struct SamplerBackend {
    sound: Arc<SharedSound>,
    sound_revision: u32,
    instrument: Arc<InstrumentHandoff>,
    peak: Arc<AtomicU32>,
    piano: Box<Voice>,
    /// The instrument a rebuild replaced, still ringing itself out.
    fading: Option<Box<Voice>>,
    /// One gain riding the instrument bus ahead of the room: the Concert
    /// Grand's own limiter, so that grand, already ridden to its ceiling,
    /// passes it untouched, and an engine trimmed up (`ScoreEngine::trim`)
    /// is held to the same level, room on or off.
    limiter: Limiter,
    /// The room both of them are heard in.
    room: Room,
    fade_frames_left: u32,
    fade_left: [f32; SCRATCH_FRAMES],
    fade_right: [f32; SCRATCH_FRAMES],
    piano_pending: [PianoTimedEvent; EVENT_CAPACITY],
    piano_pending_len: usize,
    /// note_id -> key, so a NoteOff (which carries no key) can reach the
    /// modelled piano. Fixed size: the audio thread never allocates.
    piano_keys: [(u32, u8); VOICE_CAPACITY],
    piano_keys_len: usize,
    piano_left: [f32; SCRATCH_FRAMES],
    piano_right: [f32; SCRATCH_FRAMES],
    sampler: Sampler<VOICE_CAPACITY>,
    pending: [TimedEvent; EVENT_CAPACITY],
    pending_len: usize,
    scratch_left: [f32; SCRATCH_FRAMES],
    scratch_right: [f32; SCRATCH_FRAMES],
    click_id: u32,
}

impl SamplerBackend {
    fn new(
        sample_rate: f32,
        sound: Arc<SharedSound>,
        instrument: Arc<InstrumentHandoff>,
        peak: Arc<AtomicU32>,
    ) -> Self {
        Self {
            sound,
            // Zero forces the first block to adopt whatever the UI published.
            sound_revision: 0,
            instrument,
            peak,
            piano: build_engine(
                ScoreEngine::Physical,
                sample_rate,
                &makepad_piano_model::PIANO_PRESETS
                    [crate::sound::default_preset_index(ScoreEngine::Physical)],
            ),
            fading: None,
            limiter: Limiter::new(sample_rate),
            room: Room::new(sample_rate),
            fade_frames_left: 0,
            fade_left: [0.0; SCRATCH_FRAMES],
            fade_right: [0.0; SCRATCH_FRAMES],
            piano_pending: [PianoTimedEvent {
                offset: 0,
                event: PianoEvent::AllSoundOff,
            }; EVENT_CAPACITY],
            piano_pending_len: 0,
            piano_keys: [(0, 0); VOICE_CAPACITY],
            piano_keys_len: 0,
            piano_left: [0.0; SCRATCH_FRAMES],
            piano_right: [0.0; SCRATCH_FRAMES],
            sampler: Sampler::new(sample_rate),
            pending: [TimedEvent {
                offset: 0,
                event: SamplerEvent::AllNotesOff,
            }; EVENT_CAPACITY],
            pending_len: 0,
            scratch_left: [0.0; SCRATCH_FRAMES],
            scratch_right: [0.0; SCRATCH_FRAMES],
            click_id: 0xf000_0000,
        }
    }

    fn remember_key(&mut self, id: u32, key: u8) {
        if let Some(slot) = self.piano_keys.get_mut(self.piano_keys_len) {
            *slot = (id, key);
            self.piano_keys_len += 1;
        }
    }

    fn take_key(&mut self, id: u32) -> Option<u8> {
        let index = self.piano_keys[..self.piano_keys_len]
            .iter()
            .position(|(slot, _)| *slot == id)?;
        let key = self.piano_keys[index].1;
        self.piano_keys_len -= 1;
        self.piano_keys[index] = self.piano_keys[self.piano_keys_len];
        Some(key)
    }

    /// One relaxed load per block unless the UI actually moved something.
    fn sync_sound(&mut self) {
        let revision = self.sound.revision.load(Ordering::Acquire);
        if revision == self.sound_revision {
            return;
        }
        self.sound_revision = revision;
        let sound = self.sound.read();
        self.apply_sound(sound);
    }

    /// The whole control surface in one place. Every setter here is documented
    /// safe between `process()` calls: plain scalars and coefficient updates,
    /// no allocation and no table rebuild.
    fn apply_sound(&mut self, sound: SoundSettings) {
        self.piano.apply_sound(&sound);
        self.room.apply(&sound.room);
    }

    /// A new score/tempo plan keeps the selected instrument, including any
    /// crossfade in progress. Only pointers/scalars move on the callback.
    fn carry_instrument_from(&mut self, old: &mut Self) {
        std::mem::swap(&mut self.piano, &mut old.piano);
        std::mem::swap(&mut self.fading, &mut old.fading);
        std::mem::swap(&mut self.fade_frames_left, &mut old.fade_frames_left);
        // The room holds the published settings the revision below vouches for.
        std::mem::swap(&mut self.room, &mut old.room);
        std::mem::swap(&mut self.limiter, &mut old.limiter);
        self.sound_revision = old.sound_revision;
        self.queue_piano(PianoEvent::AllSoundOff);
        self.room.fade_out();
    }

    /// Adopt an instrument the UI rebuilt for a preset that changes the
    /// physical design. The one it replaces keeps sounding while it fades, so
    /// a preset change during a phrase is a crossfade rather than a cut; new
    /// strikes go to the new instrument only.
    fn sync_instrument(&mut self) {
        if self.fading.is_some() {
            // Still owe the last one back; the offer waits one fade.
            return;
        }
        let Some(fresh) = self.instrument.take() else {
            return;
        };
        self.fading = Some(std::mem::replace(&mut self.piano, fresh));
        self.fade_frames_left = INSTRUMENT_FADE_FRAMES;
        // The rebuild carries the preset's own voicing; re-apply the
        // published sound so anything the user nudged off the preset survives.
        // Revision zero means nothing has been published at all — the cell is
        // still all zeros, and writing that over a built instrument would
        // silence every mechanism in it.
        let revision = self.sound.revision.load(Ordering::Acquire);
        if revision != 0 {
            let sound = self.sound.read();
            self.apply_sound(sound);
            self.sound_revision = revision;
        }
    }

    /// Publish what this span actually rendered. A peak with a fall-back, so
    /// the level reads as a level rather than flickering per block.
    fn meter(&mut self, count: usize) {
        let mut peak = f32::from_bits(self.peak.load(Ordering::Relaxed))
            - count as f32 * PEAK_FALL_PER_FRAME;
        if !(peak > 0.0) {
            peak = 0.0;
        }
        for index in 0..count {
            peak = peak
                .max(self.scratch_left[index].abs())
                .max(self.scratch_right[index].abs());
        }
        self.peak.store(peak.min(16.0).to_bits(), Ordering::Relaxed);
    }

    /// Mix the outgoing instrument's tail under the new one, on the
    /// instrument bus ahead of the room.
    fn render_fade(&mut self, count: usize) {
        let Some(mut fading) = self.fading.take() else {
            return;
        };
        fading.process(&[], &mut self.fade_left[..count], &mut self.fade_right[..count]);
        let span = INSTRUMENT_FADE_FRAMES as f32;
        for index in 0..count {
            let left = self.fade_frames_left.saturating_sub(index as u32) as f32;
            let gain = (left / span).clamp(0.0, 1.0);
            self.piano_left[index] += self.fade_left[index] * gain;
            self.piano_right[index] += self.fade_right[index] * gain;
        }
        self.fade_frames_left = self.fade_frames_left.saturating_sub(count as u32);
        if self.fade_frames_left == 0 {
            // Back to the UI thread, which is where it gets dropped.
            self.instrument.retire(fading);
        } else {
            self.fading = Some(fading);
        }
    }

    fn queue_piano(&mut self, event: PianoEvent) {
        if let Some(slot) = self.piano_pending.get_mut(self.piano_pending_len) {
            *slot = PianoTimedEvent { offset: 0, event };
            self.piano_pending_len += 1;
        }
    }

    fn queue(&mut self, event: SamplerEvent) {
        if let Some(slot) = self.pending.get_mut(self.pending_len) {
            *slot = TimedEvent { offset: 0, event };
            self.pending_len += 1;
        }
    }
}

impl SynthBackend for SamplerBackend {
    fn dispatch(&mut self, event: SynthEvent, _timing: SynthEventTiming) {
        match event.kind {
            SynthEventKind::NoteOn {
                note_id,
                key,
                velocity,
                ..
            } => {
                let id = sampler_note_id(event.source, note_id);
                let velocity = ((u32::from(velocity) * 127) / 65_535).max(1) as u8;
                self.remember_key(id, key);
                self.queue_piano(PianoEvent::NoteOn { key, velocity })
            }
            SynthEventKind::NoteOff { note_id, .. } => {
                let id = sampler_note_id(event.source, note_id);
                if let Some(key) = self.take_key(id) {
                    self.queue_piano(PianoEvent::NoteOff { key });
                }
                self.queue(SamplerEvent::NoteOff { note_id: id });
            }
            SynthEventKind::Click { level } => {
                self.click_id = self.click_id.wrapping_add(1).max(0xf000_0001);
                self.queue(SamplerEvent::NoteOn {
                    note_id: self.click_id,
                    parameters: metronome_click(matches!(level, makepad_score_play::ClickLevel::Bar)),
                });
            }
            SynthEventKind::AllNotesOff { .. } | SynthEventKind::TransportReset { .. } => {
                self.piano_keys_len = 0;
                self.queue_piano(PianoEvent::AllSoundOff);
                if matches!(event.kind, SynthEventKind::TransportReset { .. }) {
                    self.room.fade_out();
                }
                self.queue(SamplerEvent::AllNotesOff)
            }
            SynthEventKind::Pedal { value, .. } => {
                let amount = (value as f32 / 65_535.0).clamp(0.0, 1.0);
                self.queue_piano(PianoEvent::Sustain { value: amount });
            }
            SynthEventKind::Control { .. } | SynthEventKind::ExpressionRamp { .. } => {}
        }
    }

    fn render_range(&mut self, channels: &mut [&mut [f32]], range: Range<usize>) {
        self.sync_instrument();
        self.sync_sound();
        let mut start = range.start;
        let mut first = true;
        while start < range.end {
            let count = (range.end - start).min(SCRATCH_FRAMES);
            let events = if first {
                &self.pending[..self.pending_len]
            } else {
                &[]
            };
            // The sampler serves the metronome click, which is procedural
            // and never reads a sample source.
            self.sampler.render(
                &NoSamples,
                events,
                &mut self.scratch_left[..count],
                &mut self.scratch_right[..count],
            );
            let piano_events = if first {
                &self.piano_pending[..self.piano_pending_len]
            } else {
                &[][..]
            };
            self.piano.process(
                piano_events,
                &mut self.piano_left[..count],
                &mut self.piano_right[..count],
            );
            if self.fading.is_some() {
                for index in 0..count {
                    let gain = 1.0 - self.fade_frames_left.saturating_sub(index as u32) as f32 / INSTRUMENT_FADE_FRAMES as f32;
                    self.piano_left[index] *= gain;
                    self.piano_right[index] *= gain;
                }
            }
            self.render_fade(count);
            // The instrument bus: the limiter, the room (which hears the
            // instruments, not the metronome), then the knee, room on or off,
            // so a transient's first millisecond past the limiter and a long
            // tail under a loud chord bend toward full scale instead of
            // clipping.
            for (left, right) in self.piano_left[..count].iter_mut().zip(self.piano_right[..count].iter_mut()) {
                (*left, *right) = self.limiter.process(*left, *right);
            }
            self.room.process(&mut self.piano_left[..count], &mut self.piano_right[..count]);
            for sample in self.piano_left[..count].iter_mut().chain(self.piano_right[..count].iter_mut()) {
                *sample = soft_clip(*sample);
            }
            for index in 0..count {
                self.scratch_left[index] += self.piano_left[index];
                self.scratch_right[index] += self.piano_right[index];
            }
            self.meter(count);
            if first {
                self.pending_len = 0;
                self.piano_pending_len = 0;
                first = false;
            }
            let mono = channels.len() == 1;
            for (channel_index, channel) in channels.iter_mut().enumerate() {
                let source = if mono {
                    None
                } else if channel_index == 0 {
                    Some(&self.scratch_left[..count])
                } else {
                    Some(&self.scratch_right[..count])
                };
                for index in 0..count {
                    channel[start + index] += source.map_or(
                        (self.scratch_left[index] + self.scratch_right[index]) * 0.5,
                        |source| source[index],
                    );
                }
            }
            start += count;
        }
    }

    fn voice_count(&self) -> usize {
        self.sampler.active_voice_count()
    }
}

fn sampler_note_id(source: makepad_score_play::EventSource, note_id: u64) -> u32 {
    let source = match source {
        makepad_score_play::EventSource::Playback => 0x0000_0000,
        makepad_score_play::EventSource::Audition => 0x4000_0000,
        makepad_score_play::EventSource::Scrub => 0x8000_0000,
        makepad_score_play::EventSource::Metronome => 0xc000_0000,
    };
    source | (note_id as u32 & 0x3fff_ffff)
}

fn compile_plan(score: &Score, bpm: f64, count_in: bool) -> PerformancePlan {
    let part_indices: std::collections::BTreeMap<_, _> = score
        .parts
        .keys()
        .enumerate()
        .map(|(index, id)| (*id, index.min(u16::MAX as usize) as u16))
        .collect();
    let mut input = PlanInput::default();
    for voice in score.voices.values() {
        let Some(staff) = score.staves.get(&voice.staff) else { continue };
        let part = part_indices.get(&staff.part).copied().unwrap_or(0);
        for event in &voice.events {
            let Some(duration) = event.duration else { continue };
            for note in event.chord_notes() {
                let Some(pitch) = note.written_pitch else { continue };
                input.notes.push(NoteInput {
                    at_quarter: rational_f64(event.onset.0) * 4.0,
                    duration_quarters: rational_f64(duration.0) * 4.0,
                    part,
                    note_id: note.id.counter(),
                    key: crate::document::pitch_to_midi(pitch),
                    // A note that was PLAYED knows how hard. Only a note that
                    // was written has to be given a dynamic.
                    dynamic: note
                        .performance
                        .map_or(ENGRAVED_DYNAMIC, |played| {
                            f32::from(played.velocity) / 127.0
                        }),
                    articulation: articulation_for_event(&event.kind),
                    swing_eligible: true,
                });
            }
        }
    }
    // `measures` is keyed by generated id, so the map's last entry is the last measure
    // in key order, not in time. The plan has to span the whole piece: take the maximum
    // measure end, and never let it fall short of the material, or `Scheduler::compile`
    // rejects every note past it with `EventAfterEnd`.
    let measures_end_quarter = score
        .measures
        .values()
        .filter_map(|measure| measure.start.checked_add(measure.extent).ok())
        .map(|end| rational_f64(end.0) * 4.0)
        .fold(0.0_f64, f64::max);
    let notes_end_quarter = input
        .notes
        .iter()
        .map(|note| note.at_quarter + note.duration_quarters)
        .fold(0.0_f64, f64::max);
    input.end_quarter = measures_end_quarter.max(notes_end_quarter);
    // The damper pedal, as performed. Without it the dampers never lift and
    // every note stops the moment its written value runs out.
    for change in &score.maps.pedal {
        input.pedals.push(makepad_score_play::PedalInput {
            at_quarter: rational_f64(change.at.0) * 4.0,
            part: 0,
            // The controller's 0..127 in the scheduler's 0..65535.
            value: (u32::from(change.value.value) * 65_535 / 127) as u16,
        });
    }
    input
        .pedals
        .retain(|pedal| pedal.at_quarter <= input.end_quarter);
    // The score's OWN tempo map, when it has one. A performance is mostly
    // rubato — a recording of this prelude carries three hundred tempo changes
    // — and flattening it to one number is what makes a performance sound
    // typed. `bpm` stays the fallback and the transport's tempo control still
    // scales the whole map through `set_tempo`.
    let tempo = score_tempo_map(score).unwrap_or_else(|| {
        TempoMap::constant(PLAN_RATE, bpm.clamp(20.0, 400.0))
            .unwrap_or_else(|_| TempoMap::constant(PLAN_RATE, 120.0).expect("fallback tempo is valid"))
    });
    let count_in = count_in.then(|| CountInSpec {
        meter: Meter::new(4, 4, &[4]).expect("4/4 is valid"),
        bars: 1,
        subdivisions_per_unit: 0,
    });
    Scheduler::compile(
        input,
        tempo,
        ScheduleOptions {
            count_in,
            ..ScheduleOptions::default()
        },
    )
    .unwrap_or_else(|error| {
        // An empty plan silences the transport, so make the reason visible instead of
        // leaving a Play press with nothing to render.
        log!("score playback: {error}; falling back to an empty plan");
        Scheduler::compile(
            PlanInput::default(),
            TempoMap::constant(PLAN_RATE, 120.0).expect("fallback tempo is valid"),
            ScheduleOptions::default(),
        )
        .expect("empty playback plan is valid")
    })
}

/// What a note with no recorded velocity is played at: an ordinary mezzo.
const ENGRAVED_DYNAMIC: f32 = 0.68;

/// Notes sound their full written value.
///
/// `Articulation::Normal` shortens a note to 90% of its value, which is a
/// sensible default for a notation program deciding how to READ an engraving.
/// It is wrong here: a performance already says exactly how long each note was
/// held, and clipping every one of them by a tenth puts a silence between
/// every pair of adjacent notes — 549 of them in a prelude of running
/// sixteenths, which is precisely the sound of a machine playing.
fn articulation_for_event(_kind: &EventKind) -> PlayArticulation {
    PlayArticulation::Custom { gate: 1.0, attack: 1.0 }
}

/// The score's tempo map as the scheduler wants it. `None` when the score has
/// no tempo of its own, or when the points do not make a usable map.
fn score_tempo_map(score: &Score) -> Option<TempoMap> {
    let mut points: Vec<makepad_score_play::TempoPoint> = Vec::new();
    for change in &score.maps.tempo {
        let quarter = rational_f64(change.at.0) * 4.0;
        if !quarter.is_finite() || quarter < 0.0 {
            continue;
        }
        let (bpm, ramp) = match change.value {
            makepad_score::model::Tempo::Instant { quarters_per_minute } => {
                (rational_f64(quarters_per_minute), false)
            }
            makepad_score::model::Tempo::Ramp { from_quarters_per_minute, .. } => {
                (rational_f64(from_quarters_per_minute), true)
            }
        };
        if !(20.0..=400.0).contains(&bpm) {
            continue;
        }
        points.push(makepad_score_play::TempoPoint {
            quarter,
            bpm,
            ramp_to_next: ramp,
        });
    }
    if points.is_empty() {
        return None;
    }
    points.sort_by(|a, b| a.quarter.total_cmp(&b.quarter));
    points.dedup_by(|a, b| a.quarter == b.quarter);
    // The map must start at zero or the scheduler has no tempo for the pickup.
    if points[0].quarter > 0.0 {
        let first = points[0];
        points.insert(0, makepad_score_play::TempoPoint { quarter: 0.0, ..first });
    }
    TempoMap::new(PLAN_RATE, points).ok()
}

fn rational_f64(value: Rational) -> f64 {
    value.numerator() as f64 / value.denominator() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    /// Counts every heap call made on a thread that armed it (the tests that
    /// stand in for the audio callback); other threads' calls pass through.
    struct CountingAllocator;

    std::thread_local! {
        static COUNT_HEAP: Cell<bool> = const { Cell::new(false) };
    }

    static HEAP_CALLS: AtomicU64 = AtomicU64::new(0);

    fn count_heap_call() {
        if COUNT_HEAP.with(|armed| armed.get()) {
            HEAP_CALLS.fetch_add(1, Ordering::Relaxed);
        }
    }

    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            count_heap_call();
            // SAFETY: the exact request goes to the system allocator.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            count_heap_call();
            // SAFETY: the exact request goes to the system allocator.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            count_heap_call();
            // SAFETY: the pointer and layout came from this allocator.
            unsafe { System.realloc(pointer, layout, size) }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            count_heap_call();
            // SAFETY: the pointer and layout came from this allocator.
            unsafe { System.dealloc(pointer, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    /// One callback block of 512 frames at 48 kHz, counted: it must make no
    /// heap call at all.
    fn audio_block(audio: &mut AudioSide, output: &mut AudioBuffer, n: u64) {
        let info = AudioInfo {
            device_id: AudioDeviceId(LiveId(0)),
            time: Some(AudioTime {
                sample_time: (n * 512) as f64,
                host_time: n * 512 * 1_000_000_000 / 48_000,
                rate_scalar: 1.0,
            }),
            sample_rate: 48_000.0,
        };
        HEAP_CALLS.store(0, Ordering::Relaxed);
        COUNT_HEAP.with(|armed| armed.set(true));
        audio.render(info, output);
        COUNT_HEAP.with(|armed| armed.set(false));
        assert_eq!(HEAP_CALLS.load(Ordering::Relaxed), 0, "callback block {n} used the heap");
    }

    /// A ring full of commands (the callback parked or behind) loses none of
    /// them: the bridge holds what does not fit and sends it in order, a plan
    /// edit in the middle hands a fresh engine over and gets the old one back,
    /// and the callback neither allocates nor frees while it adopts, retires
    /// and renders.
    #[test]
    fn a_full_queue_loses_no_command_and_the_callback_touches_no_heap() {
        let document = crate::ScoreDocument::demo().unwrap();
        let mut bridge = PlaybackBridge::new(document.score(), 108.0, false);
        let mut audio = bridge.audio.take().unwrap();
        let mut output = AudioBuffer::new_with_size(512, 2);
        // The app clock's one-time setup (its launch configuration) runs on
        // the UI thread long before a callback, as it does in the app.
        let _ = Cx::time_now();
        // The first block finds no engine: it reports the rate and is silent.
        audio_block(&mut audio, &mut output, 0);
        assert!(bridge.device_rate().is_some());

        // The callback parks while the UI sends twice what the ring holds.
        let base = bridge.plan().tempo_map().initial_bpm().max(1.0);
        let mut sent = 0_u64;
        for i in 0..2 * MESSAGE_CAPACITY {
            bridge.set_tempo(base * (0.5 + i as f64 / (4 * MESSAGE_CAPACITY) as f64));
            sent += 1;
        }
        let last_bpm = base * 0.75;
        bridge.set_tempo(last_bpm);
        bridge.play();
        sent += 2;
        assert!(bridge.holds_commands(), "a full ring must hold, not drop");
        assert!(bridge.held.len() > MESSAGE_CAPACITY / 2);

        let first_plan = bridge.plan.clone();
        let mut n = 1;
        while n < 64 && (bridge.holds_commands() || !audio.commands.is_empty() || audio.refused.is_some()) {
            if n == 2 {
                // A plan edit while commands are held: its engine goes first.
                bridge.rebuild_plan(document.score(), 108.0, false);
            }
            audio_block(&mut audio, &mut output, n);
            let _ = bridge.service();
            n += 1;
        }
        for _ in 0..4 {
            audio_block(&mut audio, &mut output, n);
            let _ = bridge.service();
            n += 1;
        }

        assert_eq!(audio.received, sent, "every message reached an engine");
        assert!(bridge.held.is_empty() && audio.commands.is_empty());
        let playing = audio.playing.as_ref().unwrap();
        assert_eq!(playing.generation, bridge.generation, "the edited plan's engine renders");
        assert!(Arc::ptr_eq(&playing.plan, &bridge.plan));
        assert_eq!(playing.engine.state(), PlaybackState::Playing, "the last Play arrived");
        assert!((playing.engine.tempo_scale() - last_bpm / base).abs() < 1e-9);
        assert_eq!(bridge.clock_snapshot().state, PlaybackState::Playing);
        // The replaced engine came back and was dropped here, plan and all.
        assert_eq!(Arc::strong_count(&first_plan), 1, "the old engine was not handed back");
    }
    use makepad_piano_model::PIANO_PRESETS;
    use makepad_score::model::{Id, Measure};
    use makepad_score_play::PlaybackState;

    #[test]
    fn demo_plan_is_nonempty_and_audio_clock_based() {
        let document = crate::ScoreDocument::demo().unwrap();
        let bridge = PlaybackBridge::new(document.score(), 108.0, true);
        assert!(!bridge.plan().events().is_empty());
        assert_eq!(bridge.plan().tempo_map().sample_rate(), PLAN_RATE);
    }

    /// Importers hand back measures keyed by generated id, and those ids are not
    /// allocated in time order. Taking the map's last entry then truncates the plan to a
    /// few bars, every later note is rejected as `EventAfterEnd`, and the whole score
    /// falls back to an empty plan that the transport stops on immediately.
    #[test]
    fn plan_end_follows_the_last_measure_in_time_not_in_key_order() {
        let document = crate::ScoreDocument::demo().unwrap();
        let mut score = document.score().clone();
        let ordered: Vec<_> = score.measures.values().cloned().collect();
        assert!(ordered.len() > 4);
        let end_quarter = ordered
            .iter()
            .map(|measure| (rational_f64(measure.start.0) + rational_f64(measure.extent.0)) * 4.0)
            .fold(0.0_f64, f64::max);
        let sounding_notes = score
            .voices
            .values()
            .flat_map(|voice| voice.events.iter())
            .filter(|event| event.duration.is_some())
            .flat_map(|event| event.chord_notes())
            .filter(|note| note.written_pitch.is_some())
            .count();

        // Re-key the measures so the map's last entry is the *first* measure in time.
        let count = ordered.len() as u64;
        score.measures.clear();
        for (index, measure) in ordered.iter().enumerate() {
            let id = Id::new(0x5c0e, count - index as u64);
            score.measures.insert(
                id,
                Measure {
                    id,
                    ..measure.clone()
                },
            );
        }
        let key_order_last = score.measures.values().last().expect("a measure");
        assert_eq!(rational_f64(key_order_last.start.0), 0.0);

        let bridge = PlaybackBridge::new(&score, 108.0, false);
        let plan = bridge.plan();
        assert_eq!(plan.events().len(), sounding_notes * 2);
        assert_eq!(plan.end_sample(), plan.score_quarter_to_sample(end_quarter));
    }

    fn piano(voice: &Voice) -> &PianoEngine {
        let Voice::Piano(piano) = voice else { panic!("expected our piano") };
        piano
    }

    #[test]
    fn every_piano_event_maps_to_channel_zero_midi() {
        let single = |event, status, data1, data2| {
            assert_eq!(midi_for(event), [Some(MidiMessage { status, data1, data2 }), None, None]);
        };
        single(PianoEvent::NoteOn { key: 60, velocity: 96 }, 0x90, 60, 96);
        single(PianoEvent::NoteOn { key: 60, velocity: 0 }, 0x90, 60, 0);
        single(PianoEvent::NoteOff { key: 60 }, 0x80, 60, 0);
        for (value, expected) in [(0.0, 0), (0.5, 64), (1.0, 127), (-1.0, 0), (2.0, 127)] {
            single(PianoEvent::Sustain { value }, 0xb0, 64, expected);
        }
        for on in [false, true] {
            single(PianoEvent::SoftPedal { on }, 0xb0, 67, if on { 127 } else { 0 });
            single(PianoEvent::Sostenuto { on }, 0xb0, 66, if on { 127 } else { 0 });
        }
        assert_eq!(midi_for(PianoEvent::AllSoundOff), [
            Some(MidiMessage { status: 0xb0, data1: 64, data2: 0 }),
            Some(MidiMessage { status: 0xb0, data1: 120, data2: 0 }),
            Some(MidiMessage { status: 0xb0, data1: 123, data2: 0 }),
        ]);
    }

    /// A warmed-up instrument plays exactly as it was built: the note it
    /// played on its building thread leaves no sound behind and no pedal
    /// down (a silent block, then a note let go after 20 ms).
    #[test]
    fn a_warmed_up_instrument_plays_as_built() {
        for engine in [ScoreEngine::Waveguide, ScoreEngine::Physical] {
            let play = |voice: &mut Voice| -> Vec<f32> {
                let (mut left, mut right) = (vec![0.0f32; 256], vec![0.0f32; 256]);
                let mut out = Vec::new();
                for block in 0..12 {
                    let events: &[PianoTimedEvent] = match block {
                        1 => &[PianoTimedEvent { offset: 0, event: PianoEvent::NoteOn { key: 67, velocity: 80 } }],
                        5 => &[PianoTimedEvent { offset: 0, event: PianoEvent::NoteOff { key: 67 } }],
                        _ => &[],
                    };
                    voice.process(events, &mut left, &mut right);
                    out.extend(left.iter().chain(&right));
                }
                out
            };
            let warm = play(&mut build_engine(engine, PLAN_RATE as f32, &PIANO_PRESETS[0]));
            let cold = play(&mut construct_engine(engine, PLAN_RATE as f32, &PIANO_PRESETS[0]));
            let first = warm.iter().zip(&cold).position(|(a, b)| a != b);
            assert!(first.is_none(), "{engine:?} after its warm-up differs from one built cold from sample {first:?}");
        }
    }

    /// The waveguide grand is a voice like the others: built by the same
    /// function, played by the same events, shaped by the same two controls.
    #[test]
    fn the_waveguide_grand_plays_the_same_events_and_controls() {
        let mut voice = build_engine(ScoreEngine::Waveguide, PLAN_RATE as f32, &PIANO_PRESETS[0]);
        assert!(matches!(&*voice, Voice::Waveguide(_)));
        let level = |voice: &mut Voice, events: &[PianoTimedEvent], blocks: usize| {
            let (mut left, mut right) = (vec![0.0f32; 256], vec![0.0f32; 256]);
            let mut sum = 0.0f64;
            for block in 0..blocks {
                voice.process(if block == 0 { events } else { &[] }, &mut left, &mut right);
                assert!(left.iter().chain(&right).all(|x| x.is_finite()));
                sum += left.iter().chain(&right).map(|&x| (x as f64).powi(2)).sum::<f64>();
            }
            (sum / (blocks * 512) as f64).sqrt()
        };
        let on = |key| PianoTimedEvent { offset: 0, event: PianoEvent::NoteOn { key, velocity: 100 } };
        let plain = level(&mut voice, &[on(60)], 40);
        assert!(plain > 0.005, "middle C sounds: {plain}");
        // More events than one conversion buffer holds: none is lost.
        let mut burst = vec![PianoTimedEvent { offset: 0, event: PianoEvent::AllSoundOff }];
        burst.extend((0..70).map(|_| PianoTimedEvent { offset: 0, event: PianoEvent::Sustain { value: 1.0 } }));
        burst.push(on(72));
        level(&mut voice, &burst, 1);
        assert!(level(&mut voice, &[], 20) > 0.002, "the note after the burst sounds");
        // Master gain reaches it.
        level(&mut voice, &[PianoTimedEvent { offset: 0, event: PianoEvent::AllSoundOff }], 1);
        let mut sound = SoundSettings::default();
        sound.master_gain = 0.25;
        voice.apply_sound(&sound);
        let quiet = level(&mut voice, &[PianoTimedEvent { offset: 0, event: PianoEvent::Sustain { value: 0.0 } }, on(60)], 40);
        assert!(quiet < 0.5 * plain && quiet > 0.1 * plain, "a quarter of the gain: {quiet} vs {plain}");
    }

    /// A long build is a request like an Audio Unit's: it reports when its
    /// instrument is handed over, and a newer request supersedes it.
    #[test]
    fn a_pool_built_instrument_reports_once_it_is_handed_over() {
        let document = crate::ScoreDocument::demo().unwrap();
        let mut bridge = PlaybackBridge::new(document.score(), 108.0, false);
        // Each of our engines takes this path, the modal piano included.
        for engine in [ScoreEngine::Waveguide, ScoreEngine::Physical, ScoreEngine::Learned, ScoreEngine::Hybrid] {
            bridge.rebuild_instrument_on_pool(engine, 0, &PIANO_PRESETS[0]);
            let built = bridge.pending_instrument.as_deref().map(|voice| match voice {
                Voice::Piano(piano) => piano.kind() == engine.kind(),
                Voice::Waveguide(_) => engine == ScoreEngine::Waveguide,
                Voice::AudioUnit(_) => false,
            });
            assert_eq!(built, Some(true), "{engine:?} builds its own voice");
            assert_eq!(bridge.service_instrument(), Some(InstrumentLoad::Built(engine, 0)));
            assert_eq!(bridge.service_instrument(), None, "reported once");
        }
        // Superseded before it is serviced: only the newer one is reported.
        bridge.rebuild_instrument_on_pool(ScoreEngine::Waveguide, 0, &PIANO_PRESETS[0]);
        bridge.rebuild_instrument(ScoreEngine::Physical, &PIANO_PRESETS[0]);
        assert_eq!(bridge.service_instrument(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_system_audio_unit_plays_and_releases_piano_events() {
        let index = audio_units().iter().position(|entry| entry.info.description == makepad_audio_unit_host::ComponentDescription::DLS_SYNTH)
            .expect("the General MIDI synth ships with macOS");
        let mut sound = SoundSettings::default();
        sound.master_gain = 0.5;
        // One note, then 1.5 s later (with or without all sound off) the
        // loudest sample of the last block. The voices stop at once; the
        // synth's own reverb tail needs that long to fall well below them.
        let play = |release: bool| -> (f32, f32) {
            let mut voice = build_audio_unit(index, PLAN_RATE as f32).expect("load the system synth");
            voice.apply_sound(&sound);
            let Voice::AudioUnit(unit) = &*voice else { panic!("expected an Audio Unit") };
            assert_eq!(unit.gain, 0.5);
            let (mut left, mut right) = ([0.0; SCRATCH_FRAMES], [0.0; SCRATCH_FRAMES]);
            let block_peak = |left: &[f32], right: &[f32]| {
                assert!(left.iter().chain(right).all(|sample| sample.is_finite()), "the synth output must be finite");
                left.iter().chain(right).fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
            };
            voice.process(&[PianoTimedEvent { offset: 0, event: PianoEvent::NoteOn { key: 60, velocity: 100 } }], &mut left, &mut right);
            let mut note = block_peak(&left, &right);
            for _ in 0..4 {
                voice.process(&[], &mut left, &mut right);
                note = note.max(block_peak(&left, &right));
            }
            let events: &[PianoTimedEvent] = if release { &[PianoTimedEvent { offset: 0, event: PianoEvent::AllSoundOff }] } else { &[] };
            voice.process(events, &mut left, &mut right);
            for _ in 0..PLAN_RATE as usize * 3 / SCRATCH_FRAMES / 2 {
                voice.process(&[], &mut left, &mut right);
            }
            (note, block_peak(&left, &right))
        };
        let (note, held) = play(false);
        let (_, released) = play(true);
        assert!(note > 1.0e-3, "the synth must sound the note: {note}");
        // A held note keeps its level; a released one is down to the reverb
        // tail, about 20 dB under it on the system synth.
        assert!(released < 0.25 * held, "all sound off must release the synth: {released} against {held} held");
    }

    fn test_backend(sound: Arc<SharedSound>) -> SamplerBackend {
        SamplerBackend::new(
            PLAN_RATE as f32,
            sound,
            Arc::new(InstrumentHandoff::default()),
            Arc::new(AtomicU32::new(0)),
        )
    }

    /// The sound binding must reach the audio thread without a lock and
    /// without the UI ever touching the synth. Publish, then render: the
    /// backend has to have adopted the new values by the end of the first
    /// block, and the piano has to be holding them.
    #[test]
    fn sound_settings_reach_the_audio_thread_through_the_shared_cell() {
        let shared = Arc::new(SharedSound::default());
        let mut wanted = SoundSettings::default();
        wanted.room = RoomSettings {
            preset: ReverbPreset::Cathedral,
            mix: 0.75,
            perspective: Perspective::Audience,
        };
        wanted.voicing.sympathetic = 2.4;
        wanted.voicing.knock = 0.25;
        wanted.eq_shelf_db = -6.0;
        wanted.eq_shelf_hz = 4000.0;
        wanted.eq_bell_hz = 900.0;
        wanted.eq_bell_db = 3.5;
        wanted.eq_bell_q = 2.0;
        wanted.tone_bass_db = 2.0;
        wanted.tone_treble_db = -1.5;
        wanted.master_gain = 1.4;
        wanted.early_reflections = 1.1;
        shared.publish(wanted);

        let read = shared.read();
        assert_eq!(read.room, wanted.room);
        assert_eq!(read.voicing, wanted.voicing);
        assert_eq!(read.master_gain, wanted.master_gain);

        let mut backend = test_backend(shared.clone());
        let mut left = vec![0.0_f32; 64];
        let mut right = vec![0.0_f32; 64];
        let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
        backend.render_range(&mut channels, 0..64);
        let adopted = backend.sound_revision;
        assert!(adopted > 0);
        // Not just adopted: actually applied to the instrument.
        assert_eq!(piano(&backend.piano).voicing(), wanted.voicing);
        // The room is the bus's, and the instrument under it stays dry.
        assert_eq!(backend.room.mix, wanted.room.mix);
        assert_eq!(backend.room.reverb.params(), wanted.room.preset.params());
        assert_eq!(piano(&backend.piano).reverb_mix(), 0.0);
        assert_eq!(piano(&backend.piano).perspective(), wanted.room.perspective);
        assert_eq!(piano(&backend.piano).early_reflection_level(), wanted.early_reflections);
        assert_eq!(piano(&backend.piano).eq_shelf(), (wanted.eq_shelf_db, wanted.eq_shelf_hz));
        assert_eq!(
            piano(&backend.piano).eq_bell(),
            (wanted.eq_bell_hz, wanted.eq_bell_db, wanted.eq_bell_q)
        );
        assert_eq!(
            piano(&backend.piano).tone(),
            (wanted.tone_bass_db, wanted.tone_treble_db)
        );
        assert_eq!(piano(&backend.piano).master_gain(), wanted.master_gain);

        // No further work while nothing moves.
        backend.render_range(&mut channels, 0..64);
        assert_eq!(backend.sound_revision, adopted);
        shared.publish(SoundSettings::default());
        backend.render_range(&mut channels, 0..64);
        assert!(backend.sound_revision > adopted);
        assert_eq!(piano(&backend.piano).voicing(), SoundSettings::default().voicing);
    }

    /// A rebuild preset is a different instrument. It is built on this thread,
    /// swapped in by the audio thread, and the one it replaced comes back here
    /// to be dropped — the callback allocates and frees nothing.
    #[test]
    fn a_rebuilt_instrument_is_handed_over_and_the_old_one_comes_back() {
        let shared = Arc::new(SharedSound::default());
        let handoff = Arc::new(InstrumentHandoff::default());
        let mut backend = SamplerBackend::new(
            PLAN_RATE as f32,
            shared.clone(),
            handoff.clone(),
            Arc::new(AtomicU32::new(0)),
        );
        // Changing instrument is the one thing that still rebuilds: the
        // electric voice is a different engine and cannot be reached with a
        // setter. UI side: publish its sound, build it, offer it.
        let electric = SoundSettings::from_preset(
            ScoreEngine::Learned,
            0,
            RoomSettings::default(),
        );
        shared.publish(electric);
        assert!(handoff
            .offer(build_engine(
                ScoreEngine::Learned,
                PLAN_RATE as f32,
                &PIANO_PRESETS[0],
            ))
            .is_none());
        // A second offer while the first is in flight comes straight back.
        assert!(handoff
            .offer(build_engine(ScoreEngine::Physical, PLAN_RATE as f32, &PIANO_PRESETS[0]))
            .is_some());

        let mut left = vec![0.0_f32; 512];
        let mut right = vec![0.0_f32; 512];
        {
            let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
            backend.render_range(&mut channels, 0..512);
        }
        // The swap happened and the outgoing instrument is fading, not gone.
        assert!(backend.fading.is_some());
        assert!(handoff.reclaim().is_none(), "nothing is handed back mid-fade");
        assert_eq!(piano(&backend.piano).kind(), makepad_piano_model::learned::EngineKind::Learned);
        // It arrives dry, in the room the published sound sets.
        assert_eq!(piano(&backend.piano).reverb_mix(), 0.0);
        assert_eq!(backend.room.mix, electric.room.mix);

        // Render the fade out.
        for _ in 0..8 {
            let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
            backend.render_range(&mut channels, 0..512);
        }
        assert!(backend.fading.is_none(), "the fade finished");
        assert!(
            handoff.reclaim().is_some(),
            "the replaced instrument must come back to be dropped here"
        );
    }

    /// Swapping the instrument mid-phrase must not cut the sound off: the
    /// replaced piano keeps ringing under the new one for the fade.
    #[test]
    fn a_preset_swap_during_a_ringing_note_fades_instead_of_clicking() {
        let shared = Arc::new(SharedSound::default());
        let handoff = Arc::new(InstrumentHandoff::default());
        let meter = Arc::new(AtomicU32::new(0));
        let mut backend = SamplerBackend::new(
            PLAN_RATE as f32,
            shared.clone(),
            handoff.clone(),
            meter.clone(),
        );
        backend.dispatch(
            SynthEvent {
                source: makepad_score_play::EventSource::Playback,
                kind: SynthEventKind::NoteOn {
                    note_id: 1,
                    key: 48,
                    velocity: 60_000,
                    part: 0,
                    attack: 32_768,
                },
            },
            SynthEventTiming {
                block_offset: 0,
                device_sample: 0,
                timeline_sample: Some(0),
                timeline_rate_q32: 1 << 32,
            },
        );
        let mut left = vec![0.0_f32; 512];
        let mut right = vec![0.0_f32; 512];
        let mut before = 0.0_f32;
        for _ in 0..8 {
            left.fill(0.0);
            right.fill(0.0);
            let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
            backend.render_range(&mut channels, 0..512);
            before = left.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        }
        assert!(before > 1.0e-4, "the note is sounding before the swap");
        // The meter is a real reading of what the instrument rendered.
        assert!(f32::from_bits(meter.load(Ordering::Relaxed)) > 1.0e-4);

        assert!(handoff
            .offer(build_engine(ScoreEngine::Physical, PLAN_RATE as f32, &PIANO_PRESETS[0]))
            .is_none());
        left.fill(0.0);
        right.fill(0.0);
        let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
        backend.render_range(&mut channels, 0..512);
        let after = left.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        // The new instrument has no voices, so anything at all in this block
        // is the old one still ringing through the crossfade.
        assert!(
            after > before * 0.25,
            "the swap cut the sound ({before} -> {after})"
        );
        // And the first sample of the block is continuous with the last of the
        // previous one: no step, which is what a click is.
        assert!(left[0].abs() < 1.0, "the swap produced a full-scale step");
    }

    /// The Sound panel's room and Reverb amount reach what the instrument
    /// puts out, whichever instrument it is: one short note, released, and
    /// the energy of its tail (0.15 to 0.35 s after the release) rendered
    /// through the audio thread's own path with the panel's settings
    /// published as the UI publishes them. Checked on the waveguide grand and
    /// on the modal piano.
    #[test]
    fn the_panel_room_reaches_the_tail_of_every_instrument() {
        const RATE: usize = PLAN_RATE as usize;
        const BLOCK: usize = 512;
        let timing = SynthEventTiming {
            block_offset: 0,
            device_sample: 0,
            timeline_sample: Some(0),
            timeline_rate_q32: 1 << 32,
        };
        let event = |kind| SynthEvent { source: makepad_score_play::EventSource::Playback, kind };
        let db = |over: f64, under: f64| 10.0 * (over / under.max(1e-30)).log10();
        let mut heard = Vec::new();
        // The backend starts on the modal piano; the waveguide grand then
        // takes its place, the way an instrument change hands it over.
        let shared = Arc::new(SharedSound::default());
        let mut backend = test_backend(shared.clone());
        for engine in [ScoreEngine::Physical, ScoreEngine::Waveguide] {
            if engine == ScoreEngine::Waveguide {
                backend.piano = build_engine(engine, PLAN_RATE as f32, &PIANO_PRESETS[0]);
            }
            // (energy while held, energy of the tail) of one note.
            let mut note = |room: ReverbPreset, amount: f32| -> (f64, f64) {
                let (mut left, mut right) = (vec![0.0_f32; BLOCK], vec![0.0_f32; BLOCK]);
                // A jump empties the room of the last note's tail.
                backend.dispatch(event(SynthEventKind::TransportReset { crossfade_frames: 0 }), timing);
                for _ in 0..RATE / 20 / BLOCK + 1 {
                    let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
                    backend.render_range(&mut channels, 0..BLOCK);
                }
                // What the panel's two room controls do to the settings.
                let mut sound = SoundSettings { engine, ..SoundSettings::default() };
                sound.room.preset = room;
                crate::sound::SoundParam::Reverb.set(&mut sound, amount);
                shared.publish(sound);
                backend.dispatch(event(SynthEventKind::NoteOn { note_id: 1, key: 60, velocity: 52_000, part: 0, attack: 32_768 }), timing);
                let (mut held, mut tail) = (0.0_f64, 0.0_f64);
                // 50 ms held, then 0.35 s after the release.
                let release = RATE * 5 / 100 / BLOCK;
                for block in 0..(RATE * 40 / 100) / BLOCK {
                    if block == release {
                        backend.dispatch(event(SynthEventKind::NoteOff { note_id: 1, part: 0, release: 32_768 }), timing);
                    }
                    left.fill(0.0);
                    right.fill(0.0);
                    let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
                    backend.render_range(&mut channels, 0..BLOCK);
                    assert!(left.iter().chain(&right).all(|x| x.is_finite()));
                    let energy = left.iter().chain(&right).map(|&x| (x as f64).powi(2)).sum::<f64>();
                    if block < release {
                        held += energy;
                    } else if block * BLOCK >= RATE * 20 / 100 {
                        tail += energy;
                    }
                }
                (held, tail)
            };
            let (held, dry) = note(ReverbPreset::Cathedral, 0.0);
            let (_, cathedral) = note(ReverbPreset::Cathedral, 0.3);
            let (_, practice) = note(ReverbPreset::PracticeRoom, 0.3);
            eprintln!(
                "{engine:?}: tail dry {:+.1} dB re the held note; at 30% cathedral {:+.1} dB, practice room {:+.1} dB",
                db(dry, held),
                db(cathedral, held),
                db(practice, held)
            );
            heard.push((engine, dry, cathedral, practice));
        }
        for (engine, dry, cathedral, practice) in heard {
            assert!(dry > 0.0, "{engine:?}: the released note leaves something to hear");
            assert!(
                db(cathedral, dry) > 10.0,
                "{engine:?}: Reverb at 30% must add a tail ({:+.1} dB over dry)",
                db(cathedral, dry)
            );
            assert!(
                db(cathedral, practice) > 6.0,
                "{engine:?}: the room choice must reach the tail (cathedral {:+.1} dB over the practice room)",
                db(cathedral, practice)
            );
        }
    }

    /// A stop leaves the room's tail ringing, the way a hall does, and a jump
    /// fades it out within a few blocks; neither cuts it. A chord in a wet
    /// cathedral, stopped at 0.3 s and jumped at 0.6 s: after the stop the
    /// tail still sounds (0.05 to 0.15 s after), after the jump nothing does
    /// (0.05 to 0.15 s after), and at neither does the output's sharpness
    /// (its largest second difference) rise above what the 50 ms before had.
    /// On the waveguide grand, whose own stop is a fade.
    #[test]
    fn a_wet_room_rings_out_on_stop_and_fades_on_a_jump() {
        const RATE: usize = PLAN_RATE as usize;
        const BLOCK: usize = 512;
        let timing = SynthEventTiming { block_offset: 0, device_sample: 0, timeline_sample: Some(0), timeline_rate_q32: 1 << 32 };
        let event = |kind| SynthEvent { source: makepad_score_play::EventSource::Playback, kind };
        let shared = Arc::new(SharedSound::default());
        let mut sound = SoundSettings { engine: ScoreEngine::Waveguide, ..SoundSettings::default() };
        sound.room.preset = ReverbPreset::Cathedral;
        crate::sound::SoundParam::Reverb.set(&mut sound, 0.5);
        shared.publish(sound);
        let mut backend = test_backend(shared);
        backend.piano = build_engine(ScoreEngine::Waveguide, PLAN_RATE as f32, &PIANO_PRESETS[0]);
        for (id, key) in [48, 60, 64, 67, 72].into_iter().enumerate() {
            backend.dispatch(event(SynthEventKind::NoteOn { note_id: id as u64 + 1, key, velocity: 58_000, part: 0, attack: 32_768 }), timing);
        }
        let (stop, jump, end) = (RATE * 3 / 10 / BLOCK, RATE * 6 / 10 / BLOCK, RATE * 9 / 10 / BLOCK);
        let mut out = Vec::new();
        let (mut left, mut right) = (vec![0.0_f32; BLOCK], vec![0.0_f32; BLOCK]);
        for block in 0..end {
            if block == stop {
                backend.dispatch(event(SynthEventKind::AllNotesOff { source: makepad_score_play::EventSource::Playback, release_frames: 0 }), timing);
            }
            if block == jump {
                backend.dispatch(event(SynthEventKind::TransportReset { crossfade_frames: 0 }), timing);
            }
            left.fill(0.0);
            right.fill(0.0);
            let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
            backend.render_range(&mut channels, 0..BLOCK);
            out.extend(left.iter().zip(&right).map(|(&l, &r)| [l, r]));
        }
        let span = |from: usize, secs: f64| from * BLOCK..from * BLOCK + (secs * RATE as f64) as usize;
        let energy = |range: std::ops::Range<usize>| out[range].iter().map(|f| f.iter().map(|&x| (x as f64).powi(2)).sum::<f64>()).sum::<f64>();
        let sharp = |range: std::ops::Range<usize>| (range.start.max(2)..range.end).map(|i| (0..2).map(|c| (out[i][c] - 2.0 * out[i - 1][c] + out[i - 2][c]).abs()).fold(0.0_f32, f32::max)).fold(0.0_f32, f32::max);
        let before = |at: usize| (at * BLOCK - RATE / 20)..at * BLOCK;
        let after = |at: usize, from: f64, to: f64| at * BLOCK + (from * RATE as f64) as usize..at * BLOCK + (to * RATE as f64) as usize;
        let held = energy(before(stop));
        let ringing = energy(after(stop, 0.05, 0.15));
        let left_over = energy(after(jump, 0.05, 0.15));
        let db = |e: f64| 10.0 * (e / held).max(1e-30).log10();
        eprintln!(
            "stop: tail {:+.1} dB re the chord, sharpness {:.4} after / {:.4} before; jump: {:+.1} dB left, sharpness {:.4} / {:.4}",
            db(ringing),
            sharp(span(stop, 0.02)),
            sharp(before(stop)),
            db(left_over),
            sharp(span(jump, 0.04)),
            sharp(before(jump))
        );
        assert!(db(ringing) > -30.0, "a stop must leave the room's tail ringing ({:+.1} dB re the chord)", db(ringing));
        assert!(sharp(span(stop, 0.02)) <= sharp(before(stop)), "a stop clicks");
        assert!(db(left_over) < -120.0, "a jump must take the old tail away ({:+.1} dB left)", db(left_over));
        assert!(sharp(span(jump, 0.04)) <= sharp(before(jump)), "a jump clicks");
    }

    /// The engines sit side by side on Score's bus. Each is calibrated on
    /// its own terms: the waveguide grand keeps 10.7 dB of headroom under its
    /// limiter for a touch that spans 18 dB, while the Concert Grand's own
    /// limiter rides its loud end. Through the bus, a mezzo-forte chord of the
    /// waveguide sits some 5.6 dB under the Concert Grand's (it was 11.6), a
    /// v110 chord of it leaves no hotter than the Concert Grand's leaves that
    /// grand's own limiter, room on or off, and a v127 one never reaches full
    /// scale. The bus does not touch the Concert Grand's mezzo forte.
    #[test]
    fn the_engines_sit_side_by_side_on_the_bus() {
        const BLOCK: usize = 512;
        const KEYS: [u8; 7] = [48, 52, 55, 60, 64, 67, 72];
        let blocks = PLAN_RATE as usize * 3 / 5 / BLOCK;
        let timing = SynthEventTiming { block_offset: 0, device_sample: 0, timeline_sample: Some(0), timeline_rate_q32: 1 << 32 };
        let preset = |engine| &PIANO_PRESETS[crate::sound::default_preset_index(engine)];
        // Through the bus: the backend Score plays with.
        let bus = |engine: ScoreEngine, velocity: u8, mix: f32| -> Vec<f32> {
            let shared = Arc::new(SharedSound::default());
            let mut sound = SoundSettings { engine, ..SoundSettings::default() };
            sound.room.mix = mix;
            shared.publish(sound);
            let mut backend = test_backend(shared);
            backend.piano = build_engine(engine, PLAN_RATE as f32, preset(engine));
            let velocity = (velocity as u32 * 65_535).div_ceil(127) as u16;
            for (id, key) in KEYS.into_iter().enumerate() {
                let kind = SynthEventKind::NoteOn { note_id: id as u64 + 1, key, velocity, part: 0, attack: 32_768 };
                backend.dispatch(SynthEvent { source: makepad_score_play::EventSource::Playback, kind }, timing);
            }
            let (mut left, mut right, mut out) = (vec![0.0_f32; BLOCK], vec![0.0_f32; BLOCK], Vec::new());
            for _ in 0..blocks {
                left.fill(0.0);
                right.fill(0.0);
                backend.render_range(&mut [&mut left[..], &mut right[..]], 0..BLOCK);
                out.extend(left.iter().chain(&right));
            }
            out
        };
        // The instrument alone, as its own limiter leaves it.
        let alone = |engine: ScoreEngine, velocity: u8| -> Vec<f32> {
            let mut voice = build_engine(engine, PLAN_RATE as f32, preset(engine));
            voice.apply_sound(&SoundSettings { engine, ..SoundSettings::default() });
            let events: Vec<PianoTimedEvent> = KEYS.iter().map(|&key| PianoTimedEvent { offset: 0, event: PianoEvent::NoteOn { key, velocity } }).collect();
            let (mut left, mut right, mut out) = (vec![0.0_f32; BLOCK], vec![0.0_f32; BLOCK], Vec::new());
            for block in 0..blocks {
                voice.process(if block == 0 { &events } else { &[] }, &mut left, &mut right);
                out.extend(left.iter().chain(&right));
            }
            out
        };
        let rms = |x: &[f32]| 10.0 * (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).log10();
        let peak = |x: &[f32]| 20.0 * x.iter().fold(0.0_f32, |m, &v| m.max(v.abs())).log10();
        let over = |x: &[f32]| 100.0 * x.iter().filter(|v| v.abs() > 0.78).count() as f64 / x.len() as f64;
        let grand_mf = bus(ScoreEngine::Physical, 67, 0.0);
        let mf = rms(&bus(ScoreEngine::Waveguide, 67, 0.0)) - rms(&grand_mf);
        let grand = alone(ScoreEngine::Physical, 110);
        let (loud, wet, loudest) = (bus(ScoreEngine::Waveguide, 110, 0.0), bus(ScoreEngine::Waveguide, 110, 0.25), bus(ScoreEngine::Waveguide, 127, 0.0));
        let untouched = rms(&grand_mf) - rms(&alone(ScoreEngine::Physical, 67));
        eprintln!(
            "v67 waveguide {mf:+.2} dB re the Concert Grand (bus changes the grand's by {untouched:+.3} dB); v110 peak / over the knee: Concert Grand alone {:+.2} dBFS {:.3} %, waveguide room off {:+.2} {:.3} %, room on {:+.2} {:.3} %; v127 room off {:+.2} {:.3} %",
            peak(&grand), over(&grand), peak(&loud), over(&loud), peak(&wet), over(&wet), peak(&loudest), over(&loudest)
        );
        assert!((-8.0..-3.0).contains(&mf), "a v67 chord of the waveguide grand sits {mf:+.1} dB against the Concert Grand's");
        assert!(untouched.abs() < 0.1, "the bus moves the Concert Grand's v67 chord by {untouched:+.2} dB");
        for (x, room) in [(&loud, "off"), (&wet, "on")] {
            assert!(peak(x) <= peak(&grand) + 0.3 && over(x) <= over(&grand) + 0.01, "a v110 chord of the waveguide grand, room {room}: {:+.2} dBFS, {:.3} % over the knee; the Concert Grand's {:+.2} dBFS, {:.3} %", peak(x), over(x), peak(&grand), over(&grand));
        }
        assert!(peak(&loudest) < 0.0, "a v127 chord of the waveguide grand, room off, reaches {:+.2} dBFS", peak(&loudest));
    }

    #[test]
    fn every_reverb_preset_round_trips_through_its_index() {
        for (preset, label) in REVERB_PRESETS {
            assert_eq!(preset_from_index(preset_index(preset)), preset);
            assert_eq!(reverb_preset_label(preset), label);
        }
    }

    /// The whole point of a plan: a Play message has to reach `Playing` and stay there.
    #[test]
    fn a_play_message_leaves_the_transport_playing_over_a_real_plan() {
        let document = crate::ScoreDocument::demo().unwrap();
        let bridge = PlaybackBridge::new(document.score(), 108.0, false);
        let plan = bridge.plan();
        let mixer = PartMixer::<PART_CAPACITY>::new();
        let clock = AtomicAudioClock::new();
        let mut engine =
            PlaybackEngine::<SamplerBackend, PENDING_CAPACITY, PART_CAPACITY>::new(test_backend(
                Arc::new(SharedSound::default()),
            ));
        assert!(engine
            .queue(AudioMessage {
                at_device_sample: 0,
                sequence: 1,
                kind: AudioMessageKind::Play,
            })
            .is_ok());
        let mut left = vec![0.0_f32; 512];
        let mut right = vec![0.0_f32; 512];
        let mut peak = 0.0_f32;
        for block in 0..64 {
            left.fill(0.0);
            right.fill(0.0);
            let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
            // Muted instances still render (so the transport clock advances and
            // cursor/state verification works); only the samples are discarded.
            let context = RenderContext {
                device_sample_rate: 0,
                first_device_sample: block * 512,
                first_presentation_host_ns: block * 512 * 1_000_000_000 / u64::from(PLAN_RATE),
                frames: 512,
                output_latency_frames: 0,
                stream_generation: 1,
                clock_quality: ClockQuality::Exact,
            };
            engine.render(context, &mut channels, plan, &mixer, &clock);
            peak = left
                .iter()
                .chain(right.iter())
                .fold(peak, |peak, sample| peak.max(sample.abs()));
        }
        assert_eq!(engine.state(), PlaybackState::Playing);
        assert_eq!(engine.timeline_sample(), 64 * 512);
        assert_eq!(clock.read().state, PlaybackState::Playing);
        assert!(clock.read().presentation_sample > 0.0);
        // The modelled piano actually sounded the notes those blocks dispatched.
        assert!(peak > 1.0e-4, "rendered audio is silent (peak {peak})");
    }

}
