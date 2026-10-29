//! Spring Reverb (type 27): a feedback loop of a main fractional delay and
//! a chain of 16 nested allpasses run on four lanes (left input, left
//! feedback, right input, right feedback), with an early-reflections
//! network, a damping low-pass, a DC blocker, chaotic delay wobble and a
//! "knock" (a short half-sine thump into the springs). EFFECTS-2.md 6.6.

use super::common::*;
use super::FxCtx;
use crate::dsp::Rng;
use crate::kit::core::{SlotDef, SlotUnit, SlotValue};

/// The reflections network's output shelf keeps ONE state for both
/// channels in the reference; mirrored so stereo images match.
const QUIRK_SHARED_REFLECTION_SHELF: bool = true;
const _: () = assert!(QUIRK_SHARED_REFLECTION_SHELF);

pub fn slot_defs() -> [SlotDef; 12] {
    [
        percent("Size", 0.5),
        about(fl("Decay", 0.0, 1.0, 0.5, SlotUnit::None), "decay time 0.5 * 9^v s"),
        percent("Reflections", 1.0),
        percent("HF Damping", 0.5),
        percent("Spin", 0.5),
        percent("Chaos", 0.0),
        about(fl("Knock", 0.0, 1.0, 0.0, SlotUnit::None), "toggle: above 0.5 = on; a rising edge knocks the springs"),
        percent("Mix", 0.5),
        unused(),
        unused(),
        unused(),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

/// A delay line read by age: a read returns the value written `d` writes
/// before the current cycle's write (reads come before the write).
struct AgeLine {
    buf: Vec<f32>,
    mask: usize,
    k: usize,
    y_prev: f32,
}

impl AgeLine {
    fn new(min_len: usize) -> AgeLine {
        let n = (min_len + 8).next_power_of_two();
        AgeLine { buf: vec![0.0; n], mask: n - 1, k: 0, y_prev: 0.0 }
    }
    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.y_prev = 0.0;
    }
    #[inline]
    fn age(&self, a: usize) -> f32 {
        self.buf[self.k.wrapping_sub(a) & self.mask]
    }
    #[inline]
    fn write(&mut self, x: f32) {
        self.buf[self.k & self.mask] = x;
        self.k = self.k.wrapping_add(1);
    }
    fn max_delay(&self) -> f32 {
        (self.mask - 4) as f32
    }
    /// Third-order Lagrange read.
    #[inline]
    fn lagrange(&self, d: f32) -> f32 {
        let d = d.clamp(1.0, self.max_delay());
        let mut i = d.floor() as usize;
        let mut f = d - i as f32;
        if i >= 1 {
            i -= 1;
            f += 1.0;
        }
        let (y0, y1, y2, y3) = (self.age(i), self.age(i + 1), self.age(i + 2), self.age(i + 3));
        let c0 = -(f - 1.0) * (f - 2.0) * (f - 3.0) / 6.0;
        y0 * c0 + f * (y1 * (f - 2.0) * (f - 3.0) / 2.0 - y2 * (f - 1.0) * (f - 3.0) / 2.0 + y3 * (f - 1.0) * (f - 2.0) / 6.0)
    }
    /// First-order Thiran allpass read (keeps its own output state).
    #[inline]
    fn thiran(&mut self, i: usize, alpha: f32) -> f32 {
        let y = self.age(i + 1) + alpha * (self.age(i) - self.y_prev);
        self.y_prev = y;
        y
    }
}

/// Split a Thiran delay into (integer age, alpha).
fn thiran_params(d: f32) -> (usize, f32) {
    let d = d.max(1.0);
    let mut i = d.floor() as usize;
    let mut f = d - i as f32;
    if f < 0.618 && i >= 1 {
        i -= 1;
        f += 1.0;
    }
    (i.max(1), (1.0 - f) / (1.0 + f))
}

/// The TPT state-variable filter (resonance 1/sqrt 2).
#[derive(Clone, Copy, Default)]
struct TptSvf {
    g: f32,
    r: f32,
    h: f32,
    s: [[f32; 2]; 2],
}

impl TptSvf {
    fn set(&mut self, fc: f32, sr: f32) {
        self.g = (std::f32::consts::PI * (fc / sr).min(0.49)).tan();
        self.r = std::f32::consts::SQRT_2;
        self.h = 1.0 / (1.0 + self.r * self.g + self.g * self.g);
    }
    /// (lp, hp) of channel `c`.
    #[inline]
    fn tick(&mut self, c: usize, x: f32) -> (f32, f32) {
        let s = &mut self.s[c];
        let hp = (x - (self.g + self.r) * s[0] - s[1]) * self.h;
        let bp = self.g * hp + s[0];
        let lp = self.g * bp + s[1];
        s[0] = self.g * hp + bp;
        s[1] = self.g * bp + lp;
        (lp, hp)
    }
}

/// The first-order shelf of Tape 6.5 A.1 (low gain gl, high gain gh).
#[derive(Clone, Copy, Default)]
struct Shelf {
    b0: f32,
    b1: f32,
    a1: f32,
    z: [f32; 2],
}

impl Shelf {
    fn set(&mut self, gl: f32, gh: f32, fc: f32, sr: f32) {
        let k = 1.0 / (std::f32::consts::PI * fc / sr).tan();
        let rho = (gh / gl).sqrt();
        let a0 = k / rho + 1.0;
        self.b0 = (gh / rho * k + gl) / a0;
        self.b1 = (-gh / rho * k + gl) / a0;
        self.a1 = (-k / rho + 1.0) / a0;
    }
    #[inline]
    fn tick(&mut self, c: usize, x: f32) -> f32 {
        let y = self.b0 * x + self.z[c];
        self.z[c] = self.b1 * x - self.a1 * y;
        y
    }
}

const REFL_SECS: [f32; 4] = [0.07, 0.17, 0.23, 0.29];
const STAGES: usize = 16;

pub struct SpringReverb {
    sr: f32,
    rng: Rng,
    /// [stage][lane][outer, inner]
    ap: Vec<[[AgeLine; 2]; 4]>,
    main: [AgeLine; 2],
    refl: [[AgeLine; 4]; 2],
    refl_shelf: Shelf,
    damp: TptSvf,
    dc: TptSvf,
    lanes_f: [f32; 2],
    chaos_v: f32,
    chaos_step: f32,
    chaos_left: f32,
    knock_on: bool,
    knock_pos: usize,
    knock_len: usize,
    mix: Line,
}

impl SpringReverb {
    pub fn new(sr: f32) -> SpringReverb {
        let ap_len = (sr * 3.35 / 1000.0) as usize + 4;
        let mut ap = Vec::with_capacity(STAGES);
        for _ in 0..STAGES {
            ap.push(std::array::from_fn(|_| [AgeLine::new(ap_len), AgeLine::new(ap_len)]));
        }
        let main_len = (1000.0 + 0.099 * sr * 1.08) as usize + 8;
        let refl_len = (0.29 * sr) as usize + 8;
        SpringReverb {
            sr,
            rng: Rng::new(0x5b1e),
            ap,
            main: [AgeLine::new(main_len), AgeLine::new(main_len)],
            refl: std::array::from_fn(|_| std::array::from_fn(|_| AgeLine::new(refl_len))),
            refl_shelf: Shelf::default(),
            damp: TptSvf::default(),
            dc: TptSvf::default(),
            lanes_f: [0.0; 2],
            chaos_v: 0.0,
            chaos_step: 0.0,
            chaos_left: 0.0,
            knock_on: false,
            knock_pos: 0,
            knock_len: 0,
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        for st in self.ap.iter_mut() {
            for lane in st.iter_mut() {
                lane[0].clear();
                lane[1].clear();
            }
        }
        for m in self.main.iter_mut() {
            m.clear();
        }
        for ch in self.refl.iter_mut() {
            for d in ch.iter_mut() {
                d.clear();
            }
        }
        self.refl_shelf.z = [0.0; 2];
        self.damp.s = [[0.0; 2]; 2];
        self.dc.s = [[0.0; 2]; 2];
        self.lanes_f = [0.0; 2];
        self.chaos_v = 0.0;
        self.chaos_left = 0.0;
        self.knock_on = false;
        self.knock_pos = 0;
        self.knock_len = 0;
        self.mix = Line::new();
    }

    /// The chaos smoother: a new target each block, a 50 ms linear glide,
    /// advanced by one block.
    fn chaos(&mut self, d: f32) -> f32 {
        let target = self.rng.unit() * d * 0.07;
        let n = 0.05 * self.sr;
        self.chaos_step = (target - self.chaos_v) / n;
        self.chaos_left = n;
        let adv = (B as f32).min(self.chaos_left);
        self.chaos_v += self.chaos_step * adv;
        self.chaos_left -= adv;
        self.chaos_v
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        let size = clamp01(p[0].v);
        let d = 1000.0 + 0.099 * size * self.sr;
        self.chaos(d);
        self.mix.set_target_smoothed(clamp01(p[7].v));
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        let size = clamp01(p[0].v);
        let decay = clamp01(p[1].v);
        let refl = clamp01(p[2].v);
        let damping = clamp01(p[3].v);
        let spin = clamp01(p[4].v);
        let chaos = clamp01(p[5].v);
        let knock = p[6].v > 0.5;

        let t60 = 0.5 * 9f32.powf(0.95 * decay - 0.7 * (1.0 - size * size));
        let mut dmain = 1000.0 + 0.099 * size * sr;
        let ch = self.chaos(dmain);
        dmain += chaos * chaos * chaos * ch;
        let g_fb = 0.001f32.powf(dmain / (t60 * sr));

        let (ap_i, ap_alpha) = thiran_params(sr * (0.35 + 3.0 * size) / 1000.0);
        let g = 0.5 - 0.4 * spin;
        let lane_g = [g, -g, g, -g];

        let skew = 0.95 * refl * refl;
        let mut refl_len = [0f32; 4];
        let mut refl_fb = [0f32; 4];
        for i in 0..4 {
            refl_len[i] = (REFL_SECS[i] * size * sr).max(1.0);
            refl_fb[i] = 0.001f32.powf(refl_len[i] / (t60 * sr)) * 0.23 * skew * (0.735 + 0.235 * size);
        }
        self.refl_shelf.set(1.0, 10f32.powf((-1.0 - 9.0 * damping) / 20.0), 800.0, sr);
        self.damp.set(4000.0 * 4.5f32.powf(1.0 - damping), sr);
        self.dc.set(40.0, sr);

        if knock && !self.knock_on && self.knock_pos >= self.knock_len {
            let amount = self.rng.unit();
            let secs = (0.0005 + 0.0005 * amount) * (1.0 + 0.5 * size);
            self.knock_len = (sr * secs) as usize;
            self.knock_pos = 0;
            self.knock_on = true;
        }
        if !knock && self.knock_on && self.knock_pos >= self.knock_len {
            self.knock_on = false;
        }

        self.mix.set_target_smoothed(clamp01(p[7].v));
        let mut wet_l = [0f32; 32];
        let mut wet_r = [0f32; 32];
        for k in 0..B {
            let kn = if self.knock_pos < self.knock_len {
                let v = 2.0 * (std::f32::consts::TAU * self.knock_pos as f32 / (2 * self.knock_len) as f32).sin();
                self.knock_pos += 1;
                v
            } else {
                0.0
            };
            let inl = pade_tanh(l[k] - g_fb * self.main[0].lagrange(dmain));
            let inr = pade_tanh(r[k] - g_fb * self.main[1].lagrange(dmain));
            let sl = self.dc.tick(0, inl).1 + kn;
            let sr_ = self.dc.tick(1, inr).1 + kn;
            let mut s = [sl, self.lanes_f[0], sr_, self.lanes_f[1]];
            for st in self.ap.iter_mut() {
                for (lane, x) in s.iter_mut().enumerate() {
                    let gl = lane_g[lane];
                    let [outer, inner] = &mut st[lane];
                    // The outer delay's output passes through the inner allpass.
                    let vi = inner.thiran(ap_i, ap_alpha);
                    let xi = outer.thiran(ap_i, ap_alpha);
                    let wi = xi + gl * vi;
                    inner.write(wi);
                    let v = vi - gl * wi;
                    let w = *x + gl * v;
                    outer.write(w);
                    *x = v - gl * w;
                }
            }
            let mut out = [0f32; 2];
            for c in 0..2 {
                self.main[c].write(s[2 * c + 1]);
                let mut sm = s[2 * c];
                // Reflections: read all four, Householder mix, weighted sum
                // through the shelf; write the main signal into all four.
                let mut o = [0f32; 4];
                for i in 0..4 {
                    o[i] = self.refl[c][i].lagrange(refl_len[i]);
                }
                let sum: f32 = o.iter().sum();
                let mut acc = 0.0;
                for i in 0..4 {
                    acc += (o[i] - 0.5 * sum) * refl_fb[i];
                }
                let shelf_ch = 0;
                sm -= self.refl_shelf.tick(shelf_ch, acc * 0.25);
                for i in 0..4 {
                    self.refl[c][i].write(sm);
                }
                let y = self.damp.tick(c, sm).0;
                out[c] = y;
                self.lanes_f[c] = y;
            }
            wet_l[k] = out[0];
            wet_r[k] = out[1];
        }
        self.mix.fade_2_blocks(l, r, &wet_l, &wet_r);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;

    fn run(p: &[SlotValue; 12], blocks: usize, input: impl Fn(usize) -> f32) -> Vec<f32> {
        let ctx = FxCtx::new(48000.0);
        let mut fx = SpringReverb::new(48000.0);
        let mut out = Vec::new();
        for b in 0..blocks {
            let mut l = [0f32; 32];
            let mut r = [0f32; 32];
            for k in 0..32 {
                l[k] = input(b * 32 + k);
                r[k] = l[k];
            }
            fx.process(p, &ctx, &mut l, &mut r);
            out.extend_from_slice(&l);
        }
        out
    }

    #[test]
    fn decays_after_input_stops() {
        let mut p = default_values(27);
        p[7].v = 1.0;
        let mut rng = Rng::new(3);
        let noise: Vec<f32> = (0..12000).map(|_| 0.5 * rng.bipolar()).collect();
        let out = run(&p, 48000 * 3 / 32, |n| if n < noise.len() { noise[n] } else { 0.0 });
        assert!(out.iter().all(|x| x.is_finite()));
        let win = 12000;
        let e: Vec<f32> = out.chunks(win).map(|c| c.iter().map(|x| x * x).sum::<f32>()).collect();
        assert!(e[1] > 1e-6, "{e:?}");
        for i in 2..e.len() - 1 {
            assert!(e[i + 1] < e[i], "{e:?}");
        }
    }

    #[test]
    fn knock_makes_sound() {
        let mut p = default_values(27);
        p[7].v = 1.0;
        p[6].v = 1.0;
        let out = run(&p, 200, |_| 0.0);
        assert!(out.iter().any(|x| x.abs() > 1e-4));
        assert!(out.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn lagrange_reads_integer_delays_exactly() {
        let mut d = AgeLine::new(64);
        for n in 0..40 {
            if n >= 10 {
                assert!((d.lagrange(7.0) - (n - 7) as f32).abs() < 1e-4);
            }
            d.write(n as f32);
        }
    }
}
