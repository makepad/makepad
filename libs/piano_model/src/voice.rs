// One voice = one key = one unison group of strings. A voice owns the modal
// state of its 2-3 oscillators (strings / polarisations), its hammer, and
// its mechanical noises. Voices are permanently assigned to keys: a re-strike
// runs a fresh hammer into the still-ringing modal state, which is exactly
// what a real re-struck string does, and means there is no voice stealing to
// mistune.

use crate::hammer::Hammer;
use crate::keys::{velocity_to_speed, KeyDesign};
use crate::modal::{run_modes, KernelPath, MAX_CHUNK};

/// Deterministic per-voice noise burst (hammer-action thump, damper felt
/// contact). One-pole-filtered xorshift noise under a half-sine envelope.
#[derive(Clone)]
pub struct NoiseBurst {
    pos: u32,
    len: u32,
    rng: u32,
    lp: f32,
    lp_c: f32,
    amp: f32,
}

impl NoiseBurst {
    pub fn new() -> Self {
        Self { pos: 0, len: 0, rng: 1, lp: 0.0, lp_c: 0.1, amp: 0.0 }
    }

    pub fn start(&mut self, len: u32, amp: f32, lp_c: f32, seed: u32) {
        self.pos = 0;
        self.len = len.max(1);
        self.rng = seed | 1;
        self.lp = 0.0;
        self.lp_c = lp_c;
        self.amp = amp;
    }

    #[inline]
    pub fn render_add(&mut self, out: &mut [f32], n: usize) {
        if self.pos >= self.len {
            return;
        }
        for slot in out.iter_mut().take(n) {
            if self.pos >= self.len {
                break;
            }
            let mut x = self.rng;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.rng = x;
            let white = (x >> 8) as f32 * (1.0 / 8_388_608.0) - 1.0;
            self.lp += self.lp_c * (white - self.lp);
            let ph = self.pos as f32 / self.len as f32;
            let env = (core::f32::consts::PI * ph).sin();
            *slot += self.amp * env * self.lp;
            self.pos += 1;
        }
    }
}

pub struct Voice {
    pub key_idx: usize,
    pub active: bool,
    pub held: bool,
    pub sost_held: bool,
    /// Damper engagement currently baked into the effective rotations
    /// (0 = damper off the string, 1 = fully seated).
    pub eng: f32,
    pub zr: Vec<f32>,
    pub zi: Vec<f32>,
    pub eff_cr: Vec<f32>,
    pub eff_ci: Vec<f32>,
    pub acc: [f32; MAX_CHUNK],
    pub noise_buf: [f32; MAX_CHUNK],
    force: [f32; MAX_CHUNK],
    pub power: f32,
    pub quiet_ticks: u32,
    pub hammer: Hammer,
    pub osc_gain: [f32; 3],
    pub thump: NoiseBurst,
    pub click: NoiseBurst,
    pub damper_noise: NoiseBurst,
    pub strike_count: u32,
    pub vel_norm: f32,
    /// One-pole lowpass on the hammer force for una-corda strikes: the
    /// un-grooved felt crown meets the string over a wider patch, which
    /// filters the excitation the way a wider strike distribution does.
    uc_lp: f32,
    uc_lp_c: f32,
}

impl Voice {
    pub fn new(key_idx: usize, key: &KeyDesign) -> Self {
        let n = key.total_modes;
        let mut v = Self {
            key_idx,
            active: false,
            held: false,
            sost_held: false,
            eng: 1.0,
            zr: vec![0.0; n],
            zi: vec![0.0; n],
            eff_cr: vec![0.0; n],
            eff_ci: vec![0.0; n],
            acc: [0.0; MAX_CHUNK],
            noise_buf: [0.0; MAX_CHUNK],
            force: [0.0; MAX_CHUNK],
            power: 0.0,
            quiet_ticks: 0,
            hammer: Hammer::new(),
            osc_gain: [1.0; 3],
            thump: NoiseBurst::new(),
            click: NoiseBurst::new(),
            damper_noise: NoiseBurst::new(),
            strike_count: 0,
            vel_norm: 0.0,
            uc_lp: 0.0,
            uc_lp_c: 0.0,
        };
        v.rebuild(key, 1.0);
        v
    }

    /// Bake damper engagement into the effective rotations. Lerping (cr,ci)
    /// between sustain and damped rotations is exact radius interpolation
    /// because both share the mode angle.
    pub fn rebuild(&mut self, key: &KeyDesign, eng: f32) {
        self.eng = eng;
        for m in 0..self.eff_cr.len() {
            let k = 1.0 + (key.damp_mul[m] - 1.0) * eng;
            self.eff_cr[m] = key.cr_sus[m] * k;
            self.eff_ci[m] = key.ci_sus[m] * k;
        }
    }

    /// Strike this key. Modal state is kept (re-strike hits ringing strings).
    pub fn note_on(&mut self, key: &KeyDesign, vel: u8, soft_pedal: bool, sample_rate: f64) {
        self.active = true;
        self.held = true;
        self.strike_count = self.strike_count.wrapping_add(1);
        self.quiet_ticks = 0;
        self.vel_norm = vel.min(127) as f32 / 127.0;
        // Una corda: the action shifts so the hammer misses one string of a
        // triple and meets the rest on softer, less-grooved felt. The softer
        // felt also compresses further before locking up, so the lock-up
        // threshold shifts with it (u ~ K^(-1/(p+1)) at equal energy).
        let k_scale: f64 = if soft_pedal { 0.4 } else { 1.0 };
        self.uc_lp = 0.0;
        self.uc_lp_c = if soft_pedal {
            let fc = (3.5 * key.f0).clamp(500.0, 3200.0);
            1.0 - (-core::f32::consts::TAU * fc / sample_rate as f32).exp()
        } else {
            0.0
        };
        // The shifted hammer meets one string fewer on a triple, so it works
        // against a lower wave impedance: the string yields more and the
        // contact lengthens — part of the una-corda mellowing.
        let z_scale: f64 = if soft_pedal && key.n_osc == 3 { 2.0 / 3.0 } else { 1.0 };
        let speed = velocity_to_speed(vel);
        // Contact roughness depth grows with hammer speed (ff pulses are
        // chopped by returning ripples; pp pulses are clean and dark). The
        // una-corda shift onto soft unworn felt smooths the contact too.
        let rough = if soft_pedal { 0.6 } else { 1.0 } * (0.35 * (speed / 6.0)).min(0.5) as f32;
        let rough_seed =
            (self.key_idx as u32).wrapping_mul(0x51ed_270b) ^ self.strike_count.wrapping_mul(0x9e37_79b9) ^ 0x5bd1;
        let u_lock = key.felt_u_lock * (1.0 / k_scale).powf(1.0 / (key.felt_p + 1.0));
        self.osc_gain = if soft_pedal && key.n_osc == 3 {
            [0.9, 0.9, 0.0]
        } else if soft_pedal {
            [0.9, 0.9, 0.9]
        } else {
            [1.0, 1.0, 1.0]
        };
        self.hammer.strike(
            speed,
            key.hammer_mass,
            key.felt_k,
            key.felt_p,
            u_lock,
            // Fresh un-compacted felt barely locks up: most of the una-corda
            // darkening at forte comes from losing that stiffening.
            key.felt_lock_w * if soft_pedal { 0.2 } else { 1.0 },
            // fresh felt is also less hysteretic: smoother, rounder pulse
            key.felt_lambda * if soft_pedal { 0.3 } else { 1.0 },
            key.z_total * z_scale,
            key.t1_seconds,
            sample_rate,
            k_scale,
            rough,
            rough_seed,
        );
        // The measured attack noise of a piano is not one broadband click:
        // it is (a) a sub-100 Hz thump the key-bottom impact pumps into the
        // board, and (b) a cluster of key/action bar resonances around
        // 290-900 Hz re-excited at escapement and key bottom (Askenfelt &
        // Jansson's transient studies). Both grow steeply with velocity and
        // both are what a plucked string does NOT have.
        let amp = if soft_pedal { 0.7 } else { 1.0 } * 3.2 * self.vel_norm * self.vel_norm;
        let seed = (self.key_idx as u32).wrapping_mul(0x9e37_79b9) ^ self.strike_count.wrapping_mul(0x85eb_ca6b);
        let lp_c = 1.0 - (-core::f32::consts::TAU * 130.0 / sample_rate as f32).exp();
        self.thump.start((0.006 * sample_rate) as u32, amp, lp_c, seed);
        // Key/action resonance burst: darker and longer than a click — a
        // ~9 ms noise burst low-passed near the top of the measured
        // key-resonance cluster.
        let camp = if soft_pedal { 0.5 } else { 1.0 } * 3.0 * self.vel_norm * self.vel_norm * self.vel_norm;
        let cseed = seed ^ 0x00c0_ffee;
        let click_hz = if soft_pedal { 650.0 } else { 950.0 };
        let clp = 1.0 - (-core::f32::consts::TAU * click_hz / sample_rate as f32).exp();
        self.click.start((0.009 * sample_rate) as u32, camp, clp, cseed);
    }

    /// Render `n` samples of bridge force into self.acc and mechanical noise
    /// into self.noise_buf (both overwritten).
    pub fn render(&mut self, key: &KeyDesign, path: KernelPath, n: usize) {
        debug_assert!(n <= MAX_CHUNK);
        for k in 0..n {
            self.acc[k] = 0.0;
            self.noise_buf[k] = 0.0;
        }
        let has_force = if self.hammer.active {
            self.hammer.render_force(&mut self.force, n)
        } else {
            false
        };
        if !has_force {
            for k in 0..n {
                self.force[k] = 0.0;
            }
        }
        if self.uc_lp_c > 0.0 && (has_force || self.uc_lp.abs() > 1e-9) {
            for k in 0..n {
                self.uc_lp += self.uc_lp_c * (self.force[k] - self.uc_lp);
                self.force[k] = self.uc_lp;
            }
        }
        let mp = key.modes_padded;
        for osc in 0..key.n_osc {
            let a = osc * mp;
            let b = a + mp;
            run_modes(
                path,
                &mut self.zr[a..b],
                &mut self.zi[a..b],
                &self.eff_cr[a..b],
                &self.eff_ci[a..b],
                &key.gin[a..b],
                &key.gout[a..b],
                &self.force[..n],
                self.osc_gain[osc],
                &mut self.acc[..n],
            );
        }
        self.thump.render_add(&mut self.noise_buf, n);
        self.click.render_add(&mut self.noise_buf, n);
        self.damper_noise.render_add(&mut self.noise_buf, n);
    }

    pub fn silence(&mut self) {
        self.active = false;
        self.zr.fill(0.0);
        self.zi.fill(0.0);
        self.power = 0.0;
        self.quiet_ticks = 0;
        self.hammer.active = false;
    }
}
