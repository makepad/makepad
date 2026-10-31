//! S&H Noise (type 3): two correlated random steps per cycle as band-limited
//! steps (blit.rs), optional hard sync with the "economy" step-rate limit,
//! then low / high cut.

use super::blit::BlitBuf;
use super::sine::{high_cut, low_cut};
use super::{abs_detune_note, detune_factor, period, spread, unison_lr, Cuts, DriftLfo, OscBlock};
use crate::dsp::Rng;
use crate::kit::core::{Lag, SlotDef, SlotUnit, SlotValue};

pub const SYNC_DEFORM: &[&str] = &["Off", "Economy"];

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef { about: "-1 jagged (anti-correlated), +1 smooth walk", ..SlotDef::float("Correlation", -1.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "time split of the two steps per cycle", ..SlotDef::float("Width", 0.0, 1.0, 0.5, SlotUnit::Percent) },
        low_cut(),
        high_cut(),
        SlotDef { deform: SYNC_DEFORM, ..SlotDef::float("Sync", 0.0, 60.0, 0.0, SlotUnit::Semis) },
        super::classic::unison_detune(0.1),
        super::classic::unison_voices(),
    ]
}

pub fn migrate(rev: i32, s: &mut [SlotValue; 7]) {
    if rev <= 12 {
        s[2].v = -60.0;
        s[2].deactivated = true;
        s[3].v = 70.0;
        s[4].deactivated = true;
    }
    if rev < 31 {
        s[4].deform = 0;
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    drift: DriftLfo,
    oscstate: f32,
    syncstate: f32,
    state: u8,
    last_level: f32,
    v_prev: f32,
    pw: f32,
    t: f32,
    lead: f32,
    lc: f32,
    gl: f32,
    gr: f32,
}

pub struct ShNoise {
    rng: Rng,
    v: [Voice; 16],
    n: usize,
    buf: BlitBuf,
    corr: Lag,
    width: Lag,
    sync: Lag,
    cuts: Cuts,
    first: bool,
}

impl ShNoise {
    pub fn new(seed: u32) -> ShNoise {
        ShNoise {
            rng: Rng::new(seed),
            v: [Voice::default(); 16],
            n: 1,
            buf: BlitBuf::default(),
            corr: Lag::new(0.05),
            width: Lag::new(0.05),
            sync: Lag::new(0.05),
            cuts: Cuts::default(),
            first: true,
        }
    }

    fn targets(&mut self, b: &OscBlock) {
        self.corr.set(b.slots[0].v.clamp(-1.0, 1.0));
        self.width.set(b.slots[1].v.clamp(0.001, 0.999));
        self.sync.set(b.slots[4].v.max(0.0));
    }

    fn timing(&mut self, b: &OscBlock, u: usize, pm: f32) {
        let det = b.drift * self.v[u].drift.d + spread(b, 5) * detune_factor(self.n, u);
        let sync = self.sync.v;
        let (mut t, lead) = if b.slots[5].absolute {
            let d = abs_detune_note(det, b.pitch);
            ((-(d + sync) / 12.0).exp2().max(0.01), 2.0 * (-d / 12.0).exp2())
        } else {
            ((-(det + sync) / 12.0).exp2(), 2.0 * (-det / 12.0).exp2())
        };
        let mut lc = 1.0;
        if b.slots[4].deform == 1 {
            let tmin = 0.5 * pm;
            if t < tmin {
                lc = (t / tmin).sqrt();
                t = tmin;
            }
        }
        let v = &mut self.v[u];
        v.t = t;
        v.lead = lead;
        v.lc = lc;
    }

    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        self.n = b.unison();
        self.buf.reset();
        self.cuts.reset();
        for l in [&mut self.corr, &mut self.width, &mut self.sync] {
            *l = Lag::new(0.05);
        }
        self.targets(b);
        let att = 1.0 / (self.n as f32).sqrt();
        let pm = 1.0 / period(b.pitch, b.fs_os);
        let mut y0 = [0.0f32; 2];
        for u in 0..self.n {
            let mut v = Voice::default();
            v.drift.init(&mut self.rng, nonzero_drift);
            let (gl, gr) = unison_lr(self.n, u);
            v.gl = gl * att;
            v.gr = gr * att;
            v.pw = self.width.v;
            self.v[u] = v;
            self.timing(b, u, pm);
            let v = &mut self.v[u];
            v.syncstate = v.lead;
            if !b.retrigger {
                // Random phase in the cycle and a random start level.
                let ph = self.rng.unit() * v.t;
                let first = v.t * v.pw;
                if ph < first {
                    v.state = 1;
                    v.oscstate = first - ph;
                } else {
                    v.state = 0;
                    v.oscstate = v.t - ph;
                }
                v.syncstate = v.lead - ph;
                let lvl = self.rng.unit() - 0.5;
                v.v_prev = lvl;
                v.last_level = lvl * v.lc;
                if b.stereo {
                    y0[0] += v.last_level * v.gl;
                    y0[1] += v.last_level * v.gr;
                } else {
                    y0[0] += v.last_level * att;
                }
            }
        }
        if !b.retrigger {
            self.buf.y = y0;
        }
        self.first = false;
    }

    fn convolute(&mut self, u: usize, b: &OscBlock, p: f32, fm: Option<(usize, f32)>) {
        let synced = self.sync.v > 0.0;
        let wf = 0.8 * self.corr.v;
        let mut v = self.v[u];
        let mut invert = false;
        if synced && v.syncstate < v.oscstate {
            invert = v.state == 1;
            v.state = 0;
            v.oscstate = v.syncstate;
            v.syncstate += v.lead;
        }
        if v.state == 0 {
            v.pw = self.width.v;
        }
        let wfx = if invert { -wf } else { wf };
        let r = self.rng.bipolar();
        let k = 1.0 - wfx.abs();
        let val = ((r * k - wfx * v.v_prev) / k).clamp(-0.5, 0.5);
        v.v_prev = val;
        let level = val * v.lc;
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
        let dur = if v.state == 0 { v.t * v.pw } else { v.t * (1.0 - v.pw) };
        v.oscstate += dur;
        v.state ^= 1;
        self.v[u] = v;
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        if self.first {
            self.init(b, false);
        }
        self.targets(b);
        for lag in [&mut self.corr, &mut self.width, &mut self.sync] {
            lag.process();
        }
        let p = period(b.pitch, b.fs_os);
        let pm = 1.0 / p;
        for u in 0..self.n {
            self.v[u].drift.next(&mut self.rng);
            self.timing(b, u, pm);
        }
        let synced = self.sync.v > 0.0;
        if b.fm {
            for k in 0..64 {
                let mul = (1.0 + b.fmdepth * b.fm_buf[k]).clamp(0.1, 1.9);
                let a = pm * mul;
                for u in 0..self.n {
                    let mut guard = 0;
                    while (self.v[u].oscstate < a || (synced && self.v[u].syncstate < a)) && guard < 4096 {
                        self.convolute(u, b, p, Some((k, mul)));
                        guard += 1;
                    }
                    self.v[u].oscstate -= a;
                    self.v[u].syncstate -= a;
                    if !synced {
                        self.v[u].syncstate = self.v[u].syncstate.max(0.0);
                    }
                }
            }
        } else {
            let a = 64.0 * pm;
            for u in 0..self.n {
                let mut guard = 0;
                while (self.v[u].oscstate < a || (synced && self.v[u].syncstate < a)) && guard < 8192 {
                    self.convolute(u, b, p, None);
                    guard += 1;
                }
                self.v[u].oscstate -= a;
                self.v[u].syncstate -= a;
                if !synced {
                    self.v[u].syncstate = self.v[u].syncstate.max(0.0);
                }
            }
        }
        let f = super::freq(b.pitch + self.sync.v);
        self.buf.set_leak(BlitBuf::leak_target(f, b.fs(), 0.995));
        self.buf.integrate(0.0, b.stereo, l, r);
        self.cuts.process(&b.slots[2], &b.slots[3], b.fs_os, b.stereo, l, r);
    }
}
