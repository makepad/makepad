//! Window (type 7): a grain per period, a grain window (9 shapes) times a
//! wavetable frame played at a formant-scaled speed, all in the integer
//! (16.16 position, i15 sample) pipeline; then low / high cut.

use super::blit::sinc8;
use super::sine::{high_cut, low_cut};
use super::table::{window_table, RefTable};
use super::{abs_detune_note, detune_factor, freq, spread, unison_pan, Cuts, DriftLfo, OscBlock};
use crate::dsp::Rng;
use crate::kit::core::{Lag, SlotDef, SlotUnit, SlotValue};
use std::sync::Arc;

pub const WINDOW_NAMES: [&str; 9] = ["Triangle", "Cosine", "Blend 1", "Blend 2", "Blend 3", "Sawtooth", "Sine", "Square", "Rectangle"];

const WIN_LEN: i64 = 1024;
const WRAP: i64 = WIN_LEN << 16;

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef { can_extend: true, about: "frame; extended = crossfade adjacent frames", ..SlotDef::float("Morph", 0.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef::float("Formant", -60.0, 60.0, 0.0, SlotUnit::Semis),
        SlotDef::choice("Window", &WINDOW_NAMES, 0.0),
        low_cut(),
        high_cut(),
        super::classic::unison_detune(0.1),
        SlotDef { deform: super::wavetable::VOICES_DEFORM, ..super::classic::unison_voices() },
    ]
}

pub fn migrate(rev: i32, s: &mut [SlotValue; 7], retrigger: &mut bool) {
    if rev <= 12 {
        s[3].v = -60.0;
        s[3].deactivated = true;
        s[4].v = 70.0;
        s[4].deactivated = true;
    }
    if rev <= 15 && s[6].v.round() as i32 == 1 {
        *retrigger = true;
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    drift: DriftLfo,
    pos: i64,
    r: i64,
    fmul: i64,
    fmul_latched: i64,
    k: usize,
    k2: usize,
    ft: f32,
    gl: i64,
    gr: i64,
    frame: i32,
    plays: i32,
    done: bool,
}

pub struct Window {
    rng: Rng,
    v: [Voice; 15],
    n: usize,
    morph: Lag,
    fmd: Lag,
    cuts: Cuts,
    table: Arc<RefTable>,
    first: bool,
}

#[inline]
fn bitwidth(x: i64) -> i32 {
    if x <= 0 { 0 } else { 64 - x.leading_zeros() as i32 }
}

impl Window {
    pub fn new(seed: u32) -> Window {
        Window {
            rng: Rng::new(seed),
            v: [Voice::default(); 15],
            n: 1,
            morph: Lag::new(0.05),
            fmd: Lag::default(),
            cuts: Cuts::default(),
            table: RefTable::shared_default().clone(),
            first: true,
        }
    }

    fn sample(&self) -> bool {
        self.table.is_sample()
    }

    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        if let Some(t) = b.table {
            self.table = t.clone();
        }
        let n = b.unison().min(15);
        let play_count = self.sample() && b.slots[6].deform == 1;
        self.n = if play_count { 1 } else { n };
        let plays = if self.table.is_loop() { -1 } else if play_count { n as i32 } else { 1 };
        self.morph = Lag::new(0.05);
        self.morph.set(b.slots[0].v.clamp(0.0, 1.0));
        self.fmd = Lag::default();
        self.cuts.reset();
        let frames = self.table.content_frames as i32;
        let start = ((frames as f32 * self.morph.v) as i32).clamp(0, frames - 1);
        for u in 0..self.n {
            let mut v = Voice::default();
            v.drift.init(&mut self.rng, nonzero_drift || self.n > 1);
            let d = unison_pan(self.n, u);
            if self.n > 1 {
                v.gl = ((128.0 * (1.0 - 0.75 * d - 0.25 * d * d)) as i64).clamp(0, 255);
                v.gr = ((128.0 * (1.0 + 0.75 * d - 0.25 * d * d)) as i64).clamp(0, 255);
            } else {
                v.gl = 128;
                v.gr = 128;
            }
            let start_win = if b.retrigger { WIN_LEN as f32 * (1.0 + u as f32 / self.n as f32) } else { WIN_LEN as f32 * (1.0 + self.rng.unit()) };
            v.pos = (start_win as i64) << 16;
            v.frame = start - 1;
            v.plays = plays;
            self.v[u] = v;
        }
        self.first = false;
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        if self.first {
            self.init(b, false);
        }
        if let Some(t) = b.table {
            if !Arc::ptr_eq(t, &self.table) {
                self.table = t.clone();
            }
        }
        let tbl = self.table.clone();
        let wt = window_table();
        let fs = b.fs();
        self.morph.set(b.slots[0].v.clamp(0.0, 1.0));
        let morph = self.morph.process();
        let size = tbl.size as i64;
        let nfr = tbl.n as i32;
        let wshape = b.slots[2].i().clamp(0, 8) as usize;
        // Formant multiplier, rescaled for the frame size.
        let mut fmul = (65536.0 * (b.slots[1].v / 12.0).exp2()) as i64;
        let ratio = size.max(1);
        if ratio > WIN_LEN {
            fmul <<= (ratio / WIN_LEN).trailing_zeros();
        } else if ratio < WIN_LEN {
            fmul >>= (WIN_LEN / ratio).trailing_zeros();
        }
        let fmul = fmul.max(1);
        let sample = self.sample();
        let table_k = ((nfr as f32 * morph) as i32).clamp(0, nfr - 1);
        let table_k2 = (table_k + 1).min(nfr - 1);
        let ft = if b.slots[0].extend && !sample { (nfr as f32 * morph - table_k as f32).clamp(0.0, 1.0) } else { 0.0 };
        let log2size = (size.max(2) as u64).trailing_zeros() as i32;
        let max_mb = (log2size - 1).min(tbl.mip_count() as i32 - 1).max(0);
        let full = tbl.full_range();
        let pi32 = 32.0 * std::f32::consts::PI;
        self.fmd.set(if b.fm { pi32 * b.fmdepth * b.fmdepth * b.fmdepth } else { 0.0 });
        let mut fdv = [0.0f32; 64];
        for x in fdv.iter_mut() {
            *x = self.fmd.process();
        }
        let s8 = sinc8();
        let mut accl = [0i64; 64];
        let mut accr = [0i64; 64];
        let sp = spread(b, 5);
        let content = tbl.content_frames as i32;
        for u in 0..self.n {
            let drift = self.v[u].drift.next(&mut self.rng);
            let v = &mut self.v[u];
            let df = detune_factor(self.n, u);
            let det = if b.slots[5].absolute { abs_detune_note(sp, b.pitch.min(148.0)) * df } else { sp * df };
            let f = freq(b.pitch + b.drift * drift + det) as f64;
            v.r = (f * 32768.0 * 1024.0 / fs as f64).round() as i64;
            v.fmul = fmul;
            if !sample {
                v.k = table_k as usize;
                v.k2 = table_k2 as usize;
            }
            v.ft = ft;
            let ma = (bitwidth(3 * v.r) - 18).clamp(0, 9) as u32;
            let mb = (bitwidth((v.r * 3 * v.fmul) >> 16) - 18).clamp(0, max_mb) as u32;
            let wmip = &wt.mips[ma as usize];
            let wmask_len = wmip.len as i64;
            let wfr = wmip.frame_i16(wshape);
            let mip = &tbl.mips[mb as usize];
            let wave_mask = size * 65536 - 1;
            for s in 0..64 {
                let rk = if b.fm { ((f * (1.0 + (fdv[s] * b.fm_buf[s]) as f64)) * 32768.0 * 1024.0 / fs as f64).round().max(0.0) as i64 } else { v.r };
                v.pos += rk;
                if v.pos >= WRAP {
                    v.pos %= WRAP;
                    v.fmul_latched = v.fmul;
                    if sample && !v.done {
                        v.frame += 1;
                        if v.frame >= content {
                            if v.plays > 0 {
                                v.plays -= 1;
                            }
                            if v.plays != 0 {
                                v.frame = 0;
                            } else {
                                v.done = true;
                            }
                        }
                        v.k = v.frame.clamp(0, nfr - 1) as usize;
                        v.k2 = v.k;
                    }
                }
                if v.done {
                    continue;
                }
                let pos = v.pos;
                let wi = ((pos >> (16 + ma)) as usize).min(wmask_len as usize - 1);
                let ws = ((pos >> (8 + ma)) & 255) as usize;
                let mut win = 0i64;
                for t in 0..8 {
                    win += s8[ws][t] as i64 * wfr[wi + t] as i64;
                }
                win >>= 13;
                let fpos = ((v.fmul_latched * pos) >> 16) & wave_mask;
                let mi = ((fpos >> (16 + mb)) as usize).min(mip.len - 1);
                let ms = ((fpos >> (8 + mb)) & 255) as usize;
                let read = |k: usize| -> i64 {
                    let fr = mip.frame_i16(k);
                    let mut a = 0i64;
                    for t in 0..8 {
                        a += s8[ms][t] as i64 * fr[mi + t] as i64;
                    }
                    let a = a >> 13;
                    if full { a >> 1 } else { a }
                };
                let mut wav = read(v.k);
                if v.ft > 0.0 {
                    let w2 = read(v.k2);
                    wav = ((1.0 - v.ft) * wav as f32 + v.ft * w2 as f32) as i64;
                }
                if b.stereo {
                    let o = (win * wav) >> 7;
                    accl[s] += (o * v.gl) >> 6;
                    accr[s] += (o * v.gr) >> 6;
                } else {
                    accl[s] += (win * wav) >> 6;
                }
            }
        }
        let scale = 1.0 / ((self.n as f32).sqrt() * 16_777_216.0);
        for s in 0..64 {
            l[s] = accl[s] as f32 * scale;
            r[s] = accr[s] as f32 * scale;
        }
        self.cuts.process(&b.slots[3], &b.slots[4], b.fs_os, b.stereo, l, r);
    }
}
