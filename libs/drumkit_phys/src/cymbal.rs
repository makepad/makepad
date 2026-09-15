// Cymbals (hi-hats, ride, ride bell, crash): thin curved bronze plates.
//
// Linear part. A cymbal has hundreds of modes; a flat plate's modal density
// is constant with frequency (dN/df = (A/2) sqrt(rho h / D)), but the bow's
// curvature stiffens every mode below the shell "ring" frequency, so the
// density ramps up from the lowest partials (a few hundred Hz on an 18")
// towards its plateau. The bank places `n_modes` partials by inverting that
// cumulative density, d(f) = D f / (f + f_ring), with a deterministic
// per-mode jitter of up to +-0.45 of a spacing: statistically a cymbal, and
// never a fixed-ratio cluster (a cowbell is five partials at fixed ratios;
// the acceptance test for these voices is that no partial stands more than
// a few dB above its neighbours after the strike has settled). Damping
// follows sigma(f) = a + b (f/1 kHz)^c with a +-30 % per-mode spread —
// radiation efficiency and internal loss both rise with frequency, which is
// why the reference ride keeps its 2-3 kHz wash for 15 s and its air for
// 1.5 s. Below the plate's coincidence frequency (~17 kHz for 1 mm bronze)
// radiation efficiency rises with f, hence the output weight (f/f_low)^slope.
//
// Nonlinear part — the bloom. A struck cymbal spreads its energy UP in
// frequency over the first few hundred milliseconds: the reference crash's
// 4-20 kHz energy peaks 155 ms after the strike (12 dB above its first 30 ms)
// when hit hard, and barely at all when hit softly. The physics is the
// geometric (von Karman) nonlinearity of a thin plate — mid-surface
// stretching couples every mode to every other through cubic terms, and
// energy cascades from the strongly excited low modes to the dense high ones.
// The bank implements that cascade as a directed chain of three
// frequency-sorted tiers: the displacement sum of tier 0 (the low, loud
// partials) is cubed and drives tier 1; tier 1's displacement, cubed, drives
// tier 2. Cubing a sum of N partials produces all the sum/difference
// combinations 3f_i, 2f_i +- f_j, f_i + f_j +- f_k — a dense comb that lands
// on the higher tiers' modes and is amplitude-cubed, so a soft hit hardly
// blooms and a hard one does (the reference: +5.6 dB late/early at P,
// +12.1 dB at FF). Energy conservation is honoured on the source side with an
// amplitude-dependent damping term on the lower tiers (their energy is what
// leaves). Because the chain only ever points upward there is no feedback
// loop: bounded input -> bounded drive -> bounded output, at every amplitude,
// with the modes still exact contractions.
//
// Hi-hat. Closed: the top cymbal is pressed onto the bottom, which clamps
// the long-wavelength modes (heavy extra damping below ~1.5 kHz) and leaves
// a short bright "chick" (reference T60 ~0.2-0.3 s in every band); struck
// hard the top bounces on the bottom, a chatter of impacts gated by the low
// modes' displacement (rattle.rs). Open: a free small cymbal — long ring,
// full cascade. Pedal: the cymbals clap together — a broad, slow contact
// (the whole top cymbal is the striker), the chatter, then the closed
// damping.
//
// Ride bell: the dome is thick and stiff — its modes are fewer, higher in
// frequency and higher in Q than the bow's. The same plate carries a set of
// "bell" modes (800 Hz - 5 kHz, Q x3); a bell strike drives them hard and
// the bow weakly, a bow ping the reverse.

use crate::contact::Striker;
use crate::membrane::{DampLaw, ProtoMode};
use crate::modal::{Bank, MAX_MODES};
use crate::util::Rng;

#[derive(Clone, Copy)]
pub struct Bell {
    pub f_lo: f64,
    pub f_hi: f64,
    /// Drive multiplier for bell modes on a bell strike (and for bow modes
    /// the reciprocal is applied), Q multiplier and mode count.
    pub drive: f64,
    pub q_mult: f64,
    pub n_modes: usize,
}

#[derive(Clone, Copy)]
pub struct CymbalDesign {
    pub n_modes: usize,
    pub f_low: f64,
    pub f_ring: f64,
    pub f_top: f64,
    pub damp: DampLaw,
    pub damp_spread: f64,
    /// Extra damping (1/s) on modes below `clamp_f` (closed hat).
    pub clamp_sig: f64,
    pub clamp_f: f64,
    /// Tier boundaries (Hz) and cascade gains.
    pub tier1_f: f64,
    pub tier2_f: f64,
    pub eps1: f64,
    pub eps2: f64,
    /// Amplitude-dependent damping of tiers 0-1 (1/s per unit of x^2).
    pub nl_damp: f64,
    /// Displacement normalisation: gh scale so a hard hit gives x ~ 1.
    pub disp_norm: f64,
    /// Radiation slope (f/f_low)^slope and output gain.
    pub out_slope: f64,
    pub out_gain: f64,
    /// Strike spectrum: modal drive is weighted by 1/(1 + (f/strike_f)^2)
    /// on top of the contact pulse (stick tip area / position averaging).
    pub strike_f: f64,
    pub bell: Option<Bell>,
    pub bell_strike: bool,
    /// Roughness of the contact force (stick tip skidding on bronze): the
    /// broadband "tick" of the ping, active only during the contact, scaling
    /// with the square root of the force (a soft tip stroke still ticks).
    pub contact_noise: f64,
    /// The continuum. A real cymbal has thousands of modes; this bank has
    /// ~130. The plate's forced response to the nonlinear (cascade) force
    /// through all the modes NOT in the bank is broadband and smooth — for
    /// a plate the point admittance is resistive, 1/(8 sqrt(D rho h)) — so
    /// that response is the cascade drive itself, passed to the output with
    /// this gain. It is what fills the spectrum between the modelled lines
    /// (the reference cymbals' 1-6 kHz partials stand only 22-31 dB above
    /// their neighbours; a bank of isolated lines scores 80-110 dB — the
    /// cowbell failure).
    pub direct_nl: f64,
    pub striker: Striker,
    pub seed: u32,
}

#[derive(Clone, Copy)]
pub struct CymbalProto {
    pub n: usize,
    pub t1: usize,
    pub t2: usize,
    pub modes: [ProtoMode; MAX_MODES],
    pub eps1: f32,
    pub eps2: f32,
    pub nl_damp: f32,
    pub out_gain: f32,
    pub contact_noise: f32,
    pub direct_nl: f32,
    pub striker: Striker,
}

impl CymbalProto {
    pub const fn empty() -> Self {
        Self {
            n: 0,
            t1: 0,
            t2: 0,
            modes: [ProtoMode::ZERO; MAX_MODES],
            eps1: 0.0,
            eps2: 0.0,
            nl_damp: 0.0,
            out_gain: 0.0,
            contact_noise: 0.0,
            direct_nl: 0.0,
            striker: Striker {
                mass: 0.01,
                k: 1.0e7,
                p: 1.5,
                lambda: 0.0,
                r_point: 50.0,
                v_min: 0.0,
                v_max: 1.0,
                v_curve: 1.0,
                f_max: 1000.0,
                timeout_ms: 10.0,
                relax_s: 0.0,
                retract: 0.0,
            },
        }
    }

    pub fn build(d: &CymbalDesign, fs: f32) -> Self {
        let mut p = Self::empty();
        p.eps1 = d.eps1 as f32;
        p.eps2 = d.eps2 as f32;
        p.nl_damp = d.nl_damp as f32;
        p.out_gain = d.out_gain as f32;
        p.contact_noise = d.contact_noise as f32;
        p.direct_nl = (d.direct_nl * d.out_gain) as f32;
        p.striker = d.striker;
        let inv_fs = 1.0 / fs as f64;
        let f_top = d.f_top.min(0.45 * fs as f64);
        let n_bow = d.n_modes.min(MAX_MODES - d.bell.map_or(0, |b| b.n_modes));
        let mut rng = Rng::new(d.seed);
        // cumulative density (unnormalised) for d(f) = f / (f + f_ring)
        let cum = |f: f64| f - d.f_ring * (f + d.f_ring).ln();
        let c0 = cum(d.f_low);
        let c1 = cum(f_top);
        // Plate modal mass ~ rho h A / 4: an 18" 1 mm bronze cymbal ~1.4 kg
        // -> M ~ 0.35 kg; the absolute value is a gain, the relative ones
        // between bow and bell (thicker) matter.
        let mass_bow = 0.35;
        let mut count = 0usize;
        let mut freqs: [f64; MAX_MODES] = [0.0; MAX_MODES];
        for k in 0..n_bow {
            let target = c0 + (c1 - c0) * ((k as f64 + 0.5 + 0.9 * (rng.unit_f64() - 0.5)) / n_bow as f64);
            // invert by bisection (cum is monotonic)
            let (mut lo, mut hi) = (d.f_low, f_top);
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                if cum(mid) < target {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            freqs[count] = 0.5 * (lo + hi);
            count += 1;
        }
        // bell modes interleaved by frequency
        let n_bell = d.bell.map_or(0, |b| b.n_modes.min(MAX_MODES - count));
        let mut bell_f: [f64; 32] = [0.0; 32];
        if let Some(b) = d.bell {
            for k in 0..n_bell.min(32) {
                let u = (k as f64 + 0.5 + 0.8 * (rng.unit_f64() - 0.5)) / n_bell as f64;
                bell_f[k] = b.f_lo * (b.f_hi / b.f_lo).powf(u);
            }
        }
        // Merge (sorted) with per-mode properties.
        let mut all: [(f64, bool); MAX_MODES] = [(0.0, false); MAX_MODES];
        let mut na = 0;
        for i in 0..count {
            all[na] = (freqs[i], false);
            na += 1;
        }
        for i in 0..n_bell.min(32) {
            if na < MAX_MODES {
                all[na] = (bell_f[i], true);
                na += 1;
            }
        }
        all[..na].sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        let mut t1 = na;
        let mut t2 = na;
        for i in 0..na {
            let (f, is_bell) = all[i];
            if t1 == na && f >= d.tier1_f {
                t1 = i;
            }
            if t2 == na && f >= d.tier2_f {
                t2 = i;
            }
            let spread = 1.0 + d.damp_spread * 2.0 * (rng.unit_f64() - 0.5);
            let mut sig = d.damp.sigma(f) * spread;
            if is_bell {
                sig /= d.bell.unwrap().q_mult;
            }
            if f < d.clamp_f {
                sig += d.clamp_sig * (1.0 - f / d.clamp_f);
            }
            // strike-point shape: random sign and magnitude, never zero
            let psi = {
                let u = rng.unit_f64();
                let mag = 0.25 + 0.75 * rng.unit_f64();
                if u < 0.5 { -mag } else { mag }
            };
            let mass = if is_bell { mass_bow * 2.5 } else { mass_bow };
            let strike_shape = 1.0 / (1.0 + (f / d.strike_f).powi(2));
            let mut drive = psi * strike_shape / mass;
            if let Some(b) = d.bell {
                if is_bell == d.bell_strike {
                    drive *= b.drive;
                } else {
                    drive /= b.drive;
                }
            }
            // cascade injection shape: another shape sample (the nonlinear
            // forcing is distributed over the plate, not at the strike point)
            let psi2 = {
                let u = rng.unit_f64();
                let mag = 0.25 + 0.75 * rng.unit_f64();
                if u < 0.5 { -mag } else { mag }
            };
            let radiation = (f / d.f_low).powf(d.out_slope);
            let pm = &mut p.modes[i];
            *pm = ProtoMode::ZERO;
            pm.f = f as f32;
            pm.sig = sig as f32;
            pm.w = if i < t2 { 1.0 } else { 0.0 };
            pm.fm = 0.0;
            pm.order = u32::MAX;
            pm.e_fixed = (drive * inv_fs) as f32;
            pm.out = (radiation * psi.abs().sqrt()) as f32;
            // displacement read (Im z = w q): q = Im z / w, normalised
            pm.read = (d.disp_norm * psi / (core::f64::consts::TAU * f)) as f32;
            pm.inject = (psi2 / mass * inv_fs) as f32;
        }
        // Tier boundaries on the kernel's lane grid so that the cascade gains
        // and the read sums agree exactly (a mode must never drive itself).
        p.n = na;
        p.t1 = crate::modal::pad(t1).min(crate::modal::pad(na));
        p.t2 = crate::modal::pad(t2).clamp(p.t1, crate::modal::pad(na));
        p
    }

    pub fn load(&self, bank: &mut Bank, fs: f32, velocity: f32) -> f64 {
        bank.clear();
        for i in 0..self.n {
            let m = &self.modes[i];
            bank.f[i] = m.f;
            bank.sig[i] = m.sig;
            bank.w[i] = m.w;
            bank.fm[i] = 0.0;
            bank.gi[i] = m.e_fixed;
            bank.go[i] = m.out * self.out_gain;
            bank.gh[i] = m.read;
            // tier 1 receives the tier-0 cube through gr, tier 2 the tier-1
            // cube through gs; tier 0 receives no cascade drive.
            bank.gr[i] = if i >= self.t1 && i < self.t2 { m.inject * self.eps1 } else { 0.0 };
            bank.gs[i] = if i >= self.t2 { m.inject * self.eps2 } else { 0.0 };
        }
        bank.finish(self.n, self.t1, self.t2);
        bank.rebuild(fs, 1.0, 0.0);
        self.striker.speed(velocity)
    }
}
