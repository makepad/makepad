//! Audio-clock-master playback bridge. UI gestures are converted to
//! `score_play` messages; the audio callback owns the realtime engine and a
//! physically modelled piano. The SoundFont sampler serves the metronome
//! click.

use crate::sound::SoundSettings;
use makepad_score::model::{EventKind, Rational, Score};
use makepad_score_play::{
    Articulation as PlayArticulation, AtomicAudioClock, AudioClockSnapshot, AudioMessage,
    AudioMessageKind, AuditionController, ClockQuality, CountInSpec, DisplayPosition, EventBatch,
    Meter, NoteInput, PartMixer, PerformancePlan, PlanInput, PlaybackEngine, RenderContext,
    ScheduleOptions, Scheduler, ScrubConfig, ScrubController, ScrubHit, ScrubOutcome, SpscRing,
    SynthBackend, SynthEvent, SynthEventKind, SynthEventTiming, TempoMap, TransportLoop,
};
use crate::sound::ScoreEngine;
use makepad_piano_model::{
    fx::{Perspective, ReverbPreset},
    learned::PianoEngine,
    Piano,
    PianoEvent, PianoPreset, TimedEvent as PianoTimedEvent, Voicing,
};
use makepad_soundfont::{metronome_click, NoSamples, Sampler, SamplerEvent, TimedEvent};
use makepad_widgets::*;
use std::{
    ops::Range,
    ptr,
    sync::{
        atomic::{AtomicI64, AtomicPtr, AtomicU32, AtomicU64, Ordering},
        Arc,
    },
};

const MESSAGE_CAPACITY: usize = 512;
const PENDING_CAPACITY: usize = 256;
const PART_CAPACITY: usize = 32;
const VOICE_CAPACITY: usize = 96;
const EVENT_CAPACITY: usize = 64;
const SCRATCH_FRAMES: usize = 2048;
const PLAN_RATE: u32 = 48_000;
/// Peak-meter fall-back, per frame. Full scale to silence in about a third of
/// a second — slow enough to read, fast enough to follow a phrase.
const PEAK_FALL_PER_FRAME: f32 = 1.0 / 16_384.0;
/// How long a replaced instrument keeps sounding while it fades out. Swapping
/// the piano mid-phrase would otherwise cut every ringing string and the room
/// tail dead on one sample, which is a click; ~40 ms of fade is inaudible.
const INSTRUMENT_FADE_FRAMES: u32 = 2048;

/// The room the piano is heard in. One part of [`SoundSettings`]; it reaches
/// the audio thread through the same shared cell as everything else there —
/// plain atomics, no lock, no allocation, applied at the top of a render
/// block.
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

/// Build the instrument the application asked for.
///
/// Hybrid is the one that is not simply a `PianoEngine::new`: it is the
/// physical instrument with [`crate::hybrid`]'s baked per-partial targets
/// applied across all 88 keys before it ever renders a block. That costs
/// 3.6 ms, which is why it belongs here on the UI thread alongside the
/// design rebuilds rather than anywhere near the callback.
fn build_engine(engine: ScoreEngine, rate: f32, preset: &PianoPreset) -> PianoEngine {
    match engine {
        ScoreEngine::Hybrid => {
            let mut piano = Piano::new_with_preset(rate, preset);
            crate::hybrid::apply_targets(&mut piano);
            PianoEngine::Physical(Box::new(piano))
        }
        other => PianoEngine::new(other.kind(), rate, preset),
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
/// all 88 key designs. So the UI thread builds either one and passes
/// ownership through `incoming`; the audio thread
/// takes it, and passes the instrument it replaced back through `retired` for
/// the UI thread to drop. Neither slot ever holds more than one instrument:
/// the audio side refuses to take a new one while it still owes the old one
/// back, and the UI side refuses to offer one until the previous handoff has
/// completed. Nothing is allocated, freed, or waited on inside the callback.
#[derive(Debug, Default)]
struct InstrumentHandoff {
    incoming: AtomicPtr<PianoEngine>,
    retired: AtomicPtr<PianoEngine>,
}

// The instrument is built on the UI thread and played on the audio thread.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<PianoEngine>();
};

impl InstrumentHandoff {
    /// UI thread: offer a freshly built instrument. Handed back when the
    /// previous swap has not finished, so the caller can simply try again on
    /// the next frame with whatever the user has landed on by then.
    fn offer(&self, piano: Box<PianoEngine>) -> Option<Box<PianoEngine>> {
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
            Ok(_) => None,
            // Safety: the exchange failed, so the pointer was never published
            // and this thread still owns it exclusively.
            Err(_) => Some(unsafe { Box::from_raw(raw) }),
        }
    }

    /// UI thread: take back the instrument the audio thread replaced, so it is
    /// dropped here rather than in the callback.
    fn reclaim(&self) -> Option<Box<PianoEngine>> {
        let raw = self.retired.swap(ptr::null_mut(), Ordering::Acquire);
        // Safety: the audio thread published this pointer with Release and
        // never touches it again.
        (!raw.is_null()).then(|| unsafe { Box::from_raw(raw) })
    }

    /// Audio thread: adopt an offered instrument, but only while the previous
    /// one has already been handed back.
    fn take(&self) -> Option<Box<PianoEngine>> {
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
    fn retire(&self, piano: Box<PianoEngine>) {
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

pub struct PlaybackBridge {
    messages: Arc<SpscRing<AudioMessage, MESSAGE_CAPACITY>>,
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
    pending_instrument: Option<Box<PianoEngine>>,
    /// What the instrument is actually putting out, written by the audio
    /// thread once per rendered span. One relaxed store; read for display.
    peak: Arc<AtomicU32>,
    installed: bool,
}

impl PlaybackBridge {
    pub fn new(score: &Score, bpm: f64, count_in: bool) -> Self {
        Self {
            messages: Arc::new(SpscRing::new()),
            clock: Arc::new(AtomicAudioClock::new()),
            mixer: Arc::new(PartMixer::new()),
            plan: Arc::new(compile_plan(score, bpm, count_in)),
            device_sample: Arc::new(AtomicU64::new(0)),
            device_rate_bits: Arc::new(AtomicU32::new((PLAN_RATE as f32).to_bits())),
            host_offset_ns: Arc::new(AtomicI64::new(0)),
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
            peak: Arc::new(AtomicU32::new(0)),
            installed: false,
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
        let rate = f32::from_bits(self.device_rate_bits.load(Ordering::Relaxed));
        let rate = if rate.is_finite() {
            rate.clamp(8_000.0, 192_000.0)
        } else {
            PLAN_RATE as f32
        };
        // Latest request wins: an older pending build is simply dropped here.
        self.pending_instrument = Some(Box::new(build_engine(engine, rate, preset)));
        self.service_instrument();
    }

    /// Drop whatever the audio thread handed back and retry a pending
    /// handoff. Called once a frame; costs two relaxed loads when idle.
    pub fn service_instrument(&mut self) {
        drop(self.instrument.reclaim());
        if let Some(piano) = self.pending_instrument.take() {
            self.pending_instrument = self.instrument.offer(piano);
        }
    }


    pub fn rebuild_plan(&mut self, score: &Score, bpm: f64, count_in: bool) {
        self.plan = Arc::new(compile_plan(score, bpm, count_in));
        self.installed = false;
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
        let _ = self.messages.push(message);
    }

    fn push_batch(&self, batch: EventBatch) {
        for message in batch.as_slice().iter().copied() {
            let _ = self.messages.push(message);
        }
    }

    /// Installs (or replaces after a plan edit) callback zero. The callback
    /// allocates nothing: engine, sampler, event storage, and scratch audio
    /// are fixed before the first quantum.
    pub fn install_audio_output(&mut self, cx: &mut Cx) {
        if self.installed {
            return;
        }
        let messages = self.messages.clone();
        let clock = self.clock.clone();
        let mixer = self.mixer.clone();
        let plan = self.plan.clone();
        let device_sample = self.device_sample.clone();
        let device_rate_bits = self.device_rate_bits.clone();
        let host_offset_ns = self.host_offset_ns.clone();
        let sound = self.sound.clone();
        let instrument = self.instrument.clone();
        let peak = self.peak.clone();
        let mut engine: Option<PlaybackEngine<SamplerBackend, PENDING_CAPACITY, PART_CAPACITY>> = None;
        let mut fallback_device_sample = 0_u64;
        let mut stream_generation = 1_u32;
        // Agent/test instances must never make sound on the user's machine.
        let muted = std::env::var_os("MAKEPAD_SCORE_MUTE").is_some();
        cx.audio_output(0, move |info, output| {
            output.zero();
            let rate = info.sample_rate.clamp(8_000.0, 384_000.0) as f32;
            device_rate_bits.store(rate.to_bits(), Ordering::Relaxed);
            if engine.is_none() {
                engine = Some(PlaybackEngine::new(SamplerBackend::new(
                    rate,
                    sound.clone(),
                    instrument.clone(),
                    peak.clone(),
                )));
                stream_generation = stream_generation.wrapping_add(1).max(1);
            }
            let frames = output.frame_count();
            let channel_count = output.channel_count();
            if frames == 0 || channel_count == 0 {
                return;
            }
            let first_device_sample = info
                .time
                .filter(|time| time.sample_time.is_finite() && time.sample_time >= 0.0)
                .map(|time| time.sample_time.round() as u64)
                .unwrap_or(fallback_device_sample);
            fallback_device_sample = first_device_sample.saturating_add(frames as u64);
            device_sample.store(fallback_device_sample, Ordering::Release);
            let ui_ns = (Cx::time_now().max(0.0) * 1_000_000_000.0) as u64;
            let presentation_host_ns = info.time.map_or(ui_ns, |time| time.host_time);
            let offset = i128::from(presentation_host_ns) - i128::from(ui_ns);
            host_offset_ns.store(
                offset.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
                Ordering::Relaxed,
            );
            // Muted instances still render (so the transport clock advances and
            // cursor/state verification works); only the samples are discarded.
            let context = RenderContext {
                first_device_sample,
                device_sample_rate: info.sample_rate.clamp(8_000.0, 384_000.0) as u32,
                first_presentation_host_ns: presentation_host_ns,
                frames,
                output_latency_frames: 0,
                stream_generation,
                clock_quality: if info.time.is_some() {
                    ClockQuality::Exact
                } else {
                    ClockQuality::Estimated
                },
            };
            let Some(engine) = engine.as_mut() else { return };
            if channel_count == 1 {
                let channel = &mut output.data[..frames];
                let mut channels: [&mut [f32]; 1] = [channel];
                let _ = engine.render(context, &mut channels, &plan, &messages, &mixer, &clock);
            } else {
                let (left, remaining) = output.data.split_at_mut(frames);
                let right = &mut remaining[..frames];
                let mut channels: [&mut [f32]; 2] = [left, right];
                let _ = engine.render(context, &mut channels, &plan, &messages, &mixer, &clock);
            }
            // A muted instance renders normally — so the transport clock, cursor
            // and engine state stay verifiable — and then discards the samples.
            if muted {
                output.zero();
            }
        });
        self.installed = true;
    }
}

struct SamplerBackend {
    sound: Arc<SharedSound>,
    sound_revision: u32,
    instrument: Arc<InstrumentHandoff>,
    peak: Arc<AtomicU32>,
    piano: Box<PianoEngine>,
    /// The instrument a rebuild replaced, still ringing itself out.
    fading: Option<Box<PianoEngine>>,
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
            piano: Box::new(build_engine(
                ScoreEngine::Physical,
                sample_rate,
                &makepad_piano_model::PIANO_PRESETS
                    [crate::sound::default_preset_index(ScoreEngine::Physical)],
            )),
            fading: None,
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
        let piano = &mut self.piano;
        piano.set_reverb_preset(sound.room.preset);
        piano.set_reverb_mix(sound.room.mix);
        piano.set_perspective(sound.room.perspective);
        piano.set_early_reflection_level(sound.early_reflections);
        piano.set_voicing(sound.voicing);
        piano.set_eq_shelf(sound.eq_shelf_db, sound.eq_shelf_hz);
        piano.set_eq_bell(sound.eq_bell_hz, sound.eq_bell_db, sound.eq_bell_q);
        piano.set_tone(sound.tone_bass_db, sound.tone_treble_db);
        piano.set_master_gain(sound.master_gain);
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
        // The rebuild carries the preset's own voicing and room; re-apply the
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

    /// Mix the outgoing instrument's tail under the new one.
    fn render_fade(&mut self, count: usize) {
        let Some(mut fading) = self.fading.take() else {
            return;
        };
        fading.process(&[], &mut self.fade_left[..count], &mut self.fade_right[..count]);
        let span = INSTRUMENT_FADE_FRAMES as f32;
        for index in 0..count {
            let left = self.fade_frames_left.saturating_sub(index as u32) as f32;
            let gain = (left / span).clamp(0.0, 1.0);
            self.scratch_left[index] += self.fade_left[index] * gain;
            self.scratch_right[index] += self.fade_right[index] * gain;
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
            for index in 0..count {
                self.scratch_left[index] += self.piano_left[index];
                self.scratch_right[index] += self.piano_right[index];
            }
            self.render_fade(count);
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
    use makepad_piano_model::{Piano, PIANO_PRESETS};
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
        assert_eq!(backend.piano.voicing(), wanted.voicing);
        assert_eq!(backend.piano.reverb_mix(), wanted.room.mix);
        assert_eq!(backend.piano.perspective(), wanted.room.perspective);
        assert_eq!(backend.piano.early_reflection_level(), wanted.early_reflections);
        assert_eq!(backend.piano.eq_shelf(), (wanted.eq_shelf_db, wanted.eq_shelf_hz));
        assert_eq!(
            backend.piano.eq_bell(),
            (wanted.eq_bell_hz, wanted.eq_bell_db, wanted.eq_bell_q)
        );
        assert_eq!(
            backend.piano.tone(),
            (wanted.tone_bass_db, wanted.tone_treble_db)
        );
        assert_eq!(backend.piano.master_gain(), wanted.master_gain);

        // No further work while nothing moves.
        backend.render_range(&mut channels, 0..64);
        assert_eq!(backend.sound_revision, adopted);
        shared.publish(SoundSettings::default());
        backend.render_range(&mut channels, 0..64);
        assert!(backend.sound_revision > adopted);
        assert_eq!(backend.piano.voicing(), SoundSettings::default().voicing);
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
            .offer(Box::new(build_engine(
                ScoreEngine::Learned,
                PLAN_RATE as f32,
                &PIANO_PRESETS[0],
            )))
            .is_none());
        // A second offer while the first is in flight comes straight back.
        assert!(handoff
            .offer(Box::new(PianoEngine::Physical(Box::new(Piano::new(
                PLAN_RATE as f32,
            )))))
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
        assert_eq!(backend.piano.kind(), makepad_piano_model::learned::EngineKind::Learned);
        assert_eq!(backend.piano.reverb_mix(), electric.room.mix);

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
            .offer(Box::new(PianoEngine::Physical(Box::new(Piano::new(
                PLAN_RATE as f32,
            )))))
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
        let ring = SpscRing::<AudioMessage, 8>::new();
        let mixer = PartMixer::<PART_CAPACITY>::new();
        let clock = AtomicAudioClock::new();
        let mut engine =
            PlaybackEngine::<SamplerBackend, PENDING_CAPACITY, PART_CAPACITY>::new(test_backend(
                Arc::new(SharedSound::default()),
            ));
        assert!(ring
            .push(AudioMessage {
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
            engine.render(context, &mut channels, plan, &ring, &mixer, &clock);
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
