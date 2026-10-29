//! The kit's arpeggiator: plays the held notes as a pattern on the tempo
//! (up, down, up-down, down-up, converge, diverge, thumb, pinky, as played,
//! random, chord), over octaves, with gate, swing, repeats (ratchets),
//! chance and a transpose per octave pass.

use super::schema::{Builder, Unit::Percent, Values};
use crate::dsp::Rng;

pub const PATTERNS: &[&str] = &["up", "down", "updown", "downup", "converge", "diverge", "thumb", "pinky", "order", "random", "chord"];
pub const RATES: &[&str] = &["n4", "n8", "n8t", "n16", "n16t", "n32"];
pub const RATE_BEATS: [f32; 6] = [1.0, 0.5, 1.0 / 3.0, 0.25, 1.0 / 6.0, 0.125];
pub const MAX_HELD: usize = 32;
const MAX_SEQ: usize = 256;

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
    step: usize,
    /// Beats into the current step (negative: before the first).
    at: f32,
    sounding: [u8; MAX_HELD],
    nsounding: usize,
    sub: usize,
    gate_end: f32,
    rng: Rng,
}

impl Default for Arp {
    fn default() -> Self {
        Arp::new()
    }
}

impl Arp {
    pub fn new() -> Arp {
        Arp { held: [(0, 0); MAX_HELD], nheld: 0, seq: [0; MAX_SEQ], vel: [0; MAX_SEQ], nseq: 0, dirty: false, step: 0, at: 0.0, sounding: [0; MAX_HELD], nsounding: 0, sub: 0, gate_end: 0.0, rng: Rng::new(0x51ed_270b) }
    }

    /// Every note let go (what sounds is stopped by the next advance).
    pub fn clear(&mut self) {
        self.nheld = 0;
        self.dirty = true;
    }

    pub fn held(&self) -> usize {
        self.nheld
    }

    pub fn press(&mut self, note: u8, vel: u8) {
        if self.nheld == 0 {
            // A fresh chord: start from the first step now.
            self.step = 0;
            self.at = 0.0;
            self.sub = 0;
        }
        if self.nheld < MAX_HELD && !self.held[..self.nheld].iter().any(|h| h.0 == note) {
            self.held[self.nheld] = (note, vel);
            self.nheld += 1;
        }
        self.dirty = true;
    }

    pub fn release(&mut self, note: u8) {
        if let Some(i) = self.held[..self.nheld].iter().position(|h| h.0 == note) {
            self.held.copy_within(i + 1..self.nheld, i);
            self.nheld -= 1;
            self.dirty = true;
        }
    }

    fn rebuild(&mut self, pattern: usize, octaves: usize, transpose: i32) {
        self.dirty = false;
        let mut sorted: Vec<(u8, u8)> = self.held[..self.nheld].to_vec();
        let order = sorted.clone();
        sorted.sort_by_key(|h| h.0);
        let mut base: Vec<(u8, u8)> = Vec::with_capacity(MAX_SEQ);
        for o in 0..octaves.max(1) {
            let src = if pattern == 8 { &order } else { &sorted };
            for (n, v) in src {
                let note = (*n as i32 + o as i32 * transpose).clamp(0, 127) as u8;
                base.push((note, *v));
            }
        }
        let seq: Vec<(u8, u8)> = match pattern {
            1 => base.iter().rev().copied().collect(),
            2 => {
                let mut s = base.clone();
                s.extend(base.iter().rev().skip(1).take(base.len().saturating_sub(2)));
                s
            }
            3 => {
                let mut s: Vec<_> = base.iter().rev().copied().collect();
                s.extend(base.iter().skip(1).take(base.len().saturating_sub(2)));
                s
            }
            4 | 5 => {
                let mut s = Vec::with_capacity(base.len());
                let (mut lo, mut hi) = (0usize, base.len());
                while lo < hi {
                    s.push(base[lo]);
                    lo += 1;
                    if lo < hi {
                        hi -= 1;
                        s.push(base[hi]);
                    }
                }
                if pattern == 5 {
                    s.reverse();
                }
                s
            }
            6 | 7 => {
                let pivot = if pattern == 6 { base.first().copied() } else { base.last().copied() };
                let mut s = Vec::new();
                if let Some(p) = pivot {
                    for n in &base {
                        if *n != p {
                            s.push(p);
                            s.push(*n);
                        }
                    }
                    if s.is_empty() {
                        s.push(p);
                    }
                }
                s
            }
            _ => base,
        };
        self.nseq = seq.len().min(MAX_SEQ);
        for (i, (n, v)) in seq.iter().take(MAX_SEQ).enumerate() {
            self.seq[i] = *n;
            self.vel[i] = *v;
        }
        if self.step >= self.nseq.max(1) {
            self.step = 0;
        }
    }

    /// Advance `beats`; the notes to start and stop, into `out` (returns how many).
    pub fn advance(&mut self, v: &(impl Values + ?Sized), x: &ArpIx, beats: f32, out: &mut [ArpEvent]) -> usize {
        let pattern = v.index(x.pattern);
        if self.dirty {
            self.rebuild(pattern, v.val(x.octaves).max(1.0) as usize, v.val(x.transpose) as i32);
        }
        let mut n = 0;
        fn emit(out: &mut [ArpEvent], n: &mut usize, e: ArpEvent) {
            if *n < out.len() {
                out[*n] = e;
                *n += 1;
            }
        }
        if self.nheld == 0 || self.nseq == 0 {
            for i in 0..self.nsounding {
                emit(out, &mut n, ArpEvent::Off(self.sounding[i]));
            }
            self.nsounding = 0;
            return n;
        }
        let step_len = RATE_BEATS[v.index(x.rate).min(RATE_BEATS.len() - 1)];
        let repeats = v.val(x.repeats).clamp(1.0, 4.0) as usize;
        let sub_len = step_len / repeats as f32;
        let gate = v.val(x.gate) * sub_len;
        let mut left = beats;
        for _ in 0..64 {
            let swing = if self.step % 2 == 1 { v.val(x.swing) * step_len } else { 0.0 };
            let hit = if self.sub < repeats { swing + self.sub as f32 * sub_len } else { f32::INFINITY };
            let off = if self.nsounding > 0 { self.gate_end } else { f32::INFINITY };
            let t = hit.min(off).min(step_len).max(self.at);
            if self.at + left < t {
                self.at += left;
                break;
            }
            left -= (t - self.at).max(0.0);
            self.at = t;
            if off <= t {
                for i in 0..self.nsounding {
                    emit(out, &mut n, ArpEvent::Off(self.sounding[i]));
                }
                self.nsounding = 0;
            } else if hit <= t {
                self.sub += 1;
                if self.rng.unit() < v.val(x.chance) {
                    for i in 0..self.nsounding {
                        emit(out, &mut n, ArpEvent::Off(self.sounding[i]));
                    }
                    self.nsounding = 0;
                    if pattern == 10 {
                        for h in 0..self.nheld {
                            let (note, vel) = self.held[h];
                            emit(out, &mut n, ArpEvent::On(note, vel));
                            self.sounding[self.nsounding] = note;
                            self.nsounding += 1;
                        }
                    } else {
                        let i = if pattern == 9 { (self.rng.next_u32() as usize) % self.nseq } else { self.step % self.nseq };
                        emit(out, &mut n, ArpEvent::On(self.seq[i], self.vel[i]));
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
