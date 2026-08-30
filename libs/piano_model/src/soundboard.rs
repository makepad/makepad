// Shared modal soundboard: turns the summed bridge force of all strings into
// radiated sound. One bank of ~72 damped resonators shared by every voice
// (this is what makes the body affordable: its cost is independent of
// polyphony), quasi-log-spaced 60 Hz up to the audio band edge with jitter
// like the dense, irregular mode lattice of a ribbed spruce plate, plus a
// direct radiation path for the high treble where the plate response is
// diffuse. The stereo taps read the two quadratures of each mode (Im left,
// Re right) with independent random sign/level patterns, which is a cheap
// physical stand-in for observing a large radiating plate from two points.
// The attack "knock" of a piano note is these modes being rung by the onset
// of the bridge force; nothing extra is needed.

use crate::modal::{pad8, run_modes_stereo, KernelPath, MAX_CHUNK};
use crate::params::DesignParams;

pub const BOARD_MODES: usize = 84;
/// Hard cap on the parameterised mode count (preallocation bound).
pub const BOARD_MODES_MAX: usize = 256;

/// Radiativity of the instrument body: how strongly a bridge-force partial
/// at `f` Hz reaches the listener, relative to the mid plateau.
///
/// Shape (normalised, dimensionless, ~1.0 across the plateau):
/// - falls below ~100 Hz (the board is smaller than the wavelength; the
///   lowest octave of a real piano is heard through its upper partials,
///   which is why A0's fundamental is nearly absent from real recordings)
/// - broad flat plateau ~150 Hz .. ~2 kHz (dense modal overlap; the
///   driving-point mobility of a ribbed board is roughly flat here)
/// - gentle roll-off (-6 dB/oct) above ~2.4 kHz (hammer-crown size, bridge
///   mass and string-to-board coupling all fall; measured radiated piano
///   spectra drop steadily above the low-kHz region)
///
/// This replaces an earlier +6 dB/oct "velocity coupling" tilt that was
/// flat only above 1.8 kHz. That tilt buried the fundamental of every note
/// below the tilt corner (C4's partial 1 sat 14 dB under its partial 7 at
/// equal modal amplitude) and held the top of the spectrum up — a bright,
/// thin balance that reads as a plucked wire, not a struck piano string.
pub fn radiativity(f: f64, p: &DesignParams) -> f64 {
    let hp1 = f / (f + p.rad_hp1);
    let hp2 = f / (f + p.rad_hp2);
    let x = (f / p.rad_lp) * (f / p.rad_lp);
    let lp = (1.0 / (1.0 + x)).powf(0.5 * p.rad_lp_pow);
    // Low-mid body emphasis: the board's main resonances sit in the
    // ~100-400 Hz region and radiate the fundamentals of the middle octaves
    // strongly — the reference recordings have the FUNDAMENTAL as the
    // strongest partial through C3..C4 even though the strike comb feeds
    // partial 2 five dB more force. A plateau that is flat down to the
    // bass high-pass cannot reproduce that and reads as thin ("tinny").
    // Fourth-order shelf keeps the emphasis out of the 500 Hz+ region, and
    // a second-order high-pass at 120 Hz keeps it out of the deep bass:
    // below the main resonance the board stops radiating again, and a
    // C2 whose fundamental outweighs its partial cluster reads as boom,
    // not body (the bass-speaks-through-partials law).
    let b = f / p.rad_body_hz;
    let b4 = b * b * b * b;
    let body = 1.0 + p.rad_body * (1.0 / (1.0 + b4)) * (f * f / (f * f + 120.0 * 120.0));
    hp1 * hp2 * lp * body
}

pub struct Soundboard {
    zr: Vec<f32>,
    zi: Vec<f32>,
    cr: Vec<f32>,
    ci: Vec<f32>,
    gin: Vec<f32>,
    gout_l: Vec<f32>,
    gout_r: Vec<f32>,
    /// Direct radiation blended with the modal response.
    pub direct: f32,
    /// Diagnostic scale on the direct path (1.0 in normal use).
    pub dbg_direct: f32,
    /// Differentiator + two lowpass poles for the direct path: velocity
    /// coupling that flattens at the bottom of the radiativity plateau and
    /// rolls off like R(f) at the top.
    dx1: f32,
    dlp: f32,
    dlp2: f32,
    dc_lp: f32,
    dc_lp2: f32,
}

fn hash01(mut x: u32) -> f32 {
    // deterministic per-mode jitter
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 * (1.0 / 16_777_216.0)
}

impl Soundboard {
    pub fn new(sample_rate: f64, p: &DesignParams) -> Self {
        let modes = (p.board_modes as usize).clamp(16, BOARD_MODES_MAX);
        let n = pad8(modes);
        let zr = vec![0.0f32; n];
        let zi = vec![0.0f32; n];
        let mut cr = vec![0.0f32; n];
        let mut ci = vec![0.0f32; n];
        let mut gin = vec![0.0f32; n];
        let mut gout_l = vec![0.0f32; n];
        let mut gout_r = vec![0.0f32; n];
        let dt = 1.0 / sample_rate;
        let norm = 1.0 / (modes as f64).sqrt();
        for m in 0..modes {
            let jitter = 0.94 + 0.12 * hash01(m as u32 * 3 + 1) as f64;
            let f = 60.0 * p.board_ratio.powi(m as i32) * jitter;
            if f >= 0.45 * sample_rate {
                continue;
            }
            // Real soundboard modes are heavily radiation-damped: T60 ~0.25 s
            // at the bottom, tens of ms in the kHz range. This is also what
            // makes the attack fast: a mode's rise time is 1/sigma, so a
            // lightly damped board *swells* instead of speaking (measured:
            // 17-47 ms to peak with sigma = 16 + f/38; < 10 ms with this).
            let sigma = (p.board_sig_base + f / p.board_sig_div).min(400.0);
            let r = (-sigma * dt).exp();
            let th = core::f64::consts::TAU * f * dt;
            cr[m] = (r * th.cos()) as f32;
            ci[m] = (r * th.sin()) as f32;
            gin[m] = 1.0;
            // Radiativity curve (see above): flat mid plateau, dark top.
            let tilt = p.board_tilt * radiativity(f, p);
            let al = (0.6 + 0.8 * hash01(m as u32 * 5 + 2) as f64) * tilt * norm;
            let ar = (0.6 + 0.8 * hash01(m as u32 * 7 + 3) as f64) * tilt * norm;
            let sl = if hash01(m as u32 * 11 + 4) < 0.5 { -1.0 } else { 1.0 };
            let sr = if hash01(m as u32 * 13 + 5) < 0.5 { -1.0 } else { 1.0 };
            // scale: modes integrate per-sample force (state grows with fs
            // and with 1/sigma resonant gain) — normalise both out
            let fs_norm = 48000.0 / sample_rate;
            gout_l[m] = (al * sl as f64 * sigma * 0.006 * fs_norm) as f32;
            gout_r[m] = (ar * sr as f64 * sigma * 0.006 * fs_norm) as f32;
        }
        Self {
            zr,
            zi,
            cr,
            ci,
            gin,
            gout_l,
            gout_r,
            // Plateau-normalised velocity coupling (see RadTilt in lib.rs):
            // the per-sample difference is scaled by fs/(2 pi f_lo), so the
            // level is sample-rate independent and `direct` is the plateau
            // gain of the path.
            direct: (p.board_direct * sample_rate / (core::f64::consts::TAU * 150.0)) as f32,
            dbg_direct: 1.0,
            dx1: 0.0,
            dlp: 0.0,
            dlp2: 0.0,
            dc_lp: (1.0 - (-core::f64::consts::TAU * 150.0 / sample_rate).exp()) as f32,
            dc_lp2: (1.0 - (-core::f64::consts::TAU * p.rad_lp / sample_rate).exp()) as f32,
        }
    }

    /// Accumulates board response to `input` (bridge force bus) into l/r.
    pub fn render(&mut self, path: KernelPath, input: &[f32], l: &mut [f32], r: &mut [f32]) {
        debug_assert!(input.len() <= MAX_CHUNK);
        run_modes_stereo(
            path,
            &mut self.zr,
            &mut self.zi,
            &self.cr,
            &self.ci,
            &self.gin,
            &self.gout_l,
            &self.gout_r,
            input,
            1.0,
            l,
            r,
        );
        for k in 0..input.len() {
            // velocity-coupled direct radiation: differentiate, flatten at
            // the bottom of the radiativity plateau (~150 Hz), roll off
            // above ~2.4 kHz like the modal board's R(f)
            let diff = input[k] - self.dx1;
            self.dx1 = input[k];
            self.dlp += self.dc_lp * (diff - self.dlp);
            self.dlp2 += self.dc_lp2 * (self.dlp - self.dlp2);
            let d = self.dbg_direct * self.direct * self.dlp2;
            l[k] += d;
            r[k] += d;
        }
    }

    pub fn reset(&mut self) {
        self.zr.fill(0.0);
        self.zi.fill(0.0);
        self.dx1 = 0.0;
        self.dlp = 0.0;
        self.dlp2 = 0.0;
    }
}
