//! The arpeggiator: plays the held notes as a pattern on the tempo (up,
//! down, up-down, down-up, converge, diverge, thumb, pinky, as played,
//! random, chord), over octaves, with gate, swing, repeats (ratchets),
//! chance and a transpose per octave pass. One implementation: Morph's
//! built-in arp and a score's `Arp` node (any note stream, a MIDI keyboard
//! included) both run this, and loop code's `arp()` and the arpeggio
//! generator take their step order from [`order`].
//!
//! Real-time: nothing here allocates; [`Arp::advance`] reports each start
//! and stop with its offset in beats, so a caller places it on its sample.

use super::schema::{Builder, Unit::Percent, Values};
use crate::dsp::Rng;

/// The patterns, by the name a patch or a score writes (`@updown`).
pub const PATTERNS: &[&str] = &["up", "down", "updown", "downup", "converge", "diverge", "thumb", "pinky", "played", "random", "chord"];
/// Indices of the patterns that are not a plain order.
pub const PLAYED: usize = 8;
pub const RANDOM: usize = 9;
pub const CHORD: usize = 10;
pub const RATES: &[&str] = &["n4", "n8", "n8t", "n16", "n16t", "n32"];
pub const RATE_BEATS: [f32; 6] = [1.0, 0.5, 1.0 / 3.0, 0.25, 1.0 / 6.0, 0.125];
pub const MAX_HELD: usize = 32;
/// The most steps a pattern holds (held notes over octaves, there and back).
pub const MAX_SEQ: usize = 256;

/// The pattern named `name` (`up`, `updown`, …).
pub fn pattern(name: &str) -> Option<usize> {
    PATTERNS.iter().position(|p| *p == name)
}

/// The order pattern `pattern` plays `n` notes in, as indices into them
/// (lowest first; as played for [`PLAYED`]), into `out`: how many steps.
/// Random and chord step through them in order (the caller picks a random
/// one, or sounds them all, at each step).
pub fn order(pattern: usize, n: usize, out: &mut [u16]) -> usize {
    fn push(out: &mut [u16], k: &mut usize, i: usize) {
        if *k < out.len() {
            out[*k] = i as u16;
            *k += 1;
        }
    }
    let mut k = 0;
    match pattern {
        1 => (0..n).rev().for_each(|i| push(out, &mut k, i)),
        2 => (0..n).chain((1..n.saturating_sub(1)).rev()).for_each(|i| push(out, &mut k, i)),
        3 => (0..n).rev().chain(1..n.saturating_sub(1)).for_each(|i| push(out, &mut k, i)),
        4 | 5 => {
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                push(out, &mut k, lo);
                lo += 1;
                if lo < hi {
                    hi -= 1;
                    push(out, &mut k, hi);
                }
            }
            if pattern == 5 {
                out[..k].reverse();
            }
        }
        6 | 7 if n > 0 => {
            let pivot = if pattern == 6 { 0 } else { n - 1 };
            for i in (0..n).filter(|i| *i != pivot) {
                push(out, &mut k, pivot);
                push(out, &mut k, i);
            }
            if k == 0 {
                push(out, &mut k, pivot);
            }
        }
        _ => (0..n).for_each(|i| push(out, &mut k, i)),
    }
    k
}

/// What an arpeggiator plays: the step order, each step's length in beats,
/// over how many octaves (each pass `transpose` semitones up), the gate as
/// a fraction of a step, swing (the off steps late by this fraction of a
/// step), repeats inside a step, and the chance a step plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArpSettings {
    pub pattern: usize,
    pub step: f64,
    pub octaves: usize,
    pub gate: f32,
    pub swing: f32,
    pub repeats: usize,
    pub chance: f32,
    pub transpose: i32,
}

impl Default for ArpSettings {
    fn default() -> Self {
        ArpSettings { pattern: 0, step: 0.25, octaves: 1, gate: 0.6, swing: 0.0, repeats: 1, chance: 1.0, transpose: 12 }
    }
}

impl ArpSettings {
    /// A patch's arp settings (Morph's `arp_*` parameters).
    pub fn from_values(v: &(impl Values + ?Sized), x: &ArpIx) -> ArpSettings {
        ArpSettings {
            pattern: v.index(x.pattern),
            step: RATE_BEATS[v.index(x.rate).min(RATE_BEATS.len() - 1)] as f64,
            octaves: v.val(x.octaves).max(1.0) as usize,
            gate: v.val(x.gate),
            swing: v.val(x.swing),
            repeats: v.val(x.repeats).clamp(1.0, 4.0) as usize,
            chance: v.val(x.chance),
            transpose: v.val(x.transpose) as i32,
        }
    }

    fn clamped(&self) -> ArpSettings {
        ArpSettings {
            pattern: self.pattern.min(PATTERNS.len() - 1),
            step: if self.step.is_finite() { self.step.clamp(1.0 / 64.0, 16.0) } else { 0.25 },
            octaves: self.octaves.clamp(1, 4),
            gate: self.gate.clamp(0.01, 1.0),
            swing: self.swing.clamp(0.0, 0.5),
            repeats: self.repeats.clamp(1, 4),
            chance: self.chance.clamp(0.0, 1.0),
            transpose: self.transpose.clamp(-24, 24),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ArpIx {
    pub on: usize,
    pub pattern: usize,
    pub rate: usize,
    pub octaves: usize,
    pub gate: usize,
    pub swing: usize,
    pub repeats: usize,
    pub chance: usize,
    pub transpose: usize,
}

pub fn declare(b: &mut Builder) -> ArpIx {
    b.group("arp", "an arpeggiator over the held notes, on the score's tempo");
    ArpIx {
        on: b.flag("arp_on".into(), "arp", false, ""),
        pattern: b.choice("arp_pattern".into(), "pattern", PATTERNS, 0, ""),
        rate: b.choice("arp_rate".into(), "rate", RATES, 3, ""),
        octaves: b.int("arp_octaves".into(), "octaves", 1.0, 4.0, 1.0, ""),
        gate: b.num("arp_gate".into(), "gate", 0.05, 1.0, 0.6, Percent, ""),
        swing: b.num("arp_swing".into(), "swing", 0.0, 0.5, 0.0, Percent, ""),
        repeats: b.int("arp_repeats".into(), "repeats", 1.0, 4.0, 1.0, "each step played this many times inside its length"),
        chance: b.num("arp_chance".into(), "chance", 0.0, 1.0, 1.0, Percent, "each step plays with this chance"),
        transpose: b.int("arp_transpose".into(), "transpose", -24.0, 24.0, 12.0, "semitones each octave pass adds"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ArpEvent {
    On(u8, u8),
    Off(u8),
}

pub struct Arp {
    held: [(u8, u8); MAX_HELD],
    nheld: usize,
    seq: [u8; MAX_SEQ],
    vel: [u8; MAX_SEQ],
    nseq: usize,
    dirty: bool,
    /// The settings the sequence was built for (a change rebuilds it).
    built: (usize, usize, i32),
    step: usize,
    /// Steps before the first, for the swing's off steps (a synced arp
    /// starts on the grid's own step).
    parity: usize,
    /// Beats into the current step (negative: before the first). Time is
    /// kept in f64: an arp on a score's clock stays on its grid for hours.
    at: f64,
    sounding: [u8; MAX_HELD],
    nsounding: usize,
    sub: usize,
    gate_end: f64,
    rng: Rng,
}

impl Default for Arp {
    fn default() -> Self {
        Arp::new()
    }
}

impl Arp {
    pub fn new() -> Arp {
        Arp {
            held: [(0, 0); MAX_HELD], nheld: 0, seq: [0; MAX_SEQ], vel: [0; MAX_SEQ], nseq: 0, dirty: false, built: (usize::MAX, 0, 0), step: 0, parity: 0, at: 0.0,
            sounding: [0; MAX_HELD], nsounding: 0, sub: 0, gate_end: 0.0, rng: Rng::new(0x51ed_270b),
        }
    }

    /// Every note let go (what sounds is stopped by the next advance).
    pub fn clear(&mut self) {
        self.nheld = 0;
        self.dirty = true;
    }

    pub fn held(&self) -> usize {
        self.nheld
    }

    /// The notes held, as pressed (note, velocity).
    pub fn held_notes(&self) -> &[(u8, u8)] {
        &self.held[..self.nheld]
    }

    /// The pattern as built for the notes held (each step's note), and the
    /// step playing.
    pub fn steps(&self) -> (&[u8], usize) {
        (&self.seq[..self.nseq], if self.nseq > 0 { self.step % self.nseq } else { 0 })
    }

    pub fn press(&mut self, note: u8, vel: u8) {
        if self.nheld == 0 {
            // A fresh chord: start from the first step now.
            self.step = 0;
            self.parity = 0;
            self.at = 0.0;
            self.sub = 0;
        }
        if self.nheld < MAX_HELD && !self.held[..self.nheld].iter().any(|h| h.0 == note) {
            self.held[self.nheld] = (note, vel);
            self.nheld += 1;
        }
        self.dirty = true;
    }

    /// Put the arp on a grid of `s.step` beats, `phase` beats into a step
    /// (`grid_step` the grid's step number there): a fresh chord pressed
    /// early in a step plays its first note at once, one pressed late
    /// waits for the next step.
    pub fn align(&mut self, s: &ArpSettings, phase: f64, grid_step: u64) {
        let s = s.clamped();
        self.at = phase.clamp(0.0, s.step);
        self.parity = (grid_step % 2) as usize;
        if self.nsounding == 0 && self.nheld > 0 && self.step == 0 && self.sub == 0 && phase > s.step * 0.5 {
            self.sub = s.repeats;
        }
    }

    pub fn release(&mut self, note: u8) {
        if let Some(i) = self.held[..self.nheld].iter().position(|h| h.0 == note) {
            self.held.copy_within(i + 1..self.nheld, i);
            self.nheld -= 1;
            self.dirty = true;
        }
    }

    fn rebuild(&mut self, s: &ArpSettings) {
        self.dirty = false;
        self.built = (s.pattern, s.octaves, s.transpose);
        // The notes over the octaves, lowest first (as played for `played`).
        let mut base = [(0u8, 0u8); MAX_SEQ];
        let mut n = 0;
        let mut one = [(0u8, 0u8); MAX_HELD];
        one[..self.nheld].copy_from_slice(&self.held[..self.nheld]);
        let one = &mut one[..self.nheld];
        if s.pattern != PLAYED {
            one.sort_unstable_by_key(|h| h.0);
        }
        for o in 0..s.octaves {
            for (note, vel) in one.iter() {
                if n < MAX_SEQ {
                    base[n] = ((*note as i32 + o as i32 * s.transpose).clamp(0, 127) as u8, *vel);
                    n += 1;
                }
            }
        }
        let mut idx = [0u16; MAX_SEQ];
        self.nseq = order(s.pattern, n, &mut idx);
        for (k, i) in idx[..self.nseq].iter().enumerate() {
            (self.seq[k], self.vel[k]) = base[*i as usize];
        }
        if self.step >= self.nseq.max(1) {
            self.step = 0;
        }
    }

    /// Advance `beats`: the notes to start and stop, with their offsets in
    /// beats from now, into `out` (returns how many).
    pub fn advance(&mut self, s: &ArpSettings, beats: f64, out: &mut [(f64, ArpEvent)]) -> usize {
        let s = s.clamped();
        if self.dirty || self.built != (s.pattern, s.octaves, s.transpose) {
            self.rebuild(&s);
        }
        let mut n = 0;
        fn emit(out: &mut [(f64, ArpEvent)], n: &mut usize, at: f64, e: ArpEvent) {
            if *n < out.len() {
                out[*n] = (at, e);
                *n += 1;
            }
        }
        if self.nheld == 0 || self.nseq == 0 {
            for i in 0..self.nsounding {
                emit(out, &mut n, 0.0, ArpEvent::Off(self.sounding[i]));
            }
            self.nsounding = 0;
            return n;
        }
        let step_len = s.step;
        let repeats = s.repeats;
        let sub_len = step_len / repeats as f64;
        let gate = s.gate as f64 * sub_len;
        let mut left = beats.max(0.0);
        for _ in 0..256 {
            let swing = if (self.step + self.parity) % 2 == 1 { s.swing as f64 * step_len } else { 0.0 };
            let hit = if self.sub < repeats { swing + self.sub as f64 * sub_len } else { f64::INFINITY };
            let off = if self.nsounding > 0 { self.gate_end } else { f64::INFINITY };
            let t = hit.min(off).min(step_len).max(self.at);
            // What falls on the end belongs to the next advance.
            if self.at + left <= t {
                self.at += left;
                break;
            }
            left -= (t - self.at).max(0.0);
            self.at = t;
            let now = beats - left;
            if off <= t {
                for i in 0..self.nsounding {
                    emit(out, &mut n, now, ArpEvent::Off(self.sounding[i]));
                }
                self.nsounding = 0;
            } else if hit <= t {
                self.sub += 1;
                if self.rng.unit() < s.chance {
                    for i in 0..self.nsounding {
                        emit(out, &mut n, now, ArpEvent::Off(self.sounding[i]));
                    }
                    self.nsounding = 0;
                    if s.pattern == CHORD {
                        for h in 0..self.nheld {
                            let (note, vel) = self.held[h];
                            emit(out, &mut n, now, ArpEvent::On(note, vel));
                            self.sounding[self.nsounding] = note;
                            self.nsounding += 1;
                        }
                    } else {
                        let i = if s.pattern == RANDOM { (self.rng.next_u32() as usize) % self.nseq } else { self.step % self.nseq };
                        emit(out, &mut n, now, ArpEvent::On(self.seq[i], self.vel[i]));
                        self.sounding[0] = self.seq[i];
                        self.nsounding = 1;
                    }
                    self.gate_end = t + gate;
                }
            } else {
                self.at = 0.0;
                self.sub = 0;
                self.step = (self.step + 1) % self.nseq.max(1);
                self.gate_end -= step_len;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(name: &str, n: usize) -> Vec<u16> {
        let mut out = [0u16; 64];
        let k = order(pattern(name).unwrap(), n, &mut out);
        out[..k].to_vec()
    }

    #[test]
    fn every_pattern_orders_the_notes() {
        assert_eq!(steps("up", 4), [0, 1, 2, 3]);
        assert_eq!(steps("down", 4), [3, 2, 1, 0]);
        assert_eq!(steps("updown", 4), [0, 1, 2, 3, 2, 1]);
        assert_eq!(steps("downup", 4), [3, 2, 1, 0, 1, 2]);
        assert_eq!(steps("converge", 4), [0, 3, 1, 2]);
        assert_eq!(steps("diverge", 4), [2, 1, 3, 0]);
        assert_eq!(steps("thumb", 3), [0, 1, 0, 2]);
        assert_eq!(steps("pinky", 3), [2, 0, 2, 1]);
        assert_eq!(steps("played", 3), [0, 1, 2]);
        assert_eq!(steps("updown", 1), [0]);
        assert_eq!(steps("thumb", 1), [0]);
    }

    #[test]
    fn steps_land_on_their_offsets() {
        let s = ArpSettings { step: 0.25, gate: 0.5, ..Default::default() };
        let mut arp = Arp::new();
        arp.press(64, 100);
        arp.press(60, 90);
        let mut out = [(0.0, ArpEvent::Off(0)); 16];
        let n = arp.advance(&s, 0.6, &mut out);
        let got: Vec<(f64, ArpEvent)> = out[..n].to_vec();
        assert_eq!(got, vec![
            (0.0, ArpEvent::On(60, 90)), (0.125, ArpEvent::Off(60)),
            (0.25, ArpEvent::On(64, 100)), (0.375, ArpEvent::Off(64)),
            (0.5, ArpEvent::On(60, 90)),
        ]);
    }

    #[test]
    fn a_synced_arp_waits_for_the_grid_when_pressed_late() {
        let s = ArpSettings { step: 0.25, gate: 0.5, ..Default::default() };
        let mut arp = Arp::new();
        arp.press(60, 90);
        arp.align(&s, 0.1875, 7);
        let mut out = [(0.0, ArpEvent::Off(0)); 16];
        let n = arp.advance(&s, 0.125, &mut out);
        // A sixteenth of a beat to the next grid step.
        assert_eq!(&out[..n], &[(0.0625f64, ArpEvent::On(60, 90))]);
    }
}
