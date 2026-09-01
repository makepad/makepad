use crate::model::{Envelope, LoopMode, SampleRead, SampleSource, VoiceParameters, VoiceSource};
use crate::queue::SpscConsumer;

const TAU: f64 = core::f64::consts::PI * 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SamplerEvent {
    NoteOn { note_id: u32, parameters: VoiceParameters },
    NoteOff { note_id: u32 },
    AllNotesOff,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedEvent {
    /// Offset from the start of the current output block.
    pub offset: u32,
    pub event: SamplerEvent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScheduledEvent {
    /// Absolute output frame on the sampler's monotonically increasing clock.
    pub frame: u64,
    pub event: SamplerEvent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderReport {
    pub frames: usize,
    pub events_applied: usize,
    pub late_events: usize,
    pub source_misses: usize,
    pub dropped_note_ons: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EnvelopeStage {
    Delay,
    Attack,
    Hold,
    Decay,
    #[default]
    Sustain,
    Release,
    Finished,
}

/// Sample-counted linear DAHDSR. Durations are rounded to the nearest frame;
/// attack reaches 1.0 on its last frame, decay reaches sustain on its last
/// frame, and release reaches zero on its last frame.
#[derive(Clone, Copy, Debug)]
pub struct EnvelopeRunner {
    pub stage: EnvelopeStage,
    pub level: f32,
    position: u64,
    delay: u64,
    attack: u64,
    hold: u64,
    decay: u64,
    release: u64,
    sustain: f32,
    release_start: f32,
}

impl EnvelopeRunner {
    pub fn new(envelope: Envelope, sample_rate: f32) -> Self {
        let mut result = Self {
            stage: EnvelopeStage::Delay,
            level: 0.0,
            position: 0,
            delay: duration_frames(envelope.delay, sample_rate),
            attack: duration_frames(envelope.attack, sample_rate),
            hold: duration_frames(envelope.hold, sample_rate),
            decay: duration_frames(envelope.decay, sample_rate),
            release: duration_frames(envelope.release, sample_rate),
            sustain: envelope.sustain.clamp(0.0, 1.0),
            release_start: 0.0,
        };
        result.skip_zero_stages();
        result
    }

    pub fn release(&mut self) {
        if matches!(self.stage, EnvelopeStage::Release | EnvelopeStage::Finished) {
            return;
        }
        self.release_start = self.level;
        self.position = 0;
        self.stage = if self.release == 0 {
            self.level = 0.0;
            EnvelopeStage::Finished
        } else {
            EnvelopeStage::Release
        };
    }

    pub fn next_value(&mut self) -> f32 {
        match self.stage {
            EnvelopeStage::Delay => {
                self.level = 0.0;
                self.advance_if_done(self.delay, EnvelopeStage::Attack);
            }
            EnvelopeStage::Attack => {
                self.level = (self.position.saturating_add(1) as f32 / self.attack as f32).min(1.0);
                self.advance_if_done(self.attack, EnvelopeStage::Hold);
            }
            EnvelopeStage::Hold => {
                self.level = 1.0;
                self.advance_if_done(self.hold, EnvelopeStage::Decay);
            }
            EnvelopeStage::Decay => {
                let progress =
                    (self.position.saturating_add(1) as f32 / self.decay as f32).min(1.0);
                self.level = 1.0 + (self.sustain - 1.0) * progress;
                self.advance_if_done(self.decay, EnvelopeStage::Sustain);
            }
            EnvelopeStage::Sustain => self.level = self.sustain,
            EnvelopeStage::Release => {
                let progress =
                    (self.position.saturating_add(1) as f32 / self.release as f32).min(1.0);
                self.level = self.release_start * (1.0 - progress);
                self.advance_if_done(self.release, EnvelopeStage::Finished);
            }
            EnvelopeStage::Finished => self.level = 0.0,
        }
        self.level
    }

    pub const fn is_finished(&self) -> bool {
        matches!(self.stage, EnvelopeStage::Finished)
    }

    fn advance_if_done(&mut self, duration: u64, next: EnvelopeStage) {
        self.position = self.position.saturating_add(1);
        if self.position >= duration {
            self.position = 0;
            self.stage = next;
            self.skip_zero_stages();
        }
    }

    fn skip_zero_stages(&mut self) {
        loop {
            match self.stage {
                EnvelopeStage::Delay if self.delay == 0 => self.stage = EnvelopeStage::Attack,
                EnvelopeStage::Attack if self.attack == 0 => {
                    self.level = 1.0;
                    self.stage = EnvelopeStage::Hold;
                }
                EnvelopeStage::Hold if self.hold == 0 => self.stage = EnvelopeStage::Decay,
                EnvelopeStage::Decay if self.decay == 0 => {
                    self.level = self.sustain;
                    self.stage = EnvelopeStage::Sustain;
                }
                _ => break,
            }
        }
    }
}

fn duration_frames(seconds: f32, sample_rate: f32) -> u64 {
    if !seconds.is_finite() || !sample_rate.is_finite() || seconds <= 0.0 || sample_rate <= 0.0 {
        0
    } else {
        (seconds as f64 * sample_rate as f64).round() as u64
    }
}

#[derive(Clone, Copy)]
struct Biquad {
    active: bool,
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    lz1: f32,
    lz2: f32,
    rz1: f32,
    rz2: f32,
}

impl Biquad {
    const OFF: Self = Self {
        active: false,
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
        lz1: 0.0,
        lz2: 0.0,
        rz1: 0.0,
        rz2: 0.0,
    };

    fn new(cutoff: f32, resonance_db: f32, sample_rate: f32) -> Self {
        if sample_rate < 50.0
            || (cutoff >= sample_rate * 0.45 && resonance_db.abs() < 0.01)
        {
            return Self::OFF;
        }
        let frequency = cutoff.clamp(20.0, sample_rate * 0.45);
        let omega = 2.0 * core::f32::consts::PI * frequency / sample_rate;
        let sine = omega.sin();
        let cosine = omega.cos();
        let q = 10.0_f32.powf(resonance_db.clamp(-12.0, 26.0) / 20.0).clamp(0.5, 20.0);
        let alpha = sine / (2.0 * q);
        let divisor = 1.0 + alpha;
        let b0 = (1.0 - cosine) * 0.5 / divisor;
        let b1 = (1.0 - cosine) / divisor;
        let b2 = b0;
        let a1 = -2.0 * cosine / divisor;
        let a2 = (1.0 - alpha) / divisor;
        Self { active: true, b0, b1, b2, a1, a2, ..Self::OFF }
    }

    fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        if !self.active {
            return (left, right);
        }
        let out_left = self.b0 * left + self.lz1;
        self.lz1 = self.b1 * left - self.a1 * out_left + self.lz2;
        self.lz2 = self.b2 * left - self.a2 * out_left;
        let out_right = self.b0 * right + self.rz1;
        self.rz1 = self.b1 * right - self.a1 * out_right + self.rz2;
        self.rz2 = self.b2 * right - self.a2 * out_right;
        (out_left, out_right)
    }
}

#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    released: bool,
    looped: bool,
    note_id: u32,
    started: u64,
    parameters: VoiceParameters,
    position: f64,
    step: f64,
    oscillator_phase: f64,
    oscillator_step: f64,
    age: u64,
    noise: u32,
    envelope: EnvelopeRunner,
    filter: Biquad,
}

impl Voice {
    const EMPTY_PARAMETERS: VoiceParameters = VoiceParameters {
        source: VoiceSource::ProceduralPiano,
        key: 60,
        velocity: 0,
        root_key: 60.0,
        tune_cents: 0.0,
        scale_tuning: 100.0,
        sample_rate: 44_100,
        start_frame: 0,
        end_frame: 0,
        loop_start: 0,
        loop_end: 0,
        loop_mode: LoopMode::NoLoop,
        release_on_note_off: true,
        envelope: Envelope {
            delay: 0.0,
            attack: 0.0,
            hold: 0.0,
            decay: 0.0,
            sustain: 0.0,
            release: 0.0,
        },
        gain: 0.0,
        pan: 0.0,
        filter_cutoff_hz: 20_000.0,
        filter_resonance_db: 0.0,
        exclusive_class: 0,
    };

    const EMPTY: Self = Self {
        active: false,
        released: false,
        looped: false,
        note_id: 0,
        started: 0,
        parameters: Self::EMPTY_PARAMETERS,
        position: 0.0,
        step: 0.0,
        oscillator_phase: 0.0,
        oscillator_step: 0.0,
        age: 0,
        noise: 1,
        envelope: EnvelopeRunner {
            stage: EnvelopeStage::Finished,
            level: 0.0,
            position: 0,
            delay: 0,
            attack: 0,
            hold: 0,
            decay: 0,
            release: 0,
            sustain: 0.0,
            release_start: 0.0,
        },
        filter: Biquad::OFF,
    };

    fn start(&mut self, note_id: u32, parameters: VoiceParameters, started: u64, sample_rate: f32) {
        let pitch_cents = (parameters.key as f64 - 69.0) * 100.0 + parameters.tune_cents as f64;
        let frequency = 440.0 * 2.0_f64.powf(pitch_cents / 1200.0);
        *self = Self {
            active: true,
            released: false,
            looped: false,
            note_id,
            started,
            parameters,
            position: parameters.start_frame as f64,
            step: parameters.pitch_ratio(sample_rate),
            oscillator_phase: 0.0,
            oscillator_step: if sample_rate > 0.0 { frequency / sample_rate as f64 } else { 0.0 },
            age: 0,
            noise: note_id ^ (u32::from(parameters.key) << 16) ^ 0x9e37_79b9,
            envelope: EnvelopeRunner::new(parameters.envelope, sample_rate),
            filter: Biquad::new(
                parameters.filter_cutoff_hz,
                parameters.filter_resonance_db,
                sample_rate,
            ),
        };
    }

    fn release(&mut self, force: bool) {
        if !force && !self.parameters.release_on_note_off {
            return;
        }
        self.released = true;
        self.envelope.release();
    }

    fn render<S: SampleSource>(&mut self, source: &S, sample_rate: f32) -> (f32, f32, bool) {
        if !self.active {
            return (0.0, 0.0, false);
        }
        let (mut left, mut right, missing) = match self.parameters.source {
            VoiceSource::Sample { .. } => self.render_sample(source),
            VoiceSource::ProceduralPiano => {
                let value = self.render_piano(sample_rate);
                (value, value, false)
            }
            VoiceSource::Metronome { accent } => {
                let value = self.render_click(sample_rate, accent);
                (value, value, false)
            }
        };
        let envelope = self.envelope.next_value();
        let pan = self.parameters.pan.clamp(-1.0, 1.0);
        let left_pan = ((1.0 - pan) * 0.5).sqrt();
        let right_pan = ((1.0 + pan) * 0.5).sqrt();
        let gain = self.parameters.gain * envelope;
        left *= gain * left_pan;
        right *= gain * right_pan;
        (left, right) = self.filter.process(left, right);
        self.age = self.age.saturating_add(1);
        if self.envelope.is_finished() {
            self.active = false;
        }
        (left, right, missing)
    }

    fn render_sample<S: SampleSource>(&mut self, source: &S) -> (f32, f32, bool) {
        let looping = self.parameters.loop_mode == LoopMode::Continuous
            || self.parameters.loop_mode == LoopMode::UntilRelease && !self.released;
        if looping && self.parameters.loop_end > self.parameters.loop_start {
            let loop_start = self.parameters.loop_start as f64;
            let loop_length = (self.parameters.loop_end - self.parameters.loop_start) as f64;
            if self.position >= self.parameters.loop_end as f64 {
                self.position = loop_start + (self.position - loop_start).rem_euclid(loop_length);
                self.looped = true;
            }
        }
        if self.position < self.parameters.start_frame as f64
            || self.position >= self.parameters.end_frame as f64
        {
            self.active = false;
            return (0.0, 0.0, false);
        }
        let floor = self.position.floor() as i64;
        let fraction = (self.position - floor as f64) as f32;
        let indices = [floor - 1, floor, floor + 1, floor + 2];
        let mut left = [0.0; 4];
        let mut right = [0.0; 4];
        for (slot, raw_index) in indices.into_iter().enumerate() {
            let index = self.map_index(raw_index, looping);
            let VoiceSource::Sample { sample_id } = self.parameters.source else { return (0.0, 0.0, false) };
            match source.read_frame(sample_id, index) {
                SampleRead::Resident { left: l, right: r } => {
                    left[slot] = l;
                    right[slot] = r;
                }
                SampleRead::Missing => {
                    self.position += self.step;
                    return (0.0, 0.0, true);
                }
            }
        }
        self.position += self.step;
        (hermite(left, fraction), hermite(right, fraction), false)
    }

    fn map_index(&self, index: i64, looping: bool) -> i64 {
        if looping && self.parameters.loop_end > self.parameters.loop_start {
            let length = self.parameters.loop_end - self.parameters.loop_start;
            if index >= self.parameters.loop_end
                || self.looped && index < self.parameters.loop_start
            {
                return self.parameters.loop_start
                    + (index - self.parameters.loop_start).rem_euclid(length);
            }
        }
        index.clamp(
            self.parameters.start_frame,
            self.parameters.end_frame.saturating_sub(1),
        )
    }

    fn render_piano(&mut self, sample_rate: f32) -> f32 {
        let phase = self.oscillator_phase * TAU;
        let mut value = phase.sin() * 0.72 + (phase * 2.01).sin() * 0.19 + (phase * 3.97).sin() * 0.09;
        let hammer_frames = (sample_rate * 0.012).max(1.0) as u64;
        if self.age < hammer_frames {
            let remaining = 1.0 - self.age as f64 / hammer_frames as f64;
            value += self.next_noise() as f64 * remaining * remaining * 0.16;
        }
        self.oscillator_phase = (self.oscillator_phase + self.oscillator_step).fract();
        value as f32
    }

    fn render_click(&mut self, sample_rate: f32, accent: bool) -> f32 {
        let length = (sample_rate * if accent { 0.055 } else { 0.038 }).max(1.0) as u64;
        if self.age >= length {
            self.active = false;
            return 0.0;
        }
        let remaining = 1.0 - self.age as f64 / length as f64;
        let base = if accent { 1_750.0 } else { 2_650.0 };
        let phase = self.age as f64 * base * TAU / sample_rate.max(1.0) as f64;
        let noise = self.next_noise() as f64;
        ((phase.sin() * 0.72 + (phase * 0.47).sin() * 0.18 + noise * 0.45)
            * remaining
            * remaining) as f32
    }

    fn next_noise(&mut self) -> f32 {
        let mut value = self.noise;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.noise = value;
        (value as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn hermite(values: [f32; 4], fraction: f32) -> f32 {
    let c0 = values[1];
    let c1 = 0.5 * (values[2] - values[0]);
    let c2 = values[0] - 2.5 * values[1] + 2.0 * values[2] - 0.5 * values[3];
    let c3 = 0.5 * (values[3] - values[0]) + 1.5 * (values[1] - values[2]);
    ((c3 * fraction + c2) * fraction + c1) * fraction + c0
}

/// Fixed-voice sampler using four-point cubic Hermite interpolation. Hermite
/// costs four reads/channel but avoids the staircase/aliasing of nearest or
/// linear interpolation while keeping fixed, predictable work. When full,
/// note-on steals the quietest envelope;
/// equal levels choose the oldest voice. This avoids recent/loud note damage
/// and makes stealing deterministic. The render methods allocate, lock, log,
/// panic, and perform I/O nowhere; all owned state is `Copy` and fixed-size.
pub struct Sampler<const VOICES: usize> {
    sample_rate: f32,
    voices: [Voice; VOICES],
    serial: u64,
    frame_clock: u64,
    pending_event: Option<ScheduledEvent>,
}

impl<const VOICES: usize> Sampler<VOICES> {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate: if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 44_100.0 },
            voices: [Voice::EMPTY; VOICES],
            serial: 0,
            frame_clock: 0,
            pending_event: None,
        }
    }

    pub const fn frame_clock(&self) -> u64 {
        self.frame_clock
    }

    pub fn active_voice_count(&self) -> usize {
        self.voices.iter().filter(|voice| voice.active).count()
    }

    /// Render block-relative events at their exact offsets. Event slice order
    /// is preserved for multiple events at one offset; input need not be sorted.
    pub fn render<S: SampleSource>(
        &mut self,
        source: &S,
        events: &[TimedEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) -> RenderReport {
        let frames = left.len().min(right.len());
        let mut report = RenderReport { frames, ..RenderReport::default() };
        for frame in 0..frames {
            for timed in events {
                if timed.offset as usize == frame {
                    if !self.apply_event(timed.event) {
                        report.dropped_note_ons += 1;
                    }
                    report.events_applied += 1;
                }
            }
            let (l, r, misses) = self.render_frame(source);
            left[frame] = l;
            right[frame] = r;
            report.source_misses += misses;
            self.frame_clock = self.frame_clock.saturating_add(1);
        }
        report
    }

    /// Consume absolute-time events from an SPSC queue without block-boundary
    /// quantization. The producer must enqueue in nondecreasing frame order.
    pub fn render_from_queue<S: SampleSource, const N: usize>(
        &mut self,
        source: &S,
        consumer: &mut SpscConsumer<'_, ScheduledEvent, N>,
        left: &mut [f32],
        right: &mut [f32],
    ) -> RenderReport {
        let frames = left.len().min(right.len());
        let mut report = RenderReport { frames, ..RenderReport::default() };
        for frame in 0..frames {
            loop {
                if self.pending_event.is_none() {
                    self.pending_event = consumer.pop();
                }
                let Some(pending) = self.pending_event else { break };
                if pending.frame > self.frame_clock {
                    break;
                }
                self.pending_event = None;
                if pending.frame < self.frame_clock {
                    report.late_events += 1;
                }
                if !self.apply_event(pending.event) {
                    report.dropped_note_ons += 1;
                }
                report.events_applied += 1;
            }
            let (l, r, misses) = self.render_frame(source);
            left[frame] = l;
            right[frame] = r;
            report.source_misses += misses;
            self.frame_clock = self.frame_clock.saturating_add(1);
        }
        report
    }

    fn apply_event(&mut self, event: SamplerEvent) -> bool {
        match event {
            SamplerEvent::NoteOn { note_id, parameters } => self.note_on(note_id, parameters),
            SamplerEvent::NoteOff { note_id } => {
                for voice in &mut self.voices {
                    if voice.active && voice.note_id == note_id {
                        voice.release(false);
                    }
                }
                true
            }
            SamplerEvent::AllNotesOff => {
                for voice in &mut self.voices {
                    if voice.active {
                        voice.release(true);
                    }
                }
                true
            }
        }
    }

    fn note_on(&mut self, note_id: u32, parameters: VoiceParameters) -> bool {
        if parameters.exclusive_class != 0 {
            for voice in &mut self.voices {
                if voice.active && voice.parameters.exclusive_class == parameters.exclusive_class {
                    voice.release(true);
                }
            }
        }
        let mut chosen = None;
        for (index, voice) in self.voices.iter().enumerate() {
            if !voice.active {
                chosen = Some(index);
                break;
            }
        }
        if chosen.is_none() {
            let mut best_level = f32::INFINITY;
            let mut best_started = u64::MAX;
            for (index, voice) in self.voices.iter().enumerate() {
                if voice.envelope.level < best_level
                    || voice.envelope.level == best_level && voice.started < best_started
                {
                    chosen = Some(index);
                    best_level = voice.envelope.level;
                    best_started = voice.started;
                }
            }
        }
        let Some(index) = chosen else { return false };
        self.serial = self.serial.wrapping_add(1);
        if let Some(voice) = self.voices.get_mut(index) {
            voice.start(note_id, parameters, self.serial, self.sample_rate);
            true
        } else {
            false
        }
    }

    fn render_frame<S: SampleSource>(&mut self, source: &S) -> (f32, f32, usize) {
        let mut left = 0.0;
        let mut right = 0.0;
        let mut misses = 0;
        for voice in &mut self.voices {
            let (voice_left, voice_right, missing) = voice.render(source, self.sample_rate);
            left += voice_left;
            right += voice_right;
            misses += usize::from(missing);
        }
        (left, right, misses)
    }
}

impl<const VOICES: usize> Default for Sampler<VOICES> {
    fn default() -> Self {
        Self::new(44_100.0)
    }
}
