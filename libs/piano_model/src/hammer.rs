// Nonlinear felt hammer against the string, the single most important
// nonlinearity in a piano: a hard blow is not a louder soft blow, because the
// felt stiffens under compression and the contact time shortens, pushing the
// force-pulse spectrum upward.
//
// Model (Hunt-Crossley contact + method-of-images string reaction):
//
//   felt compression      u = x_h - y_s
//   felt force            F = K u^p (1 + w (u/u_lock)^5) (1 + lambda du/dt)
//                         clamped to [0, F_MAX]
//   The w (u/u_lock)^5 factor is felt lock-up: past normal playing
//   compressions the felt pad compacts and stiffens dramatically, which is
//   what makes a fortissimo pulse sharply peaked (bright) while leaving
//   pianissimo pulses smooth (dark). u_lock sits just below the estimated
//   mezzo-forte compression, the weight w is graded across the compass
//   (thick bass felt locks hard, thin treble felt is near-compacted
//   already), and both the lock factor and the loading/unloading
//   (1 + lambda du/dt) modulation are clamped: compacted felt is hard but
//   finite, and the du/dt linearisation is invalid at the tens-of-m/s
//   strain rates the agraffe-image chatter produces in the treble (letting
//   either run free used to slam the force into the F_MAX safety clamp,
//   which flatlined fortissimo dynamics up there).
//
//   On top of the integrated pulse, the emitted force carries a small
//   multiplicative roughness (deterministic per-strike noise, depth rising
//   with hammer speed, active only during felt contact). A perfectly smooth
//   symmetric pulse has deep spectral sidelobe nulls that no measured
//   hammer force shows — real pulses are chopped by returning string
//   ripples and felt-fibre micro-slip. Without it the rendered treble lost
//   its upper partials entirely (C7's 2nd partial sat 37 dB under its 1st;
//   measured force spectra put them within a few dB).
//   hammer                m_h x_h'' = -F
//   string point          y_s' = (F(t) - S{F}(t - T1)) / (2 Z n_s)
//
// The string reaction is the method of images for a string that is
// semi-infinite on the bridge side and terminated at the agraffe a distance
// x0 away (one image source of opposite sign; T1 = 2 x0 / c). The delayed
// term is what throws the string back against the hammer and produces the
// multiple contacts of real bass notes, and the F/(2Z) term is the wave
// impedance the felt works against. S{} is a one-pole smoothing plus a
// slight per-trip attenuation on the returning wave: stiffness dispersion
// and losses over the doubled x0 segment mean successive ripples come back
// progressively rounded and weakened (published force histories show
// decaying secondary humps). Replaying the ring verbatim instead produced
// an equal-amplitude T1 comb whose null at 1/(2 T1) (~1.15 kHz at C4, on
// partial 4) hollowed out fortissimo mid spectra. Bounded as before: F is
// clamped and finite, and everything downstream is a contraction.
//
// The ODE is integrated with symplectic Euler at 4x the audio rate (the felt
// stiffness at fortissimo puts the contact resonance around 1-2 kHz; at
// 192 kHz substeps the scheme has an enormous stability margin, and a hard
// F_MAX clamp plus a contact timeout are belt-and-braces on top). The audio-
// rate output force is the mean of the substeps (a free anti-alias filter).
//
// All hammer state is f64: the felt power law works with compressions of
// 1e-4 m raised to powers up to ~3.2, which is where f32 runs out of grace.

/// Substeps per audio sample for the contact integration.
pub const NSUB: usize = 4;
/// Force history ring (substep rate). Longest T1 in the design (A0,
/// x0/L = 0.132, L = 1.9 m, c = 103 m/s) is ~4.9 ms = 940 substeps @ 48 kHz.
/// At 96 kHz audio rate T1 doubles in substeps, hence 4096.
pub const RING: usize = 4096;
/// Hard force clamp (N). Real fortissimo peaks are ~10-60 N; this is a
/// safety net that guarantees boundedness, not a musical limiter.
pub const F_MAX: f64 = 5000.0;

#[derive(Clone)]
pub struct Hammer {
    pub active: bool,
    started: bool,       // felt has touched at least once
    x: f64,              // hammer position relative to string rest line (m)
    v: f64,              // hammer velocity, +toward string (m/s)
    y: f64,              // string displacement at the contact point (m)
    v_str: f64,          // last string point velocity (for du/dt)
    ring: Box<[f32]>,    // outgoing force history at substep rate
    rpos: usize,
    delay: usize,        // T1 in substeps
    inv_2z: f64,         // 1 / (2 * Z * n_strings)
    k_felt: f64,
    p_felt: f64,
    inv_ulock: f64,
    lock_w: f64,
    lock_cap: f64,
    lambda: f64,
    rough: f32,      // contact roughness depth (0..~0.5), scales with speed
    rough_lp: f32,
    rough_rng: u32,
    inv_m: f64,
    dt: f64,             // substep dt
    steps: u32,
    timeout: u32,
    img_lp: f64,         // smoothing state for the agraffe return
    img_c: f64,          // one-pole coefficient for that smoothing
    img_g: f64,          // per-round-trip amplitude survival
}

impl Hammer {
    pub fn new() -> Self {
        Self {
            active: false,
            started: false,
            x: 0.0,
            v: 0.0,
            y: 0.0,
            v_str: 0.0,
            ring: vec![0.0f32; RING].into_boxed_slice(),
            rpos: 0,
            delay: 1,
            inv_2z: 0.0,
            k_felt: 0.0,
            p_felt: 2.5,
            inv_ulock: 0.0,
            lock_w: 1.0,
            lock_cap: 14.0,
            lambda: 0.0,
            rough: 0.0,
            rough_lp: 0.0,
            rough_rng: 1,
            inv_m: 0.0,
            dt: 0.0,
            steps: 0,
            timeout: 0,
            img_lp: 0.0,
            img_c: 1.0,
            img_g: 1.0,
        }
    }

    /// Launch the hammer at the string. Called at note-on, at the exact event
    /// sample. `k_scale` < 1 models the una-corda shift onto softer felt.
    pub fn strike(
        &mut self,
        velocity_ms: f64,
        mass: f64,
        k_felt: f64,
        p_felt: f64,
        u_lock: f64,
        lock_w: f64,
        lambda: f64,
        z_total: f64,
        t1_seconds: f64,
        sample_rate: f64,
        k_scale: f64,
        rough: f32,
        rough_seed: u32,
    ) {
        self.active = true;
        self.started = false;
        self.x = 0.0;
        self.v = velocity_ms.max(0.01);
        self.y = 0.0;
        self.v_str = 0.0;
        self.ring.fill(0.0);
        self.rpos = 0;
        self.dt = 1.0 / (sample_rate * NSUB as f64);
        self.delay = ((t1_seconds / self.dt) as usize).clamp(1, RING - 1);
        self.inv_2z = 1.0 / (2.0 * z_total);
        self.k_felt = k_felt * k_scale;
        self.p_felt = p_felt;
        self.inv_ulock = 1.0 / u_lock.max(1e-6);
        self.lock_w = lock_w;
        self.lock_cap = 2.2 * lock_w;
        self.lambda = lambda;
        self.rough = rough;
        self.rough_lp = 0.0;
        self.rough_rng = rough_seed | 1;
        self.inv_m = 1.0 / mass;
        self.steps = 0;
        // The wave that returns from the agraffe is not the outgoing force
        // replayed verbatim: the round trip over the stiff, lossy x0 segment
        // smears it (dispersion) and sheds a little energy, so successive
        // contact ripples come back progressively smoothed and weakened —
        // published force histories show decaying secondary humps, not an
        // equal-amplitude T1 comb. A verbatim replay dug a deep spectral
        // null at 1/(2 T1) (~1.15 kHz at C4, right on partial 4) and held
        // the pulse in a flat pedestal. One pole at ~0.8/T1 plus a 0.85
        // per-trip survival reproduces the measured decaying-ripple shape.
        let fc_img = (0.62 / t1_seconds.max(1e-5)).min(0.45 * sample_rate * NSUB as f64);
        self.img_c = 1.0 - (-core::f64::consts::TAU * fc_img * self.dt).exp();
        // Long bass round trips come back nearly intact (the lowest octaves
        // are the two-wave regime: incoming + one strong reflection); short
        // mid/treble trips repeat many times inside one contact and disperse
        // a little more each pass.
        self.img_g = (0.72 + 50.0 * t1_seconds).min(0.97);
        self.img_lp = 0.0;
        // 60 ms hard timeout: no physical contact lasts anywhere near this.
        self.timeout = (0.060 * sample_rate * NSUB as f64) as u32;
    }

    /// Render `n` audio samples of contact force into `force[0..n]`.
    /// Returns false (and writes nothing) once the contact is over.
    pub fn render_force(&mut self, force: &mut [f32], n: usize) -> bool {
        if !self.active {
            return false;
        }
        for slot in force.iter_mut().take(n) {
            let mut acc = 0.0f64;
            for _ in 0..NSUB {
                let u = self.x - self.y;
                let mut f = 0.0f64;
                if u > 0.0 {
                    let du = self.v - self.v_str;
                    let lock = u * self.inv_ulock;
                    let l2 = lock * lock;
                    // Fully compacted felt is hard but not infinitely so:
                    // the lock-up factor saturates instead of exploding into
                    // the F_MAX safety clamp (which flatlined ff dynamics in
                    // the top octaves).
                    // soft-saturating lock-up: linear onset, approaches
                    // 1 + lock_cap asymptotically (a hard min() flatlined
                    // the top-octave ff step: v112 and v127 landed on the
                    // same cap)
                    let x = self.lock_w * l2 * l2 * lock;
                    let lockf = 1.0 + x * self.lock_cap / (x + self.lock_cap);
                    // The loading/unloading (hysteresis) modulation is a
                    // linearisation valid for moderate felt strain rates;
                    // clamp it so agraffe-image chatter in the treble (du of
                    // tens of m/s) cannot run it into the F_MAX safety net.
                    let hyst = (1.0 + self.lambda * du).clamp(0.15, 2.6);
                    f = self.k_felt * u.powf(self.p_felt) * lockf * hyst;
                    if f < 0.0 {
                        f = 0.0;
                    } else if f > F_MAX {
                        f = F_MAX;
                    }
                    self.started = true;
                }
                // symplectic Euler on the hammer
                self.v -= f * self.inv_m * self.dt;
                self.x += self.v * self.dt;
                // string point velocity: direct wave + inverted image from
                // the agraffe
                let f_del = self.ring[(self.rpos + RING - self.delay) % RING] as f64;
                self.img_lp += self.img_c * (self.img_g * f_del - self.img_lp);
                self.v_str = (f - self.img_lp) * self.inv_2z;
                self.y += self.v_str * self.dt;
                self.ring[self.rpos] = f as f32;
                self.rpos = (self.rpos + 1) % RING;
                self.steps += 1;
                acc += f;
            }
            let mut fmean = (acc * (1.0 / NSUB as f64)) as f32;
            if self.rough > 0.0 && fmean > 0.0 {
                let mut x = self.rough_rng;
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                self.rough_rng = x;
                let white = (x >> 8) as f32 * (1.0 / 8_388_608.0) - 1.0;
                self.rough_lp += 0.62 * (white - self.rough_lp);
                fmean *= 1.0 + self.rough * self.rough_lp;
            }
            *slot = fmean;
        }
        // Contact over: felt separated and hammer moving away, or safety net.
        let u = self.x - self.y;
        if (self.started && u <= 0.0 && self.v < 0.0) || self.steps > self.timeout || self.x < -0.05 {
            self.active = false;
        }
        true
    }
}
