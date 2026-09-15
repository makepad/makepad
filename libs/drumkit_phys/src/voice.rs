// One sounding hit: a modal bank plus its exciter and its nonlinear
// controllers, rendered sample by sample. All control decisions (tension
// modulation, amplitude damping, lifetime) happen on the voice's own
// absolute 32-sample grid counted from its trigger, never on host-buffer
// boundaries, so a trigger stream renders bit-identically for any block
// decomposition.

use crate::contact::Contact;
use crate::cymbal::CymbalProto;
use crate::design;
use crate::membrane::MembraneProto;
use crate::modal::Bank;
use crate::rattle::{Clap, Rattle};
use crate::util::Rng;
use crate::DrumVoice;

/// Control tick (samples).
pub const TICK: u32 = 32;

#[derive(Clone, Copy, PartialEq)]
enum Mech {
    Membrane,
    Cymbal,
    Clap,
}

#[derive(Clone, Copy)]
pub struct Voice {
    pub active: bool,
    pub kind: DrumVoice,
    pub serial: u64,
    age: u32,
    mech: Mech,
    bank: Bank,
    contact: Contact,
    rattle: Rattle,
    has_rattle: bool,
    clap: Clap,
    /// Membrane: tension gamma. Cymbal: amplitude-damping coefficient.
    gamma: f32,
    /// Cymbal cascade active (tier cubes feed gr/gs).
    cascade: bool,
    /// Cymbal continuum gain (cascade drive straight to the output) and the
    /// previous continuum sample (its output is the first difference: the
    /// un-modelled modes' forced velocity radiates as acceleration, the
    /// same rising-with-frequency weight the modelled partials carry).
    direct_nl: f32,
    direct_prev: f32,
    last_factor: f32,
    x0: f32,
    x1: f32,
    /// Output power accumulated over the current tick, its ~30 ms smoothed
    /// value (a 43 Hz kick is 35 ticks per period) and the smoothed peak.
    pow_acc: f32,
    pow_lp: f32,
    peak_e: f32,
    /// Contact roughness depth (membranes) and its noise stream.
    contact_noise: f32,
    noise: Rng,
    /// Radiation cut-off: a third-order high-pass below the lowest mode
    /// (nothing radiates below a plate's or head's first mode; without it
    /// the modal sum passes the force pulse itself, a sub-modal thump).
    hp_a: f32,
    hp_lp: f32,
    hp_lp2: f32,
    hp_lp3: f32,
    pan_l: f32,
    pan_r: f32,
    fs: f32,
}

/// All prototypes, built once at `DrumKit::new`.
pub struct Protos {
    pub kick: MembraneProto,
    pub snare: MembraneProto,
    pub sidestick: MembraneProto,
    pub tom_high: MembraneProto,
    pub tom_mid: MembraneProto,
    pub tom_low: MembraneProto,
    pub tom_floor: MembraneProto,
    pub hat_closed: CymbalProto,
    pub hat_open: CymbalProto,
    pub hat_pedal: CymbalProto,
    pub ride: CymbalProto,
    pub ride_bell: CymbalProto,
    pub crash: CymbalProto,
    pub clap: MembraneProto,
}

impl Protos {
    pub fn build(fs: f32) -> Self {
        // The clap is "a membrane with no membrane": only fixed body modes
        // driven by the burst generator through gr.
        let clap_design = crate::membrane::MembraneDesign { n_modes: 0, head_drive: 0.0, shell: &[], reso: None, cavity: None, out_gain: design::CLAP_OUT_GAIN as f64, ..design::SNARE };
        Self {
            kick: MembraneProto::build(&design::KICK, fs, &[]),
            snare: MembraneProto::build(&design::SNARE, fs, design::SNARE_WIRES),
            sidestick: MembraneProto::build(&design::SIDESTICK, fs, design::SNARE_WIRES),
            tom_high: MembraneProto::build(&design::TOM_HIGH, fs, &[]),
            tom_mid: MembraneProto::build(&design::TOM_MID, fs, &[]),
            tom_low: MembraneProto::build(&design::TOM_LOW, fs, &[]),
            tom_floor: MembraneProto::build(&design::TOM_FLOOR, fs, &[]),
            hat_closed: CymbalProto::build(&design::HAT_CLOSED, fs),
            hat_open: CymbalProto::build(&design::HAT_OPEN, fs),
            hat_pedal: CymbalProto::build(&design::HAT_PEDAL_D, fs),
            ride: CymbalProto::build(&design::RIDE, fs),
            ride_bell: CymbalProto::build(&design::RIDE_BELL, fs),
            crash: CymbalProto::build(&design::CRASH, fs),
            clap: MembraneProto::build(&clap_design, fs, design::CLAP_BODY),
        }
    }
}

#[inline(always)]
fn cube(x: f32) -> f32 {
    // x^3 for small x, ~x for large: the drive never exceeds the source
    let x2 = x * x;
    x * x2 / (1.0 + x2)
}

impl Voice {
    pub const fn idle() -> Self {
        Self {
            active: false,
            kind: DrumVoice::Kick,
            serial: 0,
            age: 0,
            mech: Mech::Membrane,
            bank: Bank::new(),
            contact: Contact::idle(),
            rattle: Rattle::idle(),
            has_rattle: false,
            clap: Clap::idle(),
            gamma: 0.0,
            cascade: false,
            direct_nl: 0.0,
            direct_prev: 0.0,
            last_factor: 1.0,
            x0: 0.0,
            x1: 0.0,
            pow_acc: 0.0,
            pow_lp: 0.0,
            peak_e: 0.0,
            contact_noise: 0.0,
            noise: Rng(1),
            hp_a: 1.0,
            hp_lp: 0.0,
            hp_lp2: 0.0,
            hp_lp3: 0.0,
            pan_l: core::f32::consts::FRAC_1_SQRT_2,
            pan_r: core::f32::consts::FRAC_1_SQRT_2,
            fs: 48000.0,
        }
    }

    pub fn start(&mut self, kind: DrumVoice, velocity: f32, serial: u64, protos: &Protos, fs: f32) {
        let seed = 0x9e37_79b9u32 ^ (kind.index()).wrapping_mul(0x85eb_ca6b) ^ (serial as u32).wrapping_mul(0x2c1b_3c6d) ^ ((serial >> 32) as u32);
        let mut rng = Rng::new(seed);
        self.active = true;
        self.kind = kind;
        self.serial = serial;
        self.age = 0;
        self.fs = fs;
        self.x0 = 0.0;
        self.x1 = 0.0;
        self.pow_acc = 0.0;
        self.pow_lp = 0.0;
        self.peak_e = 0.0;
        self.last_factor = 1.0;
        self.has_rattle = false;
        self.cascade = false;
        self.direct_nl = 0.0;
        self.direct_prev = 0.0;
        self.gamma = 0.0;
        self.contact = Contact::idle();
        self.rattle = Rattle::idle();
        self.clap = Clap::idle();
        self.contact_noise = 0.0;
        self.noise = Rng::new(rng.next_u32());
        self.hp_lp = 0.0;
        self.hp_lp2 = 0.0;
        self.hp_lp3 = 0.0;
        let pan = design::PAN[kind.index() as usize];
        let angle = (pan + 1.0) * core::f32::consts::PI * 0.25;
        self.pan_l = angle.cos() * design::MASTER;
        self.pan_r = angle.sin() * design::MASTER;

        match kind {
            DrumVoice::Kick => self.start_membrane(&protos.kick, None, velocity, fs, &mut rng),
            DrumVoice::Snare => self.start_membrane(&protos.snare, Some(&design::SNARE_RATTLE), velocity, fs, &mut rng),
            DrumVoice::SideStick => self.start_membrane(&protos.sidestick, Some(&design::SNARE_RATTLE), velocity, fs, &mut rng),
            DrumVoice::TomHigh => self.start_membrane(&protos.tom_high, None, velocity, fs, &mut rng),
            DrumVoice::TomMid => self.start_membrane(&protos.tom_mid, None, velocity, fs, &mut rng),
            DrumVoice::TomLow => self.start_membrane(&protos.tom_low, None, velocity, fs, &mut rng),
            DrumVoice::TomFloor => self.start_membrane(&protos.tom_floor, None, velocity, fs, &mut rng),
            DrumVoice::HiHatClosed => self.start_cymbal(&protos.hat_closed, Some(&design::HAT_CHATTER), velocity, fs, &mut rng),
            DrumVoice::HiHatOpen => self.start_cymbal(&protos.hat_open, None, velocity, fs, &mut rng),
            DrumVoice::HiHatPedal => self.start_cymbal(&protos.hat_pedal, Some(&design::PEDAL_CHATTER), velocity, fs, &mut rng),
            DrumVoice::Ride => self.start_cymbal(&protos.ride, None, velocity, fs, &mut rng),
            DrumVoice::RideBell => self.start_cymbal(&protos.ride_bell, None, velocity, fs, &mut rng),
            DrumVoice::Crash => self.start_cymbal(&protos.crash, None, velocity, fs, &mut rng),
            DrumVoice::Clap => {
                self.mech = Mech::Clap;
                protos.clap.load(&mut self.bank, fs, velocity, &mut rng);
                // harder claps open the upper formants (flatter palms, more
                // of the impact spectrum survives the cupped hands)
                for i in 0..self.bank.n {
                    if self.bank.f[i] > 2500.0 {
                        self.bank.gr[i] *= 0.3 + 0.7 * velocity;
                    }
                }
                self.clap.start(&design::CLAP, fs, velocity, rng.next_u32());
                self.set_radiation_cutoff(0.6, fs);
            }
        }
    }

    pub(crate) fn start_membrane(&mut self, p: &MembraneProto, rattle: Option<&crate::rattle::RattleDesign>, velocity: f32, fs: f32, rng: &mut Rng) {
        self.mech = Mech::Membrane;
        let v0 = p.load(&mut self.bank, fs, velocity, rng);
        self.contact.strike(&p.striker, v0, fs);
        self.gamma = p.tension_gamma;
        self.contact_noise = p.contact_noise;
        self.set_radiation_cutoff(0.75, fs);
        if let Some(r) = rattle {
            self.rattle.start(r, fs, rng.next_u32());
            self.has_rattle = true;
        }
    }

    pub(crate) fn start_cymbal(&mut self, p: &CymbalProto, rattle: Option<&crate::rattle::RattleDesign>, velocity: f32, fs: f32, rng: &mut Rng) {
        self.mech = Mech::Cymbal;
        let v0 = p.load(&mut self.bank, fs, velocity);
        self.contact.strike(&p.striker, v0, fs);
        self.gamma = p.nl_damp;
        self.cascade = p.eps1 > 0.0 || p.eps2 > 0.0;
        self.direct_nl = p.direct_nl * (48000.0 / fs);
        self.contact_noise = p.contact_noise;
        self.set_radiation_cutoff(0.7, fs);
        if let Some(r) = rattle {
            // chatter uses gr: give every mode its inject weight
            for i in 0..self.bank.n {
                self.bank.gr[i] = p.modes[i].inject;
                self.bank.gs[i] = 0.0;
            }
            self.rattle.start(r, fs, rng.next_u32());
            self.has_rattle = true;
        }
    }

    fn set_radiation_cutoff(&mut self, ratio: f32, fs: f32) {
        let mut f_min = f32::MAX;
        for m in 0..self.bank.n {
            if self.bank.f[m] > 0.0 {
                f_min = f_min.min(self.bank.f[m]);
            }
        }
        let fc = if f_min == f32::MAX { 20.0 } else { ratio * f_min };
        self.hp_a = 1.0 - (-core::f32::consts::TAU * fc / fs).exp();
    }

    /// Renders and ADDS this voice into `out`.
    #[inline]
    pub fn render(&mut self, out: &mut [[f32; 2]]) {
        for frame in out.iter_mut() {
            let s = self.step();
            frame[0] += s * self.pan_l;
            frame[1] += s * self.pan_r;
            if !self.active {
                break;
            }
        }
    }

    #[inline(always)]
    fn step(&mut self) -> f32 {
        let y_bank = if self.mech == Mech::Membrane && self.contact.active { self.bank.strike_point_displacement() as f64 } else { 0.0 };
        let mut f = self.contact.step(y_bank);
        let mut direct = 0.0f32;
        let (r, s) = match self.mech {
            Mech::Membrane => {
                let mut r = if self.has_rattle { self.rattle.step(self.x0) } else { 0.0 };
                if f > 0.0 && self.contact_noise > 0.0 {
                    // roughness depth grows with compression (felt/dimple
                    // stiffening): harder hits click more than they thump
                    r += self.contact_noise * f * (f * 0.05).sqrt() * self.noise.bipolar();
                }
                (r, 0.0)
            }
            Mech::Cymbal => {
                if f > 0.0 && self.contact_noise > 0.0 {
                    f += self.contact_noise * (f * 20.0).sqrt() * self.noise.bipolar();
                }
                if self.has_rattle {
                    (self.rattle.step(self.x0), 0.0)
                } else if self.cascade {
                    let (c0, c1) = (cube(self.x0), cube(self.x1));
                    // the continuum is mostly the mid tier's products (the
                    // low tier's land on the modelled partials themselves)
                    let d = 0.15 * c0 + c1;
                    direct = self.direct_nl * (d - self.direct_prev);
                    self.direct_prev = d;
                    (c0, c1)
                } else {
                    (0.0, 0.0)
                }
            }
            Mech::Clap => (self.clap.step(), 0.0),
        };
        let sums = self.bank.step(f, r, s);
        self.x0 = sums.x0;
        self.x1 = sums.x1;
        self.age = self.age.wrapping_add(1);
        if self.age % TICK == 0 {
            self.tick();
        }
        // third-order radiation cut-off (three one-poles, 18 dB/oct)
        let out = sums.out + direct;
        self.hp_lp += self.hp_a * (out - self.hp_lp);
        let y = out - self.hp_lp;
        self.hp_lp2 += self.hp_a * (y - self.hp_lp2);
        let y = y - self.hp_lp2;
        self.hp_lp3 += self.hp_a * (y - self.hp_lp3);
        let y = y - self.hp_lp3;
        self.pow_acc += y * y;
        y
    }

    fn tick(&mut self) {
        match self.mech {
            Mech::Membrane => {
                if self.gamma > 0.0 {
                    let s = self.bank.weighted_energy();
                    let factor = (1.0 + self.gamma * s).sqrt();
                    if (factor - self.last_factor).abs() > 2.0e-5 {
                        self.bank.rebuild(self.fs, factor, 0.0);
                        self.last_factor = factor;
                    }
                }
            }
            Mech::Cymbal => {
                if self.gamma > 0.0 {
                    // amplitude-dependent damping of the lower tiers: the
                    // energy the cascade takes out of them
                    let e = self.bank.tier_energy();
                    let extra = self.gamma * e;
                    let factor = extra; // reuse last_factor as last extra
                    if (factor - self.last_factor).abs() > 0.02 * (1.0 + factor) {
                        self.bank.rebuild(self.fs, 1.0, extra);
                        self.last_factor = factor;
                    }
                }
            }
            Mech::Clap => {}
        }
        // Lifetime: gone when the output power of the last tick has fallen
        // 66 dB under the loudest tick (and the exciters are quiet), or under
        // an absolute floor. A ride's mid partials have T60 ~12 s; the voice
        // is freed once it is inaudible under anything else, not when it is
        // mathematically gone.
        self.pow_lp += 0.03 * (self.pow_acc - self.pow_lp);
        self.pow_acc = 0.0;
        let e = self.pow_lp;
        if e > self.peak_e {
            self.peak_e = e;
        }
        let quiet = !self.contact.active && (self.mech != Mech::Clap || self.age > (0.45 * self.fs) as u32);
        if quiet && self.age > (0.05 * self.fs) as u32 && (e < self.peak_e * 2.5e-7 || e < 1.0e-14) {
            self.active = false;
        }
        if self.age > (20.0 * self.fs) as u32 {
            self.active = false;
        }
    }
}

#[cfg(test)]
mod diag {
    // Internal-scale diagnostics for calibrating the nonlinear controllers
    // (ignored by default):
    //   cargo test -p makepad-drumkit-phys --release --lib diag -- --ignored --nocapture
    use super::*;

    #[test]
    #[ignore]
    fn controller_scales() {
        let fs = 48000.0;
        let protos = Protos::build(fs);
        for kind in DrumVoice::ALL {
            for vel in [0.3f32, 1.0] {
                let mut v = Voice::idle();
                v.start(kind, vel, 1, &protos, fs);
                let mut max_s = 0.0f32;
                let mut max_x0 = 0.0f32;
                let mut max_x1 = 0.0f32;
                let mut contact_samples = 0u32;
                let mut max_factor = 1.0f32;
                let mut n = 0u32;
                while v.active && n < (4.0 * fs) as u32 {
                    let f_before = v.contact.active;
                    let _ = v.step();
                    if f_before {
                        contact_samples += 1;
                    }
                    max_x0 = max_x0.max(v.x0.abs());
                    max_x1 = max_x1.max(v.x1.abs());
                    if n % 8 == 0 {
                        let s = v.bank.weighted_energy();
                        max_s = max_s.max(s);
                        max_factor = max_factor.max(v.last_factor);
                    }
                    n += 1;
                }
                println!(
                    "{kind:?} v{vel}: contact {:.2} ms, max S {:.3e}, max factor {:.3}, max |x0| {:.3e}, max |x1| {:.3e}, life {:.2} s, modes {} (t1 {} t2 {})",
                    contact_samples as f32 / fs * 1000.0,
                    max_s,
                    max_factor,
                    max_x0,
                    max_x1,
                    n as f32 / fs,
                    v.bank.n,
                    v.bank.t1,
                    v.bank.t2
                );
            }
        }
    }
}

#[cfg(test)]
mod diag_modes {
    // Per-mode energy share of a hit: which partials carry the sound
    // (ignored by default):
    //   cargo test -p makepad-drumkit-phys --release --lib diag_modes -- --ignored --nocapture
    use super::*;

    #[test]
    #[ignore]
    fn mode_energy_table() {
        let fs = 48000.0;
        let protos = Protos::build(fs);
        for kind in [DrumVoice::TomHigh, DrumVoice::Snare, DrumVoice::Kick, DrumVoice::TomLow] {
            let mut v = Voice::idle();
            v.start(kind, 1.0, 1, &protos, fs);
            let n = v.bank.n;
            let mut e = vec![0.0f64; n];
            let mut e_late = vec![0.0f64; n];
            for k in 0..(0.4 * fs) as usize {
                let _ = v.step();
                for m in 0..n {
                    let o = (v.bank.go[m] * v.bank.zr[m]) as f64;
                    e[m] += o * o;
                    if k > (0.05 * fs) as usize {
                        e_late[m] += o * o;
                    }
                }
            }
            let tot: f64 = e.iter().sum();
            let mut idx: Vec<usize> = (0..n).collect();
            idx.sort_by(|a, b| e[*b].partial_cmp(&e[*a]).unwrap());
            println!("== {kind:?}: top modes by energy in 0-400 ms (f Hz, T60 s, share dB, late share dB, gi, go)");
            for &m in idx.iter().take(14) {
                println!(
                    "  {:8.1} Hz  T60 {:5.2}  {:6.1} dB  late {:6.1} dB  gi {:9.2e}  go {:8.3}  gd {:8.2e}",
                    v.bank.f[m],
                    6.9078 / v.bank.sig[m],
                    10.0 * (e[m] / tot).log10(),
                    10.0 * (e_late[m] / tot).log10(),
                    v.bank.gi[m],
                    v.bank.go[m],
                    v.bank.gd[m]
                );
            }
        }
    }
}
