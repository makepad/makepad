//! A polyphonic synth whose patch is a [`Recipe`]: note-on starts a held
//! recipe voice re-pitched to the note, note-off releases it. Any sound an
//! author can write as a recipe is therefore also a playable instrument,
//! and the built-in patches below cover the usual band.

use crate::dsp::{finite, midi_to_hz};
use crate::instrument::{Control, Instrument};
use crate::recipe::{Recipe, RecipeVoice};
use crate::reverb::Reverb;

pub const POLY_VOICES: usize = 16;

/// Built-in patches (layer `freq` is relative to A4 = 440 Hz).
pub fn patch(name: &str) -> Option<&'static str> {
    Some(match name {
        "lead" => "saw freq=440 detune=6 a=0.005 d=0.3 s=0.7 r=0.15 gain=0.25 ladder=3200 q=3 vib=5.5 vib_depth=0.08; saw freq=440 detune=-6 a=0.005 d=0.3 s=0.7 r=0.15 gain=0.25 ladder=3200 q=3",
        "bass" => "saw freq=440 a=0.002 d=0.25 s=0.6 r=0.08 gain=0.35 ladder=1200>500 glide=0.2 q=5 drive=1.5; sine freq=220 a=0.002 d=0.2 s=0.8 r=0.08 gain=0.35",
        "sub" | "808" => "sine freq=440 a=0.002 d=0.9 s=0.3 r=0.2 gain=0.6 drive=1; sine freq=880>440 glide=0.03 d=0.03 gain=0.2",
        "pad" | "strings" => "reverb=0.4; saw freq=440 detune=9 a=0.5 d=1.0 s=0.8 r=1.2 gain=0.16 lp=2200 vib=4 vib_depth=0.05; saw freq=440 detune=-9 a=0.55 d=1.0 s=0.8 r=1.2 gain=0.16 lp=2200; tri freq=880 a=0.6 d=1.0 s=0.6 r=1.2 gain=0.08",
        "pluck" | "harp" | "guitar" => "reverb=0.25; pluck=0.35 freq=440 a=0.001 d=2.5 s=0 r=0.3 gain=0.5",
        "bell" | "glock" => "reverb=0.4; fm=3.5 osc=sine freq=440 index=3>0.1 glide=1.5 a=0.001 d=2.5 r=0.8 gain=0.3; sine freq=1760 d=0.8 gain=0.05",
        "epiano" | "rhodes" => "reverb=0.25; fm=1 osc=sine freq=440 index=2.2>0.3 glide=1.0 a=0.002 d=2.0 s=0.2 r=0.4 gain=0.3; fm=14 osc=sine freq=440 index=1.2>0 glide=0.1 d=0.15 gain=0.06",
        "organ" => "reverb=0.3; sine freq=440 a=0.005 d=0.05 s=1 r=0.06 gain=0.18; sine freq=880 a=0.005 d=0.05 s=1 r=0.06 gain=0.12; sine freq=1320 a=0.005 d=0.05 s=1 r=0.06 gain=0.08; sine freq=220 a=0.005 d=0.05 s=1 r=0.06 gain=0.12; noise=white bp=3000 d=0.01 gain=0.03",
        "brass" => "reverb=0.25; saw freq=440 a=0.04 d=0.3 s=0.75 r=0.12 gain=0.3 ladder=600>3000 glide=0.08 q=1.5 drive=1 vib=5 vib_depth=0.06",
        "flute" => "reverb=0.3; sine freq=440 a=0.06 d=0.2 s=0.85 r=0.15 gain=0.35 vib=5 vib_depth=0.1; noise=pink bp=2000 q=2 a=0.04 d=0.2 s=0.3 r=0.1 gain=0.05",
        "chip" | "8bit" => "reverb=0.1; pulse width=0.25 freq=440 a=0.001 d=0.1 s=0.6 r=0.05 gain=0.18 lp=9000",
        "choir" | "voice" => "reverb=0.45; saw freq=440 detune=5 a=0.25 d=0.5 s=0.8 r=0.6 gain=0.18 bp=800 q=2 vib=5 vib_depth=0.1; saw freq=440 detune=-5 a=0.25 d=0.5 s=0.8 r=0.6 gain=0.14 bp=1200 q=3; saw freq=440 a=0.25 d=0.5 s=0.8 r=0.6 gain=0.08 bp=2600 q=4",
        "kalimba" | "marimba" => "reverb=0.25; sine freq=440 a=0.001 d=0.6 gain=0.4; sine freq=1760 d=0.08 gain=0.1; noise=pink bp=2000 q=3 d=0.01 gain=0.05",
        _ => return None,
    })
}

pub const PATCH_NAMES: &[&str] = &[
    "lead", "bass", "sub", "pad", "pluck", "bell", "epiano", "organ", "brass", "flute", "chip", "choir", "kalimba",
];

pub struct PolySynth {
    rate: f32,
    patch: Recipe,
    voices: [Option<(u8, RecipeVoice)>; POLY_VOICES],
    serial: [u32; POLY_VOICES],
    next_serial: u32,
    sustain: bool,
    sustained: [bool; 128],
    bend: f32,
    volume: f32,
    reverb: Reverb,
    mono: [f32; 256],
    send: [f32; 256],
}

impl PolySynth {
    pub fn new(rate: f32, patch: Recipe) -> Self {
        let rate = finite(rate, 48_000.0).clamp(8_000.0, 384_000.0);
        let mut reverb = Reverb::new(rate);
        reverb.snap_room(crate::reverb::RoomPreset::Hall.params());
        PolySynth {
            rate,
            patch,
            voices: [None; POLY_VOICES],
            serial: [0; POLY_VOICES],
            next_serial: 1,
            sustain: false,
            sustained: [false; 128],
            bend: 1.0,
            volume: 1.0,
            reverb,
            mono: [0.0; 256],
            send: [0.0; 256],
        }
    }

    pub fn set_patch(&mut self, patch: Recipe) {
        self.patch = patch;
    }
}

impl Instrument for PolySynth {
    fn name(&self) -> &str {
        "synth"
    }

    fn sample_rate(&self) -> f32 {
        self.rate
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        if velocity == 0 {
            return self.note_off(note);
        }
        let note = note.min(127);
        // Re-strike the same note in its slot; else a free slot; else the
        // oldest.
        let slot = self
            .voices
            .iter()
            .position(|v| matches!(v, Some((n, _)) if *n == note))
            .or_else(|| self.voices.iter().position(|v| v.is_none()))
            .unwrap_or_else(|| (0..POLY_VOICES).min_by_key(|&i| self.serial[i]).unwrap_or(0));
        let vel = velocity as f32 / 127.0;
        let mut voice = RecipeVoice::new(&self.patch, 1.0, Some(midi_to_hz(note as f32)), true, self.next_serial);
        voice.recipe.volume *= 0.25 + 0.75 * vel * vel;
        self.voices[slot] = Some((note, voice));
        self.serial[slot] = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1);
        self.sustained[note as usize] = false;
    }

    fn note_off(&mut self, note: u8) {
        if self.sustain {
            self.sustained[note.min(127) as usize] = true;
            return;
        }
        for (n, v) in self.voices.iter_mut().flatten() {
            if *n == note && v.held {
                v.release();
            }
        }
    }

    fn control(&mut self, control: Control, value: f32) {
        match control {
            Control::Sustain => {
                self.sustain = value >= 0.5;
                if !self.sustain {
                    for note in 0..128u8 {
                        if self.sustained[note as usize] {
                            self.sustained[note as usize] = false;
                            self.note_off(note);
                        }
                    }
                }
            }
            Control::PitchBend => self.bend = 2f32.powf(finite(value, 0.0).clamp(-12.0, 12.0) / 12.0),
            Control::Volume => self.volume = finite(value, 1.0).clamp(0.0, 2.0),
            _ => {}
        }
    }

    fn all_notes_off(&mut self) {
        self.sustain = false;
        self.sustained = [false; 128];
        for (_, v) in self.voices.iter_mut().flatten() {
            v.release();
        }
    }

    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len().min(out_r.len());
        let mut at = 0;
        while at < n {
            let len = (n - at).min(256);
            let mono = &mut self.mono[..len];
            mono.fill(0.0);
            for slot in self.voices.iter_mut() {
                if let Some((_, v)) = slot {
                    v.render(mono, self.rate, self.bend);
                    if v.finished() {
                        *slot = None;
                    }
                }
            }
            let send = self.patch.reverb;
            for i in 0..len {
                let x = mono[i] * self.volume;
                self.send[i] = x * send;
                out_l[at + i] = x;
                out_r[at + i] = x;
            }
            self.reverb.process(&self.send[..len], &mut out_l[at..at + len], &mut out_r[at..at + len]);
            at += len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_patch_plays_a_note_and_releases() {
        for name in PATCH_NAMES {
            let p = Recipe::parse(patch(name).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
            let mut s = PolySynth::new(48_000.0, p);
            s.note_on(60, 100);
            let (mut l, mut r) = (vec![0.0; 24_000], vec![0.0; 24_000]);
            s.render(&mut l, &mut r);
            let peak = l.iter().fold(0.0f32, |a, x| a.max(x.abs()));
            assert!(peak > 0.01, "{name} peak {peak}");
            s.note_off(60);
            let (mut l, mut r) = (vec![0.0; 48_000 * 4], vec![0.0; 48_000 * 4]);
            s.render(&mut l, &mut r);
            assert!(s.voices.iter().all(|v| v.is_none()), "{name} never ended");
        }
    }
}
