//! Reverb 2 (type 11): a pre-delay and four input allpass diffusers feed a
//! loop of four blocks (two allpasses, low/high damping and a modulated
//! delay each), run as a figure of eight; stereo output taps are read
//! inside each block's delay. All lengths scale with the room size.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{db_to_linear, SlotDef, SlotUnit, SlotValue};

const MAX_LEN: usize = 131072;
const DMASK: i32 = (MAX_LEN - 1) as i32;
const PRE_LEN: usize = 3 * 384000;

const IN_AP_MS: [f32; 4] = [4.76, 6.81, 10.13, 16.72];
/// Per loop block: allpass A, allpass B, delay (ms).
const LOOP_MS: [[f32; 3]; 4] = [[38.2, 53.4, 178.8], [44.0, 41.0, 126.5], [48.3, 60.5, 106.1], [38.9, 42.2, 139.4]];
const TAP_L_MS: [f32; 4] = [80.3, 59.3, 97.7, 122.6];
const TAP_R_MS: [f32; 4] = [35.5, 101.6, 73.9, 80.3];
const TAP_GAIN: [f32; 4] = [1.5 / 4.0, 1.2 / 4.0, 1.0 / 4.0, 0.8 / 4.0];

pub fn slot_defs() -> [SlotDef; 12] {
    [
        envtime("Pre-Delay", -8.0, 1.0, -4.0),
        bipolar("Room Size", 0.0),
        fl("Decay Time", -4.0, 6.0, 0.75, SlotUnit::Log2Secs),
        percent("Diffusion", 1.0),
        percent("Buildup", 1.0),
        percent("Modulation", 0.5),
        percent("LF Damping", 0.2),
        percent("HF Damping", 0.2),
        width_db("Width"),
        percent("Mix", 0.33),
        unused(),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

/// Twice the decay plus twice the pre-delay, in blocks (tempo ratio 1).
pub fn ringout_blocks(p: &[SlotValue; 12], sr: f32) -> i32 {
    (sr * (p[2].v.exp2().max(1.0) * 2.0 + p[0].v.exp2().max(0.1) * 2.0) / B as f32) as i32
}

/// Schroeder allpass circulating over a variable length.
struct Allpass {
    buf: Vec<f32>,
    k: usize,
}

impl Allpass {
    fn new(ms: f32, sr: f32) -> Allpass {
        let cap = ((ms * sr / 1000.0 * 2.0) as usize + 2).min(MAX_LEN);
        Allpass { buf: vec![0.0; cap], k: 0 }
    }
    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.k = 0;
    }
    #[inline]
    fn tick(&mut self, x: f32, c: f32, len: usize) -> f32 {
        let len = len.clamp(1, self.buf.len());
        if self.k >= len {
            self.k = 0;
        }
        let b = self.buf[self.k];
        let w = x - c * b;
        let y = b + c * w;
        self.buf[self.k] = w;
        self.k += 1;
        if self.k >= len {
            self.k = 0;
        }
        y
    }
}

/// A delay with two fixed output taps and a modulated main read.
struct ModDelay {
    buf: Vec<f32>,
    k: i32,
}

impl ModDelay {
    fn new() -> ModDelay {
        ModDelay { buf: vec![0.0; MAX_LEN], k: 0 }
    }
    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.k = 0;
    }
    /// `md` = modulation in 1/256 samples. Returns (main, tap left, tap right).
    #[inline]
    fn tick(&mut self, x: f32, len: i32, md: i32, tl: i32, tr: i32) -> (f32, f32, f32) {
        let k = self.k;
        let tap_l = self.buf[((k - tl) & DMASK) as usize];
        let tap_r = self.buf[((k - tr) & DMASK) as usize];
        let i = md >> 8;
        let f = (md & 255) as f32;
        let a = self.buf[((k - len + i + 1) & DMASK) as usize];
        let b = self.buf[((k - len + i) & DMASK) as usize];
        let y = (a * f + b * (256.0 - f)) / 256.0;
        self.buf[(k & DMASK) as usize] = x;
        self.k = (k + 1) & DMASK;
        (y, tap_l, tap_r)
    }
}

pub struct Reverb2 {
    sr: f32,
    pre: Vec<f32>,
    pre_w: usize,
    in_ap: [Allpass; 4],
    ap_a: [Allpass; 4],
    ap_b: [Allpass; 4],
    dl: [ModDelay; 4],
    lp: [f32; 4],
    hp: [f32; 4],
    state: f32,
    lfo: Quadrature,
    decay_mult: Line,
    c_in: Line,
    c_loop: Line,
    hf: Line,
    depth: Line,
    mix: Line,
    width: Line,
}

fn aps(ms: [f32; 4], sr: f32) -> [Allpass; 4] {
    [Allpass::new(ms[0], sr), Allpass::new(ms[1], sr), Allpass::new(ms[2], sr), Allpass::new(ms[3], sr)]
}

impl Reverb2 {
    pub fn new(sr: f32) -> Reverb2 {
        let mut lfo = Quadrature::new(0.0);
        lfo.set_rate(2.0 * std::f64::consts::PI * 0.25 / sr as f64);
        Reverb2 {
            sr,
            pre: vec![0.0; PRE_LEN],
            pre_w: 0,
            in_ap: aps(IN_AP_MS, sr),
            ap_a: aps([LOOP_MS[0][0], LOOP_MS[1][0], LOOP_MS[2][0], LOOP_MS[3][0]], sr),
            ap_b: aps([LOOP_MS[0][1], LOOP_MS[1][1], LOOP_MS[2][1], LOOP_MS[3][1]], sr),
            dl: [ModDelay::new(), ModDelay::new(), ModDelay::new(), ModDelay::new()],
            lp: [0.0; 4],
            hp: [0.0; 4],
            state: 0.0,
            lfo,
            decay_mult: Line::new(),
            c_in: Line::new(),
            c_loop: Line::new(),
            hf: Line::new(),
            depth: Line::new(),
            mix: Line::new(),
            width: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        self.pre.iter_mut().for_each(|s| *s = 0.0);
        self.pre_w = 0;
        for a in self.in_ap.iter_mut().chain(self.ap_a.iter_mut()).chain(self.ap_b.iter_mut()) {
            a.clear();
        }
        for d in &mut self.dl {
            d.clear();
        }
        self.lp = [0.0; 4];
        self.hp = [0.0; 4];
        self.state = 0.0;
        let mut lfo = Quadrature::new(0.0);
        lfo.set_rate(2.0 * std::f64::consts::PI * 0.25 / self.sr as f64);
        self.lfo = lfo;
        self.decay_mult = Line::new();
        self.c_in = Line::new();
        self.c_loop = Line::new();
        self.hf = Line::new();
        self.depth = Line::new();
        self.mix = Line::new();
        self.width = Line::new();
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        let scale = p[1].v.exp2();
        let n = |ms: f32| ((sr * ms / 1000.0 * scale) as usize).clamp(1, MAX_LEN);
        let in_len = [n(IN_AP_MS[0]), n(IN_AP_MS[1]), n(IN_AP_MS[2]), n(IN_AP_MS[3])];
        let mut a_len = [0usize; 4];
        let mut b_len = [0usize; 4];
        let mut d_len = [0i32; 4];
        let mut t_l = [0i32; 4];
        let mut t_r = [0i32; 4];
        for b in 0..4 {
            a_len[b] = n(LOOP_MS[b][0]);
            b_len[b] = n(LOOP_MS[b][1]);
            // Room for the modulation (up to 5 ms) inside the buffer.
            d_len[b] = n(LOOP_MS[b][2]).min(MAX_LEN - 2 - (0.005 * sr) as usize).max(2) as i32;
            t_l[b] = n(TAP_L_MS[b]) as i32;
            t_r[b] = n(TAP_R_MS[b]) as i32;
        }

        self.decay_mult.set_target(0.001f32.powf((0.5508 * scale) / (4.0 * p[2].v.exp2())));
        self.c_in.set_target(0.7 * p[3].v);
        self.c_loop.set_target(0.7 * p[4].v);
        self.hf.set_target(0.8 * p[7].v);
        self.depth.set_target(p[5].v * sr * 0.005);
        let lf = (0.2 * p[6].v).clamp(0.01, 0.99);
        let pdt = ((sr * time_secs(&p[0], ctx.tsr)) as usize).clamp(1, PRE_LEN - 1);

        let mut wl = [0f32; B];
        let mut wr = [0f32; B];
        for k in 0..B {
            let c_in = self.c_in.at(k);
            let c_loop = self.c_loop.at(k);
            let hf = self.hf.at(k).clamp(0.01, 0.99);
            let depth = self.depth.at(k);
            let dm = self.decay_mult.at(k);

            self.pre[self.pre_w] = 0.5 * (l[k] + r[k]);
            let mut inp = self.pre[(self.pre_w + PRE_LEN - pdt) % PRE_LEN];
            self.pre_w = (self.pre_w + 1) % PRE_LEN;
            for i in 0..4 {
                inp = self.in_ap[i].tick(inp, c_in, in_len[i]);
            }

            self.lfo.step();
            let lv = [self.lfo.r as f32, self.lfo.i as f32, -self.lfo.r as f32, -self.lfo.i as f32];
            let mut x = self.state;
            let (mut ol, mut or) = (0.0, 0.0);
            for b in 0..4 {
                x += inp;
                x = self.ap_a[b].tick(x, c_loop, a_len[b]);
                x = self.ap_b[b].tick(x, c_loop, b_len[b]);
                self.lp[b] = self.lp[b] * hf + x * (1.0 - hf);
                x = self.lp[b];
                self.hp[b] = self.hp[b] * (1.0 - lf) + x * lf;
                x -= self.hp[b];
                let md = (depth * lv[b] * 256.0) as i32;
                let (y, tl, tr) = self.dl[b].tick(x, d_len[b], md, t_l[b], t_r[b]);
                x = y;
                ol += tl * TAP_GAIN[b];
                or += tr * TAP_GAIN[b];
                x *= dm;
            }
            self.state = x;
            wl[k] = ol;
            wr[k] = or;
        }

        self.width.set_target_smoothed(db_to_linear(p[8].v));
        apply_width(&mut wl, &mut wr, &self.width);
        self.mix.set_target_smoothed(p[9].v);
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;
    use crate::dsp::Rng;

    fn render(fx: &mut Reverb2, p: &[SlotValue; 12], input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let ctx = FxCtx::new(48000.0);
        let (mut ol, mut or) = (Vec::new(), Vec::new());
        for chunk in input.chunks(B) {
            let mut l = [0f32; B];
            l[..chunk.len()].copy_from_slice(chunk);
            let mut r = l;
            fx.process(p, &ctx, &mut l, &mut r);
            ol.extend_from_slice(&l);
            or.extend_from_slice(&r);
        }
        (ol, or)
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn tail_decays_and_ends_within_ringout() {
        let mut p = default_values(11);
        p[9].v = 1.0;
        let mut fx = Reverb2::new(48000.0);
        let mut rng = Rng::new(5);
        let ro = ringout_blocks(&p, 48000.0) as usize * B;
        let n = 4800 + ro;
        let x: Vec<f32> = (0..n).map(|i| if i < 4800 { 0.5 * rng.bipolar() } else { 0.0 }).collect();
        let (l, r) = render(&mut fx, &p, &x);
        assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
        let w = 9600;
        let mut prev = f32::MAX;
        for i in 3..(n / w) {
            let e = rms(&l[i * w..(i + 1) * w]);
            assert!(e < prev * 1.05, "window {i}: {e} vs {prev}");
            prev = prev.min(e);
        }
        let peak = l.iter().fold(0f32, |a, s| a.max(s.abs()));
        let tail = l[n - B..].iter().fold(0f32, |a, s| a.max(s.abs()));
        assert!(tail < 1e-3 * peak, "tail {tail} peak {peak}");
    }
}
