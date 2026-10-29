//! Classic (type 0): pulse <-> saw <-> double saw with a sub-octave square,
//! hard sync and unison, as band-limited steps plus ramps (blit.rs).

use super::blit::BlitBuf;
use super::{abs_detune_note, detune_factor, period, spread, unison_lr, CharFilter, DriftLfo, OscBlock};
use crate::dsp::Rng;
use crate::kit::core::{Lag, SlotDef, SlotUnit};

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef { about: "-1 pulse, 0 saw, +1 double saw", ..SlotDef::float("Shape", -1.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef::float("Width 1", 0.0, 1.0, 0.5, SlotUnit::Percent),
        SlotDef { about: "pulse width of the sub-octave square", ..SlotDef::float("Width 2", 0.0, 1.0, 0.5, SlotUnit::Percent) },
        SlotDef { about: "crossfade to the sub-octave square", ..SlotDef::float("Sub Mix", 0.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "hard-sync follower offset", ..SlotDef::float("Sync", 0.0, 60.0, 0.0, SlotUnit::Semis) },
        unison_detune(0.1),
        unison_voices(),
    ]
}

/// The shared Unison Detune slot (0..1 = 100 cents, extendable x12, absolute = Hz).
pub fn unison_detune(default: f32) -> SlotDef {
    SlotDef {
        can_extend: true,
        can_absolute: true,
        about: "unison spread, 1 = 100 cents (extended x12; absolute = Hz)",
        ..SlotDef::float("Unison Detune", 0.0, 1.0, default, SlotUnit::Percent)
    }
}

pub fn unison_voices() -> SlotDef {
    SlotDef { about: "unison voice count", ..SlotDef::int("Unison Voices", 1.0, 16.0, 1.0) }
}

/// Under FM the events advance at pm * fmmul but the ramp (DC) term keeps
/// subtracting at the unmodulated pm (spec 2.5), so a ramp waveform wobbles at
/// the modulator rate (large at high notes); mirrored as specified.
pub const QUIRK_CLASSIC_FM_RAMP_UNSCALED: bool = true;
/// In Wide mode the steps are panned per unison voice but the shared ramp
/// term is not; mirrored.
pub const QUIRK_CLASSIC_RAMP_UNPANNED: bool = true;

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    drift: DriftLfo,
    oscstate: f32,
    syncstate: f32,
    state: u8,
    level: f32,
    slope: f32,
    pw: f32,
    pw2: f32,
    t: f32,
    lead: f32,
    gl: f32,
    gr: f32,
}

pub struct Classic {
    rng: Rng,
    v: [Voice; 16],
    n: usize,
    buf: BlitBuf,
    shape: Lag,
    w1: Lag,
    w2: Lag,
    sub: Lag,
    sync: Lag,
    chr: CharFilter,
    first: bool,
}

/// Segment heights and durations of one Classic state.
struct Seg {
    g_abs: Option<f32>,
    g: f32,
    dur: f32,
}

fn segment(state: u8, wf: f32, sub: f32, pw: f32, pw2: f32) -> Seg {
    match state {
        0 => {
            let tg = ((1.0 + wf) * 0.5 + (1.0 - pw) * (-wf)) * (1.0 - sub) + 0.5 * sub * (2.0 - pw2);
            Seg { g_abs: Some(tg), g: 0.0, dur: pw * pw2 }
        }
        1 => Seg { g_abs: None, g: wf * (1.0 - sub) - sub, dur: (1.0 - pw) * (2.0 - pw2) },
        2 => Seg { g_abs: None, g: 1.0 - sub, dur: pw * (2.0 - pw2) },
        _ => Seg { g_abs: None, g: wf * (1.0 - sub) + sub, dur: (1.0 - pw) * pw2 },
    }
}

impl Classic {
    pub fn new(seed: u32) -> Classic {
        Classic {
            rng: Rng::new(seed),
            v: [Voice::default(); 16],
            n: 1,
            buf: BlitBuf::default(),
            shape: Lag::new(0.05),
            w1: Lag::new(0.05),
            w2: Lag::new(0.05),
            sub: Lag::new(0.05),
            sync: Lag::new(0.05),
            chr: CharFilter::default(),
            first: true,
        }
    }

    fn targets(&mut self, b: &OscBlock) {
        self.shape.set(b.slots[0].v.clamp(-1.0, 1.0));
        self.w1.set(b.slots[1].v.clamp(0.001, 0.999));
        self.w2.set(b.slots[2].v.clamp(0.001, 0.999));
        self.sub.set(b.slots[3].v.clamp(0.0, 1.0));
        self.sync.set(b.slots[4].v.max(0.0));
    }

    /// Follower segment unit and leader period of voice `u` this block.
    fn timing(&self, b: &OscBlock, u: usize) -> (f32, f32) {
        let det = b.drift * self.v[u].drift.d + spread(b, 5) * detune_factor(self.n, u);
        let sync = self.sync.v.min(156.0 - b.pitch).max(0.0);
        if b.slots[5].absolute {
            let d = abs_detune_note(det, b.pitch);
            ((-(d + sync) / 12.0).exp2().max(0.01), 2.0 * (-d / 12.0).exp2())
        } else {
            ((-(det + sync) / 12.0).exp2(), 2.0 * (-det / 12.0).exp2())
        }
    }

    pub fn init(&mut self, b: &OscBlock, nonzero_drift: bool) {
        self.n = b.unison();
        self.buf.reset();
        self.chr.reset_zero();
        for l in [&mut self.shape, &mut self.w1, &mut self.w2, &mut self.sub, &mut self.sync] {
            *l = Lag::new(0.05);
        }
        self.targets(b);
        let att = 1.0 / (self.n as f32).sqrt();
        let (wf, sub) = (self.shape.v, self.sub.v);
        let mut y0 = [0.0f32; 2];
        let mut dc0 = 0.0;
        for u in 0..self.n {
            let mut v = Voice::default();
            v.drift.init(&mut self.rng, nonzero_drift);
            let (gl, gr) = unison_lr(self.n, u);
            v.gl = gl * att;
            v.gr = gr * att;
            v.pw = self.w1.v;
            v.pw2 = 2.0 * self.w2.v;
            self.v[u] = v;
            let (t, lead) = self.timing(b, u);
            let v = &mut self.v[u];
            v.t = t;
            v.lead = lead;
            v.syncstate = lead;
            if !b.retrigger {
                // Replay the state machine to a random point of the cycle.
                let synced = self.sync.v > 0.0;
                let span = if synced { lead } else { 2.0 * t };
                let target = self.rng.unit() * span;
                let mut tm = 0.0f32;
                let slope = (1.0 + wf) * (1.0 - sub) / t;
                let mut guard = 0;
                loop {
                    let s = segment(v.state, wf, sub, v.pw, v.pw2);
                    if let Some(tg) = s.g_abs {
                        v.level = tg;
                    } else {
                        v.level += s.g;
                    }
                    let dur = s.dur * t;
                    v.level -= s.dur * (1.0 + wf) * (1.0 - sub);
                    v.state = (v.state + 1) & 3;
                    tm += dur;
                    guard += 1;
                    if tm > target || guard > 64 {
                        break;
                    }
                }
                let remain = tm - target;
                v.oscstate = remain;
                v.syncstate = lead - target;
                v.slope = slope;
                dc0 += slope;
                let start = v.level + slope * remain;
                y0[0] += start * v.gl;
                y0[1] += start * v.gr;
            }
        }
        if !b.retrigger {
            self.buf.y = if b.stereo { y0 } else { [0.5 * (y0[0] + y0[1]); 2] };
            if !b.stereo {
                // Mono writes g * 1/sqrt(N) (pan 1) into L only.
                self.buf.y[0] = self.v[..self.n].iter().map(|v| (v.level + v.slope * v.oscstate) * att).sum();
            }
            self.buf.dcacc = dc0;
        }
        self.first = false;
    }

    fn convolute(&mut self, u: usize, b: &OscBlock, fm: Option<(usize, f32)>, pm: f32) {
        let (wf, sub) = (self.shape.v, self.sub.v);
        let synced = self.sync.v > 0.0;
        let stereo = b.stereo;
        let p = 1.0 / pm;
        let v = &mut self.v[u];
        if synced && v.syncstate < v.oscstate {
            // Hard-sync reset: undo the ramp not yet travelled, restart.
            v.level += v.slope * (v.oscstate - v.syncstate);
            v.state = 0;
            v.oscstate = v.syncstate;
            v.syncstate += v.lead;
        }
        if v.state == 0 {
            v.pw = self.w1.v;
            v.pw2 = 2.0 * self.w2.v;
        }
        let s = segment(v.state, wf, sub, v.pw, v.pw2);
        let g = match s.g_abs {
            Some(tg) => tg - v.level,
            None => s.g,
        };
        let (gl, gr) = if stereo { (g * v.gl, g * v.gr) } else { (g * self_att(self.n), 0.0) };
        let d = match fm {
            None => self.buf.place(v.oscstate * p, gl, gr, stereo),
            Some((k, mul)) => {
                let frac = ((v.oscstate * p / mul).clamp(0.0, 0.999_999) as f64 * 16_777_216.0) as u32;
                self.buf.place_at(k, frac, gl, gr, stereo);
                k
            }
        };
        let slope = (1.0 + wf) * (1.0 - sub) / v.t;
        self.buf.add_slope(d, slope - v.slope);
        v.slope = slope;
        v.level += g - s.dur * (1.0 + wf) * (1.0 - sub);
        v.oscstate += s.dur * v.t;
        v.state = (v.state + 1) & 3;
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        if self.first {
            self.init(b, false);
        }
        self.targets(b);
        for lag in [&mut self.shape, &mut self.w1, &mut self.w2, &mut self.sub, &mut self.sync] {
            lag.process();
        }
        let p = period(b.pitch, b.fs_os);
        let pm = 1.0 / p;
        let synced = self.sync.v > 0.0;
        for u in 0..self.n {
            self.v[u].drift.next(&mut self.rng);
            let (t, lead) = self.timing(b, u);
            self.v[u].t = t;
            self.v[u].lead = lead;
        }
        if b.fm {
            for k in 0..64 {
                let mul = (1.0 + b.fmdepth * b.fm_buf[k]).clamp(0.1, 1.9);
                let a = pm * mul;
                for u in 0..self.n {
                    while self.v[u].oscstate < a || (synced && self.v[u].syncstate < a) {
                        self.convolute(u, b, Some((k, mul)), pm);
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
                while (self.v[u].oscstate < a || (synced && self.v[u].syncstate < a)) && guard < 4096 {
                    self.convolute(u, b, None, pm);
                    guard += 1;
                }
                self.v[u].oscstate -= a;
                self.v[u].syncstate -= a;
                if !synced {
                    self.v[u].syncstate = self.v[u].syncstate.max(0.0);
                }
            }
        }
        let fsync = if synced { self.sync.v.min(156.0 - b.pitch).max(0.0) } else { 0.0 };
        let f = super::freq(b.pitch + fsync);
        self.buf.set_leak(BlitBuf::leak_target(f, b.fs(), 0.995));
        let att = self_att(self.n);
        self.buf.integrate(att * pm, b.stereo, l, r);
        self.chr.process(b.character, b.fs(), b.stereo, l, r);
    }
}

#[inline]
fn self_att(n: usize) -> f32 {
    1.0 / (n as f32).sqrt()
}
