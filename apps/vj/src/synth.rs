//! The VJ's clocked instrument rack.
//!
//! This is deliberately not an audio graph. The one VJ device callback owns
//! this value outright, sends it absolute output frames, and asks it for one
//! block from each concrete instrument. UI edits are small copyable commands;
//! no setting is polled through atomics from the DSP hot path.

use makepad_drumkit::{DrumKit, DrumVoice, SampleBank};
use makepad_piano_model::{Piano, PianoEvent, TimedEvent};
use std::sync::Arc;

pub const STEPS: usize = 16;
pub const MELODY_ROWS: usize = 12;
pub const DRUM_ROWS: usize = 8;
pub const MAX_BLOCK: usize = 4_096;
const PIANO_EVENT_CAPACITY: usize = 512;

pub type StepPattern = [u16; STEPS];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum SynthTrack {
    #[default]
    Piano = 0,
    Ironfish = 1,
    Drums = 2,
}

impl SynthTrack {
    pub const ALL: [Self; 3] = [Self::Piano, Self::Ironfish, Self::Drums];

    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SynthClock {
    /// Absolute output frame of the next whole beat.
    pub beat_frame: u64,
    pub frames_per_beat: f64,
    /// Bar-relative index of `beat_frame`, 0..3.
    pub beat_index: u8,
}

impl SynthClock {
    fn valid(self) -> bool {
        self.frames_per_beat.is_finite() && self.frames_per_beat >= 32.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RackPatterns {
    pub piano: StepPattern,
    pub ironfish: StepPattern,
    pub drums: StepPattern,
}

impl Default for RackPatterns {
    fn default() -> Self {
        let mut piano = [0u16; STEPS];
        // C minor: a quiet answer every half beat.
        for (step, row) in [(0, 0), (2, 3), (4, 7), (6, 10), (8, 7), (10, 3), (12, 0), (14, 7)] {
            piano[step] = 1 << row;
        }
        let mut ironfish = [0u16; STEPS];
        for (step, row) in [(0, 0), (4, 7), (8, 3), (12, 10)] {
            ironfish[step] = 1 << row;
        }
        let mut drums = [0u16; STEPS];
        for step in 0..STEPS {
            if step % 4 == 0 {
                drums[step] |= 1 << 0; // kick
            }
            if step % 8 == 4 {
                drums[step] |= 1 << 1; // snare
            }
            if step % 2 == 0 {
                drums[step] |= 1 << 2; // closed hat
            }
            if step == 15 {
                drums[step] |= 1 << 3; // open hat
            }
        }
        Self { piano, ironfish, drums }
    }
}

impl RackPatterns {
    pub fn get(&self, track: SynthTrack) -> StepPattern {
        match track {
            SynthTrack::Piano => self.piano,
            SynthTrack::Ironfish => self.ironfish,
            SynthTrack::Drums => self.drums,
        }
    }

    pub fn set(&mut self, track: SynthTrack, pattern: StepPattern) {
        match track {
            SynthTrack::Piano => self.piano = pattern,
            SynthTrack::Ironfish => self.ironfish = pattern,
            SynthTrack::Drums => self.drums = pattern,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RackSnapshot {
    pub playing: bool,
    pub step: u8,
    pub piano_notes: u8,
    pub ironfish_voices: u8,
    pub drums_active: bool,
    pub dropped_events: u32,
}

pub use crate::ironfish::{
    FilterKind, Ironfish, IronfishParam, IronfishPatch, LfoWave, OscillatorKind, RootNote,
    ScaleKind,
};

pub struct SynthEngines {
    sample_rate: u32,
    piano: Box<Piano>,
    ironfish: Ironfish,
    drums: DrumKit,
    drum_bank: Option<Arc<SampleBank>>,
}

impl SynthEngines {
    pub fn new(sample_rate: u32, patch: IronfishPatch, bank: Option<Arc<SampleBank>>) -> Self {
        let sample_rate = sample_rate.clamp(8_000, 384_000);
        let mut drums = DrumKit::new(sample_rate as f32);
        if let Some(bank) = &bank {
            drums.set_bank(bank.clone());
        }
        Self {
            sample_rate,
            piano: Box::new(Piano::new(sample_rate as f32)),
            ironfish: Ironfish::new(sample_rate as f32, patch),
            drums,
            drum_bank: bank,
        }
    }
}

pub struct SynthRack {
    engines: Box<SynthEngines>,
    patterns: RackPatterns,
    clock: Option<SynthClock>,
    playing: bool,
    current_step: u8,
    last_boundary: Option<u64>,
    active_piano: u16,
    active_ironfish: u16,
    dropped_events: u32,
    scratch: [Vec<[f32; 2]>; 3],
    piano_left: Vec<f32>,
    piano_right: Vec<f32>,
    piano_events: Vec<TimedEvent>,
}

impl SynthRack {
    pub fn new(sample_rate: u32) -> Self {
        let patch = IronfishPatch::default();
        Self {
            engines: Box::new(SynthEngines::new(sample_rate, patch, None)),
            patterns: RackPatterns::default(),
            clock: None,
            playing: false,
            current_step: 0,
            last_boundary: None,
            active_piano: 0,
            active_ironfish: 0,
            dropped_events: 0,
            scratch: std::array::from_fn(|_| vec![[0.0; 2]; MAX_BLOCK]),
            piano_left: vec![0.0; MAX_BLOCK],
            piano_right: vec![0.0; MAX_BLOCK],
            piano_events: Vec::with_capacity(PIANO_EVENT_CAPACITY),
        }
    }

    pub fn replace_engines(&mut self, engines: Box<SynthEngines>) -> Box<SynthEngines> {
        self.active_piano = 0;
        self.active_ironfish = 0;
        std::mem::replace(&mut self.engines, engines)
    }

    pub fn set_drum_bank(&mut self, bank: Arc<SampleBank>) -> Option<Arc<SampleBank>> {
        self.engines.drums.set_bank(bank.clone());
        self.engines.drum_bank.replace(bank)
    }

    pub fn sample_rate(&self) -> u32 {
        self.engines.sample_rate
    }

    pub fn set_clock(&mut self, clock: SynthClock) {
        if clock.valid() {
            self.clock = Some(clock);
        }
    }

    pub fn set_pattern(&mut self, track: SynthTrack, pattern: StepPattern) {
        let row_mask = if track == SynthTrack::Drums {
            (1u16 << DRUM_ROWS) - 1
        } else {
            (1u16 << MELODY_ROWS) - 1
        };
        self.patterns.set(track, pattern.map(|column| column & row_mask));
    }

    pub fn set_patch(&mut self, patch: IronfishPatch) {
        let previous = self.engines.ironfish.patch();
        if previous.root != patch.root || previous.scale != patch.scale {
            self.engines.ironfish.all_notes_off();
            self.active_ironfish = 0;
        }
        self.engines.ironfish.set_patch(patch);
    }

    pub fn set_param(&mut self, param: IronfishParam, value: f32) {
        self.engines.ironfish.set_param(param, value);
    }

    pub fn set_playing(&mut self, playing: bool) {
        if self.playing == playing {
            return;
        }
        self.playing = playing;
        self.last_boundary = None;
        if !playing {
            self.all_notes_off();
        }
    }

    fn all_notes_off(&mut self) {
        let mut left = [0.0];
        let mut right = [0.0];
        self.engines.piano.process(
            &[TimedEvent { offset: 0, event: PianoEvent::AllSoundOff }],
            &mut left,
            &mut right,
        );
        self.engines.ironfish.all_notes_off();
        self.engines.drums.all_off();
        self.active_piano = 0;
        self.active_ironfish = 0;
    }

    fn push_piano_event(&mut self, offset: usize, event: PianoEvent) {
        if self.piano_events.len() < self.piano_events.capacity() {
            self.piano_events.push(TimedEvent { offset: offset as u32, event });
        } else {
            self.dropped_events = self.dropped_events.saturating_add(1);
        }
    }

    fn melody_step(&mut self, track: SynthTrack, mask: u16, offset: usize) {
        let active = match track {
            SynthTrack::Piano => self.active_piano,
            SynthTrack::Ironfish => self.active_ironfish,
            SynthTrack::Drums => return,
        };
        let released = active & !mask;
        let pressed = mask & !active;
        for row in 0..MELODY_ROWS {
            let bit = 1u16 << row;
            let note = if track == SynthTrack::Ironfish {
                self.engines.ironfish.grid_note(row)
            } else {
                48 + row as u8
            };
            if released & bit != 0 {
                match track {
                    SynthTrack::Piano => self.push_piano_event(offset, PianoEvent::NoteOff { key: note }),
                    SynthTrack::Ironfish => self.engines.ironfish.note_off(note),
                    SynthTrack::Drums => {}
                }
            }
            if pressed & bit != 0 {
                match track {
                    SynthTrack::Piano => self.push_piano_event(
                        offset,
                        PianoEvent::NoteOn { key: note, velocity: 88 },
                    ),
                    SynthTrack::Ironfish => self.engines.ironfish.note_on(note, 104),
                    SynthTrack::Drums => {}
                }
            }
        }
        match track {
            SynthTrack::Piano => self.active_piano = mask,
            SynthTrack::Ironfish => self.active_ironfish = mask,
            SynthTrack::Drums => {}
        }
    }

    fn fire_step(&mut self, step: usize, offset: usize) {
        self.current_step = step as u8;
        self.melody_step(SynthTrack::Piano, self.patterns.piano[step], offset);
        self.melody_step(SynthTrack::Ironfish, self.patterns.ironfish[step], offset);
        self.engines.ironfish.clock_step();
        let mask = self.patterns.drums[step];
        for row in 0..DRUM_ROWS {
            if mask & (1 << row) != 0 {
                self.engines.drums.trigger(DRUM_VOICES[row], if row < 2 { 0.92 } else { 0.68 });
            }
        }
    }

    fn boundaries(&self, start: u64, frames: usize) -> ([Option<(u64, usize)>; 8], usize) {
        let mut out = [None; 8];
        let Some(clock) = self.clock.filter(|clock| clock.valid()) else { return (out, 0) };
        let step_frames = clock.frames_per_beat / 4.0;
        let end = start.saturating_add(frames as u64);
        let relative = (start as f64 - clock.beat_frame as f64) / step_frames;
        let mut k = relative.ceil() as i64;
        let mut len = 0;
        while len < out.len() {
            let at = (clock.beat_frame as f64 + k as f64 * step_frames).round().max(0.0) as u64;
            if at >= end {
                break;
            }
            if at >= start {
                let anchor_step = (clock.beat_index as i64 * 4).rem_euclid(STEPS as i64);
                let step = (anchor_step + k).rem_euclid(STEPS as i64) as usize;
                out[len] = Some((at, step));
                len += 1;
            }
            k += 1;
        }
        (out, len)
    }

    pub fn render_block(&mut self, buffer_start: u64, frames: usize, device_rate: f64) {
        let frames = frames.min(MAX_BLOCK);
        for track in &mut self.scratch {
            track[..frames].fill([0.0; 2]);
        }
        self.piano_left[..frames].fill(0.0);
        self.piano_right[..frames].fill(0.0);
        self.piano_events.clear();
        if frames == 0 || (device_rate - self.engines.sample_rate as f64).abs() >= 0.5 {
            return;
        }

        let (boundaries, boundary_len) = if self.playing {
            self.boundaries(buffer_start, frames)
        } else {
            ([None; 8], 0)
        };
        let mut boundary_index = 0;
        for frame in 0..frames {
            let absolute = buffer_start + frame as u64;
            while boundary_index < boundary_len {
                let Some((at, step)) = boundaries[boundary_index] else { break };
                if at > absolute {
                    break;
                }
                boundary_index += 1;
                if self.last_boundary.is_none_or(|last| at > last) {
                    self.fire_step(step, frame);
                    self.last_boundary = Some(at);
                }
            }
            self.scratch[SynthTrack::Ironfish.index()][frame] = self.engines.ironfish.next_frame();
            self.engines.drums.process(std::slice::from_mut(
                &mut self.scratch[SynthTrack::Drums.index()][frame],
            ));
        }
        self.engines.piano.process(
            &self.piano_events,
            &mut self.piano_left[..frames],
            &mut self.piano_right[..frames],
        );
        for frame in 0..frames {
            self.scratch[SynthTrack::Piano.index()][frame] = [
                self.piano_left[frame] * 0.42,
                self.piano_right[frame] * 0.42,
            ];
        }
    }

    pub fn frame(&self, track: SynthTrack, frame: usize) -> [f32; 2] {
        self.scratch[track.index()].get(frame).copied().unwrap_or([0.0; 2])
    }

    pub fn snapshot(&self) -> RackSnapshot {
        RackSnapshot {
            playing: self.playing,
            step: self.current_step,
            piano_notes: self.active_piano.count_ones() as u8,
            ironfish_voices: self.engines.ironfish.active_voices(),
            drums_active: self.engines.drums.active(),
            dropped_events: self.dropped_events,
        }
    }
}

const DRUM_VOICES: [DrumVoice; DRUM_ROWS] = [
    DrumVoice::Kick,
    DrumVoice::Snare,
    DrumVoice::HiHatClosed,
    DrumVoice::HiHatOpen,
    DrumVoice::TomLow,
    DrumVoice::TomHigh,
    DrumVoice::Ride,
    DrumVoice::Crash,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_keep_melody_and_drums_in_their_real_lane_counts() {
        let mut rack = SynthRack::new(48_000);
        rack.set_pattern(SynthTrack::Piano, [u16::MAX; STEPS]);
        rack.set_pattern(SynthTrack::Drums, [u16::MAX; STEPS]);
        assert_eq!(rack.patterns.piano[0], (1 << MELODY_ROWS) - 1);
        assert_eq!(rack.patterns.drums[0], (1 << DRUM_ROWS) - 1);
    }

    #[test]
    fn beat_anchor_produces_exact_sixteenth_boundaries() {
        let mut rack = SynthRack::new(48_000);
        rack.set_clock(SynthClock { beat_frame: 24_000, frames_per_beat: 24_000.0, beat_index: 1 });
        let (events, len) = rack.boundaries(17_900, 256);
        assert_eq!(len, 1);
        assert_eq!(events[0], Some((18_000, 3)));
        let (events, len) = rack.boundaries(23_900, 256);
        assert_eq!(len, 1);
        assert_eq!(events[0], Some((24_000, 4)));
    }

    #[test]
    fn ironfish_extreme_patch_stays_finite() {
        let mut patch = IronfishPatch::default();
        patch.filter.resonance = 99.0;
        patch.delay.feedback = f32::INFINITY;
        patch.filter.cutoff = f32::NAN;
        let mut fish = Ironfish::new(48_000.0, patch);
        fish.note_on(60, 127);
        for _ in 0..96_000 {
            let frame = fish.next_frame();
            assert!(frame[0].is_finite() && frame[1].is_finite());
        }
    }

    #[test]
    fn stop_releases_every_track_without_losing_the_patterns() {
        let mut rack = SynthRack::new(48_000);
        let before = rack.patterns;
        rack.set_playing(true);
        rack.set_playing(false);
        assert_eq!(rack.patterns, before);
        assert_eq!(rack.active_piano, 0);
        assert_eq!(rack.active_ironfish, 0);
    }
}
