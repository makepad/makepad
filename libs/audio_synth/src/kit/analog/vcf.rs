//! The voltage-controlled filter: four variable-gain cells with their
//! buffers, and a transconductance cell for the resonance, wired the ways
//! the sheet draws them.
//!
//! A cell is a current-in integrator: the current into it (coupled through a
//! resistor from the stage before, and from its own output through the
//! feedback resistor) charges the pole capacitor. Coupling a signal into
//! the capacitor instead makes a high-pass stage; both together an all-pass.
//! The resonance current always enters the first stage's resistor input,
//! whatever the signal does, and that is why the sheet's high-pass resonates
//! 2.4 times above the low-pass and its band-pass and all-pass 0.42 times
//! below: the loop has one low-pass stage more than the signal path has.
//!
//! What saturates, per the sheet:
//! - the resonance cell limits its current at its own control current, so
//!   past the oscillation point more control only means a larger sine;
//!   its transconductance bends over with control current (figure 6);
//! - a stage's input diode shuts off when the signal current reaches the
//!   reference current, the first stage clips, the apparent resonance falls,
//!   the DC level shifts and the oscillation's pitch moves;
//! - the gain cell's small error is mostly second harmonic.
//!
//! Each sample is solved implicitly, the nonlinearities by two secant
//! passes, the cutoff and resonance ramped per sample. Levels: 6 V at a
//! buffer = 1 inside; input 1 is 3 dB below clipping (the sheet's distortion
//! figure), output 1 the same.

use super::{part, tol, Thermal};
use crate::dsp::Rng;

/// The responses, in the order of the kit's filter types.
pub const LP: u8 = 0;
pub const HP: u8 = 1;
pub const BP: u8 = 2;
pub const AP: u8 = 3;
pub const SV_LP: u8 = 4;
pub const SV_HP: u8 = 5;
pub const SV_BP: u8 = 6;
pub const SV_NOTCH: u8 = 7;

/// (resistor coupling, capacitor coupling) of each stage from the one before.
const WIRING: [[(f32, f32); 4]; 4] = [
    [(1.0, 0.0); 4],
    [(0.0, 1.0); 4],
    [(0.0, 1.0), (0.0, 1.0), (1.0, 0.0), (1.0, 0.0)],
    [(1.0, 1.0); 4],
];

/// Signal 1 at the input and output, as a fraction of clipping.
const HEADROOM: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Figure 6, fitted: gm = GM_MAX * I / (I + GM_BEND); 1 mS at 100 uA.
const GM_MAX: f32 = 2200.0;
const GM_BEND: f32 = 120.0;
/// Reference current at each cell, uA (the unit of signal current here).
const I_REF: f32 = 63.0;
/// Full control current, uA, and where it starts to oscillate: the sheet's
/// example picks the input resistor for oscillation at 90% of the control.
const I_MAX: f32 = 100.0;
const OSC_AT: f32 = 0.9;
/// The sheet's `A_osc` per wiring: the loop gain needed at the frequency the
/// wiring resonates at, which its input resistor is chosen for. Low-pass: 12
/// dB (45 degrees each). High-pass and band-pass: one low-pass and three
/// high-pass poles, or three and one, at 67.5 and 22.5 degrees. All-pass:
/// only the first stage's resistor input loses level. (The last entry is
/// the state-variable wiring, where the same current sets the Q.)
const OSC_GAIN: [f32; 5] = [4.0, 3.31, 3.31, 1.11, 4.0];
/// Second-harmonic term of a cell (calibrated in the tests: 0.1% in the
/// passband, signal 3 dB below clipping).
const EVEN: f32 = 0.08;
/// Signal current limits on the two sides of a cell, in reference currents:
/// the input diode cuts off at -1; the other side has more room.
const LIMIT_LOW: f32 = 1.0;
const LIMIT_HIGH: f32 = 1.3;
/// The resonance input's coupling corner (1 uF into 54.6 k), Hz.
const FEEDBACK_CORNER: f32 = 2.9;
/// Output noise: -86 dB re the maximum output.
const NOISE: f32 = 5e-5;
/// The state-variable wiring's Q: 0.5 with no control, this much at the top
/// (the feedback resistor's job, past the 1 M limit across the integrators).
const SV_MAX_Q: f32 = 30.0;

#[inline]
fn gm(ua: f32) -> f32 {
    GM_MAX * ua / (ua + GM_BEND)
}

#[inline]
fn tanh_over(x: f32) -> f32 {
    // tanh(x) / x by the kit's rational fit, saturating beyond 3.
    let x2 = x * x;
    if x2 < 9.0 { (27.0 + x2) / (27.0 + 9.0 * x2) } else { 1.0 / x.abs() }
}

#[inline]
fn knee(z: f32, limit: f32) -> f32 {
    let k = 0.75 * limit;
    if z <= k {
        z
    } else {
        let u = (z - k) / (limit - k);
        k + (limit - k) * u / (1.0 + u)
    }
}

/// A cell's current over its error (the secant conductance).
#[inline]
fn cell(e: f32, even: f32) -> f32 {
    let bend = (1.0 + even * e).clamp(0.5, 1.5);
    let z = e * bend;
    let y = if z >= 0.0 { knee(z, LIMIT_HIGH) } else { -knee(-z, LIMIT_LOW) };
    if e.abs() < 1e-6 { bend } else { y / e }
}

#[derive(Clone, Copy, Debug)]
pub struct Vcf {
    mode: u8,
    /// Integrator states, last pass's conductances and OTA ratio.
    s: [f32; 4],
    a: [f32; 4],
    b: f32,
    dc: f32,
    dc_k: f32,
    /// Ramps: pole coefficient, feedback, limit, output offset.
    g: f32,
    dg: f32,
    k: f32,
    dk: f32,
    lim: f32,
    dlim: f32,
    offset: f32,
    doffset: f32,
    even: f32,
    input: f32,
    left: u32,
    rng: Rng,
    thermal: Thermal,
    seed: u32,
    fresh: bool,
}

impl Vcf {
    pub fn new(seed: u32) -> Vcf {
        Vcf {
            mode: LP,
            s: [0.0; 4],
            a: [1.0; 4],
            b: 1.0,
            dc: 0.0,
            dc_k: 0.0,
            g: 0.1,
            dg: 0.0,
            k: 0.0,
            dk: 0.0,
            lim: 1.0,
            dlim: 0.0,
            offset: 0.0,
            doffset: 0.0,
            even: EVEN,
            input: HEADROOM,
            left: 0,
            rng: Rng::new(seed.wrapping_mul(0x9e37_79b1) | 1),
            thermal: Thermal::new(seed ^ 0x51ed),
            seed,
            fresh: true,
        }
    }

    pub fn reset(&mut self) {
        self.s = [0.0; 4];
        self.a = [1.0; 4];
        self.b = 1.0;
        self.dc = 0.0;
        self.fresh = true;
    }

    /// The pole frequency a part makes of `hz` (the exponential error is in
    /// the upper two octaves and flat; the compensated scale wanders with
    /// temperature by what the sheet leaves after the tempco resistor).
    #[inline]
    fn pole_hz(&self, hz: f32, drift: f32, celsius: f32) -> f32 {
        if drift <= 0.0 {
            return hz;
        }
        let top = ((hz / 5000.0).max(1.0).log2() * 0.5).min(1.0);
        let flat = 1.0 - tol(drift, 0.0, 0.04, 0.12) * top * top;
        let scale = part(self.seed, 0) * tol(drift, 0.0, 100e-6, 300e-6) * celsius;
        632.0 * ((hz / 632.0).log2() * (1.0 + scale)).exp2() * flat
    }

    /// Settle the next `frames` samples: pole frequency, resonance control
    /// 0..1 (oscillation from 0.9), drive (input gain), drift.
    #[inline]
    pub fn prepare(&mut self, mode: u8, hz: f32, res: f32, drive: f32, drift: f32, rate: f32, frames: usize) {
        if mode != self.mode {
            self.mode = mode;
            self.reset();
        }
        let dt = frames as f32 / rate;
        let celsius = if drift > 0.0 { self.thermal.step(dt) } else { 0.0 };
        let hz = self.pole_hz(hz, drift, celsius).clamp(5.0, rate * 0.45);
        let g = (std::f32::consts::PI * hz / rate).tan();
        let res = res.clamp(0.0, 1.0);
        let ua = res * I_MAX;
        let k = OSC_GAIN[(mode as usize).min(4)] * gm(ua) / gm(OSC_AT * I_MAX);
        // The resonance cell's output cannot pass its control current; keep
        // the ratio finite as it closes.
        let lim = (ua / I_REF).max(1e-3);
        // Control feedthrough: 60 mV typ, 200 max over the pole frequency's
        // control range; 0.2 V typ, 1.5 max over the resonance's. Volts over
        // the 6 V unit, then output scale.
        let cv = (hz / 632.0).log2() / 10.0;
        let offset = (part(self.seed, 1).signum() * tol(drift, 0.0, 0.01, 0.033) * cv + part(self.seed, 2).signum() * tol(drift, 0.0, 0.033, 0.25) * res) / HEADROOM;
        self.even = EVEN * tol(drift, 0.5, 1.0, 3.0);
        self.input = HEADROOM * drive;
        self.dc_k = (std::f32::consts::TAU * FEEDBACK_CORNER / rate).min(1.0);
        if self.fresh {
            self.fresh = false;
            self.g = g;
            self.k = k;
            self.lim = lim;
            self.offset = offset;
            self.dg = 0.0;
            self.dk = 0.0;
            self.dlim = 0.0;
            self.doffset = 0.0;
        } else {
            let inv = 1.0 / frames.max(1) as f32;
            self.dg = (g - self.g) * inv;
            self.dk = (k - self.k) * inv;
            self.dlim = (lim - self.lim) * inv;
            self.doffset = (offset - self.offset) * inv;
        }
        self.left = frames as u32;
    }

    /// One sample in, one out.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        if self.left > 0 {
            self.left -= 1;
            self.g += self.dg;
            self.k += self.dk;
            self.lim += self.dlim;
            self.offset += self.doffset;
        }
        let x = x * self.input + self.rng.bipolar() * NOISE;
        let y = if self.mode >= SV_LP { self.tick_sv(x) } else { self.tick_cascade(x) };
        y / HEADROOM + self.offset
    }

    #[inline]
    fn tick_cascade(&mut self, x: f32) -> f32 {
        let w = &WIRING[self.mode as usize];
        let (g, k, lim, even, dc) = (self.g, self.k, self.lim, self.even, self.dc);
        let mut gk = [0.0f32; 4];
        let mut y = [0.0f32; 4];
        for pass in 0..2 {
            // Each stage is y = S + T * (stage before); the first also takes
            // the resonance current, which closes the loop through the last.
            let mut inv = [0.0f32; 4];
            for i in 0..4 {
                gk[i] = g * self.a[i];
                inv[i] = 1.0 / (1.0 + gk[i]);
            }
            let t = |i: usize| (w[i].1 - gk[i] * w[i].0) * inv[i];
            let (t2, t3, t4) = (t(1), t(2), t(3));
            let c1 = gk[0] * inv[0] * k * self.b;
            let c0 = (self.s[0] + (w[0].1 - gk[0] * w[0].0) * x) * inv[0] - c1 * dc;
            let s2 = self.s[1] * inv[1];
            let s3 = self.s[2] * inv[2];
            let s4 = self.s[3] * inv[3];
            let p = t2 * t3 * t4;
            let z = s4 + t4 * (s3 + t3 * s2);
            let den = 1.0 - p * c1;
            let den = if den.abs() < 0.05 { 0.05 } else { den };
            y[3] = (z + p * c0) / den;
            y[0] = c0 + c1 * y[3];
            y[1] = s2 + t2 * y[0];
            y[2] = s3 + t3 * y[1];
            if pass == 0 {
                // The nonlinear elements at this estimate, for the second pass.
                let v = k * (y[3] - dc) / lim;
                self.b = tanh_over(v);
                let fb = k * self.b * (y[3] - dc);
                self.a[0] = cell(-(w[0].0 * x - fb + y[0]), even);
                self.a[1] = cell(-(w[1].0 * y[0] + y[1]), even);
                self.a[2] = cell(-(w[2].0 * y[1] + y[2]), even);
                self.a[3] = cell(-(w[3].0 * y[2] + y[3]), even);
            }
        }
        // The buffers' rails, then the integrators move on.
        let mut prev = x;
        for i in 0..4 {
            let c = w[i].1 * prev;
            let out = y[i].clamp(-1.0, 1.0);
            let charge = out - c;
            self.s[i] = 2.0 * charge - self.s[i];
            prev = out;
            y[i] = out;
        }
        self.dc += (y[3] - self.dc) * self.dc_k;
        y[3] - self.dc
    }

    /// Figure 5: two integrator stages; the other two sections are the
    /// summer and the inverter. The resonance cell cancels part of the
    /// damping, limited at its control current.
    #[inline]
    fn tick_sv(&mut self, x: f32) -> f32 {
        let (g, even) = (self.g, self.even);
        let lift = (2.0 - 1.0 / SV_MAX_Q) * self.k / (OSC_GAIN[4] * gm(I_MAX) / gm(OSC_AT * I_MAX));
        let (mut lp, mut bp, mut hp) = (0.0, 0.0, 0.0);
        for pass in 0..2 {
            let d = 2.0 - lift * self.b;
            let g1 = g * self.a[0];
            let g2 = g * self.a[1];
            hp = (x - (d + g2) * self.s[0] - self.s[1]) / (1.0 + (d + g2) * g1);
            bp = self.s[0] + g1 * hp;
            lp = self.s[1] + g2 * bp;
            if pass == 0 {
                self.b = tanh_over(lift * bp / self.lim.max(0.05));
                self.a[0] = cell(hp, even);
                self.a[1] = cell(bp, even);
            }
        }
        let g1 = g * self.a[0];
        let g2 = g * self.a[1];
        let bp_out = bp.clamp(-1.0, 1.0);
        let lp_out = lp.clamp(-1.0, 1.0);
        self.s[0] = bp_out + g1 * hp;
        self.s[1] = lp_out + g2 * bp_out;
        // The summer's output sits on the same rails.
        let y = match self.mode {
            SV_HP => hp.clamp(-1.0, 1.0),
            SV_BP => bp_out,
            SV_NOTCH => (lp_out + hp).clamp(-1.0, 1.0),
            _ => lp_out,
        };
        self.dc += (y - self.dc) * self.dc_k;
        y - self.dc
    }

    /// The first stage's DC, after clipping (the sheet's warning).
    pub fn dc_shift(&self) -> f32 {
        self.dc
    }
}
