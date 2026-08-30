// One voice = one key = one unison group of strings. A voice owns the modal
// state of its 2-3 oscillators (strings / polarisations), its hammer, and
// its mechanical noises. Voices are permanently assigned to keys: a re-strike
// runs a fresh hammer into the still-ringing modal state, which is exactly
// what a real re-struck string does, and means there is no voice stealing to
// mistune.

use crate::hammer::Hammer;
use crate::keys::{velocity_to_speed, KeyDesign, PH_MODES};
use crate::modal::{run_modes, KernelPath, MAX_CHUNK};
use crate::params::Voicing;

/// Deterministic per-voice noise burst (hammer-action thump, damper felt
/// contact). One-pole-filtered xorshift noise under a half-sine envelope.
#[derive(Clone)]
pub struct NoiseBurst {
    pos: u32,
    len: u32,
    rng: u32,
    lp: f32,
    lp2: f32,
    lp_c: f32,
    amp: f32,
}

impl NoiseBurst {
    pub fn new() -> Self {
        Self { pos: 0, len: 0, rng: 1, lp: 0.0, lp2: 0.0, lp_c: 0.1, amp: 0.0 }
    }

    pub fn start(&mut self, len: u32, amp: f32, lp_c: f32, seed: u32) {
        self.pos = 0;
        self.len = len.max(1);
        self.rng = seed | 1;
        self.lp = 0.0;
        self.lp2 = 0.0;
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
            // two poles: measured key/action noises are band clusters; a
            // single pole leaks a -6 dB/oct spray into the top octaves
            // that reads as a synthetic "sharp" sheen on every strike
            self.lp += self.lp_c * (white - self.lp);
            self.lp2 += self.lp_c * (self.lp - self.lp2);
            let ph = self.pos as f32 / self.len as f32;
            let env = (core::f32::consts::PI * ph).sin();
            *slot += self.amp * env * self.lp2;
            self.pos += 1;
        }
    }
}

/// Commuted-style body-tap excitation: deterministic noise through a
/// lowpass whose bandwidth contracts exponentially over the burst, with a
/// decaying envelope — J.O. Smith's observation that a soundboard tap
/// sounds like noise through a contracting lowpass. Injected into the
/// STRING input (with the hammer force), so the string filters it the way
/// commuted synthesis plays the body response into the string.
#[derive(Clone)]
pub struct BodyTap {
    pos: u32,
    len: u32,
    rng: u32,
    lp: f32,
    lp2: f32,
    c_hi: f32,
    ratio: f32, // per-burst c decay: c(t) = c_hi * ratio^(pos/len) precomputed as per-sample factor
    c_cur: f32,
    amp: f32,
}

impl BodyTap {
    pub fn new() -> Self {
        Self { pos: 0, len: 0, rng: 1, lp: 0.0, lp2: 0.0, c_hi: 0.1, ratio: 1.0, c_cur: 0.0, amp: 0.0 }
    }

    pub fn start(&mut self, len: u32, amp: f32, c_hi: f32, c_lo: f32, seed: u32) {
        self.pos = 0;
        self.len = len.max(1);
        self.rng = seed | 1;
        self.lp = 0.0;
        self.lp2 = 0.0;
        self.c_hi = c_hi;
        self.c_cur = c_hi;
        // per-sample multiplicative contraction from c_hi to c_lo over len
        self.ratio = (c_lo.max(1e-6) / c_hi.max(1e-6)).powf(1.0 / self.len as f32);
        self.amp = amp;
    }

    #[inline]
    pub fn render_add(&mut self, out: &mut [f32], n: usize) {
        if self.pos >= self.len || self.amp == 0.0 {
            return;
        }
        let inv_len = 1.0 / self.len as f32;
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
            // two poles (see NoiseBurst): a body tap is a dark, contracting
            // rumble, not a bright noise spray
            self.lp += self.c_cur * (white - self.lp);
            self.lp2 += self.c_cur * (self.lp - self.lp2);
            self.c_cur *= self.ratio;
            let ph = self.pos as f32 * inv_len;
            let env = (1.0 - ph) * (1.0 - ph);
            *slot += self.amp * env * self.lp2;
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
    /// Bath-loading radius factor currently baked in (1.0 = none).
    pub extra_r: f32,
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
    /// Longitudinal/phantom bank state (see keys.rs): a small resonator
    /// bank at the string's longitudinal mode frequencies, driven by the
    /// high-passed SQUARE of this voice's bridge force — the tension-
    /// modulation mechanism that generates phantom partials at sums and
    /// differences of the transverse partials.
    ph_zr: [f32; PH_MODES],
    ph_zi: [f32; PH_MODES],
    ph_lp: f32,
    /// two-pole band-limit on the drive BEFORE squaring: squaring doubles
    /// bandwidth, so anything above fs/4 in the drive folds. The phantom
    /// parents are partials below ~4 kHz; content above contributes only
    /// aliases. Corner ~5.2 kHz.
    ph_pre1: f32,
    ph_pre2: f32,
    ph_buf: [f32; MAX_CHUNK],
    pub body_tap: BodyTap,
    knock_lp: f32,
    /// voicing amounts cached at note-on (a strike keeps the voicing it
    /// was played with; new strikes pick up slider moves)
    vc_knock: f32,
    vc_phantoms: f32,
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
            extra_r: 1.0,
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
            ph_zr: [0.0; PH_MODES],
            ph_zi: [0.0; PH_MODES],
            ph_lp: 0.0,
            ph_pre1: 0.0,
            ph_pre2: 0.0,
            ph_buf: [0.0; MAX_CHUNK],
            body_tap: BodyTap::new(),
            knock_lp: 0.0,
            vc_knock: 1.0,
            vc_phantoms: 1.0,
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
    /// because both share the mode angle. `extra_r` multiplies every mode
    /// radius on top: the bath-loading loss into open sympathetic strings
    /// (<= 1.0, so it can only add damping).
    pub fn rebuild(&mut self, key: &KeyDesign, eng: f32) {
        let extra = self.extra_r;
        self.rebuild_with(key, eng, extra)
    }

    pub fn rebuild_with(&mut self, key: &KeyDesign, eng: f32, extra_r: f32) {
        self.eng = eng;
        self.extra_r = extra_r;
        let x = extra_r.min(1.0);
        for m in 0..self.eff_cr.len() {
            let k = (1.0 + (key.damp_mul[m] - 1.0) * eng) * x;
            self.eff_cr[m] = key.cr_sus[m] * k;
            self.eff_ci[m] = key.ci_sus[m] * k;
        }
    }

    /// Strike this key. Modal state is kept (re-strike hits ringing strings).
    pub fn note_on(&mut self, key: &KeyDesign, vel: u8, soft_pedal: bool, sample_rate: f64, vc: &Voicing) {
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
        let rough = if soft_pedal { 0.6 } else { 1.0 }
            * vc.roughness
            * (key.rough_depth as f64 * (speed / 6.0)).min(0.5) as f32;
        let rough = rough.min(0.85);
        self.vc_knock = vc.knock;
        self.vc_phantoms = vc.phantoms;
        let rough_seed =
            (self.key_idx as u32).wrapping_mul(0x51ed_270b) ^ self.strike_count.wrapping_mul(0x9e37_79b9) ^ 0x5bd1;
        let u_lock = key.felt_u_lock * (1.0 / k_scale).powf(1.0 / (key.felt_p + 1.0));
        self.osc_gain = if soft_pedal && key.n_osc == 3 {
            [0.9, 0.9, key.uc_third]
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
            key.img_fc_mul,
            key.img_g_base,
            key.img_g_slope,
        );
        // The measured attack noise of a piano is not one broadband click:
        // it is (a) a sub-100 Hz thump the key-bottom impact pumps into the
        // board, and (b) a cluster of key/action bar resonances around
        // 290-900 Hz re-excited at escapement and key bottom (Askenfelt &
        // Jansson's transient studies). Both grow steeply with velocity and
        // both are what a plucked string does NOT have.
        let amp = vc.attack_noise * if soft_pedal { 0.7 } else { 1.0 } * key.thump_amp * self.vel_norm.powf(key.thump_vpow);
        let seed = (self.key_idx as u32).wrapping_mul(0x9e37_79b9) ^ self.strike_count.wrapping_mul(0x85eb_ca6b);
        self.thump.start(key.thump_len, amp, key.thump_lp_c, seed);
        // Key/action resonance burst: darker and longer than a click — a
        // ~9 ms noise burst low-passed near the top of the measured
        // key-resonance cluster.
        if key.cs_amp != 0.0 && vc.body_tap != 0.0 {
            let tamp = vc.body_tap * key.cs_amp * ((speed / 6.0) as f32).powf(key.cs_vpow)
                * if soft_pedal { 0.6 } else { 1.0 };
            let tseed = (self.key_idx as u32).wrapping_mul(0x2545_f491)
                ^ self.strike_count.wrapping_mul(0x9e37_79b9) ^ 0x0b0d_15ea;
            self.body_tap.start(key.cs_len, tamp, key.cs_c_hi, key.cs_c_lo, tseed);
        }
        let camp = vc.attack_noise * if soft_pedal { 0.5 } else { 1.0 } * key.click_amp * self.vel_norm.powf(key.click_vpow);
        let cseed = seed ^ 0x00c0_ffee;
        let clp = if soft_pedal { key.click_lp_c * 0.68 } else { key.click_lp_c };
        self.click.start(key.click_len, camp, clp, cseed);
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
        self.body_tap.render_add(&mut self.force, n);
        if key.knock_amp != 0.0 && self.vc_knock != 0.0 && (has_force || self.knock_lp.abs() > 1e-9) {
            // high-passed copy of the blow, straight into the board bus
            let ka = key.knock_amp * self.vc_knock;
            for k in 0..n {
                let f = self.force[k];
                self.knock_lp += key.knock_hp_c * (f - self.knock_lp);
                self.noise_buf[k] += ka * (f - self.knock_lp);
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
        if key.ph_gain != 0.0 && self.vc_phantoms != 0.0 {
            // squared bridge force, high-passed: the tension-modulation
            // drive. Feedforward only (drive is read before phantoms are
            // added), so no loop exists anywhere.
            for k in 0..n {
                let s = self.acc[k] * key.ph_drive;
                self.ph_pre1 += key.ph_pre_c * (s - self.ph_pre1);
                self.ph_pre2 += key.ph_pre_c * (self.ph_pre1 - self.ph_pre2);
                let sq = self.ph_pre2 * self.ph_pre2;
                self.ph_lp += key.ph_hp_c * (sq - self.ph_lp);
                self.ph_buf[k] = sq - self.ph_lp;
            }
            run_modes(
                path,
                &mut self.ph_zr,
                &mut self.ph_zi,
                &key.ph_cr,
                &key.ph_ci,
                &key.ph_gin,
                &key.ph_gout,
                &self.ph_buf[..n],
                key.ph_gain * self.vc_phantoms,
                &mut self.acc[..n],
            );
            if key.ph_direct != 0.0 {
                for k in 0..n {
                    self.acc[k] += key.ph_direct * self.ph_buf[k];
                }
            }
        }
        self.thump.render_add(&mut self.noise_buf, n);
        self.click.render_add(&mut self.noise_buf, n);
        self.damper_noise.render_add(&mut self.noise_buf, n);
    }

    pub fn silence(&mut self) {
        self.active = false;
        self.zr.fill(0.0);
        self.zi.fill(0.0);
        self.ph_zr.fill(0.0);
        self.ph_zi.fill(0.0);
        self.ph_lp = 0.0;
        self.ph_pre1 = 0.0;
        self.ph_pre2 = 0.0;
        self.knock_lp = 0.0;
        self.power = 0.0;
        self.quiet_ticks = 0;
        self.hammer.active = false;
    }
}
