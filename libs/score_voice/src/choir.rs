//! A choir: several singers sharing one line or split into harmony parts.
//! Each singer is its own voice with its own seed, detune, timing offset,
//! vocal-tract size, vibrato and profile (cycled from the list given), and
//! its own place in the stereo field. A one-singer choir is a solo voice.
//!
//! Realtime: everything is allocated in `Choir::new`; `note_on`, `release`
//! and `render_stereo` never allocate. `render_stereo` ADDS.

use crate::pitch::{Rng, Style};
use crate::profile::VoiceProfile;
use crate::singer::{Singer, SungNote};
use std::sync::Arc;

pub const MAX_SINGERS: usize = 8;
/// Every part sings the note (unison).
pub const ALL_PARTS: u8 = u8::MAX;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChoirStyle {
    pub base: Style,
    /// Harmony parts the singers split into (1 = everyone in unison).
    pub parts: u8,
    /// Spread of the singers' fixed detune (cents, ±), centred on each
    /// part's pitch.
    pub detune_cents: f32,
    /// Largest random delay of a singer's entry (seconds).
    pub timing_jitter: f32,
    /// Spread of vocal-tract size (± fraction of the formant scale).
    pub formant_spread: f32,
    /// Stereo spread 0..1.
    pub width: f32,
}

impl Default for ChoirStyle {
    fn default() -> Self {
        ChoirStyle { base: Style::default(), parts: 1, detune_cents: 9.0, timing_jitter: 0.025, formant_spread: 0.04, width: 0.7 }
    }
}

struct Member {
    singer: Singer,
    part: u8,
    pan_l: f32,
    pan_r: f32,
    /// The choir serial each of this singer's notes answers to.
    choir_serial: u32,
    own_serial: u32,
    /// This singer's entry delay for the current note (frames).
    delay: u32,
}

pub struct Choir {
    members: Box<[Member]>,
    scratch: Box<[f32]>,
    rng: Rng,
    rate: f32,
    jitter: f32,
    next_serial: u32,
    gain: f32,
}

const SCRATCH: usize = 1024;

impl Choir {
    /// `singers` voices (at most [`MAX_SINGERS`]), their profiles cycled
    /// from `profiles`.
    pub fn new(profiles: &[Arc<VoiceProfile>], singers: usize, rate: f32, style: ChoirStyle, seed: u32) -> Choir {
        assert!(!profiles.is_empty(), "a choir needs at least one profile");
        let n = singers.clamp(1, MAX_SINGERS);
        let parts = style.parts.max(1);
        let mut rng = Rng::new(seed ^ 0xC401);
        // Each singer's detune, then each part's spread centred on the
        // part's pitch: the section sings in tune, its singers around it (a
        // part of one singer sings the note).
        let mut detune: Vec<f32> = (0..n).map(|_| if n > 1 { rng.bipolar() * style.detune_cents } else { 0.0 }).collect();
        for part in 0..parts as usize {
            let members: Vec<usize> = (part..n).step_by(parts as usize).collect();
            let mean = members.iter().map(|i| detune[*i]).sum::<f32>() / members.len().max(1) as f32;
            for i in members {
                detune[i] -= mean;
            }
        }
        let members = (0..n)
            .map(|i| {
                let mut s = style.base;
                if n > 1 {
                    s.detune_cents += detune[i];
                    s.formant_scale *= 1.0 + rng.bipolar() * style.formant_spread;
                    s.vibrato_rate *= 1.0 + rng.bipolar() * 0.08;
                    s.vibrato_depth *= 1.0 + rng.bipolar() * 0.25;
                    s.breath = (s.breath + rng.unit() * 0.15).min(1.0);
                }
                // Spread across the field, alternating sides.
                let pan = if n == 1 { 0.0 } else { style.width * (i as f32 / (n - 1) as f32 * 2.0 - 1.0) };
                let p = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
                Member {
                    singer: Singer::new(profiles[i % profiles.len()].clone(), rate, s, seed.wrapping_add(i as u32 * 7919)),
                    part: (i % parts as usize) as u8, pan_l: p.cos(), pan_r: p.sin(), choir_serial: 0, own_serial: 0, delay: 0,
                }
            })
            .collect();
        Choir {
            members, scratch: vec![0.0; SCRATCH].into_boxed_slice(), rng, rate, jitter: if n > 1 { style.timing_jitter } else { 0.0 },
            next_serial: 1, gain: 1.0 / (n as f32).sqrt(),
        }
    }

    pub fn singers(&self) -> usize {
        self.members.len()
    }

    /// Sing `note` from `offset` frames into the next render, on `part`
    /// (or [`ALL_PARTS`]). Returns a serial for `release`.
    pub fn note_on(&mut self, offset: u32, note: SungNote, part: u8) -> u32 {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        let parts = self.members.iter().map(|m| m.part).max().unwrap_or(0) + 1;
        let part = if part == ALL_PARTS { ALL_PARTS } else { part % parts };
        let jitter = self.jitter * self.rate;
        for m in self.members.iter_mut() {
            if part != ALL_PARTS && m.part != part {
                continue;
            }
            // A slurred note joins at once; a fresh entry is a little late.
            let delay = if note.syl.flags & crate::singer::PhonemeSeq::LEGATO != 0 { 0 } else { (self.rng.unit() * jitter) as u32 };
            m.delay = delay;
            m.own_serial = m.singer.note_on(offset + delay, note);
            m.choir_serial = serial;
        }
        serial
    }

    pub fn release(&mut self, offset: u32, serial: u32) {
        for m in self.members.iter_mut() {
            if m.choir_serial == serial {
                m.singer.release(offset + m.delay, m.own_serial);
            }
        }
    }

    pub fn release_all(&mut self, offset: u32) {
        for m in self.members.iter_mut() {
            m.singer.release_all(offset);
        }
    }

    pub fn is_active(&self) -> bool {
        self.members.iter().any(|m| m.singer.is_active())
    }

    /// Output gain of every singer (a knob).
    pub fn set_gain(&mut self, gain: f32) {
        for m in self.members.iter_mut() {
            m.singer.style.gain = gain;
        }
    }

    /// ADD the choir into `l` / `r`.
    pub fn render_stereo(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        let mut at = 0;
        while at < n {
            let len = (n - at).min(SCRATCH);
            for m in self.members.iter_mut() {
                let scratch = &mut self.scratch[..len];
                scratch.fill(0.0);
                m.singer.render(scratch);
                let (gl, gr) = (m.pan_l * self.gain, m.pan_r * self.gain);
                for i in 0..len {
                    l[at + i] += scratch[i] * gl;
                    r[at + i] += scratch[i] * gr;
                }
            }
            at += len;
        }
    }

    /// ADD the choir, mono.
    pub fn render(&mut self, out: &mut [f32]) {
        let n = out.len();
        let mut at = 0;
        while at < n {
            let len = (n - at).min(SCRATCH);
            for m in self.members.iter_mut() {
                let scratch = &mut self.scratch[..len];
                scratch.fill(0.0);
                m.singer.render(scratch);
                for i in 0..len {
                    out[at + i] += scratch[i] * self.gain;
                }
            }
            at += len;
        }
    }
}
