// Membrane drums (kick, snare, toms, side stick): the head as a 2-D circular
// membrane, the second head and the air between them, the shell, and the
// two nonlinearities that make a drum a drum — tension modulation of the
// head and (for the snare) the wires, which live in rattle.rs.
//
// Modes. A circular membrane fixed at the rim has modes psi_mn(r, th) =
// J_m(j_mn r/a) cos(m th) at f_mn = (j_mn / j_01) f_01, j_mn the n-th zero
// of J_m: the inharmonic ladder 1 : 1.594 : 2.136 : 2.296 : 2.653 : 2.918 ...
// for (0,1) (1,1) (2,1) (0,2) (3,1) (1,2). The air the head has to move adds
// mass, most to the lowest modes (the entrained air depth scales with the
// wavelength 1/k): f = f_ideal / sqrt(1 + beta / j_mn), then re-anchored so
// (0,1) sits at the design f_01 — which lifts the ratios of the upper modes
// (a 12" tom's (1,1)/(0,1) measures ~1.7 rather than 1.59).
//
// Strike position. A blow at radius r_s excites mode (m,n) in proportion to
// psi_mn(r_s) / M_mn with modal mass M_mn = sigma pi a^2 J_{m+1}(j_mn)^2
// (halved for m >= 1): a centre hit kills every m >= 1 family (J_m(0) = 0),
// a hit at a third of the radius (the normal snare/tom stroke) brings in
// (1,1) and (2,1). The radius is jittered slightly per hit.
//
// Two heads. The batter and resonant (0,n) modes each displace a net volume
// A_n q with A_n = 2 pi a^2 J_1(j_0n) / j_0n, so the enclosed air (spring
// K_air = rho c^2 / V, weakened by a vent or a port) couples them. The 2 x 2
// mass-normalised stiffness matrix
//     [ w_b^2 + K A^2/M_b        K A^2 / sqrt(M_b M_r) ]
//     [        ...               w_r^2 + K A^2/M_r      ]
// is diagonalised at construction; its two normal modes are the doublet
// every two-headed drum shows. The in-phase member (both heads outward, air
// compressed) is the higher one and radiates as a MONOPOLE: loud, and
// dying fast; the anti-phase member (air pumped from head to head) radiates
// as a dipole: quieter and long. The reference kick is exactly this — a 57 Hz
// partial that dominates the first 30 ms and dies at T60 ~ 0.1 s over a 42 Hz
// partial that lasts 0.7 s — and its apparent 400-cent "pitch drop" is the
// same at every velocity, which is the fingerprint of a doublet rather than
// of tension. (m >= 1) modes displace no net volume and do not couple.
//
// Tension modulation (the velocity-dependent glide of toms and the extra
// kick drop at fortissimo): a displaced membrane is stretched, the tension
// rises with the strain energy, and every mode's frequency follows
//     f_m(t) = f_m0 sqrt(1 + gamma S(t)),   S = sum_m k_m^2 q_m^2 M_m
// The k^2 weighting matters: early on the strain lives in the HIGH modes,
// which die in tens of milliseconds, so the pitch settles far faster than the
// fundamental decays — the reference 12" tom falls 15 Hz in 100 ms while its
// (0,1) still has 1.9 s to live. Evaluated every control tick from the modal
// state (|z|^2 is the smooth envelope of a rotator, no 2f ripple), and
// physically quadratic in amplitude, so a soft hit hardly glides at all.
//
// Damping: sigma(f) = a + b (f/1 kHz)^c per head (radiation + internal), a
// kick pillow as an additional term linear in frequency, per-doublet-member
// radiation terms as above.
//
// Shell and cavity: a few weak high-Q shell modes (excited more by rim
// contact — the side stick is mostly shell) and, for the kick, the
// Helmholtz resonance of the ported cavity.
//
// State convention (see modal.rs): every uncoupled mode's z is the velocity
// of its modal coordinate q (drive F psi dt / M); a coupled normal mode's z
// is the velocity of its mass-normalised coordinate xi (drive v_b F psi dt /
// sqrt(M_b)); output, strain and wire-read gains are derived consistently
// from those definitions below, so the two kinds mix at physical levels.

use crate::contact::Striker;
use crate::modal::{Bank, MAX_MODES};
use crate::util::{bessel_j, bessel_zero, Rng};

/// sigma(f) = a + b (f / 1000 Hz)^c  (1/s). T60 = 6.91 / sigma.
#[derive(Clone, Copy)]
pub struct DampLaw {
    pub a: f64,
    pub b: f64,
    pub c: f64,
}

impl DampLaw {
    pub fn sigma(&self, f: f64) -> f64 {
        self.a + self.b * (f / 1000.0).powf(self.c)
    }
}

#[derive(Clone, Copy)]
pub struct ResoHead {
    /// Uncoupled (0,1) of the resonant head relative to the batter's.
    pub ratio: f64,
    pub sigma_area: f64,
    pub damp: DampLaw,
    /// How much of the resonant head's radiation the listening point sees,
    /// relative to the batter (it faces away from an overhead mic).
    pub radiate: f64,
    /// Fraction of the sealed-cavity air spring that survives the vent/port.
    pub air_spring: f64,
    /// Extra radiation damping (1/s) on the in-phase (monopole) member and
    /// on the anti-phase (dipole) member of each doublet.
    pub sym_sig: f64,
    pub asym_sig: f64,
    /// How many (0,n) pairs to couple.
    pub n_pairs: usize,
    /// Share of the wire impacts fed into this head's (> 800 Hz) modes
    /// (the rest is carried by the wire formant set).
    pub wire_inject: f64,
}

#[derive(Clone, Copy)]
pub struct MembraneDesign {
    pub radius: f64,
    pub depth: f64,
    /// Uncoupled batter (0,1) including air loading (Hz).
    pub f01: f64,
    pub sigma_area: f64,
    pub air_load: f64,
    pub n_modes: usize,
    pub damp: DampLaw,
    /// Pillow/muffling: extra sigma (1/s) at f01, growing linearly with f.
    pub muffle: f64,
    pub strike_r: f64,
    pub strike_jitter: f64,
    pub tension_gamma: f64,
    /// Radiation weight (f/f01)^out_slope, the multipole penalty for m >= 1
    /// modes (no net volume displacement), and the ring penalty n^-ring for
    /// (0, n >= 2) modes (their alternating rings cancel towards a listener
    /// above the head beyond what the net volume velocity says).
    pub out_slope: f64,
    pub multipole: f64,
    pub ring: f64,
    pub out_gain: f64,
    pub reso: Option<ResoHead>,
    /// (Hz, T60 s, gain) shell modes.
    pub shell: &'static [(f64, f64, f64)],
    /// Fraction of the stick force reaching the shell modes (the contact
    /// noise path), and the share of that which the smooth force itself
    /// drives linearly (the rest of a shell mode's excitation is the
    /// roughness, which grows faster than the force).
    pub shell_drive: f64,
    pub shell_linear: f64,
    /// Fraction reaching the membrane (a rim shot / side stick starves it).
    pub head_drive: f64,
    /// Helmholtz resonance of the cavity: (Hz, T60, gain).
    pub cavity: Option<(f64, f64, f64)>,
    /// Contact roughness: the striker's force carries a multiplicative
    /// broadband component (felt fibres, stick tip skidding, the head's
    /// local buckling) of this depth, active only while in contact. It
    /// drives the shell/high modes through their `inject` gains — the
    /// 2-8 kHz "click" every real drum hit has and a smooth force pulse
    /// cannot produce.
    pub contact_noise: f64,
    pub striker: Striker,
}

/// One built mode, everything but the strike-dependent gains.
#[derive(Clone, Copy)]
pub struct ProtoMode {
    pub f: f32,
    pub sig: f32,
    pub w: f32,
    pub fm: f32,
    /// Bessel order and zero for psi(r_s) at trigger; order == u32::MAX
    /// means "not a membrane shape" (fixed excitation `e_fixed`).
    pub order: u32,
    pub zero: f32,
    /// Strike->state gain, multiplied by psi(r_s) at trigger.
    pub e_drive: f32,
    pub e_fixed: f32,
    /// Output weight, wire read (gh, displacement of the snare-side head)
    /// and wire inject (gr) weights.
    pub out: f32,
    pub read: f32,
    pub inject: f32,
    /// Batter displacement under the striker per unit state (times psi at
    /// trigger): the two-way contact's read.
    pub disp: f32,
}

impl ProtoMode {
    pub const ZERO: Self = Self { f: 0.0, sig: 0.0, w: 0.0, fm: 0.0, order: 0, zero: 0.0, e_drive: 0.0, e_fixed: 0.0, out: 0.0, read: 0.0, inject: 0.0, disp: 0.0 };
}

#[derive(Clone, Copy)]
pub struct MembraneProto {
    pub n: usize,
    pub modes: [ProtoMode; MAX_MODES],
    pub strike_r: f32,
    pub strike_jitter: f32,
    pub tension_gamma: f32,
    pub head_drive: f32,
    pub out_gain: f32,
    pub contact_noise: f32,
    pub striker: Striker,
}

const RESERVE: usize = 24; // slots kept for shell / cavity / wire modes

impl MembraneProto {
    pub const fn empty() -> Self {
        Self {
            n: 0,
            modes: [ProtoMode::ZERO; MAX_MODES],
            strike_r: 0.0,
            strike_jitter: 0.0,
            tension_gamma: 0.0,
            head_drive: 1.0,
            out_gain: 0.0,
            contact_noise: 0.0,
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

    /// A fixed (non-membrane) mode. `drive`/`inject` are relative to the
    /// batter's (0,1): gain 1 means "driven as strongly per newton as the
    /// fundamental" (the mode's modal mass is taken as M_01 / gain).
    fn push_fixed(&mut self, f: f64, t60: f64, drive: f64, inject: f64, out: f64, fs: f32, inv_m01: f64) {
        if self.n < MAX_MODES && f < 0.45 * fs as f64 {
            let inv_fs = 1.0 / fs as f64;
            let pm = &mut self.modes[self.n];
            *pm = ProtoMode::ZERO;
            pm.f = f as f32;
            pm.sig = (6.9078 / t60) as f32;
            pm.order = u32::MAX;
            pm.e_fixed = (drive * inv_m01 * inv_fs) as f32;
            pm.out = out as f32;
            pm.inject = (inject * inv_m01 * inv_fs) as f32;
            self.n += 1;
        }
    }

    /// Builds the prototype at the sample rate (construction time only).
    /// `wires` are the snare-wire formant modes (Hz, T60, inject gain).
    pub fn build(d: &MembraneDesign, fs: f32, wires: &[(f64, f64, f64)]) -> Self {
        use core::f64::consts::{PI, TAU};
        let mut p = Self::empty();
        p.strike_r = d.strike_r as f32;
        p.strike_jitter = d.strike_jitter as f32;
        p.tension_gamma = d.tension_gamma as f32;
        p.head_drive = d.head_drive as f32;
        p.out_gain = d.out_gain as f32;
        p.contact_noise = d.contact_noise as f32;
        p.striker = d.striker;
        let a = d.radius;
        let j01 = bessel_zero(0, 1);
        let inv_fs = 1.0 / fs as f64;

        // Candidate (j, m, n) sorted by frequency.
        const M_MAX: usize = 14;
        const N_MAX: usize = 8;
        let mut cand: [(f64, u32, u32); (M_MAX + 1) * N_MAX] = [(0.0, 0, 0); (M_MAX + 1) * N_MAX];
        let mut nc = 0;
        for m in 0..=M_MAX {
            for n in 1..=N_MAX {
                cand[nc] = (bessel_zero(m, n), m as u32, n as u32);
                nc += 1;
            }
        }
        cand[..nc].sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());

        let air = |j: f64| (1.0 + d.air_load / j01) / (1.0 + d.air_load / j);
        let freq_of = |j: f64| d.f01 * (j / j01) * air(j).sqrt();
        let m01 = d.sigma_area * PI * a * a * bessel_j(1, j01).powi(2);
        let a1 = TAU * a * a * bessel_j(1, j01) / j01;
        let w01 = TAU * d.f01;
        let nyq = 0.45 * fs as f64;
        let budget = d.n_modes.min(MAX_MODES - RESERVE);
        for &(j, m, n) in &cand[..nc] {
            if p.n >= budget {
                break;
            }
            let f = freq_of(j);
            if f >= nyq {
                break;
            }
            let mass = d.sigma_area * PI * a * a * bessel_j(m as usize + 1, j).powi(2) * if m == 0 { 1.0 } else { 0.5 };
            let w = TAU * f;
            let sig = d.damp.sigma(f) + d.muffle * (f / d.f01);
            let k2 = (j / j01).powi(2);
            let radiation = (f / d.f01).powf(d.out_slope) * if m == 0 { (n as f64).powf(-d.ring) } else { 1.0 };
            let coupled = m == 0 && d.reso.map_or(false, |r| (n as usize) <= r.n_pairs);
            if coupled {
                let r = d.reso.unwrap();
                let area = TAU * a * a * bessel_j(1, j) / j;
                let f_r = f * r.ratio;
                let w_r = TAU * f_r;
                let mass_r = r.sigma_area * PI * a * a * bessel_j(1, j).powi(2);
                let volume = PI * a * a * d.depth;
                let k_air = r.air_spring * 1.2 * 343.0 * 343.0 / volume;
                let k11 = w * w + k_air * area * area / mass;
                let k22 = w_r * w_r + k_air * area * area / mass_r;
                let k12 = k_air * area * area / (mass * mass_r).sqrt();
                let tr = k11 + k22;
                let det = k11 * k22 - k12 * k12;
                let disc = (tr * tr * 0.25 - det).max(0.0).sqrt();
                for &l in &[tr * 0.5 - disc, tr * 0.5 + disc] {
                    if p.n >= budget {
                        break;
                    }
                    let (mut vb, mut vr) = if k12.abs() > 1e-9 { (k12, l - k11) } else if (l - k11).abs() < (l - k22).abs() { (1.0, 0.0) } else { (0.0, 1.0) };
                    let nrm = (vb * vb + vr * vr).sqrt();
                    vb /= nrm;
                    vr /= nrm;
                    if vb.abs() < 1e-4 {
                        continue; // not reachable from the batter
                    }
                    let f_k = l.max(1.0).sqrt() / TAU;
                    if f_k >= nyq {
                        continue;
                    }
                    let sig_b = d.damp.sigma(f_k) + d.muffle * (f_k / d.f01);
                    let sig_r = r.damp.sigma(f_k);
                    let monopole = (vb * vr) > 0.0;
                    let sig_k = vb * vb * sig_b + vr * vr * sig_r + if monopole { r.sym_sig } else { r.asym_sig };
                    let pm = &mut p.modes[p.n];
                    *pm = ProtoMode::ZERO;
                    pm.f = f_k as f32;
                    pm.sig = sig_k as f32;
                    // strain: k^2 q_b^2 M_b with q_b = vb xi / sqrt(M_b), |z| = w |xi|
                    pm.w = (k2 * (w01 / (TAU * f_k)).powi(2) * vb * vb / m01) as f32;
                    pm.fm = 1.0;
                    pm.order = 0;
                    pm.zero = j as f32;
                    pm.e_drive = (vb / mass.sqrt() * inv_fs) as f32;
                    pm.out = ((area / a1) * radiation * (vb / mass.sqrt() + r.radiate * vr / mass_r.sqrt())) as f32;
                    // batter displacement q_b = vb xi / sqrt(M_b), Im z = w xi
                    pm.disp = (vb / (mass.sqrt() * TAU * f_k)) as f32;
                    // The wires' gate reads the long-wavelength members only;
                    // the members the wires drive (> 800 Hz) are never read,
                    // so there is no rattle -> head -> rattle loop.
                    pm.read = if f_k < 800.0 { (vr / (TAU * f_k * mass_r.sqrt())) as f32 } else { 0.0 };
                    // The wire impacts are short taps: they excite the
                    // snare-side head's high modes (the formant set), not the
                    // long-wavelength members that gate them — injecting into
                    // those would close a rattle -> head -> rattle loop.
                    pm.inject = if f_k > 800.0 { (r.wire_inject * vr / mass_r.sqrt() * inv_fs) as f32 } else { 0.0 };
                    p.n += 1;
                }
            } else {
                let pm = &mut p.modes[p.n];
                *pm = ProtoMode::ZERO;
                pm.f = f as f32;
                pm.sig = sig as f32;
                pm.w = (k2 * (mass / m01) * (w01 / w).powi(2)) as f32;
                pm.fm = 1.0;
                pm.order = m;
                pm.zero = j as f32;
                pm.e_drive = (inv_fs / mass) as f32;
                pm.disp = (1.0 / w) as f32;
                // m >= 1 modes displace no net volume: their radiation is
                // multipole (weaker with every extra nodal diameter m and
                // nodal circle n; the design's `multipole` is the (1,1) value)
                pm.out = (radiation * if m == 0 { TAU * a * a * bessel_j(1, j) / j / a1 } else { d.multipole * (m as f64).powf(-0.7) * (n as f64).powf(-0.7) }) as f32;
                p.n += 1;
            }
        }
        let inv_m01 = 1.0 / m01;
        for &(f, t60, g) in d.shell {
            p.push_fixed(f, t60, g * d.shell_drive * d.shell_linear, g * d.shell_drive, 1.0, fs, inv_m01);
        }
        if let Some((f, t60, g)) = d.cavity {
            p.push_fixed(f, t60, g, 0.0, 1.0, fs, inv_m01);
        }
        for &(f, t60, g) in wires {
            p.push_fixed(f, t60, 0.0, g, 1.0, fs, inv_m01);
        }
        p
    }

    /// Loads the bank for one hit: strike-point weights, gains, base
    /// coefficients. Returns the impact speed (m/s).
    pub fn load(&self, bank: &mut Bank, fs: f32, velocity: f32, rng: &mut Rng) -> f64 {
        bank.clear();
        let r_s = (self.strike_r + self.strike_jitter * rng.bipolar()).clamp(0.0, 0.95);
        for i in 0..self.n {
            let m = &self.modes[i];
            bank.f[i] = m.f;
            bank.sig[i] = m.sig;
            bank.w[i] = m.w;
            bank.fm[i] = m.fm;
            if m.order == u32::MAX {
                bank.gi[i] = m.e_fixed;
                bank.gd[i] = 0.0;
            } else {
                let psi = bessel_j(m.order as usize, (m.zero * r_s) as f64) as f32;
                bank.gi[i] = psi * m.e_drive * self.head_drive;
                bank.gd[i] = psi * m.disp * self.head_drive;
            }
            bank.go[i] = m.out * self.out_gain;
            bank.gh[i] = m.read;
            bank.gr[i] = m.inject;
            bank.gs[i] = 0.0;
        }
        bank.finish(self.n, self.n, self.n);
        bank.rebuild(fs, 1.0, 0.0);
        self.striker.speed(velocity)
    }
}
