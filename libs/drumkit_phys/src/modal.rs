// The modal resonator bank — every drum head partial, shell mode, cavity
// resonance, cymbal plate mode and snare-wire formant in the kit is one
// exponentially damped complex rotator:
//
//     z[k+1] = C z[k] + x[k],   C = r e^{i theta},  r = exp(-sigma/fs),
//                               theta = 2 pi f / fs
//
// with the drive injected on the real axis. For a force-driven mode
// q'' + 2 sigma q' + w^2 q = F/M the impulse response of the displacement is
// (1/(M w_d)) e^{-sigma t} sin(w_d t) and of the velocity ~ e^{-sigma t}
// cos(w_d t): with x = F dt / M, Im(z) is w_d * displacement and Re(z) is
// the velocity. Radiated pressure from a head or a plate follows velocity
// (acceleration in the far field), so the OUTPUT reads Re(z); the geometric
// nonlinearities (membrane tension, plate stretching) and the contact
// detectors (snare wires, hi-hat chatter) are functions of DISPLACEMENT and
// read Im(z).
//
// Why modal and not FDTD/waveguide: exactly the piano's reasons (see
// libs/piano_model/src/lib.rs) — the modes are the analytic solution of the
// membrane and plate equations, each is a contraction (|C| < 1) so the bank
// is unconditionally stable for any input, and per-mode frequency/damping
// control is what the reference measurements are expressed in.
//
// Layout is SoA, padded to a multiple of 8 with C = 0, gains = 0 so the
// kernel runs over whole 8-lane groups without branching (padding modes stay
// exactly zero forever). The per-sample loop is samples-outer / modes-inner
// because the nonlinear drives of sample k+1 need the sums of sample k; the
// modes-inner loop is written as independent 8-lane groups so the compiler
// vectorises the state update and the partial accumulations, and the
// accumulation order is fixed (lane groups in index order, then one
// horizontal add) — bit-identical for any host block decomposition.
//
// Drives (scalars per sample, each with a per-mode gain array):
//   F  strike force (gi)          — the Hunt-Crossley contact
//   R  rattle/contact noise (gr)  — snare wires, hi-hat chatter, clap bursts;
//                                   cymbals reuse gr for the tier-1 cascade
//   S  secondary nonlinear (gs)   — cymbal tier-2 cascade
// Reads (per-mode gain arrays over Im(z) = displacement):
//   gh: the nonlinearity / detector input, accumulated separately over three
//       contiguous frequency-sorted tiers [0,t1) [t1,t2) [t2,n) so cymbals
//       can cascade low -> mid -> high without any feedback loop
//       (the drive of a tier comes from the tier below it only: a DAG of
//       contractions driven by bounded inputs is bounded).

pub const MAX_MODES: usize = 160;
pub const LANES: usize = 8;

#[derive(Clone, Copy)]
pub struct Bank {
    /// Padded mode count (multiple of LANES).
    pub n: usize,
    /// Tier boundaries (multiples of LANES): [0,t1) [t1,t2) [t2,n).
    pub t1: usize,
    pub t2: usize,
    pub zr: [f32; MAX_MODES],
    pub zi: [f32; MAX_MODES],
    pub cr: [f32; MAX_MODES],
    pub ci: [f32; MAX_MODES],
    pub gi: [f32; MAX_MODES],
    pub gr: [f32; MAX_MODES],
    pub gs: [f32; MAX_MODES],
    pub go: [f32; MAX_MODES],
    pub gh: [f32; MAX_MODES],
    /// Strike-point displacement read (sum gd * Im z = the head's
    /// displacement under the striker, for the two-way contact).
    pub gd: [f32; MAX_MODES],
    /// Base frequency (Hz) and damping (1/s) of each mode: the tick-rate
    /// controllers (tension modulation, amplitude damping) rebuild cr/ci
    /// from these, never from the previous coefficients.
    pub f: [f32; MAX_MODES],
    pub sig: [f32; MAX_MODES],
    /// Per-mode weight in the controller's energy sum (membrane: k^2 for the
    /// strain energy; cymbal: 1 for the amplitude).
    pub w: [f32; MAX_MODES],
    /// Per-mode multiplier on the tick-rate frequency shift (1 for membrane
    /// modes, 0 for shell/cavity/wire modes that do not ride the tension).
    pub fm: [f32; MAX_MODES],
}

/// Per-sample sums the bank produces.
#[derive(Clone, Copy, Default)]
pub struct Sums {
    /// Output (sum go * Re z) — velocity-like, the radiated signal.
    pub out: f32,
    /// Tier displacement sums (sum gh * Im z) for tiers 0 and 1 (tier 2
    /// drives nothing, so its sum is not formed).
    pub x0: f32,
    pub x1: f32,
}

impl Bank {
    pub const fn new() -> Self {
        Self {
            n: 0,
            t1: 0,
            t2: 0,
            zr: [0.0; MAX_MODES],
            zi: [0.0; MAX_MODES],
            cr: [0.0; MAX_MODES],
            ci: [0.0; MAX_MODES],
            gi: [0.0; MAX_MODES],
            gr: [0.0; MAX_MODES],
            gs: [0.0; MAX_MODES],
            go: [0.0; MAX_MODES],
            gh: [0.0; MAX_MODES],
            gd: [0.0; MAX_MODES],
            f: [0.0; MAX_MODES],
            sig: [0.0; MAX_MODES],
            w: [0.0; MAX_MODES],
            fm: [0.0; MAX_MODES],
        }
    }

    /// Clears the state and every coefficient/gain (a slot being re-used for
    /// a different instrument must not inherit padding garbage).
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Pads n up to the lane multiple and fixes the tier boundaries.
    pub fn finish(&mut self, n_used: usize, t1: usize, t2: usize) {
        let n = pad(n_used.min(MAX_MODES));
        self.n = n;
        self.t1 = pad(t1).min(n);
        self.t2 = pad(t2).clamp(self.t1, n);
        for m in n_used..MAX_MODES {
            self.cr[m] = 0.0;
            self.ci[m] = 0.0;
            self.gi[m] = 0.0;
            self.gr[m] = 0.0;
            self.gs[m] = 0.0;
            self.go[m] = 0.0;
            self.gh[m] = 0.0;
            self.gd[m] = 0.0;
            self.f[m] = 0.0;
            self.sig[m] = 0.0;
            self.w[m] = 0.0;
            self.fm[m] = 0.0;
            self.zr[m] = 0.0;
            self.zi[m] = 0.0;
        }
    }

    /// Rebuilds the rotation coefficients from the base frequency/damping,
    /// with a global frequency factor applied to the tension-riding modes
    /// (fm = 1) and an extra damping term per mode (`extra_sig` in 1/s,
    /// scaled per mode by w). Modes that would land above 0.47 fs are
    /// silenced (their coefficient set to 0), not aliased.
    pub fn rebuild(&mut self, fs: f32, freq_factor: f32, extra_sig: f32) {
        let inv_fs = 1.0 / fs;
        let nyq = 0.47 * fs;
        for m in 0..self.n {
            let f = self.f[m] * (1.0 + (freq_factor - 1.0) * self.fm[m]);
            if f <= 0.0 || f >= nyq {
                self.cr[m] = 0.0;
                self.ci[m] = 0.0;
                continue;
            }
            let sig = self.sig[m] + extra_sig * self.w[m];
            let r = (-sig * inv_fs).exp();
            let th = core::f32::consts::TAU * f * inv_fs;
            let (s, c) = th.sin_cos();
            self.cr[m] = r * c;
            self.ci[m] = r * s;
        }
    }

    /// Weighted energy sum_m w_m |z_m|^2 (used by the controllers).
    pub fn weighted_energy(&self) -> f32 {
        let mut e = [0.0f32; LANES];
        for g in (0..self.n).step_by(LANES) {
            for l in 0..LANES {
                let m = g + l;
                e[l] += self.w[m] * (self.zr[m] * self.zr[m] + self.zi[m] * self.zi[m]);
            }
        }
        e.iter().sum()
    }

    /// Displacement under the striker: sum gd_m Im(z_m).
    pub fn strike_point_displacement(&self) -> f32 {
        let mut e = [0.0f32; LANES];
        for g in (0..self.n).step_by(LANES) {
            for l in 0..LANES {
                let m = g + l;
                e[l] += self.gd[m] * self.zi[m];
            }
        }
        hsum(&e)
    }

    /// Nonlinearity input energy sum_{m < t2} gh_m^2 |z_m|^2 (cymbal
    /// amplitude damping).
    pub fn tier_energy(&self) -> f32 {
        let mut e = [0.0f32; LANES];
        for g in (0..self.t2).step_by(LANES) {
            for l in 0..LANES {
                let m = g + l;
                e[l] += self.gh[m] * self.gh[m] * (self.zr[m] * self.zr[m] + self.zi[m] * self.zi[m]);
            }
        }
        e.iter().sum()
    }

    /// One sample: inject the drives, rotate, accumulate the sums. Three
    /// straight loops (one per tier) over 8-lane groups: no per-group
    /// branching, so the compiler vectorises the rotation and the partial
    /// sums; the accumulation order is fixed.
    #[inline(always)]
    pub fn step(&mut self, drive_f: f32, drive_r: f32, drive_s: f32) -> Sums {
        let mut out = [0.0f32; LANES];
        let x0 = self.run_range(0, self.t1, drive_f, drive_r, drive_s, &mut out);
        let x1 = self.run_range(self.t1, self.t2, drive_f, drive_r, drive_s, &mut out);
        let _ = self.run_range(self.t2, self.n, drive_f, drive_r, drive_s, &mut out);
        Sums { out: hsum(&out), x0, x1 }
    }

    #[inline(always)]
    fn run_range(&mut self, from: usize, to: usize, drive_f: f32, drive_r: f32, drive_s: f32, out: &mut [f32; LANES]) -> f32 {
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        {
            // SAFETY: NEON is baseline on aarch64 and SSE2 on x86_64; every
            // access is within the padded arrays (from/to are lane multiples
            // <= MAX_MODES).
            unsafe { self.run_range_simd(from, to, drive_f, drive_r, drive_s, out) }
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        {
            self.run_range_scalar(from, to, drive_f, drive_r, drive_s, out)
        }
    }

    #[allow(dead_code)]
    #[inline(always)]
    fn run_range_scalar(&mut self, from: usize, to: usize, drive_f: f32, drive_r: f32, drive_s: f32, out: &mut [f32; LANES]) -> f32 {
        let mut xs = [0.0f32; LANES];
        let mut g = from;
        while g < to {
            for l in 0..LANES {
                let m = g + l;
                let zr = self.zr[m];
                let zi = self.zi[m];
                let cr = self.cr[m];
                let ci = self.ci[m];
                let inj = self.gi[m] * drive_f + self.gr[m] * drive_r + self.gs[m] * drive_s;
                let nzr = cr * zr - ci * zi + inj;
                let nzi = ci * zr + cr * zi;
                self.zr[m] = nzr;
                self.zi[m] = nzi;
                out[l] += self.go[m] * nzr;
                xs[l] += self.gh[m] * nzi;
            }
            g += LANES;
        }
        hsum(&xs)
    }

    /// Same arithmetic as the scalar kernel, four lanes at a time (two
    /// vectors per 8-lane group), same accumulation order per lane.
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    #[inline(always)]
    unsafe fn run_range_simd(&mut self, from: usize, to: usize, drive_f: f32, drive_r: f32, drive_s: f32, out: &mut [f32; LANES]) -> f32 {
        use crate::simd::*;
        let df = splat(drive_f);
        let dr = splat(drive_r);
        let ds = splat(drive_s);
        let mut out0 = load(out.as_ptr());
        let mut out1 = load(out.as_ptr().add(4));
        let mut xs0 = zero();
        let mut xs1 = zero();
        let mut g = from;
        while g < to {
            for h in 0..2 {
                let m = g + 4 * h;
                let zr = load(self.zr.as_ptr().add(m));
                let zi = load(self.zi.as_ptr().add(m));
                let cr = load(self.cr.as_ptr().add(m));
                let ci = load(self.ci.as_ptr().add(m));
                let inj = add(add(mul(load(self.gi.as_ptr().add(m)), df), mul(load(self.gr.as_ptr().add(m)), dr)), mul(load(self.gs.as_ptr().add(m)), ds));
                let nzr = add(sub(mul(cr, zr), mul(ci, zi)), inj);
                let nzi = add(mul(ci, zr), mul(cr, zi));
                store(self.zr.as_mut_ptr().add(m), nzr);
                store(self.zi.as_mut_ptr().add(m), nzi);
                let go = load(self.go.as_ptr().add(m));
                let gh = load(self.gh.as_ptr().add(m));
                if h == 0 {
                    out0 = add(out0, mul(go, nzr));
                    xs0 = add(xs0, mul(gh, nzi));
                } else {
                    out1 = add(out1, mul(go, nzr));
                    xs1 = add(xs1, mul(gh, nzi));
                }
            }
            g += LANES;
        }
        store(out.as_mut_ptr(), out0);
        store(out.as_mut_ptr().add(4), out1);
        let mut xs = [0.0f32; LANES];
        store(xs.as_mut_ptr(), xs0);
        store(xs.as_mut_ptr().add(4), xs1);
        hsum(&xs)
    }
}

#[inline(always)]
fn hsum(v: &[f32; LANES]) -> f32 {
    ((v[0] + v[4]) + (v[2] + v[6])) + ((v[1] + v[5]) + (v[3] + v[7]))
}

pub fn pad(n: usize) -> usize {
    (n + LANES - 1) & !(LANES - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_rings_at_its_frequency_and_decays_at_its_rate() {
        let fs = 48000.0;
        let mut b = Bank::new();
        b.f[0] = 440.0;
        b.sig[0] = 20.0;
        b.go[0] = 1.0;
        b.gi[0] = 1.0;
        b.finish(1, 8, 8);
        b.rebuild(fs, 1.0, 0.0);
        let mut y = Vec::new();
        for k in 0..48000 {
            let s = b.step(if k == 0 { 1.0 } else { 0.0 }, 0.0, 0.0);
            y.push(s.out);
        }
        // zero crossings in the first 0.1 s -> ~440 Hz
        let zc = y[..4800].windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count();
        assert!((zc as f32 / 0.1 / 2.0 - 440.0).abs() < 12.0, "zc {zc}");
        // envelope: e^{-20 t}: at t = 0.1 s amplitude ratio 0.135
        let a0 = y[..200].iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let a1 = y[4800..5000].iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(((a1 / a0) - (-2.0f32).exp()).abs() < 0.02, "{}", a1 / a0);
    }
}
