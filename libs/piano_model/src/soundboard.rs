// Shared modal soundboard: turns bridge force into radiated sound at TWO
// listening points, with the bridge treated as four coupling regions (the
// bass bridge and three sections of the long bridge) rather than one
// scalar point.
//
// Space, in three physical layers:
// - COUPLING: each board mode is driven through a per-(mode, region)
//   weight. Long-wavelength modes span the whole board, so every bridge
//   region drives them coherently; short-wavelength modes alternate sign
//   region-to-region the way real mode shapes do. This is what lets a bass
//   note and a treble note excite the same board differently — the board
//   itself has a left-right image, not just the panned direct strings.
// - RADIATION / IMAGE: the two listening points hear each mode with a
//   per-mode interchannel phase phi = 2 pi f tau, where tau is a
//   physically sized time difference (sub-millisecond) set by the region's
//   azimuth plus a small per-mode scatter, and a mild level difference
//   that grows with frequency (radiation lobes). Low modes therefore stay
//   nearly coherent between channels and the coherence falls raggedly
//   with frequency — the measured interaural envelope of a real
//   instrument in a room. An earlier scheme read Im(z) left / Re(z) right
//   with independent random signs: a blanket 90-degree offset that pinned
//   midrange interchannel correlation near zero and read as a phasey
//   wash rather than an instrument with a place.
// - The per-region DIRECT (velocity-coupled) component is panned by its
//   region azimuth, so the instant part of the sound sits in the same
//   image as the modal part.
//
// Mode damping is pinned to measured soundboard physics (see params.rs:
// board_sig_*): quality factors ~19-37 across the band, the Giordano
// range. The attack still speaks instantly through the direct paths; the
// board's own modes bloom over tens of milliseconds underneath, which is
// the "body" of a large instrument.

use crate::modal::{pad8, run_modes_stereo, KernelPath, MAX_CHUNK};
use crate::params::DesignParams;

pub const BOARD_REGIONS: usize = 4;
/// Hard cap on the parameterised per-region mode count (preallocation bound).
pub const BOARD_MODES_MAX: usize = 256;

/// Region azimuths in pan units (player perspective, bass left) and the
/// interchannel time difference (seconds) each azimuth produces at a
/// close listening position.
const REGION_AZ: [f64; BOARD_REGIONS] = [-0.42, -0.14, 0.12, 0.38];
const AZ_ITD_S: f64 = 0.00045;

/// Radiativity of the instrument body: how strongly a bridge-force partial
/// at `f` Hz reaches the listener, relative to the mid plateau.
///
/// Shape (normalised, dimensionless, ~1.0 across the plateau):
/// - falls below ~100 Hz (the board is smaller than the wavelength)
/// - broad plateau ~150 Hz .. ~2 kHz with a low-mid body emphasis
/// - gentle roll-off above the top corner
pub fn radiativity(f: f64, p: &DesignParams) -> f64 {
    // second-order collapse below the first board resonance (see
    // params::rad_hp1): the real bottom octave speaks through its partial
    // cluster, not its fundamental
    let hp1 = f * f / (f * f + p.rad_hp1 * p.rad_hp1);
    // first-order (a realisable magnitude: RadFilter runs exactly this
    // term on the direct paths)
    let hp2 = f / (f * f + p.rad_hp2 * p.rad_hp2).sqrt();
    let x = (f / p.rad_lp) * (f / p.rad_lp);
    let lp = (1.0 / (1.0 + x)).powf(0.5 * p.rad_lp_pow);
    // Low-mid body emphasis: the board's main resonances sit in the
    // ~100-400 Hz region and radiate the fundamentals of the middle octaves
    // strongly — the reference recordings have the FUNDAMENTAL as the
    // strongest partial through C3..C4 even though the strike comb feeds
    // partial 2 five dB more force. A plateau that is flat down to the
    // bass high-pass cannot reproduce that and reads as thin ("tinny").
    // Fourth-order shelf keeps the emphasis out of the 500 Hz+ region, and
    // a second-order high-pass at 85 Hz keeps it out of the deep bass:
    // below the main resonance the board stops radiating again, and a
    // C2 whose fundamental outweighs its partial cluster reads as boom,
    // not body (the bass-speaks-through-partials law).
    let b = f / p.rad_body_hz;
    let b4 = b * b * b * b;
    let body = 1.0 + p.rad_body * (1.0 / (1.0 + b4)) * (f * f / (f * f + 85.0 * 85.0));
    // (A 50-88 Hz "first-resonance step" used to lift the A1/C2
    // fundamentals here. It was calibrated against the MP3 GM corpus's
    // C2 row, which claimed the fundamental strongest; the real
    // multi-velocity corpus measures the real C2 fundamental 26 dB BELOW
    // its cluster at every layer, and the listener heard the lifted
    // version as a bass guitar. Deleted 2026-08-31.)
    hp1 * hp2 * lp * body
}

/// Bridge-admittance proxy, complex: (Re, Im) at `f`, both normalised by
/// the real part's 40..1200 Hz median so params::bridge_couple is a plain
/// 1/s scale at a typical partial. Re decides how strongly the bridge
/// drains a string mode (the per-partial prompt loss); Im is the reactive
/// part, changing sign across each bridge resonance and pulling a coupled
/// partial's frequency — the per-partial mistuning irregularity of a real
/// bridge.
///
/// NOT the radiating lattice above: that one is deliberately DENSE and
/// extra-damped at the bottom (to radiate smoothly), which would make the
/// Lorentzian sum uniformly high below ~150 Hz — the opposite of a real
/// bridge, whose first modes are sparse, distinct resonances (Q ~60) (the
/// Salamander C2 fundamental RINGS at sigma ~0.5 while A0's second
/// partial 10 Hz away drains at ~30). Hence a sparse lattice: ratio 1.19
/// from 48 Hz, Q ~44 narrowing toward the peaks, widths growing into the
/// kHz range where real modal overlap smooths the curve. Low-mode
/// placement is FIXED (no jitter below 300 Hz): each low mode's position
/// decides which bass fundamentals ring versus drain, and this spacing
/// reproduces the reference assignment (58, 70, 269, 325 Hz on/near
/// drains — C2's fifth partial and C4's fundamental region drain as
/// measured; A1's 55 and C2's 65 fundamentals sit in valleys). Above 300 Hz partial density
/// makes individual placement anonymous and light jitter de-grids it.
pub fn bridge_admittance_c(f: f64, p: &DesignParams) -> (f64, f64) {
    let _ = p;
    fn raw_c(f: f64) -> (f64, f64) {
        let (mut re, mut im) = (0.0, 0.0);
        let mut m = 0u32;
        loop {
            let base = 48.0 * 1.21f64.powi(m as i32);
            let jitter =
                if base < 300.0 { 1.0 } else { 0.96 + 0.08 * hash01(m * 3 + 1) as f64 };
            let fm = base * jitter;
            if fm > 4.0 * f + 600.0 || fm > 20000.0 {
                break;
            }
            let w = 0.5 + 1.0 * hash01(m * 7 + 5) as f64;
            let hw = fm / 60.0 * (1.0 + fm / 1500.0);
            let d = f - fm;
            let den = d * d + hw * hw;
            re += w * hw * hw / den;
            im += -w * hw * d / den;
            m += 1;
        }
        (re, im)
    }
    // Normalise by the 85th percentile, NOT the median: with sparse
    // resonant peaks the median sits at the between-peak level, and
    // dividing by it put half of all frequencies at y >= 1 — through the
    // squared-shape coupling law that made nearly EVERY bass partial a
    // drain (measured C2: five of its first eight partials at sigma
    // 11-30/s where the real C2 drains one and its cluster RINGS at
    // 1.9-2.8/s). A bass note whose cluster all drains reduces to its
    // fundamental within 300 ms: the plucked bass-guitar signature.
    // Near-peak normalisation keeps drains sparse (~a quarter of
    // partials, like the measured corpus) and valleys genuinely quiet.
    let mut med = [0.0f64; 25];
    for (i, slot) in med.iter_mut().enumerate() {
        let g = 40.0 * (1200.0f64 / 40.0).powf(i as f64 / 24.0);
        *slot = raw_c(g).0;
    }
    med.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let norm = med[21].max(1e-6);
    let (re, im) = raw_c(f);
    (re / norm, im / norm)
}

/// One-pole high-pass, |H| ~ f / sqrt(f^2 + fc^2).
#[derive(Clone, Copy)]
struct Hp1 {
    a: f32,
    x1: f32,
    y1: f32,
}

impl Hp1 {
    fn new(fc: f64, fs: f64) -> Self {
        Self { a: (-core::f64::consts::TAU * fc / fs).exp() as f32, x1: 0.0, y1: 0.0 }
    }

    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.a * (self.y1 + x - self.x1);
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// RBJ peaking biquad, transposed direct form II.
#[derive(Clone, Copy)]
struct Peak {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    s1: f32,
    s2: f32,
}

impl Peak {
    fn new(fc: f64, gain_db: f64, q: f64, fs: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = core::f64::consts::TAU * fc / fs;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha / a;
        Self {
            b0: ((1.0 + alpha * a) / a0) as f32,
            b1: (-2.0 * w.cos() / a0) as f32,
            b2: ((1.0 - alpha * a) / a0) as f32,
            a1: (-2.0 * w.cos() / a0) as f32,
            a2: ((1.0 - alpha / a) / a0) as f32,
            s1: 0.0,
            s2: 0.0,
        }
    }

    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.s1;
        self.s1 = self.b1 * x - self.a1 * y + self.s2;
        self.s2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// The radiativity curve R(f) as a running filter, for the instant
/// (non-modal) radiation paths: the board's direct velocity coupling and
/// the panned direct string path. Both used to run their own shape (a
/// differentiator flattened at ~150 Hz): -15 dB at A0's fundamental where
/// R(f) itself is -27.5, and without the 100-250 Hz body emphasis — so the
/// two loudest paths spoke a different, fundamental-heavy bottom octave
/// from the one the board and the key voicing assume. Now every path
/// radiates one curve: the two low knees as realised first-order sections
/// (hp1 twice, hp2 once), the body emphasis as a minimum-phase peaking
/// section (a parallel band-pass sum notched out near its corner), and the
/// same top corner the paths always had. Plateau gain ~1, so the path gains
/// stay plateau gains. Matches radiativity() within ~1 dB from 20 Hz to
/// 2 kHz (test: direct_filter_follows_radiativity).
#[derive(Clone, Copy)]
pub(crate) struct RadFilter {
    hp: [Hp1; 3],
    body: Peak,
    top_c: f32,
    top: f32,
}

impl RadFilter {
    pub(crate) fn new(sample_rate: f64, p: &DesignParams) -> Self {
        // peak of radiativity's body term (it sits at ~0.75 rad_body_hz),
        // placed as a Q 0.68 peak at 0.64 rad_body_hz: fitted to the
        // term's shape, max error 0.6 dB at the defaults
        let peak = (1..400)
            .map(|i| {
                let f = i as f64 * p.rad_body_hz / 200.0;
                let b = f / p.rad_body_hz;
                1.0 + p.rad_body * (1.0 / (1.0 + b * b * b * b)) * (f * f / (f * f + 85.0 * 85.0))
            })
            .fold(1.0, f64::max);
        Self {
            hp: [Hp1::new(p.rad_hp1, sample_rate), Hp1::new(p.rad_hp1, sample_rate), Hp1::new(p.rad_hp2, sample_rate)],
            body: Peak::new(0.64 * p.rad_body_hz, 1.06 * 20.0 * peak.log10(), 0.68, sample_rate),
            top_c: (1.0 - (-core::f64::consts::TAU * p.rad_lp / sample_rate).exp()) as f32,
            top: 0.0,
        }
    }

    #[inline(always)]
    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = self.body.process(x);
        let y = self.hp[0].process(y);
        let y = self.hp[1].process(y);
        let y = self.hp[2].process(y);
        self.top += self.top_c * (y - self.top);
        self.top
    }

    pub(crate) fn reset(&mut self) {
        for h in &mut self.hp {
            h.x1 = 0.0;
            h.y1 = 0.0;
        }
        self.body.s1 = 0.0;
        self.body.s2 = 0.0;
        self.top = 0.0;
    }
}

pub struct Soundboard {
    // Region banks concatenated: region r occupies [r*n .. (r+1)*n).
    zr: Vec<f32>,
    zi: Vec<f32>,
    cr: Vec<f32>,
    ci: Vec<f32>,
    gin: Vec<f32>,
    gout_l: Vec<f32>,
    gout_ri: Vec<f32>,
    gout_rr: Vec<f32>,
    n_padded: usize,
    /// Direct radiation plateau gain (velocity coupling), per region.
    pub direct: f32,
    /// Per-region pan of the direct radiation (the region's azimuth).
    pub dir_pl: [f32; BOARD_REGIONS],
    pub dir_pr: [f32; BOARD_REGIONS],
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
        let total = n * BOARD_REGIONS;
        let mut cr = vec![0.0f32; total];
        let mut ci = vec![0.0f32; total];
        let mut gin = vec![0.0f32; total];
        let mut gout_l = vec![0.0f32; total];
        let mut gout_ri = vec![0.0f32; total];
        let mut gout_rr = vec![0.0f32; total];
        let dt = 1.0 / sample_rate;
        let norm = 1.0 / (modes as f64).sqrt();
        let mut dir_pl = [0.0f32; BOARD_REGIONS];
        let mut dir_pr = [0.0f32; BOARD_REGIONS];
        for (r, az) in REGION_AZ.iter().enumerate() {
            let ang = ((az * 0.5 + 1.0) * core::f64::consts::FRAC_PI_4) as f32;
            dir_pl[r] = core::f32::consts::SQRT_2 * ang.cos();
            dir_pr[r] = core::f32::consts::SQRT_2 * ang.sin();
        }
        for m in 0..modes {
            let jitter = 0.94 + 0.12 * hash01(m as u32 * 3 + 1) as f64;
            // Lattice from 55 Hz: a concert grand's first board resonance
            // sits ~55-90 Hz. Starting at 60 left C2's fundamental hanging
            // on the sparse jittered edge of the stack (a 13 dB notch);
            // starting at 47 handed the BOTTOM octave's fundamentals a
            // dedicated resonance no real board gives them (F#1 measured
            // +17 dB over the recorded-piano trend: boom, not body).
            let f = 55.0 * p.board_ratio.powi(m as i32) * jitter;
            if f >= 0.45 * sample_rate {
                continue;
            }
            // The first resonances are strongly radiation-loaded (that is
            // what makes a board a radiator): extra damping below ~150 Hz
            // widens the bottom modes so the lattice edge is smooth. With
            // laboratory low-mid Q at the bottom too, the 3-4 Hz mode
            // spacing left audible notches between modes — C2's
            // fundamental measured 13 dB into one such gap.
            let sigma_rad = 18.0 / (1.0 + (f / 85.0) * (f / 85.0));
            let sigma = (p.board_sig_base + sigma_rad + f / p.board_sig_div).min(400.0);
            let rr = (-sigma * dt).exp();
            let th = core::f64::consts::TAU * f * dt;
            let (crm, cim) = ((rr * th.cos()) as f32, (rr * th.sin()) as f32);
            // Radiativity curve: mid plateau + body emphasis, dark top.
            let tilt = p.board_tilt * radiativity(f, p);
            // radiation lobe sign, common to both listening points
            let s = if hash01(m as u32 * 11 + 4) < 0.5 { -1.0 } else { 1.0 };
            // base radiated amplitude (state-integration normalised)
            let fs_norm = 48000.0 / sample_rate;
            let g0 = s * tilt * norm * sigma * 0.006 * fs_norm
                * (0.75 + 0.5 * hash01(m as u32 * 5 + 2) as f64);
            // How coherently the bridge regions drive this mode: a mode
            // whose half-wavelength exceeds the bridge span is pushed the
            // same way by every region; short modes alternate.
            let coh = 1.0 / (1.0 + (f / 180.0) * (f / 180.0));
            for r in 0..BOARD_REGIONS {
                let i = r * n + m;
                cr[i] = crm;
                ci[i] = cim;
                let sgn = if hash01((m as u32) * 17 + r as u32 * 7 + 9) < 0.5 { -1.0 } else { 1.0 };
                gin[i] = (coh / BOARD_REGIONS as f64
                    + (1.0 - coh) * sgn / (BOARD_REGIONS as f64).sqrt()) as f32;
                // interchannel phase: region azimuth ITD + per-mode scatter
                let tau = REGION_AZ[r] * AZ_ITD_S
                    + (hash01(m as u32 * 23 + r as u32 * 13 + 3) as f64 - 0.5) * 0.00044;
                let phi = core::f64::consts::TAU * f * tau;
                // level image: region pan, plus a lobe-level difference
                // that grows with frequency
                let lobe = 1.0 + (0.15 + 0.95 * (f / 4000.0).min(1.0))
                    * (hash01(m as u32 * 29 + r as u32 * 19 + 5) as f64 - 0.5);
                let al = g0 * dir_pl[r] as f64;
                let ar = g0 * dir_pr[r] as f64 * lobe;
                gout_l[i] = al as f32;
                gout_ri[i] = (ar * phi.cos()) as f32;
                gout_rr[i] = (ar * phi.sin()) as f32;
            }
        }
        Self {
            zr: vec![0.0f32; total],
            zi: vec![0.0f32; total],
            cr,
            ci,
            gin,
            gout_l,
            gout_ri,
            gout_rr,
            n_padded: n,
            // plateau gain of the direct path (RadFilter is plateau-normalised)
            direct: p.board_direct as f32,
            dir_pl,
            dir_pr,
        }
    }

    /// Accumulates the modal board response to the per-region bridge-force
    /// inputs. The board's direct (non-modal) radiation is panned per region
    /// with dir_pl/dir_pr and filtered together with the direct string path
    /// in lib.rs: both run the same R(f), so one filter per channel serves
    /// every instant path.
    pub fn render(
        &mut self,
        path: KernelPath,
        inputs: &[[f32; MAX_CHUNK]; BOARD_REGIONS],
        n: usize,
        l: &mut [f32],
        r: &mut [f32],
    ) {
        debug_assert!(n <= MAX_CHUNK);
        let np = self.n_padded;
        for reg in 0..BOARD_REGIONS {
            let a = reg * np;
            let b = a + np;
            run_modes_stereo(
                path,
                &mut self.zr[a..b],
                &mut self.zi[a..b],
                &self.cr[a..b],
                &self.ci[a..b],
                &self.gin[a..b],
                &self.gout_l[a..b],
                &self.gout_ri[a..b],
                &self.gout_rr[a..b],
                &inputs[reg][..n],
                1.0,
                l,
                r,
            );
        }
    }

    pub fn reset(&mut self) {
        self.zr.fill(0.0);
        self.zi.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steady-state sine gain of the direct-path filter.
    fn gain(f: f64, fs: f64, p: &DesignParams) -> f64 {
        let mut filt = RadFilter::new(fs, p);
        let n = (fs * 1.5) as usize;
        let (mut acc_y, mut acc_x) = (0.0, 0.0);
        for i in 0..n {
            let x = (core::f64::consts::TAU * f * i as f64 / fs).sin();
            let y = filt.process(x as f32) as f64;
            if i > n / 2 {
                acc_y += y * y;
                acc_x += x * x;
            }
        }
        (acc_y / acc_x).sqrt()
    }

    #[test]
    fn direct_filter_follows_radiativity() {
        let p = DesignParams::default();
        for fs in [44100.0, 48000.0, 96000.0] {
            for f in [20.0, 27.5, 41.0, 55.0, 82.0, 110.0, 165.0, 220.0, 330.0, 500.0, 1000.0, 2000.0] {
                let want = 20.0 * radiativity(f, &p).log10();
                let got = 20.0 * gain(f, fs, &p).log10();
                assert!((got - want).abs() < 1.0, "fs {fs} f {f}: filter {got:.2} dB vs R(f) {want:.2} dB");
            }
        }
    }
}
