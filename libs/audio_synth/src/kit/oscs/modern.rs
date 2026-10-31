//! Modern (type 8): saw, pulse and a third waveform (triangle / square /
//! parabolic sine, optionally as a sub oscillator) as second-order
//! differentiated polynomial waveforms from the instantaneous increment,
//! with hard sync, FM and unison; then the character filter.

use super::{detune_factor, freq, spread, unison_lr, CharFilter, DriftLfo, OscBlock};
use crate::dsp::Rng;
use crate::kit::core::{Lag, SlotDef, SlotUnit, SlotValue};

pub const THIRD_NAMES: &[&str] = &["Triangle", "Square", "Sine"];
pub const SUB_BIT: i32 = 1 << 10;
pub const SUB_NO_SYNC_BIT: i32 = 1 << 11;
pub const SUB_TWO_OCT_BIT: i32 = 1 << 12;

pub fn slot_defs() -> [SlotDef; 7] {
    let mix = |name| SlotDef { can_deactivate: true, ..SlotDef::float(name, -1.0, 1.0, 0.0, SlotUnit::Percent) };
    [
        SlotDef { default: 0.5, ..mix("Sawtooth") },
        mix("Pulse"),
        SlotDef {
            deform: THIRD_NAMES,
            about: "deform low 4 bits = waveform; bit 10 sub oscillator, bit 11 sub ignores sync, bit 12 sub two octaves down",
            ..mix("Triangle")
        },
        SlotDef { about: "pulse width", ..SlotDef::float("Width", 0.0, 1.0, 0.5, SlotUnit::Percent) },
        SlotDef::float("Sync", 0.0, 60.0, 0.0, SlotUnit::Semis),
        super::classic::unison_detune(0.2),
        super::classic::unison_voices(),
    ]
}

pub fn migrate(rev: i32, s: &mut [SlotValue; 7]) {
    if rev <= 29 {
        for v in s.iter_mut().take(3) {
            v.deactivated = false;
        }
    }
}

#[inline]
fn wrap01(x: f64) -> f64 {
    x - x.floor()
}

#[inline]
fn g_saw(q: f64) -> f64 {
    (q * q * q - q) / 6.0
}
#[inline]
fn g_square(q: f64) -> f64 {
    let qq = if q < 0.0 { 1.0 } else { -1.0 };
    q * (qq * q + 1.0) * 0.5
}
#[inline]
fn g_tri(q: f64) -> f64 {
    let mut u = q + 0.5;
    if u > 1.0 {
        u -= 2.0;
    }
    let qq = if u < 0.0 { -1.0 } else { 1.0 };
    (2.0 + u * u * (3.0 - 2.0 * qq * u)) / 6.0
}
#[inline]
fn g_sine(q: f64) -> f64 {
    let sg = if q < 0.0 { 1.0 } else { -1.0 };
    -(sg * q * q * q * q + 2.0 * q * q * q - q) / 3.0
}

/// The second backward difference of `g` at phase p, increment dp.
#[inline]
fn dpw(g: fn(f64) -> f64, p: f64, dp: f64) -> f64 {
    let q = |x: f64| 2.0 * wrap01(x) - 1.0;
    (g(q(p)) - 2.0 * g(q(p - dp)) + g(q(p - 2.0 * dp))) / (4.0 * dp * dp)
}

#[derive(Clone, Copy, Debug, Default)]
struct Line {
    v: f64,
    dv: f64,
    started: bool,
}
impl Line {
    fn set(&mut self, t: f64) {
        if !self.started {
            self.v = t;
            self.dv = 0.0;
            self.started = true;
        } else {
            self.dv = (t - self.v) / 64.0;
        }
    }
    #[inline]
    fn next(&mut self) -> f64 {
        self.v += self.dv;
        self.v
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    drift: DriftLfo,
    lead: f64,
    follow: f64,
    wraps: u32,
    prev: f64,
    dp: Line,
    dsp: Line,
    gl: f32,
    gr: f32,
}

impl Voice {
    /// Advance one sample; returns (phase, increment, reset happened).
    #[inline]
    fn step(&mut self, synced: bool, shift: f64) -> (f64, f64, bool) {
        let dp = self.dp.next().max(1e-9);
        let dsp = self.dsp.next().max(1e-9);
        self.lead += dp;
        let mut reset = false;
        if self.lead >= 1.0 {
            self.lead -= 1.0;
            self.wraps = self.wraps.wrapping_add(1);
            reset = synced && self.wraps % 2 == 0;
        }
        if !synced {
            self.follow = self.lead;
            return (wrap01(self.follow + shift), dp, false);
        }
        if reset {
            self.follow = wrap01(self.lead * dsp / dp);
        } else {
            self.follow = wrap01(self.follow + dsp);
        }
        (wrap01(self.follow + shift), dsp, reset)
    }
}

pub struct Modern {
    rng: Rng,
    v: [Voice; 16],
    sub: Voice,
    n: usize,
    saw: Lag,
    pulse: Lag,
    third: Lag,
    width: Lag,
    sync: Lag,
    fmd: Lag,
    chr: CharFilter,
    first: bool,
}

impl Modern {
    pub fn new(seed: u32) -> Modern {
        Modern {
            rng: Rng::new(seed),
            v: [Voice::default(); 16],
            sub: Voice::default(),
            n: 1,
            saw: Lag::default(),
            pulse: Lag::default(),
            third: Lag::default(),
            width: Lag::new(0.001),
            sync: Lag::new(0.064),
            fmd: Lag::default(),
            chr: CharFilter::default(),
            first: true,
        }
    }

    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        self.n = b.unison();
        let start = super::dphase(b.pitch, b.fs_os);
        for u in 0..self.n {
            let mut v = Voice::default();
            v.drift.init(&mut self.rng, nonzero_drift);
            let (gl, gr) = unison_lr(self.n, u);
            v.gl = gl;
            v.gr = gr;
            v.lead = if b.retrigger { start } else { self.rng.unit() as f64 };
            v.follow = v.lead;
            self.v[u] = v;
        }
        self.sub = Voice { lead: self.v[0].lead, follow: self.v[0].lead, ..Voice::default() };
        self.saw = Lag::default();
        self.pulse = Lag::default();
        self.third = Lag::default();
        self.width = Lag::new(0.001);
        self.sync = Lag::new(0.064);
        self.fmd = Lag::default();
        self.chr.reset();
        self.first = false;
    }

    fn inc(&self, b: &OscBlock, note: f32, hz_off: f32) -> f64 {
        let f = (freq(note) + hz_off).max(1.0);
        (f as f64 / b.fs_os as f64).min(0.5)
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        if self.first {
            self.init(b, false);
        }
        let mixv = |s: &SlotValue| if s.deactivated { 0.0 } else { 0.5 * s.v.clamp(-2.0, 2.0) };
        self.saw.set(mixv(&b.slots[0]));
        self.pulse.set(mixv(&b.slots[1]));
        self.third.set(mixv(&b.slots[2]));
        self.width.set((1.0 - b.slots[3].v).clamp(0.01, 0.99));
        self.sync.set(b.slots[4].v.max(0.0));
        let sync = self.sync.process();
        let synced = sync > 0.0;
        self.fmd.set(if b.fm { 16.0 * b.fmdepth * b.fmdepth * b.fmdepth } else { 0.0 });
        let deform = b.slots[2].deform;
        let third_g: fn(f64) -> f64 = match deform & 15 {
            1 => g_square,
            2 => g_sine,
            _ => g_tri,
        };
        let sub_mode = deform & SUB_BIT != 0;
        let sp = spread(b, 5);
        for u in 0..self.n {
            let d = self.v[u].drift.next(&mut self.rng);
            let df = detune_factor(self.n, u);
            let (note, hz) = if b.slots[5].absolute { (b.pitch + b.drift * d, 16.0 * sp * df) } else { (b.pitch + b.drift * d + sp * df, 0.0) };
            let dp = self.inc(b, note, hz);
            let dsp = self.inc(b, note + sync, hz);
            self.v[u].dp.set(dp);
            self.v[u].dsp.set(dsp);
        }
        let main_active = !(sub_mode && b.slots[0].deactivated && b.slots[1].deactivated);
        let mut sawv = [0.0f64; 64];
        let mut pulv = [0.0f64; 64];
        let mut thv = [0.0f64; 64];
        let mut wv = [0.0f64; 64];
        let mut fdv = [0.0f64; 64];
        for k in 0..64 {
            sawv[k] = self.saw.process() as f64;
            pulv[k] = self.pulse.process() as f64;
            thv[k] = self.third.process() as f64;
            wv[k] = self.width.process() as f64;
            fdv[k] = (self.fmd.process() * b.fm_buf[k]) as f64;
        }
        let att = 1.0 / (self.n as f32).sqrt();
        let mut ol = [0.0f32; 64];
        let mut or = [0.0f32; 64];
        if main_active {
            for u in 0..self.n {
                let v = &mut self.v[u];
                for k in 0..64 {
                    let (ph, inc, reset) = v.step(synced, fdv[k]);
                    let saw = dpw(g_saw, ph, inc);
                    let mut res = sawv[k] * saw + pulv[k] * (dpw(g_saw, ph + wv[k], inc) - saw);
                    if !sub_mode {
                        res += thv[k] * dpw(third_g, ph, inc);
                    }
                    if reset {
                        let ext = res + (v.prev - res) * inc;
                        res = 0.5 * (res + ext);
                    }
                    v.prev = res;
                    let res = res as f32 * att;
                    ol[k] += res * v.gl;
                    or[k] += res * v.gr;
                }
            }
        }
        if sub_mode {
            let mul = if deform & SUB_TWO_OCT_BIT != 0 { 0.25 } else { 0.5 };
            let sub_sync = synced && deform & SUB_NO_SYNC_BIT == 0;
            let note = b.pitch + b.drift * self.v[0].drift.d;
            let dp = mul * self.inc(b, note, 0.0);
            let dsp = if sub_sync { mul * self.inc(b, note + sync, 0.0) } else { dp };
            self.sub.dp.set(dp);
            self.sub.dsp.set(dsp);
            for k in 0..64 {
                let (ph, inc, reset) = self.sub.step(sub_sync, fdv[k]);
                let mut res = thv[k] * dpw(third_g, ph, inc);
                if reset {
                    let ext = res + (self.sub.prev - res) * inc;
                    res = 0.5 * (res + ext);
                }
                self.sub.prev = res;
                ol[k] += res as f32;
                or[k] += res as f32;
            }
        }
        if b.stereo {
            *l = ol;
            *r = or;
        } else {
            for k in 0..64 {
                l[k] = 0.5 * (ol[k] + or[k]);
            }
        }
        self.chr.process(b.character, b.fs(), b.stereo, l, r);
    }
}
