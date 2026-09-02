// Learned additive piano — a second, independent instrument beside the
// physical `Piano`, reimplemented in Rust from the algorithm of:
//
//   PianoForte, by Carlos Tarjano (tesserato)
//   https://github.com/tesserato/PianoForte — MIT licence
//   "Piano synthesizer based on micro (~8KB, ~1500 parameters) neural
//    networks and a novel representation for Quasi-Periodic signals",
//   trained on publicly available Creative Commons piano recordings
//   (University of Iowa MIS, bitKlavier, Salamander).
//
// What we adopted from that project (see resources/pianoforte-LICENSE.txt,
// which ships beside the model file):
//  - the trained network `engineMain` itself, embedded verbatim as
//    resources/pianoforte-engineMain.bin (an ONNX protobuf, parsed at
//    construction by the shared pure-Rust AI loader — no runtime dependency);
//  - the synthesis algorithm: a 30-partial harmonic additive voice whose
//    per-partial amplitudes are the network's output, crossfaded against a
//    13-partial measured inharmonic profile (three register buckets) with
//    fixed constant-Q decay, under an analytic attack/decay/tail envelope;
//  - the numeric constants of that algorithm (register tables G1-G3,
//    envelope laws, phase statistics, crossfade law).
// No C++ code was copied; this is a from-scratch reimplementation with the
// crate's own real-time conventions (allocation-free process, 64-sample
// control grid, deterministic phases instead of a shared global RNG).
//
// -----------------------------------------------------------------------
// How the original renders (the resolution of the "fixed decay vs learned
// envelope" question, from reading Source/Voices.h / Voices.cpp):
//
// Each voice runs TWO additive branches, crossfaded by key position
// (alpha = 1 - (1-pitch)*0.95, pitch = (key-21)/87):
//
//  1. the NETWORK branch (dominant in the treble): 30 strictly harmonic
//     partials (i+1)*f0. The network input is [pitch, velocity, pc] where
//     pc = tanh(elapsed_periods / 4511) is a TIME coordinate — the network
//     output IS the amplitude envelope, sampled along the note. Rendered
//     amps slew toward the latest network output with a one-pole
//     (coefficient 2000/fs per sample). There is no fixed decay here.
//  2. the PROFILE branch (dominant in the bass): 13 partials at measured
//     inharmonic ratios with fixed measured amplitudes (G1/G2/G3 by
//     register) and the fixed constant-Q decay exp(-0.0003 * phase) —
//     i.e. sigma_i = 0.0003 * 2*pi * f_i, each partial decays in
//     proportion to its own frequency. This branch is where the fixed
//     exponential lives; it never touches the network branch.
//
// Both branches are then shaped by a shared analytic envelope
// m = min(6*periods, 1/(1 + (0.003*periods)^2)) * velocity and, after
// release, a tail-off of 0.9997 per 44.1kHz sample.
// -----------------------------------------------------------------------
//
// Honest scope, versus the physical model: 13+30 partials with three
// register-bucketed profiles cannot express velocity-dependent spectral
// bloom (the network's velocity input bends the envelope, not the strike
// physics), sympathetic resonance, duplex/phantom content, half-pedal
// damper contact, una-corda felt, or re-strikes into ringing strings. What
// it DOES carry is the thing the physical model's objective has lacked: a
// per-key, per-time picture of real recorded partial ladders, learned from
// calibrated CC recordings rather than 14 MP3 GM samples. That picture is
// exported through `learned_partial_amps` / `profile_for_key` for the
// physical model's verification tooling (see tests/learned_targets.rs).
//
// Real-time contract: identical to `Piano` — process never allocates,
// locks, blocks or panics; all control decisions on the absolute 64-sample
// grid; output bit-identical for any block-size decomposition of the same
// event stream. Construction parses the embedded network and allocates;
// process touches none of that.

use crate::fx::{soft_clip, DcBlock, EarlyReflections, Eq, Perspective, Reverb, ReverbParams, ReverbPreset, Tone};
use crate::keys::{FIRST_KEY, LAST_KEY, NUM_KEYS};
use crate::modal::{detect_path, run_modes, KernelPath, MAX_CHUNK};
use crate::params::{PianoPreset, Voicing};
use crate::simd::*;
use crate::{Instrument, Piano, PianoEvent, TimedEvent};
use makepad_ai_loader::formats::onnx::{OnnxAttribute, OnnxModel};

// ---------------------------------------------------------------------------
// The PianoForte register profiles (measured partial ratios and amplitudes
// of real recordings; values verbatim from Voices.h, MIT — see header).
// G1: keys <= MIDI 30, G2: <= 50, G3: the rest. Note G1/G2 start at ratio
// 2.0 — the recorded bass speaks through its overtones, the fundamental is
// left to the (weak) network share.
// ---------------------------------------------------------------------------

pub const G1_RATIOS: [f32; 13] =
    [2.00, 3.01, 4.01, 5.02, 7.03, 9.07, 10.09, 12.13, 16.34, 18.47, 23.97, 43.46, 44.6];
pub const G1_AMPS: [f32; 13] = [
    0.083613121, 0.177226640, 0.078563445, 0.055689120, 0.076268673, 0.068125753, 0.081955909, 0.122625511,
    0.047938599, 0.052834103, 0.066100082, 0.043777002, 0.045282042,
];
pub const G2_RATIOS: [f32; 13] =
    [2.00, 3.01, 4.01, 6.01, 9.04, 11.09, 13.13, 14.15, 14.19, 15.19, 20.47, 22.57, 23.69];
pub const G2_AMPS: [f32; 13] = [
    0.242693843, 0.070528518, 0.047061190, 0.108408767, 0.091886403, 0.081442038, 0.042285556, 0.096102265,
    0.037857573, 0.067261067, 0.036687511, 0.041375190, 0.036410080,
];
pub const G3_RATIOS: [f32; 13] =
    [1.0, 2.0, 3.01, 4.01, 5.03, 6.04, 7.05, 8.07, 9.1, 10.13, 11.17, 13.25, 15.38];
pub const G3_AMPS: [f32; 13] = [
    0.101971614, 0.258188383, 0.062758582, 0.146710819, 0.054338347, 0.037401030, 0.139032937, 0.061960627,
    0.053050202, 0.031100251, 0.021438881, 0.015956460, 0.016091866,
];

/// (ratios, amplitudes) of the measured register profile PianoForte uses
/// for `key` (MIDI). Public for the calibration tooling.
pub fn profile_for_key(key: u8) -> (&'static [f32; 13], &'static [f32; 13]) {
    if key <= 30 {
        (&G1_RATIOS, &G1_AMPS)
    } else if key <= 50 {
        (&G2_RATIOS, &G2_AMPS)
    } else {
        (&G3_RATIOS, &G3_AMPS)
    }
}

/// Fundamental of a MIDI key as PianoForte computes it: equal temperament
/// with the beta = 3.3e-5 stiffness correction on the fundamental itself.
pub fn forte_f0(key: u8) -> f64 {
    const BETA: f64 = 0.000033;
    440.0 * 2f64.powf((key as f64 - 69.0) / 12.0) * (1.0 + BETA).sqrt()
}

const NN_PARTIALS: usize = 30;
const NN_PAD: usize = 32; // padded to the 4-lane kernel granularity
const TAB_PARTIALS: usize = 13;
const TAB_PAD: usize = 16; // run_modes requires a multiple of 8

/// Envelope laws (Voices.cpp constants).
const MAX_PERIODS: f64 = 4511.0;
const ATTACK_PER_PERIOD: f64 = 6.0;
const DECAY_PER_PERIOD: f64 = 0.003;
const TAILOFF_44K: f64 = 0.9997;
const PROFILE_SIGMA: f64 = 0.0003; // decay per radian of partial phase
/// Amp slew toward the network output, per 44.1k-equivalent sample.
const AMP_SLEW_RATE: f32 = 2000.0;
/// Crossfade: alpha = 1 - (1-pitch)*CROSSFADE_SPAN; network share 0.7*alpha.
const CROSSFADE_SPAN: f32 = 0.95;
const NN_SHARE: f32 = 0.7;
/// Random-phase sigma (PHASES_NORM in Voices.h).
const PHASE_SIGMA: f32 = 1.5;

/// Output gain, chosen so the same performances land at the same loudness
/// as the physical model's calibrated MASTER_GAIN (median classical
/// material near -20 dBFS RMS; see tests/learned.rs level check).
const LEARNED_MASTER: f32 = 1.03;

/// Sustain value at/above which the pedal fully holds (same convention as
/// the physical engine's PEDAL_FULL_LIFT).
const PEDAL_FULL_LIFT: f32 = 0.75;

/// A slot whose 64-sample output power stays below this for ~16 ms sleeps.
const SLOT_SILENCE_POWER: f32 = 1e-8;

// ---------------------------------------------------------------------------
// The network: a 3 -> 11 -> 13 -> 18 -> 25 -> 30 MLP with per-layer scaled
// tanh activations, 1707 weights + 4 scalars, stored in the ONNX protobuf
// resources/pianoforte-engineMain.bin (producer "pytorch 1.13.0"):
//
//   h0 = tanh(a * (x  W0 + b0) - s)      x = [pitch, velocity, pc]
//   h1 = tanh(b * (h0 W1 + b1))
//   h2 = tanh(b * (h1 W2 + b2))
//   h3 = tanh(b * (h2 W3 + b3))
//   h4 = tanh(c * (h3 W4 + b4))
//   y  = 0.5 * h4 + 0.5                  30 partial amplitudes in (0,1)
//
// a=3.2815297, b=10.7221365, c=2.6967137, s=1.6407648 (all read from the
// file, not hardcoded). Weight matrices are [in, out] row-major, applied
// as vector-matrix products exactly as ONNX MatMul does.
// ---------------------------------------------------------------------------

const L0: usize = 11;
const L1: usize = 13;
const L2: usize = 18;
const L3: usize = 25;
const L4: usize = 30;

pub struct ForteNet {
    w0: [f32; 3 * L0],
    b0: [f32; L0],
    w1: [f32; L0 * L1],
    b1: [f32; L1],
    w2: [f32; L1 * L2],
    b2: [f32; L2],
    w3: [f32; L2 * L3],
    b3: [f32; L3],
    w4: [f32; L3 * L4],
    b4: [f32; L4],
    act_a: f32,
    act_b: f32,
    act_c: f32,
    act_s: f32,
    out_scale: f32,
    out_off: f32,
}

impl ForteNet {
    /// Evaluate the network. Stack-only; no allocation, no panic for any
    /// finite input.
    pub fn eval(&self, pitch: f32, vel: f32, pc: f32, out: &mut [f32; L4]) {
        let x = [pitch, vel, pc];
        let mut h0 = [0.0f32; L0];
        for j in 0..L0 {
            let mut s = self.b0[j];
            for i in 0..3 {
                s += x[i] * self.w0[i * L0 + j];
            }
            h0[j] = (self.act_a * s - self.act_s).tanh();
        }
        let mut h1 = [0.0f32; L1];
        for j in 0..L1 {
            let mut s = self.b1[j];
            for i in 0..L0 {
                s += h0[i] * self.w1[i * L1 + j];
            }
            h1[j] = (self.act_b * s).tanh();
        }
        let mut h2 = [0.0f32; L2];
        for j in 0..L2 {
            let mut s = self.b2[j];
            for i in 0..L1 {
                s += h1[i] * self.w2[i * L2 + j];
            }
            h2[j] = (self.act_b * s).tanh();
        }
        let mut h3 = [0.0f32; L3];
        for j in 0..L3 {
            let mut s = self.b3[j];
            for i in 0..L2 {
                s += h2[i] * self.w3[i * L3 + j];
            }
            h3[j] = (self.act_b * s).tanh();
        }
        for j in 0..L4 {
            let mut s = self.b4[j];
            for i in 0..L3 {
                s += h3[i] * self.w4[i * L4 + j];
            }
            out[j] = self.out_scale * (self.act_c * s).tanh() + self.out_off;
        }
    }

    /// Parse the embedded ONNX file. Called once at construction; panics on
    /// a malformed resource (the resource is compiled in, so this is a
    /// build defect, not a runtime condition).
    pub fn from_onnx(data: &[u8]) -> ForteNet {
        let mut net = ForteNet {
            w0: [0.0; 3 * L0],
            b0: [0.0; L0],
            w1: [0.0; L0 * L1],
            b1: [0.0; L1],
            w2: [0.0; L1 * L2],
            b2: [0.0; L2],
            w3: [0.0; L2 * L3],
            b3: [0.0; L3],
            w4: [0.0; L3 * L4],
            b4: [0.0; L4],
            act_a: 0.0,
            act_b: 0.0,
            act_c: 0.0,
            act_s: 0.0,
            out_scale: 0.0,
            out_off: 0.0,
        };
        let model = OnnxModel::parse(data)
            .unwrap_or_else(|error| panic!("pianoforte-engineMain: malformed ONNX: {error}"));
        let mut found = 0u32;
        for (name, tensor) in &model.graph.initializers {
            let dst: Option<&mut [f32]> = match name.as_str() {
                "onnx::MatMul_46" => Some(&mut net.w0),
                "L.0.bias" => Some(&mut net.b0),
                "onnx::MatMul_48" => Some(&mut net.w1),
                "L.1.bias" => Some(&mut net.b1),
                "onnx::MatMul_49" => Some(&mut net.w2),
                "L.2.bias" => Some(&mut net.b2),
                "onnx::MatMul_50" => Some(&mut net.w3),
                "L.3.bias" => Some(&mut net.b3),
                "onnx::MatMul_51" => Some(&mut net.w4),
                "L.4.bias" => Some(&mut net.b4),
                "a" => Some(core::slice::from_mut(&mut net.act_a)),
                "b" => Some(core::slice::from_mut(&mut net.act_b)),
                "c" => Some(core::slice::from_mut(&mut net.act_c)),
                "onnx::Sub_47" => Some(core::slice::from_mut(&mut net.act_s)),
                _ => None,
            };
            if let Some(dst) = dst {
                let values = tensor.f32_values().unwrap_or_else(|error| {
                    panic!("pianoforte-engineMain: tensor {name}: {error}")
                });
                assert_eq!(
                    values.len(),
                    dst.len(),
                    "pianoforte-engineMain: tensor {name} data length"
                );
                dst.copy_from_slice(&values);
                found += 1;
            }
        }
        let constants: Vec<f32> = model
            .graph
            .nodes
            .iter()
            .filter(|node| node.op_type == "Constant")
            .filter_map(|node| {
                node.attributes.values().find_map(|attribute| {
                    let OnnxAttribute::Tensor(tensor) = attribute else {
                        return None;
                    };
                    let values = tensor.f32_values().ok()?;
                    (values.len() == 1).then_some(values[0])
                })
            })
            .collect();
        if let [out_scale, out_off] = constants.as_slice() {
            net.out_scale = *out_scale;
            net.out_off = *out_off;
        }
        assert_eq!(found, 14, "pianoforte-engineMain: found {found} of 14 expected tensors");
        assert_eq!(constants.len(), 2, "pianoforte-engineMain: expected the 2 output-stage constants");
        for v in [net.act_a, net.act_b, net.act_c, net.act_s, net.out_scale, net.out_off] {
            assert!(v.is_finite(), "pianoforte-engineMain: non-finite scalar");
        }
        net
    }
}

// ---------------------------------------------------------------------------
// Per-key static design (frequencies, rotations, crossfade, pan) — computed
// once at construction, shared by both slots of the key.
// ---------------------------------------------------------------------------

struct LKey {
    /// Samples per fundamental period at the engine rate.
    inv_period: f64,
    /// Equal-power pan position (player perspective, bass left) — the same
    /// key->pan law as the physical instrument, so switching engines keeps
    /// the image.
    pan: f32,
    /// Network-branch share 0.7*alpha and profile-branch share (1-alpha).
    nn_gain: f32,
    tab_gain: f32,
    /// Unit rotations of the 30 harmonic partials ((i+1)*f0), zeroed above
    /// 0.45*fs. `nn_on` masks the network output for those.
    nn_cr: [f32; NN_PAD],
    nn_ci: [f32; NN_PAD],
    nn_on: [f32; NN_PAD],
    /// Damped rotations of the 13-partial measured profile (constant-Q
    /// decay baked into the radius) for the existing run_modes kernel.
    tab_cr: [f32; TAB_PAD],
    tab_ci: [f32; TAB_PAD],
    tab_gin: [f32; TAB_PAD],
    tab_gout: [f32; TAB_PAD],
    /// Initial amplitude of each profile partial (the G table).
    tab_amp: [f32; TAB_PAD],
}

impl LKey {
    fn build(key: u8, fs: f64) -> LKey {
        let t = (key - FIRST_KEY) as f32 / (NUM_KEYS - 1) as f32;
        let pitch = t;
        let alpha = 1.0 - (1.0 - pitch) * CROSSFADE_SPAN;
        let f0 = forte_f0(key);
        let mut k = LKey {
            inv_period: f0 / fs,
            pan: -0.55 + 1.1 * t,
            nn_gain: NN_SHARE * alpha,
            tab_gain: 1.0 - alpha,
            nn_cr: [0.0; NN_PAD],
            nn_ci: [0.0; NN_PAD],
            nn_on: [0.0; NN_PAD],
            tab_cr: [0.0; TAB_PAD],
            tab_ci: [0.0; TAB_PAD],
            tab_gin: [0.0; TAB_PAD],
            tab_gout: [0.0; TAB_PAD],
            tab_amp: [0.0; TAB_PAD],
        };
        let nyq = 0.45 * fs;
        for i in 0..NN_PARTIALS {
            let f = (i + 1) as f64 * f0;
            if f >= nyq {
                continue;
            }
            let th = core::f64::consts::TAU * f / fs;
            k.nn_cr[i] = th.cos() as f32;
            k.nn_ci[i] = th.sin() as f32;
            k.nn_on[i] = 1.0;
        }
        let (ratios, amps) = profile_for_key(key);
        for i in 0..TAB_PARTIALS {
            let f = ratios[i] as f64 * f0;
            if f >= nyq {
                continue;
            }
            // d = exp(-0.0003 * phase): sigma = 0.0003 * 2 pi f per second.
            let sigma = PROFILE_SIGMA * core::f64::consts::TAU * f;
            let r = (-sigma / fs).exp();
            let th = core::f64::consts::TAU * f / fs;
            k.tab_cr[i] = (r * th.cos()) as f32;
            k.tab_ci[i] = (r * th.sin()) as f32;
            k.tab_gout[i] = 1.0;
            k.tab_amp[i] = amps[i];
        }
        k
    }
}

// ---------------------------------------------------------------------------
// Voice slots. Two per key: a re-strike moves to the sibling slot and lets
// the old note tail off underneath (the JUCE synthesiser steal-with-tailoff
// behaviour of the original, made deterministic).
// ---------------------------------------------------------------------------

struct LSlot {
    active: bool,
    held: bool,
    sost_held: bool,
    /// Sample count since the strike (drives the analytic envelope).
    x: u64,
    /// Velocity level 0..=1 (post soft-pedal scaling).
    level: f32,
    /// Network inputs cached at strike.
    in_pitch: f32,
    in_vel: f32,
    /// Release tail-off state (1.0 until the note is neither held nor
    /// pedalled; then multiplied down every sample).
    tailoff: f32,
    /// Network branch: unit phasors L/R, slewed amps, latest net output.
    nzr_l: [f32; NN_PAD],
    nzi_l: [f32; NN_PAD],
    nzr_r: [f32; NN_PAD],
    nzi_r: [f32; NN_PAD],
    amp: [f32; NN_PAD],
    target: [f32; NN_PAD],
    /// Profile branch: free-ringing damped phasors L/R (amplitude and decay
    /// live in the phasor state itself).
    tzr_l: [f32; TAB_PAD],
    tzi_l: [f32; TAB_PAD],
    tzr_r: [f32; TAB_PAD],
    tzi_r: [f32; TAB_PAD],
    /// 64-sample output power accumulated by the combine loop (sleep logic).
    power: f32,
    quiet_ticks: u32,
}

impl LSlot {
    fn new() -> LSlot {
        LSlot {
            active: false,
            held: false,
            sost_held: false,
            x: 0,
            level: 0.0,
            in_pitch: 0.0,
            in_vel: 0.0,
            tailoff: 1.0,
            nzr_l: [0.0; NN_PAD],
            nzi_l: [0.0; NN_PAD],
            nzr_r: [0.0; NN_PAD],
            nzi_r: [0.0; NN_PAD],
            amp: [0.0; NN_PAD],
            target: [0.0; NN_PAD],
            tzr_l: [0.0; TAB_PAD],
            tzi_l: [0.0; TAB_PAD],
            tzr_r: [0.0; TAB_PAD],
            tzi_r: [0.0; TAB_PAD],
            power: 0.0,
            quiet_ticks: 0,
        }
    }

    fn silence(&mut self) {
        self.active = false;
        self.held = false;
        self.sost_held = false;
        self.tailoff = 1.0;
        self.x = 0;
        self.power = 0.0;
        self.quiet_ticks = 0;
        self.nzr_l.fill(0.0);
        self.nzi_l.fill(0.0);
        self.nzr_r.fill(0.0);
        self.nzi_r.fill(0.0);
        self.amp.fill(0.0);
        self.target.fill(0.0);
        self.tzr_l.fill(0.0);
        self.tzi_l.fill(0.0);
        self.tzr_r.fill(0.0);
        self.tzi_r.fill(0.0);
    }
}

/// Deterministic N(0, 1) via Box-Muller over xorshift32 (the original uses
/// std::normal_distribution over a shared global engine; per-strike seeded
/// determinism is this crate's convention).
struct Gauss {
    rng: u32,
}

impl Gauss {
    fn new(seed: u32) -> Gauss {
        Gauss { rng: seed | 1 }
    }
    fn uniform(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        // (0, 1]: never 0, so ln() below is finite.
        ((x >> 8) as f32 + 1.0) * (1.0 / 16_777_216.0)
    }
    fn next(&mut self) -> f32 {
        let u1 = self.uniform();
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (core::f32::consts::TAU * u2).cos()
    }
}

// ---------------------------------------------------------------------------
// The oscillator kernel of the network branch: 4-lane rotation of the L and
// R phasor pairs with the per-partial amp slewing toward `target` inside
// the sample loop (the amp state is shared by both channels, which is why
// this is one fused kernel and not two run_modes calls). Scalar twin below;
// tests/learned.rs holds them together. AVX2 hosts run the 4-lane path too
// (this bank is 32 lanes total — not worth a third kernel).
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_nn_osc(
    path: KernelPath,
    zr_l: &mut [f32; NN_PAD],
    zi_l: &mut [f32; NN_PAD],
    zr_r: &mut [f32; NN_PAD],
    zi_r: &mut [f32; NN_PAD],
    amp: &mut [f32; NN_PAD],
    target: &[f32; NN_PAD],
    cr: &[f32; NN_PAD],
    ci: &[f32; NN_PAD],
    slew: f32,
    n: usize,
    acc_l: &mut [f32; MAX_CHUNK],
    acc_r: &mut [f32; MAX_CHUNK],
) {
    debug_assert!(n <= MAX_CHUNK);
    if path == KernelPath::Scalar {
        for m in 0..NN_PAD {
            let (crm, cim, tm) = (cr[m], ci[m], target[m]);
            let (mut rl, mut il) = (zr_l[m], zi_l[m]);
            let (mut rr, mut ir) = (zr_r[m], zi_r[m]);
            let mut a = amp[m];
            for k in 0..n {
                a += slew * (tm - a);
                let t0 = crm * rl - cim * il;
                il = cim * rl + crm * il;
                rl = t0;
                let t1 = crm * rr - cim * ir;
                ir = cim * rr + crm * ir;
                rr = t1;
                acc_l[k] += a * il;
                acc_r[k] += a * ir;
            }
            zr_l[m] = rl;
            zi_l[m] = il;
            zr_r[m] = rr;
            zi_r[m] = ir;
            amp[m] = a;
        }
        return;
    }
    let mut vacc_l = [zero_v4(); MAX_CHUNK];
    let mut vacc_r = [zero_v4(); MAX_CHUNK];
    let slew_v = splat_v4(slew);
    let mut m = 0;
    while m < NN_PAD {
        let mut rl = load_v4(&zr_l[m..]);
        let mut il = load_v4(&zi_l[m..]);
        let mut rr = load_v4(&zr_r[m..]);
        let mut ir = load_v4(&zi_r[m..]);
        let mut a = load_v4(&amp[m..]);
        let crv = load_v4(&cr[m..]);
        let civ = load_v4(&ci[m..]);
        let tv = load_v4(&target[m..]);
        for k in 0..n {
            a = fma_v4(slew_v, sub_v4(tv, a), a);
            let t0 = sub_v4(mul_v4(crv, rl), mul_v4(civ, il));
            il = fma_v4(civ, rl, mul_v4(crv, il));
            rl = t0;
            let t1 = sub_v4(mul_v4(crv, rr), mul_v4(civ, ir));
            ir = fma_v4(civ, rr, mul_v4(crv, ir));
            rr = t1;
            vacc_l[k] = fma_v4(a, il, vacc_l[k]);
            vacc_r[k] = fma_v4(a, ir, vacc_r[k]);
        }
        store_v4(&mut zr_l[m..], rl);
        store_v4(&mut zi_l[m..], il);
        store_v4(&mut zr_r[m..], rr);
        store_v4(&mut zi_r[m..], ir);
        store_v4(&mut amp[m..], a);
        m += 4;
    }
    for k in 0..n {
        acc_l[k] += hsum_v4(vacc_l[k]);
        acc_r[k] += hsum_v4(vacc_r[k]);
    }
}

// ---------------------------------------------------------------------------
// The instrument
// ---------------------------------------------------------------------------

pub struct LearnedPiano {
    sample_rate: f32,
    net: ForteNet,
    keys: Vec<LKey>,
    slots: Vec<LSlot>, // 2 per key: key*2 + (strike & 1)
    strike_parity: Vec<u32>,
    // Envelope coefficients at this rate.
    amp_slew: f32,
    tailoff_base: f64,
    // Pedals.
    sustain: f32,
    soft: bool,
    // Output stage (same chain as the physical engine, minus the board:
    // the network was trained on radiated recordings, so its output is
    // already in the recording domain).
    er: EarlyReflections,
    reverb: Reverb,
    tone: Tone,
    eq: Eq,
    dc_l: DcBlock,
    dc_r: DcBlock,
    perspective: Perspective,
    pan_sign: f32,
    dry: f32,
    wet: f32,
    er_level: f32,
    soft_clip_on: bool,
    master: f32,
    master_user: f32,
    tone_bass_db: f32,
    tone_treble_db: f32,
    /// Accepted for engine-swap compatibility; every field is inert here
    /// (the mechanisms voicing scales do not exist in this synthesis).
    voicing: Voicing,
    global_sample: u64,
    path: KernelPath,
    // Chunk scratch.
    zero_in: [f32; MAX_CHUNK],
    nn_l: [f32; MAX_CHUNK],
    nn_r: [f32; MAX_CHUNK],
    tab_l: [f32; MAX_CHUNK],
    tab_r: [f32; MAX_CHUNK],
    bus_l: [f32; MAX_CHUNK],
    bus_r: [f32; MAX_CHUNK],
}

impl LearnedPiano {
    /// Builds the learned instrument: parses the embedded PianoForte
    /// network and precomputes all 88 key designs. Allocation happens only
    /// here.
    pub fn new(sample_rate: f32) -> LearnedPiano {
        assert!((8000.0..=192_000.0).contains(&sample_rate), "unsupported sample rate {sample_rate}");
        let fs = sample_rate as f64;
        let net = ForteNet::from_onnx(include_bytes!("../resources/pianoforte-engineMain.bin"));
        let keys: Vec<LKey> = (FIRST_KEY..=LAST_KEY).map(|k| LKey::build(k, fs)).collect();
        let slots: Vec<LSlot> = (0..NUM_KEYS * 2).map(|_| LSlot::new()).collect();
        LearnedPiano {
            sample_rate,
            net,
            keys,
            slots,
            strike_parity: vec![0; NUM_KEYS],
            amp_slew: (AMP_SLEW_RATE / sample_rate).min(1.0),
            tailoff_base: TAILOFF_44K.powf(44100.0 / fs),
            sustain: 0.0,
            soft: false,
            er: EarlyReflections::new(sample_rate),
            reverb: Reverb::new(sample_rate),
            tone: Tone::new(sample_rate),
            eq: Eq::new(fs),
            dc_l: DcBlock::new(sample_rate),
            dc_r: DcBlock::new(sample_rate),
            perspective: Perspective::Player,
            pan_sign: 1.0,
            dry: 1.0,
            wet: 0.3,
            er_level: 0.7,
            soft_clip_on: true,
            master: LEARNED_MASTER,
            master_user: 1.0,
            tone_bass_db: 0.0,
            tone_treble_db: 0.0,
            voicing: Voicing::default(),
            global_sample: 0,
            path: detect_path(),
            zero_in: [0.0; MAX_CHUNK],
            nn_l: [0.0; MAX_CHUNK],
            nn_r: [0.0; MAX_CHUNK],
            tab_l: [0.0; MAX_CHUNK],
            tab_r: [0.0; MAX_CHUNK],
            bus_l: [0.0; MAX_CHUNK],
            bus_r: [0.0; MAX_CHUNK],
        }
    }

    // --- control surface (mirrors Piano's; safe between process calls) ---

    pub fn set_force_scalar(&mut self, scalar: bool) {
        self.path = if scalar { KernelPath::Scalar } else { detect_path() };
    }

    pub fn kernel_path(&self) -> KernelPath {
        self.path
    }

    pub fn set_reverb_preset(&mut self, preset: ReverbPreset) {
        self.reverb.set_preset(preset);
    }

    pub fn set_reverb_params(&mut self, params: ReverbParams) {
        self.reverb.set_params(params);
    }

    pub fn reverb_params(&self) -> ReverbParams {
        self.reverb.params()
    }

    pub fn set_reverb_mix(&mut self, wet: f32) {
        self.wet = if wet.is_finite() { wet.clamp(0.0, 1.5) } else { 0.0 };
    }

    pub fn reverb_mix(&self) -> f32 {
        self.wet
    }

    pub fn set_early_reflection_level(&mut self, level: f32) {
        self.er_level = if level.is_finite() { level.clamp(0.0, 1.5) } else { 0.0 };
    }

    pub fn early_reflection_level(&self) -> f32 {
        self.er_level
    }

    pub fn set_perspective(&mut self, p: Perspective) {
        self.perspective = p;
        self.pan_sign = match p {
            Perspective::Player => 1.0,
            Perspective::Audience => -1.0,
        };
        let sr = self.sample_rate;
        self.er.set_perspective(p, sr);
    }

    pub fn perspective(&self) -> Perspective {
        self.perspective
    }

    /// Accepted so an app can swap engines without special-casing; the
    /// learned synthesis has none of the mechanisms these amounts scale,
    /// so the values are stored and nothing else (see module docs).
    pub fn set_voicing(&mut self, v: Voicing) {
        self.voicing = v.clamped();
    }

    pub fn voicing(&self) -> Voicing {
        self.voicing
    }

    pub fn set_eq_shelf(&mut self, gain_db: f32, corner_hz: f32) {
        self.eq.set_shelf(gain_db, corner_hz);
    }

    pub fn eq_shelf(&self) -> (f32, f32) {
        self.eq.shelf()
    }

    pub fn set_eq_bell(&mut self, freq_hz: f32, gain_db: f32, q: f32) {
        self.eq.set_bell(freq_hz, gain_db, q);
    }

    pub fn eq_bell(&self) -> (f32, f32, f32) {
        self.eq.bell()
    }

    pub fn set_tone(&mut self, bass_db: f32, treble_db: f32) {
        self.tone_bass_db = bass_db.clamp(-12.0, 12.0);
        self.tone_treble_db = treble_db.clamp(-12.0, 12.0);
        self.tone.set(self.tone_bass_db, self.tone_treble_db);
    }

    pub fn tone(&self) -> (f32, f32) {
        (self.tone_bass_db, self.tone_treble_db)
    }

    pub fn set_soft_clip(&mut self, on: bool) {
        self.soft_clip_on = on;
    }

    pub fn soft_clip(&self) -> bool {
        self.soft_clip_on
    }

    pub fn set_master_gain(&mut self, gain: f32) {
        let g = if gain.is_finite() { gain.clamp(0.0, 10.0) } else { 1.0 };
        self.master_user = g;
        self.master = g * LEARNED_MASTER;
    }

    pub fn master_gain(&self) -> f32 {
        self.master_user
    }

    /// The room/voicing part of a physical preset, applied to this engine
    /// (voicing is stored, room and mix are live — same contract as
    /// `Piano::apply_preset_live`; the design part has no meaning here).
    pub fn apply_preset_live(&mut self, preset: &PianoPreset) {
        self.set_voicing(preset.voicing);
        self.set_reverb_preset(preset.room);
        self.set_reverb_mix(preset.reverb_mix);
    }

    // --- analysis surface (for the physical model's calibration) ---------

    /// The learned per-partial amplitude ladder of `key` at `velocity`,
    /// `t_seconds` after the strike: the network output (30 harmonic
    /// partials, linear amplitude in (0,1)), with the time coordinate
    /// mapped the way the render path maps it (pc = tanh(periods/4511)).
    /// Analysis/calibration surface — the render path does not use it.
    pub fn learned_partial_amps(&self, key: u8, velocity: u8, t_seconds: f64, out: &mut [f32; 30]) {
        let key = key.clamp(FIRST_KEY, LAST_KEY);
        let pitch = (key - FIRST_KEY) as f32 / (NUM_KEYS - 1) as f32;
        let vel = velocity.min(127) as f32 / 127.0;
        let periods = t_seconds.max(0.0) * forte_f0(key);
        let pc = (periods / MAX_PERIODS).tanh() as f32;
        self.net.eval(pitch, vel, pc, out);
    }

    /// The analytic common envelope at `t_seconds` for `key` (attack ramp,
    /// long-decay Lorentzian; excludes velocity and tail-off) — what the
    /// render path multiplies both branches by.
    pub fn learned_envelope(&self, key: u8, t_seconds: f64) -> f64 {
        let key = key.clamp(FIRST_KEY, LAST_KEY);
        let p = t_seconds.max(0.0) * forte_f0(key);
        let attack = ATTACK_PER_PERIOD * p;
        let d = DECAY_PER_PERIOD * p;
        let decay = 1.0 / (1.0 + d * d);
        attack.min(decay)
    }

    // --- events / render --------------------------------------------------

    fn apply_event(&mut self, ev: &PianoEvent) {
        match *ev {
            PianoEvent::NoteOn { key, velocity } => {
                if velocity == 0 {
                    return self.apply_event(&PianoEvent::NoteOff { key });
                }
                if !(FIRST_KEY..=LAST_KEY).contains(&key) {
                    return;
                }
                self.note_on(key, velocity);
            }
            PianoEvent::NoteOff { key } => {
                if !(FIRST_KEY..=LAST_KEY).contains(&key) {
                    return;
                }
                let i = (key - FIRST_KEY) as usize;
                self.slots[i * 2].held = false;
                self.slots[i * 2 + 1].held = false;
            }
            PianoEvent::Sustain { value } => {
                self.sustain = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
            }
            PianoEvent::Sostenuto { on } => {
                for s in &mut self.slots {
                    s.sost_held = on && s.held;
                }
            }
            PianoEvent::SoftPedal { on } => {
                self.soft = on;
            }
            PianoEvent::AllSoundOff => {
                for s in &mut self.slots {
                    s.silence();
                }
                self.er.reset();
                self.reverb.reset();
            }
        }
    }

    fn note_on(&mut self, key: u8, velocity: u8) {
        let i = (key - FIRST_KEY) as usize;
        let strike = self.strike_parity[i].wrapping_add(1);
        self.strike_parity[i] = strike;
        // The sibling slot keeps ringing (full if pedalled, tail-off if
        // not), exactly like the original's voice steal.
        let other = i * 2 + ((strike as usize + 1) & 1);
        self.slots[other].held = false;
        let seed = (i as u32).wrapping_mul(0x9e37_79b9) ^ strike.wrapping_mul(0x85eb_ca6b) ^ 0x5f0f_ee01;
        let mut g = Gauss::new(seed);
        let vel = velocity.min(127) as f32 / 127.0;
        let (in_vel, level) = if self.soft {
            // Soft pedal: quieter, and slightly darker through the learned
            // velocity axis (velocity is a spectral input to the network).
            (vel * 0.85, vel * 0.75)
        } else {
            (vel, vel)
        };
        let pitch = i as f32 / (NUM_KEYS - 1) as f32;
        let slot = &mut self.slots[i * 2 + (strike as usize & 1)];
        slot.silence();
        slot.active = true;
        slot.held = true;
        slot.x = 0;
        slot.level = level;
        slot.in_pitch = pitch;
        slot.in_vel = in_vel;
        slot.tailoff = 1.0;
        // Network branch phases: independent N(0, 1.5) per partial/channel.
        let kd = &self.keys[i];
        for m in 0..NN_PARTIALS {
            if kd.nn_on[m] == 0.0 {
                continue;
            }
            let pl = PHASE_SIGMA * g.next();
            let pr = PHASE_SIGMA * g.next();
            slot.nzr_l[m] = pl.cos();
            slot.nzi_l[m] = pl.sin();
            slot.nzr_r[m] = pr.cos();
            slot.nzi_r[m] = pr.sin();
        }
        // Profile branch phases: cumulative Gaussian walks, amplitude baked
        // into the phasor magnitude (free-ringing bank).
        let mut wl = 0.0f32;
        let mut wr = 0.0f32;
        for m in 0..TAB_PARTIALS {
            wl += PHASE_SIGMA * g.next();
            wr += PHASE_SIGMA * g.next();
            let a = kd.tab_amp[m];
            if a == 0.0 {
                continue;
            }
            slot.tzr_l[m] = a * wl.cos();
            slot.tzi_l[m] = a * wl.sin();
            slot.tzr_r[m] = a * wr.cos();
            slot.tzi_r[m] = a * wr.sin();
        }
        // First network evaluation at pc = 0, adopted immediately (the
        // original copies targetAmps into currentAmps at startNote).
        let mut amps = [0.0f32; L4];
        self.net.eval(pitch, in_vel, 0.0, &mut amps);
        for m in 0..NN_PARTIALS {
            let a = amps[m] * kd.nn_on[m];
            slot.amp[m] = a;
            slot.target[m] = a;
        }
    }

    /// Runs on the absolute 64-sample grid: network envelope updates, unit
    /// phasor renormalisation, sleep decisions.
    fn control_tick(&mut self) {
        let pedal_held = self.sustain >= PEDAL_FULL_LIFT;
        for i in 0..NUM_KEYS {
            for p in 0..2 {
                let slot = &mut self.slots[i * 2 + p];
                if !slot.active {
                    continue;
                }
                let periods = slot.x as f64 * self.keys[i].inv_period;
                // Long-decay envelope exhausted (the original's
                // currentDecay <= 0.001 cutoff).
                let d = DECAY_PER_PERIOD * periods;
                if 1.0 / (1.0 + d * d) <= 0.001 {
                    slot.silence();
                    continue;
                }
                if !(slot.held || slot.sost_held || pedal_held) && slot.tailoff < 0.01 {
                    slot.silence();
                    continue;
                }
                if slot.power < SLOT_SILENCE_POWER {
                    slot.quiet_ticks += 1;
                    if slot.quiet_ticks > 12 {
                        slot.silence();
                        continue;
                    }
                } else {
                    slot.quiet_ticks = 0;
                }
                slot.power = 0.0;
                // Envelope update: the render path's equivalent of the
                // original's async re-evaluation, on the deterministic grid.
                let pc = (periods / MAX_PERIODS).tanh() as f32;
                let mut amps = [0.0f32; L4];
                self.net.eval(slot.in_pitch, slot.in_vel, pc, &mut amps);
                let kd = &self.keys[i];
                for m in 0..NN_PARTIALS {
                    slot.target[m] = amps[m] * kd.nn_on[m];
                }
                // One Newton step pulls each unit phasor back to |z| = 1
                // (f32 rotation drift is ~1e-7 per sample; unchecked it
                // compounds over minutes).
                for m in 0..NN_PARTIALS {
                    let (r, im) = (slot.nzr_l[m], slot.nzi_l[m]);
                    let s = 0.5 * (3.0 - (r * r + im * im));
                    slot.nzr_l[m] = r * s;
                    slot.nzi_l[m] = im * s;
                    let (r, im) = (slot.nzr_r[m], slot.nzi_r[m]);
                    let s = 0.5 * (3.0 - (r * r + im * im));
                    slot.nzr_r[m] = r * s;
                    slot.nzi_r[m] = im * s;
                }
            }
        }
    }

    fn render_chunk(&mut self, n: usize, out_l: &mut [f32], out_r: &mut [f32]) {
        for k in 0..n {
            self.bus_l[k] = 0.0;
            self.bus_r[k] = 0.0;
        }
        let pedal_held = self.sustain >= PEDAL_FULL_LIFT;
        // Half-pedal: the tail-off ratio approaches 1 (no decay) as the
        // pedal approaches the full-lift point.
        let part = (self.sustain / PEDAL_FULL_LIFT).clamp(0.0, 1.0) as f64;
        let tail_ratio = self.tailoff_base.powf(1.0 - part) as f32;
        for i in 0..NUM_KEYS {
            let kd = &self.keys[i];
            let pan = kd.pan * self.pan_sign;
            let ang = (pan + 1.0) * core::f32::consts::FRAC_PI_4;
            let (pl, pr) = (ang.cos(), ang.sin());
            for p in 0..2 {
                let slot = &mut self.slots[i * 2 + p];
                if !slot.active {
                    continue;
                }
                for k in 0..n {
                    self.nn_l[k] = 0.0;
                    self.nn_r[k] = 0.0;
                    self.tab_l[k] = 0.0;
                    self.tab_r[k] = 0.0;
                }
                run_nn_osc(
                    self.path,
                    &mut slot.nzr_l,
                    &mut slot.nzi_l,
                    &mut slot.nzr_r,
                    &mut slot.nzi_r,
                    &mut slot.amp,
                    &slot.target,
                    &kd.nn_cr,
                    &kd.nn_ci,
                    self.amp_slew,
                    n,
                    &mut self.nn_l,
                    &mut self.nn_r,
                );
                if kd.tab_gain > 0.0 {
                    run_modes(
                        self.path,
                        &mut slot.tzr_l,
                        &mut slot.tzi_l,
                        &kd.tab_cr,
                        &kd.tab_ci,
                        &kd.tab_gin,
                        &kd.tab_gout,
                        &self.zero_in[..n],
                        0.0,
                        &mut self.tab_l[..n],
                    );
                    run_modes(
                        self.path,
                        &mut slot.tzr_r,
                        &mut slot.tzi_r,
                        &kd.tab_cr,
                        &kd.tab_ci,
                        &kd.tab_gin,
                        &kd.tab_gout,
                        &self.zero_in[..n],
                        0.0,
                        &mut self.tab_r[..n],
                    );
                }
                let ringing = slot.held || slot.sost_held || pedal_held;
                let mut tail = slot.tailoff;
                let mut pow = slot.power;
                let inv_period = kd.inv_period;
                let x0 = slot.x;
                for k in 0..n {
                    let periods = (x0 + k as u64) as f64 * inv_period;
                    let attack = ATTACK_PER_PERIOD * periods;
                    let d = DECAY_PER_PERIOD * periods;
                    let decay = 1.0 / (1.0 + d * d);
                    let m = attack.min(decay) as f32 * slot.level;
                    if !ringing {
                        tail *= tail_ratio;
                    }
                    let g = m * tail;
                    let l = g * (kd.nn_gain * self.nn_l[k] + kd.tab_gain * self.tab_l[k]);
                    let r = g * (kd.nn_gain * self.nn_r[k] + kd.tab_gain * self.tab_r[k]);
                    self.bus_l[k] += pl * l;
                    self.bus_r[k] += pr * r;
                    pow += l * l + r * r;
                }
                slot.tailoff = tail;
                slot.power = pow;
                slot.x = x0 + n as u64;
            }
        }
        for k in 0..n {
            let pl = self.master * self.bus_l[k];
            let pr = self.master * self.bus_r[k];
            let (pl, pr) = self.eq.process(pl, pr);
            let (el, er) = self.er.process(pl, pr);
            let (wl, wr) = self.reverb.process(pl, pr);
            let l = self.dry * pl + self.er_level * el + self.wet * wl;
            let r = self.dry * pr + self.er_level * er + self.wet * wr;
            let (tl, tr) = self.tone.process(l, r);
            let mut l = self.dc_l.process(tl);
            let mut r = self.dc_r.process(tr);
            if self.soft_clip_on {
                l = soft_clip(l);
                r = soft_clip(r);
            }
            out_l[k] = l;
            out_r[k] = r;
        }
    }

    /// See `Instrument::process`; the real-time entry point. Chunking and
    /// the 64-sample control grid are identical to the physical engine's.
    pub fn process(&mut self, events: &[TimedEvent], out_l: &mut [f32], out_r: &mut [f32]) {
        let len = out_l.len().min(out_r.len());
        debug_assert!(events.windows(2).all(|w| w[0].offset <= w[1].offset), "events must be sorted by offset");
        debug_assert!(events.iter().all(|e| (e.offset as usize) < len.max(1)), "event offsets must lie inside the block");
        let mut pos = 0usize;
        let mut ev = 0usize;
        while pos < len {
            if self.global_sample % MAX_CHUNK as u64 == 0 {
                self.control_tick();
            }
            while ev < events.len() && (events[ev].offset as usize) <= pos {
                let e = events[ev].event;
                self.apply_event(&e);
                ev += 1;
            }
            let next_ev = events.get(ev).map(|e| (e.offset as usize).min(len)).unwrap_or(len);
            let room = MAX_CHUNK - (self.global_sample % MAX_CHUNK as u64) as usize;
            let n = (len - pos).min(next_ev - pos).min(room);
            // Split borrows: render_chunk writes out through &mut self.
            let (l, r) = (&mut out_l[pos..pos + n], &mut out_r[pos..pos + n]);
            self.render_chunk(n, l, r);
            pos += n;
            self.global_sample += n as u64;
        }
    }

    /// Full state reset (voices, pedals, effects, clock).
    pub fn reset(&mut self) {
        for s in &mut self.slots {
            s.silence();
        }
        self.strike_parity.fill(0);
        self.sustain = 0.0;
        self.soft = false;
        self.er.reset();
        self.reverb.reset();
        self.tone.reset();
        self.eq.reset();
        self.dc_l.reset();
        self.dc_r.reset();
        self.global_sample = 0;
    }
}

impl Instrument for LearnedPiano {
    fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    fn process(&mut self, events: &[TimedEvent], out_l: &mut [f32], out_r: &mut [f32]) {
        LearnedPiano::process(self, events, out_l, out_r)
    }

    fn reset(&mut self) {
        LearnedPiano::reset(self)
    }
}

// ---------------------------------------------------------------------------
// Engine selection: one value an app can hold that is either instrument,
// with the full shared control surface forwarded. `PIANO_PRESETS` keeps its
// names and meanings; the learned instrument is a second ENGINE, not a
// preset — select it with `PianoEngine::new`, restyle it live with any
// preset's room/voicing via `apply_preset_live`.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    /// The physically modelled instrument (`Piano`).
    Physical,
    /// The learned additive instrument (`LearnedPiano`, from PianoForte).
    Learned,
}

impl EngineKind {
    pub const ALL: [EngineKind; 2] = [EngineKind::Physical, EngineKind::Learned];

    pub fn name(self) -> &'static str {
        match self {
            EngineKind::Physical => "Physical Model",
            EngineKind::Learned => "Learned (PianoForte)",
        }
    }
}

pub enum PianoEngine {
    Physical(Box<Piano>),
    Learned(Box<LearnedPiano>),
}

macro_rules! fwd {
    ($self:ident, $p:ident => $e:expr) => {
        match $self {
            PianoEngine::Physical($p) => $e,
            PianoEngine::Learned($p) => $e,
        }
    };
}

impl PianoEngine {
    /// Builds the chosen engine, styled by `preset` (for the physical
    /// engine this is `Piano::new_with_preset`; the learned engine takes
    /// the preset's room/mix/voicing and ignores its design overrides,
    /// which describe physics it does not run).
    pub fn new(kind: EngineKind, sample_rate: f32, preset: &PianoPreset) -> PianoEngine {
        match kind {
            EngineKind::Physical => PianoEngine::Physical(Box::new(Piano::new_with_preset(sample_rate, preset))),
            EngineKind::Learned => {
                let mut p = LearnedPiano::new(sample_rate);
                p.apply_preset_live(preset);
                PianoEngine::Learned(Box::new(p))
            }
        }
    }

    pub fn kind(&self) -> EngineKind {
        match self {
            PianoEngine::Physical(_) => EngineKind::Physical,
            PianoEngine::Learned(_) => EngineKind::Learned,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        fwd!(self, p => p.sample_rate())
    }

    pub fn process(&mut self, events: &[TimedEvent], out_l: &mut [f32], out_r: &mut [f32]) {
        fwd!(self, p => p.process(events, out_l, out_r))
    }

    pub fn reset(&mut self) {
        fwd!(self, p => p.reset())
    }

    pub fn apply_preset_live(&mut self, preset: &PianoPreset) {
        fwd!(self, p => p.apply_preset_live(preset))
    }

    pub fn set_voicing(&mut self, v: Voicing) {
        fwd!(self, p => p.set_voicing(v))
    }

    pub fn voicing(&self) -> Voicing {
        fwd!(self, p => p.voicing())
    }

    pub fn set_reverb_preset(&mut self, preset: ReverbPreset) {
        fwd!(self, p => p.set_reverb_preset(preset))
    }

    pub fn set_reverb_params(&mut self, params: ReverbParams) {
        fwd!(self, p => p.set_reverb_params(params))
    }

    pub fn reverb_params(&self) -> ReverbParams {
        fwd!(self, p => p.reverb_params())
    }

    pub fn set_reverb_mix(&mut self, wet: f32) {
        fwd!(self, p => p.set_reverb_mix(wet))
    }

    pub fn reverb_mix(&self) -> f32 {
        fwd!(self, p => p.reverb_mix())
    }

    pub fn set_early_reflection_level(&mut self, level: f32) {
        fwd!(self, p => p.set_early_reflection_level(level))
    }

    pub fn early_reflection_level(&self) -> f32 {
        fwd!(self, p => p.early_reflection_level())
    }

    pub fn set_perspective(&mut self, persp: Perspective) {
        fwd!(self, p => p.set_perspective(persp))
    }

    pub fn perspective(&self) -> Perspective {
        fwd!(self, p => p.perspective())
    }

    pub fn set_eq_shelf(&mut self, gain_db: f32, corner_hz: f32) {
        fwd!(self, p => p.set_eq_shelf(gain_db, corner_hz))
    }

    pub fn eq_shelf(&self) -> (f32, f32) {
        fwd!(self, p => p.eq_shelf())
    }

    pub fn set_eq_bell(&mut self, freq_hz: f32, gain_db: f32, q: f32) {
        fwd!(self, p => p.set_eq_bell(freq_hz, gain_db, q))
    }

    pub fn eq_bell(&self) -> (f32, f32, f32) {
        fwd!(self, p => p.eq_bell())
    }

    pub fn set_tone(&mut self, bass_db: f32, treble_db: f32) {
        fwd!(self, p => p.set_tone(bass_db, treble_db))
    }

    pub fn tone(&self) -> (f32, f32) {
        fwd!(self, p => p.tone())
    }

    pub fn set_master_gain(&mut self, gain: f32) {
        fwd!(self, p => p.set_master_gain(gain))
    }

    pub fn master_gain(&self) -> f32 {
        fwd!(self, p => p.master_gain())
    }

    pub fn set_soft_clip(&mut self, on: bool) {
        fwd!(self, p => p.set_soft_clip(on))
    }

    pub fn soft_clip(&self) -> bool {
        fwd!(self, p => p.soft_clip())
    }
}

impl Instrument for PianoEngine {
    fn sample_rate(&self) -> f32 {
        PianoEngine::sample_rate(self)
    }

    fn process(&mut self, events: &[TimedEvent], out_l: &mut [f32], out_r: &mut [f32]) {
        PianoEngine::process(self, events, out_l, out_r)
    }

    fn reset(&mut self) {
        PianoEngine::reset(self)
    }
}
