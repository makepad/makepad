//! Wavetable (type 2): every table sample is a band-limited step (blit.rs)
//! read from the mip level that suits the pitch, with morph between frames,
//! vertical / horizontal skew, saturation, formant and sample playback.

use super::blit::BlitBuf;
use super::table::RefTable;
use super::{abs_detune_note, detune_factor, period, spread, unison_lr, DriftLfo, OscBlock};
use crate::dsp::Rng;
use crate::kit::core::{Lag, SlotDef, SlotUnit, SlotValue};
use std::sync::Arc;

/// Morph deform 1 ("1.4"): the reference lags the unmodulated value and adds
/// the modulation offset instantly; our block carries only the modulated
/// value, so this mode follows it directly (no lag).
pub const MORPH_DEFORM: &[&str] = &["Pre-1.4 Morph", "1.4 Morph"];
pub const VOICES_DEFORM: &[&str] = &["Unison Voices", "Sample Play Count"];

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef {
            can_extend: true,
            deform: MORPH_DEFORM,
            about: "frame position; extended = continuous crossfade",
            ..SlotDef::float("Morph", 0.0, 1.0, 0.0, SlotUnit::Percent)
        },
        SlotDef::float("Skew Vertical", -1.0, 1.0, 0.0, SlotUnit::Percent),
        SlotDef::float("Saturate", 0.0, 1.0, 0.0, SlotUnit::Percent),
        SlotDef { about: "plays the cycle compressed, then holds", ..SlotDef::float("Formant", 0.0, 60.0, 0.0, SlotUnit::Semis) },
        SlotDef::float("Skew Horizontal", -1.0, 1.0, 0.0, SlotUnit::Percent),
        super::classic::unison_detune(0.1),
        SlotDef { deform: VOICES_DEFORM, ..super::classic::unison_voices() },
    ]
}

pub fn migrate(rev: i32, s: &mut [SlotValue; 7]) {
    if rev <= 16 {
        s[0].extend = true;
    }
    if rev <= 25 {
        s[0].deform = 0;
    }
}

pub fn migrate_patch(rev: i32, s: &mut [SlotValue; 7], table_is_sample: bool) {
    if rev <= 30 && table_is_sample {
        s[6].deform = 1;
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    drift: DriftLfo,
    oscstate: f32,
    state: usize,
    mip: usize,
    last_level: f32,
    hskew: f32,
    formant: f32,
    formant_prev: f32,
    gl: f32,
    gr: f32,
    // sample mode
    frame: i32,
    plays: i32,
    done: bool,
}

pub struct Wavetable {
    rng: Rng,
    v: [Voice; 16],
    n: usize,
    buf: BlitBuf,
    vskew: Lag,
    hskew: Lag,
    sat: Lag,
    morph: Lag,
    formant: Lag,
    pos_last: f32,
    pos_cur: f32,
    // pre-1.4 seamless frame state
    id: i32,
    ip: f32,
    ip_prev: f32,
    id_prev: i32,
    table: Arc<RefTable>,
    first: bool,
}

impl Wavetable {
    pub fn new(seed: u32) -> Wavetable {
        Wavetable {
            rng: Rng::new(seed),
            v: [Voice::default(); 16],
            n: 1,
            buf: BlitBuf::default(),
            vskew: Lag::new(0.05),
            hskew: Lag::new(0.05),
            sat: Lag::new(0.05),
            morph: Lag::new(0.05),
            formant: Lag::new(0.05),
            pos_last: 0.0,
            pos_cur: 0.0,
            id: 0,
            ip: 0.0,
            ip_prev: 0.0,
            id_prev: 0,
            table: RefTable::shared_default().clone(),
            first: true,
        }
    }

    fn sample_mode(&self) -> bool {
        self.table.is_sample()
    }

    fn targets(&mut self, b: &OscBlock) {
        self.vskew.set(b.slots[1].v.clamp(-1.0, 1.0));
        self.hskew.set(b.slots[4].v.clamp(-1.0, 1.0));
        let a = b.slots[2].v.clamp(0.0, 1.0);
        self.sat.set(-8.0 * a * a * a);
        self.morph.set(b.slots[0].v.clamp(0.0, 1.0));
        self.formant.set(b.slots[3].v.max(0.0));
    }

    fn voices(&self, b: &OscBlock) -> (usize, i32) {
        let n = b.unison();
        if self.sample_mode() && b.slots[6].deform == 1 {
            (1, if self.table.is_loop() { -1 } else { n as i32 })
        } else if self.sample_mode() {
            (n, if self.table.is_loop() { -1 } else { 1 })
        } else {
            (n, -1)
        }
    }

    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        if let Some(t) = b.table {
            if !Arc::ptr_eq(t, &self.table) {
                self.table = t.clone();
            }
        }
        let (n, plays) = self.voices(b);
        self.n = n;
        self.buf.reset();
        for l in [&mut self.vskew, &mut self.hskew, &mut self.sat, &mut self.morph, &mut self.formant] {
            *l = Lag::new(0.05);
        }
        self.targets(b);
        let att = 1.0 / (n as f32).sqrt();
        let frames = self.table.n as i32;
        let start_frame = ((self.morph.v * frames as f32) as i32).clamp(0, frames - 1);
        for u in 0..n {
            let mut v = Voice::default();
            v.drift.init(&mut self.rng, nonzero_drift);
            let (gl, gr) = unison_lr(n, u);
            v.gl = gl * att;
            v.gr = gr * att;
            v.hskew = self.hskew.v;
            v.formant = self.formant.v;
            v.formant_prev = self.formant.v;
            v.plays = plays;
            v.frame = start_frame - 1;
            if !b.retrigger && !self.sample_mode() {
                let size = self.table.size;
                v.state = ((self.rng.unit() * size as f32) as usize) % size;
            }
            self.v[u] = v;
        }
        let pos = self.morph_pos(b);
        self.pos_last = pos;
        self.pos_cur = pos;
        let nointerp = !b.slots[0].extend;
        let top = (self.table.n as i32 - 2 + nointerp as i32).max(0);
        self.id = (pos as i32).clamp(0, top);
        self.ip = pos - self.id as f32;
        self.ip_prev = self.ip;
        self.id_prev = self.id;
        self.first = false;
    }

    fn morph_pos(&self, b: &OscBlock) -> f32 {
        let n = self.table.n as f32;
        let nointerp = if b.slots[0].extend { 0.0 } else { 1.0 };
        let shape = if b.slots[0].deform == 1 { b.slots[0].v.clamp(0.0, 1.0) } else { self.morph.v };
        shape * (n - 1.0 + nointerp) * 0.99999
    }

    /// Choose a mip level for P samples per cycle.
    fn pick_mip(&self, p: f32) -> usize {
        let size = self.table.size;
        let a = p / size as f32;
        let mut m = 0usize;
        while m < 15 && m + 1 < self.table.mip_count() && (size >> (m + 1)) >= 2 && a * ((1u32 << m) as f32) < 0.9 {
            m += 1;
        }
        m
    }

    /// The table value at index `idx` of mip `m`, morph position at `bp`.
    fn value(&self, b: &OscBlock, v: &Voice, bp: f32) -> f32 {
        let t = &*self.table;
        let mip = &t.mips[v.mip.min(t.mips.len() - 1)];
        let idx = v.state.min(mip.len - 1);
        let n = t.n as i32;
        let nointerp = !b.slots[0].extend;
        if self.sample_mode() {
            let k = v.frame.clamp(0, n - 1) as usize;
            return mip.frame(k)[idx];
        }
        if b.slots[0].deform == 1 {
            let bp = if nointerp { 1.0 } else { bp };
            let p = self.pos_last + (self.pos_cur - self.pos_last) * bp;
            let k = (p.floor() as i32).clamp(0, n - 1);
            let k2 = (k + 1).min(n - 1);
            let f = if nointerp { 0.0 } else { p - k as f32 };
            (1.0 - f) * mip.frame(k as usize)[idx] + f * mip.frame(k2 as usize)[idx]
        } else {
            let ip1 = self.ip_prev + (self.ip - self.ip_prev) * bp;
            let ip2 = if nointerp { 0.0 } else { ip1 };
            let k = self.id.clamp(0, n - 1);
            let k2 = (self.id + 1 - nointerp as i32).clamp(0, n - 1);
            (1.0 - ip2) * mip.frame(k as usize)[idx] + ip2 * mip.frame(k2 as usize)[idx]
        }
    }

    fn distort(&self, x: f32) -> f32 {
        let a = 0.5 * self.vskew.v;
        let x = x - a * x * x + a;
        let cl = self.sat.v;
        (x * (1.0 - cl) + cl * x * x * x).clamp(-1.0, 1.0)
    }

    fn convolute(&mut self, u: usize, b: &OscBlock, p: f32, fm: Option<(usize, f32)>) {
        let size = self.table.size;
        let frames = self.table.n as i32;
        let sample = self.sample_mode();
        let mut v = self.v[u];
        if v.done {
            v.oscstate += 1.0;
            self.v[u] = v;
            return;
        }
        if v.state == 0 {
            v.formant_prev = v.formant;
            v.formant = self.formant.v;
            v.hskew = self.hskew.v;
            v.mip = self.pick_mip(p);
            if sample {
                v.frame += 1;
                let end = if b.slots[0].deform == 1 { frames - 4 } else { frames - 3 + (!b.slots[0].extend) as i32 };
                if v.frame > end {
                    if v.plays > 0 {
                        v.plays -= 1;
                    }
                    if v.plays != 0 {
                        v.frame = 0;
                    } else {
                        v.frame = if b.slots[0].deform == 1 { frames - 1 } else { frames - 2 + (!b.slots[0].extend) as i32 };
                        v.done = true;
                        // Settle to the parked frame's first value, then stop.
                    }
                }
            }
        }
        let len = (size >> v.mip).max(1);
        let bp = (v.oscstate * p / 64.0).clamp(0.0, 1.0);
        let dt0 = (1u32 << v.mip) as f32 / size as f32;
        let x = (v.state as f32 + 0.5) * dt0;
        let xt = 1.0 + v.hskew * 4.0 * x * (x - 1.0) * (2.0 * x - 1.0) * (27.0f32 / 4.0).sqrt();
        let ft = bp * v.formant + (1.0 - bp) * v.formant_prev;
        let fr = (-ft / 12.0).exp2();
        let mut dt = dt0 * fr * xt;
        if v.state + 1 >= len {
            dt += 1.0 - fr;
        }
        let det = b.drift * v.drift.d + spread(b, 5) * detune_factor(self.n, u);
        let t = if b.slots[5].absolute {
            dt * (-abs_detune_note(det, b.pitch) / 12.0).exp2().max(0.1)
        } else {
            dt * (-det / 12.0).exp2()
        };
        let level = self.distort(self.value(b, &v, bp));
        let g = level - v.last_level;
        v.last_level = level;
        let (gl, gr) = if b.stereo { (g * v.gl, g * v.gr) } else { (g / (self.n as f32).sqrt(), 0.0) };
        match fm {
            None => {
                self.buf.place(v.oscstate * p, gl, gr, b.stereo);
            }
            Some((k, mul)) => {
                let frac = ((v.oscstate * p / mul).clamp(0.0, 0.999_999) as f64 * 16_777_216.0) as u32;
                self.buf.place_at(k, frac, gl, gr, b.stereo);
            }
        }
        v.oscstate += t.max(1e-6);
        v.state = (v.state + 1) % len;
        self.v[u] = v;
    }

    fn update_morph(&mut self, b: &OscBlock) {
        let pos = self.morph_pos(b);
        let n = self.table.n as i32;
        if n <= 1 {
            self.pos_last = 0.0;
            self.pos_cur = 0.0;
            self.id = 0;
            self.ip = 0.0;
            return;
        }
        self.pos_last = self.pos_cur;
        self.pos_cur = pos;
        if b.slots[0].deform != 1 {
            let nointerp = !b.slots[0].extend;
            let top = (n - 2 + nointerp as i32).max(0);
            let id = (pos as i32).clamp(0, top);
            let ip = (pos - id as f32).clamp(0.0, 1.0);
            let (oid, oip) = (self.id, self.ip);
            if id > oid {
                if oip < 1.0 {
                    self.id = oid;
                    self.ip_prev = oip;
                    self.ip = 1.0;
                } else {
                    self.id = id;
                    self.ip_prev = 0.0;
                    self.ip = ip;
                }
            } else if id < oid {
                if oip > 0.0 {
                    self.id = oid;
                    self.ip_prev = oip;
                    self.ip = 0.0;
                } else {
                    self.id = id;
                    self.ip_prev = 1.0;
                    self.ip = ip;
                }
            } else {
                self.ip_prev = oip;
                self.ip = ip;
            }
            self.id_prev = self.id;
        }
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        let table_changed = b.table.is_some_and(|t| !Arc::ptr_eq(t, &self.table));
        if self.first || table_changed {
            if let Some(t) = b.table {
                self.table = t.clone();
            }
            let n = self.table.n as i32;
            if self.id >= n || self.pos_cur >= n as f32 {
                self.id = 0;
                self.ip = 0.0;
                self.pos_cur = 0.0;
                self.pos_last = 0.0;
            }
            if self.first {
                self.init(b, false);
            }
        }
        self.targets(b);
        for lag in [&mut self.vskew, &mut self.hskew, &mut self.sat, &mut self.morph, &mut self.formant] {
            lag.process();
        }
        self.update_morph(b);
        let p = period(b.pitch, b.fs_os);
        let pm = 1.0 / p;
        for u in 0..self.n {
            self.v[u].drift.next(&mut self.rng);
        }
        if b.fm {
            for k in 0..64 {
                let mul = (1.0 + b.fmdepth * b.fm_buf[k]).clamp(0.1, 1.9);
                let a = pm * mul;
                for u in 0..self.n {
                    let mut guard = 0;
                    while self.v[u].oscstate < a && guard < 4096 {
                        self.convolute(u, b, p, Some((k, mul)));
                        guard += 1;
                    }
                    self.v[u].oscstate -= a;
                }
            }
        } else {
            let a = 64.0 * pm;
            for u in 0..self.n {
                let mut guard = 0;
                while self.v[u].oscstate < a && guard < 8192 {
                    self.convolute(u, b, p, None);
                    guard += 1;
                }
                self.v[u].oscstate -= a;
            }
        }
        self.buf.set_leak(BlitBuf::leak_target(super::freq(b.pitch), b.fs(), 0.99));
        self.buf.integrate(0.0, b.stereo, l, r);
    }
}
