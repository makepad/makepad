// One voice = one key = one unison group of strings. A voice owns the modal
// state of its 2-3 oscillators (strings / polarisations), its hammer, and
// its mechanical noises. Voices are permanently assigned to keys: a re-strike
// runs a fresh hammer into the still-ringing modal state, which is exactly
// what a real re-struck string does, and means there is no voice stealing to
// mistune.

use crate::calibration::{CalibrationNote, CALIBRATION_PARTIALS};
use crate::hammer::Hammer;
use crate::keys::{velocity_to_speed, KeyDesign, PH_MODES, PH_PARENTS};

/// Row stride of the parent-slope scratch: parents plus a zero tail wide
/// enough for the 4-wide read at m + 5 from the last parent.
const PH_Q_STRIDE: usize = PH_PARENTS + PH_MODES;
use crate::modal::{run_modes_c, KernelPath, MAX_CHUNK};
use crate::simd::{fma_v4, hsum_v4, load_v4, mul_v4, splat_v4, store_v4, sub_v4, zero_v4};
use crate::params::Voicing;

/// Deterministic per-voice noise burst (hammer-action thump, damper felt
/// contact). One-pole-filtered xorshift noise under a half-sine envelope.
#[derive(Clone)]
pub struct NoiseBurst {
    pos: u32,
    len: u32,
    /// samples still to wait before the burst begins (see `start`)
    delay: u32,
    rng: u32,
    lp: f32,
    lp2: f32,
    lp_c: f32,
    amp: f32,
}

impl NoiseBurst {
    pub fn new() -> Self {
        Self { pos: 0, len: 0, delay: 0, rng: 1, lp: 0.0, lp2: 0.0, lp_c: 0.1, amp: 0.0 }
    }

    /// Arms a burst of `len` samples that begins `delay` samples from now.
    /// The delay is consumed sample by sample inside `render_add`, so it
    /// is exact to the sample and independent of host block boundaries.
    pub fn start(&mut self, len: u32, amp: f32, lp_c: f32, seed: u32, delay: u32) {
        self.pos = 0;
        self.len = len.max(1);
        self.delay = delay;
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
        let mut skip = 0usize;
        if self.delay > 0 {
            skip = (self.delay as usize).min(n);
            self.delay -= skip as u32;
        }
        for slot in out.iter_mut().take(n).skip(skip) {
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
    att: u32, // attack ramp length (samples)
    rng: u32,
    lp: f32,
    lp2: f32,
    lp3: f32,
    lp4: f32,
    c_hi: f32,
    c_tilt: f32,
    ratio: f32, // per-burst c decay: c(t) = c_hi * ratio^(pos/len) precomputed as per-sample factor
    c_cur: f32,
    amp: f32,
}

impl BodyTap {
    pub fn new() -> Self {
        Self {
            pos: 0,
            len: 0,
            att: 1,
            rng: 1,
            lp: 0.0,
            lp2: 0.0,
            lp3: 0.0,
            lp4: 0.0,
            c_hi: 0.1,
            c_tilt: 0.1,
            ratio: 1.0,
            c_cur: 0.0,
            amp: 0.0,
        }
    }

    pub fn start(&mut self, len: u32, att: u32, amp: f32, c_hi: f32, c_lo: f32, c_tilt: f32, seed: u32) {
        self.pos = 0;
        self.len = len.max(1);
        self.att = att.clamp(1, self.len);
        self.rng = seed | 1;
        self.lp = 0.0;
        self.lp2 = 0.0;
        self.lp3 = 0.0;
        self.lp4 = 0.0;
        self.c_hi = c_hi;
        self.c_tilt = c_tilt;
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
        let inv_att = 1.0 / self.att as f32;
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
            // One FIXED pole at the tilt corner (~1.2 kHz) plus three
            // cascaded poles at the contracting corner. The fixed pole
            // gives the burst the falling spectrum of a real board tap
            // (-6 dB/oct above ~1 kHz); the three contracting poles put a
            // -18 dB/oct lid above cs_hi. Flat noise out to 5 kHz on every
            // strike — the earlier shape — measured as sample steps 25x the
            // programme median in dense music and was heard as radio
            // static/rasp twice. Passband below the tilt corner unchanged.
            self.lp += self.c_tilt * (white - self.lp);
            self.lp2 += self.c_cur * (self.lp - self.lp2);
            self.lp3 += self.c_cur * (self.lp2 - self.lp3);
            self.lp4 += self.c_cur * (self.lp3 - self.lp4);
            self.c_cur *= self.ratio;
            let ph = self.pos as f32 * inv_len;
            // The board's diffuse response BUILDS over a couple of
            // milliseconds (a plate mode rings up over ~1/sigma); a
            // half-cosine attack ramp replaces the old step-to-maximum
            // envelope, which put a discontinuity at the front of every
            // strike.
            let rise = if self.pos < self.att {
                let a = self.pos as f32 * inv_att;
                0.5 - 0.5 * (core::f32::consts::PI * a).cos()
            } else {
                1.0
            };
            let env = (1.0 - ph) * (1.0 - ph) * rise;
            *slot += self.amp * env * self.lp4;
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
    /// Immutable pitch-interpolated table, owned by this key's voice.
    pub(crate) calibration: Option<CalibrationNote>,
    /// Excitation only: changing velocity must never rescale ringing state
    /// or its output residues. Allocated once, rebuilt only at note-on.
    eff_gin: Vec<f32>,
    pub acc: [f32; MAX_CHUNK],
    pub noise_buf: [f32; MAX_CHUNK],
    /// Structure-borne case noise (key-bottom thump, action click, damper
    /// felt): radiated by the CASE, not driven through the bridge into the
    /// soundboard resonators. When these went into the board bus, the
    /// restored (livelier) low-mid board rang a 10 ms key thump for 300 ms
    /// — "hammer noise way over represented". The knock stays bridge-borne
    /// (it IS the string's compression precursor into the bridge).
    pub case_buf: [f32; MAX_CHUNK],
    force: [f32; MAX_CHUNK],
    pub power: f32,
    /// loudest control-tick power since the last strike (the relative
    /// silence gate in lib.rs)
    pub peak_power: f32,
    pub quiet_ticks: u32,
    pub hammer: Hammer,
    pub osc_gain: [f32; 3],
    pub thump: NoiseBurst,
    pub click: NoiseBurst,
    pub damper_noise: NoiseBurst,
    /// Longitudinal/phantom bank state (see keys.rs): the string's
    /// longitudinal modes, each driven by its own sum of transverse modal
    /// products.
    ph_zr: [f32; PH_MODES],
    ph_zi: [f32; PH_MODES],
    /// Own copy of the parent partials' vertical modal states: advanced by
    /// the same recursion as the kernel but never re-synchronised from it,
    /// so the products do not depend on where chunk boundaries fall (the
    /// kernels round differently per backend; a per-chunk resync made the
    /// output differ in the last bit between host block sizes).
    ph_mr: [f32; PH_PARENTS],
    ph_mi: [f32; PH_PARENTS],
    /// per-sample slope amplitudes of the parent partials this chunk,
    /// sample-major with a zero tail (the product sums read past the last
    /// parent in 4-wide steps)
    ph_q: [[f32; PH_Q_STRIDE]; MAX_CHUNK],
    /// per-sample drive of each longitudinal mode this chunk
    ph_d: [[f32; PH_MODES]; MAX_CHUNK],
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
    pub fn new(key_idx: usize, key: &KeyDesign, calibration: Option<CalibrationNote>) -> Self {
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
            calibration,
            eff_gin: key.gin.clone(),
            acc: [0.0; MAX_CHUNK],
            noise_buf: [0.0; MAX_CHUNK],
            case_buf: [0.0; MAX_CHUNK],
            force: [0.0; MAX_CHUNK],
            power: 0.0,
            peak_power: 0.0,
            quiet_ticks: 0,
            hammer: Hammer::new(),
            osc_gain: [1.0; 3],
            thump: NoiseBurst::new(),
            click: NoiseBurst::new(),
            damper_noise: NoiseBurst::new(),
            ph_zr: [0.0; PH_MODES],
            ph_zi: [0.0; PH_MODES],
            ph_mr: [0.0; PH_PARENTS],
            ph_mi: [0.0; PH_PARENTS],
            ph_q: [[0.0; PH_Q_STRIDE]; MAX_CHUNK],
            ph_d: [[0.0; PH_MODES]; MAX_CHUNK],
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
    pub fn note_on(&mut self, key: &KeyDesign, vel: u8, soft_pedal: bool, sample_rate: f64, vc: &Voicing) {
        if let Some(note) = &self.calibration {
            for m in 0..key.modes_per_osc.min(CALIBRATION_PARTIALS + 15) {
                let db = note.gain_at(m, vel);
                let gain = 10.0f32.powf(db / 20.0);
                for osc in 0..key.n_osc {
                    let i = osc * key.modes_padded + m;
                    self.eff_gin[i] = key.gin[i] * gain;
                }
            }
        }
        self.active = true;
        self.held = true;
        self.strike_count = self.strike_count.wrapping_add(1);
        self.quiet_ticks = 0;
        self.peak_power = 0.0;
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
        // The raw velocity curve, then the key's treble speed-range
        // compression about the mezzo-forte pivot (see keys.rs speed_q):
        // the top octaves' dynamic span is the narrowest on a real
        // instrument, not the widest.
        let speed = velocity_to_speed(vel);
        let speed = key.speed_pivot * (speed / key.speed_pivot).powf(key.speed_q);
        // Per-STRIKE variation (deterministic: seeded by key and strike
        // count, so renders stay bit-identical for identical event
        // streams). The per-KEY voicing scatter makes the 88 keys
        // individuals, but every strike of one key was the same strike:
        // on flat-velocity material (an engraving export with every note
        // at velocity 90) repeated notes came back near-identical and
        // read as looped samples. A real action never repeats itself —
        // felt condition, strike point and seating vary a little every
        // blow. A few percent on the contact parameters varies timbre,
        // not loudness.
        let sj = {
            let mut x = (self.key_idx as u32)
                .wrapping_mul(0x9e37_79b9)
                .wrapping_add(self.strike_count.wrapping_mul(0x85eb_ca6b))
                ^ 0x2545_f491;
            let mut f = move || {
                x ^= x >> 16;
                x = x.wrapping_mul(0x7feb_352d);
                x ^= x >> 15;
                x = x.wrapping_mul(0x846c_a68b);
                x ^= x >> 16;
                (x >> 8) as f64 * (2.0 / 16_777_216.0) - 1.0
            };
            [f(), f(), f(), f()]
        };
        let speed = speed * (1.0 + 0.025 * sj[0]);
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
            key.felt_k * (1.0 + 0.08 * sj[1]),
            key.felt_p,
            u_lock * (1.0 + 0.03 * sj[2]),
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
            key.core_k * (1.0 + 0.25 * sj[3]) * if soft_pedal { 0.4 } else { 1.0 },
            key.u_core,
        );
        // The measured attack noise of a piano is not one broadband click:
        // it is (a) a sub-100 Hz thump the key-bottom impact pumps into the
        // board, and (b) a cluster of key/action bar resonances around
        // 290-900 Hz re-excited at escapement and key bottom (Askenfelt &
        // Jansson's transient studies). Both grow steeply with velocity and
        // both are what a plucked string does NOT have.
        let amp = vc.attack_noise * if soft_pedal { 0.7 } else { 1.0 } * key.thump_amp * self.vel_norm.powf(key.thump_vpow);
        let seed = (self.key_idx as u32).wrapping_mul(0x9e37_79b9) ^ self.strike_count.wrapping_mul(0x85eb_ca6b);
        // Key-bottom lag. The hammer leaves the jack ~1-2 mm before the
        // string, and the key still has its aftertouch (~1 mm) to travel
        // at roughly 1/5.5 of the hammer speed, so the key bottoms — and
        // the thump and the action-rail resonances it pumps happen —
        // 1-10 ms AFTER the string is struck: nearly coincident at
        // forte, well behind at piano (Askenfelt & Jansson, "From touch
        // to string vibrations"; Goebl, Bresin & Galembo 2005 measure the
        // hammer-string to key-bottom interval at a few ms). Firing them
        // at t = 0 put a case-radiated click 10-15 dB above the
        // recordings in the first 5 ms of every bass note, before the
        // string's own wave had even reached the bridge (the modal sum
        // is wave-exact: A0's bridge force starts 16 ms after the strike,
        // C2's after 7 ms) — a pick-like precursor the real instrument
        // does not have (Salamander bass onsets sit at -33..-40 dB re
        // peak at 2 ms).
        let key_lag = ((0.0066 / speed).clamp(0.0010, 0.012) * sample_rate) as u32;
        self.thump.start(key.thump_len, amp, key.thump_lp_c, seed, key_lag);
        // Key/action resonance burst: darker and longer than a click — a
        // ~9 ms noise burst low-passed near the top of the measured
        // key-resonance cluster.
        if key.cs_amp != 0.0 && vc.body_tap != 0.0 {
            let tamp = vc.body_tap * key.cs_amp * ((speed / 6.0) as f32).powf(key.cs_vpow)
                * if soft_pedal { 0.6 } else { 1.0 };
            let tseed = (self.key_idx as u32).wrapping_mul(0x2545_f491)
                ^ self.strike_count.wrapping_mul(0x9e37_79b9) ^ 0x0b0d_15ea;
            let att = (0.0015 * sample_rate) as u32;
            self.body_tap.start(key.cs_len, att, tamp, key.cs_c_hi, key.cs_c_lo, key.cs_c_tilt, tseed);
        }
        let camp = vc.attack_noise * if soft_pedal { 0.5 } else { 1.0 } * key.click_amp * self.vel_norm.powf(key.click_vpow);
        let cseed = seed ^ 0x00c0_ffee;
        let clp = if soft_pedal { key.click_lp_c * 0.68 } else { key.click_lp_c };
        // the action-rail resonance cluster is re-excited at key bottom
        // (Askenfelt & Jansson), so it shares the key-bottom lag
        self.click.start(key.click_len, camp, clp, cseed, key_lag);
    }

    /// Render `n` samples of bridge force into self.acc and mechanical noise
    /// into self.noise_buf (both overwritten).
    pub fn render(&mut self, key: &KeyDesign, path: KernelPath, n: usize) {
        debug_assert!(n <= MAX_CHUNK);
        for k in 0..n {
            self.acc[k] = 0.0;
            self.noise_buf[k] = 0.0;
            self.case_buf[k] = 0.0;
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
        // Raw instruments read the original table directly, preserving exact
        // identity and the existing diagnostic shaping path.
        let gin = if self.calibration.is_some() { &self.eff_gin } else { &key.gin };
        let phantoms = key.ph_gain != 0.0 && self.vc_phantoms != 0.0;
        if phantoms {
            // Per-sample vertical modal states of the parent partials (the
            // voice's own copy, see ph_mr), scaled to slope amplitude; four
            // parents per lane group like the kernels (a scalar recursion
            // per parent is latency-bound and cost more than the products).
            // Lanes past ph_parents carry zero slope weight.
            let g = self.osc_gain[0];
            let mut m = 0;
            while m < key.ph_parents {
                let crv = load_v4(&self.eff_cr[m..]);
                let civ = load_v4(&self.eff_ci[m..]);
                let ginv = mul_v4(load_v4(&gin[m..]), splat_v4(g));
                let wv = mul_v4(load_v4(&key.ph_slope[m..]), splat_v4(key.ph_drive));
                let mut zr = load_v4(&self.ph_mr[m..]);
                let mut zi = load_v4(&self.ph_mi[m..]);
                for k in 0..n {
                    let f = splat_v4(self.force[k]);
                    let t = fma_v4(ginv, f, sub_v4(mul_v4(crv, zr), mul_v4(civ, zi)));
                    zi = fma_v4(civ, zr, mul_v4(crv, zi));
                    zr = t;
                    store_v4(&mut self.ph_q[k][m..], mul_v4(wv, zi));
                }
                store_v4(&mut self.ph_mr[m..], zr);
                store_v4(&mut self.ph_mi[m..], zi);
                m += 4;
            }
        }
        for osc in 0..key.n_osc {
            let a = osc * mp;
            let b = a + mp;
            run_modes_c(
                path,
                &mut self.zr[a..b],
                &mut self.zi[a..b],
                &self.eff_cr[a..b],
                &self.eff_ci[a..b],
                &gin[a..b],
                &key.gout[a..b],
                &key.gout_re[a..b],
                &self.force[..n],
                self.osc_gain[osc],
                &mut self.acc[..n],
            );
        }
        if phantoms {
            // Longitudinal mode j is driven by the j-th spatial component
            // of d/dx (y_x)^2: j * (sum_m s_m s_(m+j) + 1/2 sum_(m+n=j) s_m s_n)
            // with s_m the slope amplitude of partial m (keys.rs), i.e. by
            // the products at f_m + f_(m+j) and f_(m+j) - f_m — the phantom
            // series, resonantly amplified near j times the longitudinal
            // fundamental. Feedforward only: no loop exists anywhere.
            let np = key.ph_parents;
            for k in 0..n {
                let q = &self.ph_q[k];
                // difference pairs, all eight modes at once: parent m
                // meets parents m+1..m+8 (the zero tail covers the end)
                let (mut lo, mut hi) = (zero_v4(), zero_v4());
                for m in 0..np {
                    let qm = splat_v4(q[m]);
                    lo = fma_v4(qm, load_v4(&q[m + 1..]), lo);
                    hi = fma_v4(qm, load_v4(&q[m + 5..]), hi);
                }
                let d = &mut self.ph_d[k];
                store_v4(&mut d[..4], lo);
                store_v4(&mut d[4..], hi);
                // sum pairs: m + n = j (1-based), each unordered pair once
                for j in 2..=PH_MODES {
                    for m in 1..=j / 2 {
                        let pair = q[m - 1] * q[j - m - 1];
                        d[j - 1] += if 2 * m == j { 0.5 * pair } else { pair };
                    }
                }
            }
            // the eight longitudinal resonators, two lane groups
            let out = splat_v4(key.ph_gain * self.vc_phantoms);
            let (cr_a, cr_b) = (load_v4(&key.ph_cr[..4]), load_v4(&key.ph_cr[4..]));
            let (ci_a, ci_b) = (load_v4(&key.ph_ci[..4]), load_v4(&key.ph_ci[4..]));
            let (gi_a, gi_b) = (load_v4(&key.ph_gin[..4]), load_v4(&key.ph_gin[4..]));
            let (go_a, go_b) = (mul_v4(load_v4(&key.ph_gout[..4]), out), mul_v4(load_v4(&key.ph_gout[4..]), out));
            let (mut zr_a, mut zr_b) = (load_v4(&self.ph_zr[..4]), load_v4(&self.ph_zr[4..]));
            let (mut zi_a, mut zi_b) = (load_v4(&self.ph_zi[..4]), load_v4(&self.ph_zi[4..]));
            for k in 0..n {
                let d = &self.ph_d[k];
                let t_a = fma_v4(gi_a, load_v4(&d[..4]), sub_v4(mul_v4(cr_a, zr_a), mul_v4(ci_a, zi_a)));
                let t_b = fma_v4(gi_b, load_v4(&d[4..]), sub_v4(mul_v4(cr_b, zr_b), mul_v4(ci_b, zi_b)));
                zi_a = fma_v4(ci_a, zr_a, mul_v4(cr_a, zi_a));
                zi_b = fma_v4(ci_b, zr_b, mul_v4(cr_b, zi_b));
                zr_a = t_a;
                zr_b = t_b;
                self.acc[k] += hsum_v4(fma_v4(go_b, zi_b, mul_v4(go_a, zi_a)));
            }
            store_v4(&mut self.ph_zr[..4], zr_a);
            store_v4(&mut self.ph_zr[4..], zr_b);
            store_v4(&mut self.ph_zi[..4], zi_a);
            store_v4(&mut self.ph_zi[4..], zi_b);
        }
        self.thump.render_add(&mut self.case_buf, n);
        self.click.render_add(&mut self.case_buf, n);
        self.damper_noise.render_add(&mut self.case_buf, n);
    }

    pub fn silence(&mut self) {
        self.active = false;
        self.zr.fill(0.0);
        self.zi.fill(0.0);
        self.ph_zr.fill(0.0);
        self.ph_zi.fill(0.0);
        self.ph_mr.fill(0.0);
        self.ph_mi.fill(0.0);
        self.knock_lp = 0.0;
        self.power = 0.0;
        self.peak_power = 0.0;
        self.quiet_ticks = 0;
        self.hammer.active = false;
    }
}

#[cfg(test)]
mod calibration_tests {
    use super::*;
    use crate::{keys::build_key, DesignParams};

    fn note(key: u8) -> CalibrationNote {
        let mut note = CalibrationNote {
            key,
            gain_db: [[-12.0; CALIBRATION_PARTIALS], [0.0; CALIBRATION_PARTIALS], [12.0; CALIBRATION_PARTIALS]],
            decay_scale: [1.0; CALIBRATION_PARTIALS],
        };
        for gains in &mut note.gain_db {
            gains[7] -= 3.0;
            gains[CALIBRATION_PARTIALS - 1] -= 5.0;
        }
        note
    }

    #[test]
    fn excitation_interpolates_velocity_and_partial_tail_for_every_oscillator() {
        for midi in [21, 36, 60, 108] {
            let key = build_key(midi, 48000.0, &DesignParams::default());
            let mut voice = Voice::new((midi - 21) as usize, &key, Some(note(midi)));
            // Endpoints, exact knots and halfway between knots, including
            // velocities outside MIDI's range (the existing event path allows them).
            for (vel, db) in [(1, -12.0), (28, -12.0), (48, -6.0), (68, 0.0), (90, 6.0), (112, 12.0), (127, 12.0 + 12.0 * (15.0 / 44.0)), (255, 12.0 + 12.0 * (15.0 / 44.0))] {
                voice.note_on(&key, vel, false, 48000.0, &Voicing::default());
                for osc in 0..key.n_osc {
                    for m in 0..key.modes_padded {
                        let i = osc * key.modes_padded + m;
                        let weight = if m < CALIBRATION_PARTIALS { 1.0 }
                            else { (CALIBRATION_PARTIALS + 15).saturating_sub(m) as f32 / 16.0 };
                        let offset = if m == 7 { -3.0 } else if m >= CALIBRATION_PARTIALS - 1 { -5.0 } else { 0.0 };
                        let expected = key.gin[i] * 10.0f32.powf((db + offset) * weight / 20.0);
                        assert_eq!(voice.eff_gin[i].to_bits(), expected.to_bits(), "key={midi}, velocity={vel}, mode={i}");
                    }
                }
            }
        }
    }

    #[test]
    fn every_a0_mode_receives_its_own_excitation_and_decay_correction() {
        let raw = build_key(21, 48000.0, &DesignParams::default());
        assert!(raw.modes_per_osc > 200 && raw.modes_per_osc <= CALIBRATION_PARTIALS);
        let mut calibration = note(21);
        for m in 0..CALIBRATION_PARTIALS {
            for v in 0..3 {
                calibration.gain_db[v][m] = -1.0 - m as f32 / 16.0 - v as f32;
            }
            calibration.decay_scale[m] = 0.5 + m as f32 / 128.0;
        }
        let mut key = build_key(21, 48000.0, &DesignParams::default());
        calibration.apply_decay(&mut key);
        let mut voice = Voice::new(0, &key, Some(calibration.clone()));
        for (v, vel) in crate::calibration::CALIBRATION_VELOCITIES.into_iter().enumerate() {
            voice.note_on(&key, vel, false, 48000.0, &Voicing::default());
            for osc in 0..key.n_osc {
                for m in 0..key.modes_per_osc {
                    let i = osc * key.modes_padded + m;
                    assert_ne!(raw.gin[i], 0.0, "A0 mode {i} must be active");
                    let expected = raw.gin[i] * 10.0f32.powf(calibration.gain_db[v][m] / 20.0);
                    assert_eq!(voice.eff_gin[i].to_bits(), expected.to_bits(), "velocity={vel}, mode={i}");
                    assert_ne!(voice.eff_gin[i].to_bits(), raw.gin[i].to_bits());
                    let radius = (raw.cr_sus[i] as f64).hypot(raw.ci_sus[i] as f64);
                    let corrected = (key.cr_sus[i] as f64).hypot(key.ci_sus[i] as f64);
                    assert!(radius > 0.0);
                    assert!((corrected - radius.powf(calibration.decay_scale[m] as f64)).abs() < 8e-8,
                        "A0 mode {i} must use its own decay correction");
                    if calibration.decay_scale[m] != 1.0 {
                        assert_ne!((key.cr_sus[i], key.ci_sus[i]), (raw.cr_sus[i], raw.ci_sus[i]));
                    }
                }
            }
        }
    }

    #[test]
    fn restrike_gain_update_leaves_zero_input_ringing_unchanged() {
        let key = build_key(36, 48000.0, &DesignParams::default());
        for path in [KernelPath::Scalar, KernelPath::Simd4] {
            let mut control = Voice::new(15, &key, Some(note(36)));
            let mut restruck = Voice::new(15, &key, Some(note(36)));
            for voice in [&mut control, &mut restruck] {
                voice.note_on(&key, 28, false, 48000.0, &Voicing::default());
                voice.rebuild(&key, 0.0);
                for _ in 0..64 {
                    voice.render(&key, path, MAX_CHUNK);
                }
            }
            let before_r = restruck.zr.clone();
            let before_i = restruck.zi.clone();
            let before_gin = restruck.eff_gin.clone();
            restruck.note_on(&key, 112, false, 48000.0, &Voicing::default());
            assert_ne!(restruck.eff_gin, before_gin);
            assert_eq!(restruck.zr, before_r);
            assert_eq!(restruck.zi, before_i);
            assert!(before_r.iter().any(|x| x.abs() > 0.0));
            // Isolate the already-ringing strings by removing fresh excitation.
            // Both voices retain the same phantom/case histories; acc contains
            // the string output whose residues must not change at the restrike.
            for voice in [&mut control, &mut restruck] {
                voice.hammer.active = false;
                voice.body_tap.amp = 0.0;
            }
            for _ in 0..8 {
                control.render(&key, path, MAX_CHUNK);
                restruck.render(&key, path, MAX_CHUNK);
                assert_eq!(control.acc, restruck.acc);
                assert_eq!(control.zr, restruck.zr);
                assert_eq!(control.zi, restruck.zi);
            }
        }
    }
}
