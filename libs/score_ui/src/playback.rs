//! Audio-clock-master playback bridge. UI gestures are converted to
//! `score_play` messages; the audio callback owns the realtime engine and a
//! physically modelled piano. The SoundFont sampler is kept only for the
//! metronome click, which is a short transient rather than an instrument.

use makepad_score::model::{EventKind, Rational, Score};
use makepad_score_play::{
    Articulation as PlayArticulation, AtomicAudioClock, AudioClockSnapshot, AudioMessage,
    AudioMessageKind, AuditionController, ClockQuality, CountInSpec, DisplayPosition, EventBatch,
    Meter, NoteInput, PartMixer, PerformancePlan, PlanInput, PlaybackEngine, RenderContext,
    ScheduleOptions, Scheduler, ScrubConfig, ScrubController, ScrubHit, ScrubOutcome, SpscRing,
    SynthBackend, SynthEvent, SynthEventKind, SynthEventTiming, TempoMap, TransportLoop,
};
use makepad_piano_model::{
    fx::{Perspective, ReverbPreset},
    Instrument, Piano, PianoEvent, TimedEvent as PianoTimedEvent,
};
use makepad_soundfont::{metronome_click, NoSamples, Sampler, SamplerEvent, TimedEvent};
use makepad_widgets::*;
use std::{
    ops::Range,
    sync::{
        atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering},
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

/// The room the piano is heard in. Values reach the audio thread through
/// [`SharedRoom`]: plain atomics, no lock, no allocation, applied at the top of
/// a render block. Keeping the whole binding in one small struct is what makes
/// it cheap to follow `makepad_piano_model`'s API if it moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoomSettings {
    pub preset: ReverbPreset,
    /// Dry/wet amount, 0.0 = dry.
    pub mix: f32,
    pub perspective: Perspective,
}

impl RoomSettings {
    pub const MIX_MAX: f32 = 1.0;

    pub fn with_mix(self, mix: f32) -> Self {
        Self {
            mix: mix.clamp(0.0, Self::MIX_MAX),
            ..self
        }
    }
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

/// UI writes, audio thread reads. `revision` is the only thing the audio side
/// polls, so an unchanged room costs one relaxed load per block.
#[derive(Debug, Default)]
struct SharedRoom {
    revision: AtomicU32,
    preset: AtomicU32,
    mix_bits: AtomicU32,
    audience: AtomicU32,
}

impl SharedRoom {
    fn publish(&self, room: RoomSettings) {
        self.preset.store(preset_index(room.preset), Ordering::Relaxed);
        self.mix_bits
            .store(room.mix.clamp(0.0, RoomSettings::MIX_MAX).to_bits(), Ordering::Relaxed);
        self.audience.store(
            u32::from(matches!(room.perspective, Perspective::Audience)),
            Ordering::Relaxed,
        );
        // Release last: the audio thread acquires this and then reads the rest.
        self.revision.fetch_add(1, Ordering::Release);
    }

    fn read(&self) -> RoomSettings {
        RoomSettings {
            preset: preset_from_index(self.preset.load(Ordering::Relaxed)),
            mix: f32::from_bits(self.mix_bits.load(Ordering::Relaxed)),
            perspective: if self.audience.load(Ordering::Relaxed) == 0 {
                Perspective::Player
            } else {
                Perspective::Audience
            },
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
    room: Arc<SharedRoom>,
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
            room: {
                let room = Arc::new(SharedRoom::default());
                room.publish(RoomSettings::default());
                room
            },
            installed: false,
        }
    }

    /// Hand new room settings to the audio thread. Lock-free by construction:
    /// the synth itself is owned by the callback and is never touched here.
    pub fn set_room(&self, room: RoomSettings) {
        self.room.publish(room);
    }

    pub fn rebuild_plan(&mut self, score: &Score, bpm: f64, count_in: bool) {
        self.plan = Arc::new(compile_plan(score, bpm, count_in));
        self.installed = false;
    }

    pub fn plan(&self) -> &PerformancePlan {
        &self.plan
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
        let room = self.room.clone();
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
                engine = Some(PlaybackEngine::new(SamplerBackend::new(rate, room.clone())));
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
    room: Arc<SharedRoom>,
    room_revision: u32,
    piano: Piano,
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
    fn new(sample_rate: f32, room: Arc<SharedRoom>) -> Self {
        Self {
            room,
            // Zero forces the first block to adopt whatever the UI published.
            room_revision: 0,
            piano: Piano::new(sample_rate),
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
    fn sync_room(&mut self) {
        let revision = self.room.revision.load(Ordering::Acquire);
        if revision == self.room_revision {
            return;
        }
        self.room_revision = revision;
        let room = self.room.read();
        self.piano.set_reverb_preset(room.preset);
        self.piano.set_reverb_mix(room.mix);
        self.piano.set_perspective(room.perspective);
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
                self.remember_key(id, key);
                self.queue_piano(PianoEvent::NoteOn {
                    key,
                    velocity: ((u32::from(velocity) * 127) / 65_535).max(1) as u8,
                })
            }
            SynthEventKind::NoteOff { note_id, .. } => {
                let id = sampler_note_id(event.source, note_id);
                if let Some(key) = self.take_key(id) {
                    self.queue_piano(PianoEvent::NoteOff { key });
                }
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
        self.sync_room();
        let mut start = range.start;
        let mut first = true;
        while start < range.end {
            let count = (range.end - start).min(SCRATCH_FRAMES);
            let events = if first {
                &self.pending[..self.pending_len]
            } else {
                &[]
            };
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
                    dynamic: 0.68,
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
    let tempo = TempoMap::constant(PLAN_RATE, bpm.clamp(20.0, 400.0))
        .unwrap_or_else(|_| TempoMap::constant(PLAN_RATE, 120.0).expect("fallback tempo is valid"));
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

fn articulation_for_event(_kind: &EventKind) -> PlayArticulation {
    PlayArticulation::Normal
}

fn rational_f64(value: Rational) -> f64 {
    value.numerator() as f64 / value.denominator() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// The reverb binding must reach the audio thread without a lock and
    /// without the UI ever touching the synth. Publish, then render: the
    /// backend has to have adopted the new room by the end of the first block.
    #[test]
    fn room_settings_reach_the_audio_thread_through_the_shared_cell() {
        let room = Arc::new(SharedRoom::default());
        room.publish(RoomSettings {
            preset: ReverbPreset::Cathedral,
            mix: 0.75,
            perspective: Perspective::Audience,
        });
        assert_eq!(
            room.read(),
            RoomSettings {
                preset: ReverbPreset::Cathedral,
                mix: 0.75,
                perspective: Perspective::Audience,
            }
        );
        let mut backend = SamplerBackend::new(PLAN_RATE as f32, room.clone());
        let mut left = vec![0.0_f32; 64];
        let mut right = vec![0.0_f32; 64];
        let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
        backend.render_range(&mut channels, 0..64);
        let adopted = backend.room_revision;
        assert!(adopted > 0);
        // No further work while nothing moves.
        backend.render_range(&mut channels, 0..64);
        assert_eq!(backend.room_revision, adopted);
        room.publish(RoomSettings::default());
        backend.render_range(&mut channels, 0..64);
        assert!(backend.room_revision > adopted);
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
            PlaybackEngine::<SamplerBackend, PENDING_CAPACITY, PART_CAPACITY>::new(
                SamplerBackend::new(PLAN_RATE as f32, Arc::new(SharedRoom::default())),
            );
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
