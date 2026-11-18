// Per-key physical design of the instrument: string scaling, inharmonicity,
// unison layout, losses, hammer and damper parameters, all precomputed at
// construction into flat mode tables the kernels stream over.
//
// The scaling laws below follow a concert-grand compass (A0..C8, MIDI
// 21..=108) with values in the ranges published for real instruments
// (string tensions ~1500 N wound bass falling to ~700 N plain wire, linear
// densities 4 g/m plain treble wire up to 140 g/m wound bass, speaking
// lengths ~2 m down to ~5 cm, hammer masses ~11.5 g bass to ~3.6 g treble,
// felt stiffness exponents 2.4-3.4; the C4 design lands on the published
// Chaigne-Askenfelt simulation values: ~5 g hammer, K ~ 5e9, T ~ 780 N).
// Everything audible is derived from these physical numbers rather than
// tuned per key. The material/geometry constants themselves live in
// params::DesignParams (defaults = the shipped instrument):
//
// - partial frequencies: f_n = n * f0 * sqrt(1 + B n^2), the stiff-string
//   dispersion law, with B measured on the reference instrument up to C5
//   (U-shaped: 2.2e-4 at A0, ~7.6e-5 across the wound/plain break, rising
//   again) and a log-linear law above, to ~6e-3 at C8
// - octave stretch: the measured tuning up to C4 (-16 cents at A0), a
//   cubic Railsback-style curve above (+30 cents at C8), so the inharmonic
//   partials of low notes line up with the fundamentals of high ones the
//   way a tuner lays a piano
// - losses: in the wound bass (to ~C3, fading out by C4) each partial's
//   aftersound decays at a measured intrinsic law and its prompt adds a
//   bridge-coupling loss linear in the admittance's real part (the
//   recordings' two-stage decay: ~8-14 dB/s prompt over a ~1-4 dB/s
//   aftersound); above, sigma_n is anchored to a fundamental T60 law plus
//   frequency-dependent terms and the squared-admittance coupling
// - unisons: 1 string A0..E1, 2 strings F1..E2, 3 strings F2..C8, detuned by
//   ~0.5-1.8 cents; per partial the strings reduce to a vertical (prompt),
//   a horizontal (aftersound) and, on unison keys, a mistuned anti-phase
//   (beat) pole with complex residues (the normal-mode reduction below)
// - the hammer strikes at x0/L ~ 0.132 (bass) to 0.082 (treble), giving
//   comb dips near the 8th-12th partials via g_in = sin(n pi x0/L); the
//   dips have a floor (comb_fill) because the felt contact is wide, the
//   contact point wanders during the blow, and the termination is not
//   rigid — measured piano spectra show shallow dips, never deep nulls
// - longitudinal modes: each string's longitudinal wave speed (steel core;
//   the copper winding adds transverse mass but little longitudinal
//   stiffness) puts longitudinal modes at m * f0 * (c_long/c_trans), a bank
//   per wound key driven by the transverse modal products q_m q_n with
//   |m - n| or m + n equal to the mode number (voice.rs) — the
//   phantom-partial mechanism of real strings (tension modulation)
// - per-key voicing scatter: a real instrument is not 88 copies of one
//   model; deterministic per-key jitter on felt, detune, losses and noise
//   levels (scatter=0 disables it exactly)

use crate::modal::pad8;
use crate::params::DesignParams;

pub const FIRST_KEY: u8 = 21; // A0
pub const LAST_KEY: u8 = 108; // C8
pub const NUM_KEYS: usize = 88;
/// Keys above this MIDI number have no damper on a real instrument.
pub const TOP_DAMPED_KEY: u8 = 88; // E6 is the last dampered key here

/// Longitudinal/phantom bank size.
pub const PH_MODES: usize = 8;
/// Transverse partials whose modal products drive the longitudinal bank.
pub const PH_PARENTS: usize = 32;
/// Most low partials per key whose aftersound has its own radiation taps
/// (`spread_aftersound`).
pub const SIDE_MODES: usize = 16;
/// The taps reach up to this frequency: above it a partial's prompt is over
/// in a few tenths of a second and its neighbours are close.
const SIDE_HZ: f64 = 2000.0;

pub struct KeyDesign {
    pub f0: f32,
    pub b_coeff: f32,
    pub n_strings: usize,
    pub n_osc: usize,
    /// Mistuning of the anti-phase unison mode against the in-phase line,
    /// as a fraction of the partial's frequency (0 on single strings).
    pub unison_detune: f64,
    /// params::anti_lock, read by `lock_unisons`.
    pub anti_lock: f64,
    /// params::after_spread, read by `spread_aftersound`.
    pub after_spread: f64,
    pub modes_per_osc: usize,        // real modes per oscillator
    pub modes_padded: usize,         // padded to 8
    pub total_modes: usize,          // n_osc * modes_padded
    pub hammer_mass: f64,
    pub felt_k: f64,
    pub felt_p: f64,
    pub felt_u_lock: f64,
    pub felt_lock_w: f64,
    pub core_k: f64,
    pub u_core: f64,
    pub felt_lambda: f64,
    pub z_total: f64,                // wave impedance seen by the hammer (n_strings * Z)
    pub t1_seconds: f64,             // agraffe reflection round trip 2 x0 / c
    /// Hammer-speed compression exponent for this key (see params::vel_q_*):
    /// speed' = speed_pivot * (speed/speed_pivot)^speed_q at note-on.
    pub speed_q: f64,
    /// Corner time of the mezzo-forte force pulse the voicing estimate
    /// uses: the pulse spectrum is ~ 1 / (1 + (2 f pulse_tau)^2).
    pub pulse_tau: f64,
    /// The mezzo-forte pivot the compression turns about (= DesignParams
    /// v_mf, the point the compass-evenness calibration was done at).
    pub speed_pivot: f64,
    pub undamped: bool,
    pub pan: f32,                    // player perspective: bass left
    pub rough_depth: f32,            // contact roughness scale
    pub img_fc_mul: f64,
    pub img_g_base: f64,
    pub img_g_slope: f64,
    // Attack complex (voice.rs reads these at note-on):
    pub thump_amp: f32,
    pub thump_lp_c: f32,
    pub thump_len: u32,
    pub thump_vpow: f32,
    pub click_amp: f32,
    pub click_lp_c: f32,
    pub click_len: u32,
    pub click_vpow: f32,
    // Commuted-style body-tap excitation (voice.rs; cs_amp 0 = off):
    pub cs_amp: f32,
    pub cs_len: u32,
    pub cs_c_hi: f32,
    pub cs_c_lo: f32,
    pub cs_c_tilt: f32,
    pub cs_vpow: f32,
    // Direct hammer-blow shock into the bridge:
    pub knock_amp: f32,
    pub knock_hp_c: f32,
    pub uc_third: f32,
    // Flat per-mode tables, osc-major, each of length total_modes:
    pub cr_sus: Vec<f32>,            // sustain rotation, real
    pub ci_sus: Vec<f32>,            // sustain rotation, imag
    pub damp_mul: Vec<f32>,          // extra radius factor at full damper contact
    pub gin: Vec<f32>,               // hammer force injection weight
    pub gout: Vec<f32>,              // bridge force output weight, Im(z) tap
    /// Re(z) tap of the complex residue (see the normal-mode reduction in
    /// build_key and modal::run_modes_c); zero for uncoupled modes.
    pub gout_re: Vec<f32>,
    /// Low partials (a multiple of 8) of the slots after VERT that carry
    /// the taps below; 0 until `spread_aftersound` has run.
    pub side_modes: usize,
    /// The aftersound's own taps to the left and right outputs, complex
    /// like gout/gout_re (`_im` reads Im(z), `_re` reads Re(z)): slot-major
    /// over the slots after VERT, SIDE_MODES entries each.
    pub side_l_im: Vec<f32>,
    pub side_l_re: Vec<f32>,
    pub side_r_im: Vec<f32>,
    pub side_r_re: Vec<f32>,
    // Longitudinal / phantom bank (all length PH_MODES; ph_gain 0 = off):
    pub ph_cr: Vec<f32>,
    pub ph_ci: Vec<f32>,
    pub ph_gin: Vec<f32>,
    pub ph_gout: Vec<f32>,
    pub ph_gain: f32,
    /// per-parent slope weight n f0 / f_n (0 beyond the key's partials)
    pub ph_slope: [f32; PH_PARENTS],
    pub ph_parents: usize,
    pub ph_drive: f32,
    // Sympathetic bank tables (small, first partials of this string group):
    pub sym_modes: usize,            // padded to 8
    pub sym_cr: Vec<f32>,
    pub sym_ci: Vec<f32>,
    pub sym_damp_mul: Vec<f32>,
    pub sym_gin: Vec<f32>,
    pub sym_gout: Vec<f32>,
}

/// Mode-count budget per oscillator across the compass: bass notes need many
/// partials for brightness, treble notes physically only have a few below
/// the audio band.
fn mode_cap(idx: usize) -> usize {
    match idx {
        // The bottom octaves' growl lives in partials far above the old
        // caps: at 128 modes A0's top partial sat at ~4.5 kHz, so the
        // whole 4.5-9 kHz band of the bottom octave — clearly tonal in
        // the reference recordings — simply did not exist. 240 reaches
        // ~9.9 kHz at A0 with the reference-fitted B.
        0..=11 => 240,
        12..=23 => 128,
        24..=39 => 88,
        40..=55 => 72,
        _ => 64,
    }
}

/// Deterministic per-key unit jitter in [-1, 1] (stream `s`).
fn kj(idx: usize, s: u32) -> f64 {
    let mut x = (idx as u32).wrapping_mul(0x9e37_79b9) ^ s.wrapping_mul(0x85eb_ca6b) ^ 0x5f35_6495;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f64 * (2.0 / 16_777_216.0) - 1.0
}

/// Minimal complex arithmetic for construction-time work (the eigen
/// reduction here, the radiation response in soundboard.rs).
#[derive(Clone, Copy, Debug)]
pub struct C64 {
    pub re: f64,
    pub im: f64,
}

impl C64 {
    pub(crate) fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    /// e^(j phase)
    pub(crate) fn expj(phase: f64) -> Self {
        Self::new(phase.cos(), phase.sin())
    }
    pub(crate) fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }
    pub(crate) fn add(self, o: Self) -> Self {
        Self::new(self.re + o.re, self.im + o.im)
    }
    pub(crate) fn sub(self, o: Self) -> Self {
        Self::new(self.re - o.re, self.im - o.im)
    }
    pub(crate) fn mul(self, o: Self) -> Self {
        Self::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
    pub(crate) fn div(self, o: Self) -> Self {
        let d = (o.re * o.re + o.im * o.im).max(1e-30);
        Self::new(
            (self.re * o.re + self.im * o.im) / d,
            (self.im * o.re - self.re * o.im) / d,
        )
    }
    pub(crate) fn scale(self, k: f64) -> Self {
        Self::new(self.re * k, self.im * k)
    }
    fn csqrt(self) -> Self {
        let r = (self.re * self.re + self.im * self.im).sqrt();
        let re = ((r + self.re) * 0.5).max(0.0).sqrt();
        let im = ((r - self.re) * 0.5).max(0.0).sqrt();
        Self::new(re, if self.im >= 0.0 { im } else { -im })
    }
    pub(crate) fn abs2(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

/// The reference instrument's measured scale at its sampled keys (every
/// minor third, A0..C5): inharmonicity B and the tuning offset of the
/// fitted fundamental (cents re equal temperament), from robust
/// (f_n/n)^2-vs-n^2 regressions of the Salamander Grand V3 recordings
/// (Yamaha C5; 24-40 resolved partials per note, identical to +-2 % across
/// velocity layers). Keys between anchors interpolate log B and cents.
/// The single log-linear law that ran the whole compass (params::b_lo/b_span,
/// still used above C5) put A0 at
/// B = 2.4e-5 where the recording measures 2.2e-4: a nearly harmonic
/// bottom octave (A0's 20th partial 8 cents sharp instead of 70) — the
/// clean, pitched line of a bass guitar rather than the clangorous stretch
/// of long wound piano strings. A real scale is U-shaped: B falls from the
/// bottom singles to its minimum across the wound/plain break (F#2..A2,
/// ~7.6e-5) and climbs through the plain-wire compass. The law stays in
/// charge above C5, where the recordings resolve too few partials.
const MEASURED_SCALE: [(u8, f64, f64); 18] = [
    (21, 2.24e-4, -16.2),
    (24, 1.63e-4, -16.0),
    (27, 1.33e-4, -8.2),
    (30, 9.10e-5, -10.0),
    (33, 1.01e-4, -14.8),
    (36, 9.10e-5, -8.1),
    (39, 7.64e-5, -3.5),
    (42, 7.57e-5, -4.0),
    (45, 8.13e-5, -1.4),
    (48, 1.12e-4, -5.4),
    (51, 1.27e-4, -2.8),
    (54, 1.72e-4, -5.1),
    (57, 2.18e-4, -0.3),
    (60, 2.92e-4, -1.0),
    (63, 3.70e-4, -0.5),
    (66, 5.10e-4, -2.5),
    (69, 6.40e-4, 0.8),
    (72, 8.30e-4, 2.0),
];

/// (B, cents) for a key inside MEASURED_SCALE's range.
fn measured_scale(key: u8) -> (f64, f64) {
    let i = MEASURED_SCALE.partition_point(|a| a.0 < key);
    let hi = MEASURED_SCALE[i.min(MEASURED_SCALE.len() - 1)];
    if hi.0 == key || i == 0 {
        return (hi.1, hi.2);
    }
    let lo = MEASURED_SCALE[i - 1];
    let w = (key - lo.0) as f64 / (hi.0 - lo.0) as f64;
    ((hi.1.ln() * w + lo.1.ln() * (1.0 - w)).exp(), lo.2 + (hi.2 - lo.2) * w)
}

/// The two polarisations of one string on the bridge, in the frame rotating
/// at the partial: the eigenmodes of the complex-symmetric
/// [[d_v, gx], [gx, d_h]], where d_v and d_h hold each polarisation's loss,
/// bridge coupling and offset, `gv` is the vertical coupling with its
/// reactive part, the horizontal one is `couple` of it, and
/// gx = -cross sqrt(gv gh) is the bridge-rocking term. The cross term
/// carries the admittance PHASE: a real cross term keeps the eigenvectors
/// essentially real and the residues collapse back to sine-only, the
/// invariant-mixture failure this reduction exists to fix.
///
/// Returns each mode's eigenvalue and residue, the vertical-dominated mode
/// first. The residue is R = (v.e)(e.u)/(e.e) for the blow u = [1, drive]
/// read as v = [1, rad]; it is 1 for the pure vertical mode.
pub(crate) fn polarisation_pair(d_v: C64, d_h: C64, gv: C64, couple: f64, cross: f64, drive: f64, rad: f64) -> [(C64, C64); 2] {
    let gh = gv.scale(couple);
    let gx = gv.mul(gh).csqrt().scale(-cross);
    let g_x = cross * (gv.re * (gv.re * couple)).sqrt();
    let mean = d_v.add(d_h).scale(0.5);
    let half = d_v.sub(d_h).scale(0.5);
    let disc = half.mul(half).add(gx.mul(gx)).csqrt();
    let mut lam = [mean.add(disc), mean.sub(disc)];
    // order so slot 0 is the vertical-dominated mode
    if lam[0].sub(d_v).abs2() > lam[1].sub(d_v).abs2() {
        lam.swap(0, 1);
    }
    core::array::from_fn(|slot| {
        let l = lam[slot];
        // eigenvector e = [-Gx, l - d_v] resp. pure modes when the cross
        // term vanishes
        let (e0, e1) = if g_x < 1e-9 {
            if slot == 0 {
                (C64::new(1.0, 0.0), C64::new(0.0, 0.0))
            } else {
                (C64::new(0.0, 0.0), C64::new(1.0, 0.0))
            }
        } else {
            (gx, l.sub(d_v))
        };
        let num_v = e0.add(e1.scale(rad));
        let num_u = e0.add(e1.scale(drive));
        let den = e0.mul(e0).add(e1.mul(e1));
        (l, num_v.mul(num_u).div(den))
    })
}

pub fn build_key(key: u8, sample_rate: f64, p: &DesignParams) -> KeyDesign {
    let idx = (key - FIRST_KEY) as usize;
    let t = idx as f64 / 87.0;
    let sc = p.scatter;

    // --- tuning and inharmonicity ---------------------------------------
    // Measured scale up to C5 (see MEASURED_SCALE), the smooth laws above.
    let cubic_cents = ((idx as f64 - 45.0) / 42.0).powi(3) * 30.0;
    let stretch_cents = if key <= 60 {
        measured_scale(key).1
    } else if key < 66 {
        let w = (key - 60) as f64 / 6.0;
        (1.0 - w) * measured_scale(60).1 + w * cubic_cents
    } else {
        cubic_cents
    };
    let f0 = 440.0 * ((key as f64 - 69.0) / 12.0 + stretch_cents / 1200.0).exp2();
    let law_b = 10f64.powf(p.b_lo + p.b_span * t);
    let top = MEASURED_SCALE[MEASURED_SCALE.len() - 1];
    let b_base = if key <= top.0 {
        measured_scale(key).0
    } else {
        // meet the law continuously: the C5 measured/law ratio fades out
        // over the next octave
        let law_top = 10f64.powf(p.b_lo + p.b_span * (top.0 - FIRST_KEY) as f64 / 87.0);
        let w = ((key - top.0) as f64 / 12.0).min(1.0);
        law_b * (top.1 / law_top).powf(1.0 - w)
    };
    let b_coeff = b_base * (1.0 + 0.15 * sc * kj(idx, 6));

    // --- string scaling -------------------------------------------------
    // Real grand scales run ~1500 N on the wound bass singles down to
    // ~700-850 N across the plain-wire compass (Chaigne-Askenfelt's C4:
    // T = 670 N, mu = 6.3 g/m, L = 0.62 m).
    let tension = p.tens_base + p.tens_span * (1.0 - t).powi(4); // N
    let mu = if idx < 24 {
        // wound bass strings
        (0.14f64.ln() + (0.011f64.ln() - 0.14f64.ln()) * idx as f64 / 24.0).exp()
    } else {
        // plain wire, ~1.05 mm down to ~0.8 mm steel
        0.0088 + (0.0040 - 0.0088) * (idx as f64 - 24.0) / 63.0
    };
    let c_wave = (tension / mu).sqrt();
    let length = c_wave / (2.0 * f0);
    let z_char = (tension * mu).sqrt();

    let n_strings = if idx < 8 {
        1
    } else if idx < 20 {
        2
    } else {
        3
    };
    // Oscillator slots per key (see the normal-mode reduction below):
    // singles carry [VERT, HORZ] — the two polarisations of the one
    // string; unison keys carry [VERT, HORZ, ANTI] — the bridge-pumping
    // in-phase mode, the horizontal aftersound, and the mistuned
    // anti-phase unison mode that carries the beat.
    let n_osc = if n_strings == 1 { 2 } else { 3 };

    // --- hammer ---------------------------------------------------------
    // Head mass ~11.5 g (A0) falling to ~3.6 g (C8), curved so the mid keys
    // land near the published effective striking masses (Chaigne-Askenfelt's
    // C4 simulation uses 2.9 g on a 3.9 g string; Hall/Giordano give
    // ~10-12 g bass, ~3-6 g treble).
    let hammer_mass = p.hm_base + p.hm_span * (1.0 - t).powf(p.hm_pow);
    // Felt stiffness scaled so mf contact times land on the measured 4 ms
    // (bass) .. <1 ms (treble); voicing scatter moves individual hammers
    // the way real felt varies needle-to-needle.
    // Bass felt regime (see params::feltp_bass): the bottom octaves'
    // exponent and stiffness are pulled down on a quadratic ramp that
    // vanishes at felt_bass_t, leaving the mid/treble law untouched.
    let bass_ramp = if p.felt_bass_t > p.felt_bass_t0 + 1e-9 {
        ((p.felt_bass_t - t) / (p.felt_bass_t - p.felt_bass_t0)).clamp(0.0, 1.0).powf(p.felt_bass_pow.max(0.1))
    } else {
        0.0
    };
    let logk_law = p.feltk_lo + p.feltk_span * t + p.feltk_top * ((t - 0.75) / 0.25).clamp(0.0, 1.0);
    let p_law = p.feltp_lo + p.feltp_span * t;
    let logk = (1.0 - bass_ramp) * logk_law + bass_ramp * (p.feltk_bass_lo + p.feltk_bass_slope * t);
    let felt_k = 10f64.powf(logk) * (2f64).powf(0.5 * sc * kj(idx, 1));
    let felt_p = ((1.0 - bass_ramp) * p_law + bass_ramp * p.feltp_bass).max(1.02);
    let felt_lambda = 1.0 - p.lambda_bass.clamp(0.0, 1.0) * bass_ramp;
    // Mezzo-forte felt compression estimate; lock-up starts just below it,
    // and bites harder in the bass (thick, graded felt) than in the treble
    // (thin felt that is near its compacted state already).
    let v_mf = p.v_mf;
    let e_mf = 0.5 * hammer_mass * v_mf * v_mf;
    let u_mf = (e_mf * (felt_p + 1.0) / felt_k).powf(1.0 / (felt_p + 1.0));
    let felt_u_lock = p.lock_frac * u_mf;
    // wood core: only the top octaves' thin felt reaches it. Absolute
    // Hertzian scale (N per m^1.5): at ~0.25 mm of over-compression the
    // stage contributes some tens of newtons — a sharp feature on top of
    // the ~75 N felt pulse, not a wall (scaling it off felt_k mixed the
    // two force laws' exponents and produced a 1300 N delta spike).
    // ramps in above ~F4, full through the C6 octave, then backs off
    // toward C8: the reference ladders of the top two octaves fall
    // steeply after their first partials (tiny hammers, sub-half-ms
    // dwell), and the full snap there overshot them by 3-6 dB.
    let core_up = ((t - 0.55) / 0.17).clamp(0.0, 1.0);
    let core_dn = 1.0 - 0.97 * ((t - 0.75) / 0.09).clamp(0.0, 1.0);
    let core_w = core_up * core_dn;
    let core_k = p.core_mul * 1.0e7 * core_w * core_w;
    let u_core = p.core_frac * u_mf;
    // Lock-up is a compacted-felt phenomenon; the thick bass felt never
    // reaches compaction under playing loads, so it is gated out with the
    // bass regime (see params::feltp_bass): a linear bass spring that still
    // locked up at forte kept a 5 dB pp->ff swing in C2's sub-kHz ladder
    // that the recordings do not have.
    let felt_lock_w = (p.lockw_lo + (p.lockw_hi - p.lockw_lo) * t) * (1.0 - bass_ramp);
    // Treble hammer-speed range compression (see params::vel_q_*): the
    // measured level span from velocity 30 to 127 was ~23-26 dB across
    // A0..C4 but 39-48 dB at C5..C7 — twice the dynamic slope, pivoting
    // at the calibrated mezzo point, so forte trebles rang out like
    // struck bells while piano trebles vanished. q < 1 narrows the
    // SPEED range about that same pivot, which scales the level span by
    // exactly q without moving the mezzo-forte sound of any key.
    let speed_q = if p.vel_q_ramp > 1e-9 {
        1.0 - p.vel_q_depth * ((t - p.vel_q_start) / p.vel_q_ramp).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let spos_bass = if p.spos_bass_t > 1e-9 { p.spos_bass * (1.0 - t / p.spos_bass_t).max(0.0) } else { 0.0 };
    let strike_pos = (p.spos_lo - (p.spos_lo - p.spos_hi) * t.powf(p.spos_pow) + spos_bass)
        * (1.0 + 0.04 * sc * kj(idx, 2));
    let t1_seconds = 2.0 * strike_pos * length / c_wave;
    let z_total = z_char * n_strings as f64;

    // --- losses ---------------------------------------------------------
    let t60_fund =
        (p.t60_base * (1.0 - t).powf(p.t60_pow) + p.t60_min) * (1.0 + 0.15 * sc * kj(idx, 3));
    let sigma_fund = 6.91 / t60_fund;
    // Quadratic-in-frequency viscoelastic/air loss.
    let a2 = p.a2_lo + p.a2_slope * t;
    let damper_strength = 0.55 + 0.75 * t;
    let undamped = key > TOP_DAMPED_KEY;

    // --- unison / polarisation normal-mode reduction --------------------
    // Per PARTIAL, the string system is reduced at construction to two or
    // three complex poles with COMPLEX residues, run on the ordinary
    // rotator kernels (Bank, Valimaki, Sujbert & Karjalainen, EUSIPCO
    // 2000: a handful of second-order resonators with independent
    // frequency, amplitude, phase and decay per partial; Woodhouse, JASA
    // 2021: the bridge admittance decides, partial by partial, which
    // normal mode radiates strongly and dies quickly and which one stores
    // energy as aftersound):
    //   slot 0  VERT — the strings moving vertically in phase, coupled to
    //           the bridge's vertical admittance: the loud prompt sound;
    //   slot 1  HORZ — the horizontal polarisation: weakly driven, weakly
    //           radiating, nearly intrinsic decay — the aftersound;
    //   slot 2  ANTI (unison keys) — the mistuned anti-phase mode:
    //           bridge-cancelling, nearly undamped, quiet — the beat.
    // VERT and HORZ are the eigenmodes of the 2x2 complex-symmetric
    // system [[d_v - Gv, -Gx], [-Gx, d_h - Gh]] (rotating frame at the
    // partial): Gv is the calibrated vertical coupling, Gh a small
    // horizontal share, Gx the bridge-rocking cross term. Eigen-derived
    // residues are complex — the published normal-mode form — which lets
    // the prompt/aftersound mixture vary partial to partial (the old
    // fixed per-oscillator multipliers gave every partial of every key
    // the same 4.3:1 split, the measured plucked-harp signature). A
    // partial's summed response to force still starts at zero: the
    // residue phases cancel at t = 0 by the eigen algebra, not by
    // assumption.
    let detune_cents = (p.det_lo + p.det_slope * t) * (1.0 + 0.25 * sc * kj(idx, 4));
    let pol_det_cents = p.pol_det * (1.0 + 0.5 * sc * kj(idx, 9));
    let anti_sign = if sc > 0.0 && kj(idx, 10) < 0.0 { -1.0 } else { 1.0 };
    // Compass coupling scale, calibrated against the Salamander
    // staircases (see params::bridge_couple_taper for the honest
    // literature discrepancy; the singles factor is the measured
    // gentleness of the real bottom octave).
    // Low-end coupling contrast: the measured staircases show the real
    // instrument's SINGLES barely draining (A0 -2.2 dB in the first half
    // second; its singles run to ~A1 on that scale) while the doubled
    // wound keys knee hard (C2 -9.5). One string cannot split into
    // bridge-pumping and bridge-cancelling unison modes — it only has
    // the weak polarisation pair — and the bass bridge presents its
    // lowest admittance at its far end, so the singles' factor is small
    // and the doubles ramp in over the first half octave above the
    // break.
    // Singles 0.12 -> 0.03 (2026-09-01): measured per partial on the
    // Salamander A0 and C1, the singles' partials 8-30 (200-850 Hz) decay
    // at 2-12 dB/s over the first second (median 4-5, sigma ~0.5) with
    // only a few drains near 12 dB/s; at 0.12 the model's ran 10-25 dB/s
    // (median 11) on top of the corrected intrinsic law — the prompt
    // stage of the bottom octave still ate its upper partials twice as
    // fast as the instrument does.
    let lo_fac = if n_strings == 1 {
        0.03
    } else if n_strings == 2 {
        0.5 + 0.5 * ((idx as f64 - 8.0) / 6.0).clamp(0.0, 1.0)
    } else {
        // the plain-wire triples sit on the long bridge's stiffer middle:
        // measured knees there (C3 -9.4, C4 -10.5) run shallower than the
        // doubled wound keys' relative to the same admittance proxy
        0.75
    };
    let couple_amt = p.bridge_couple * (1.0 - t).powf(p.bridge_couple_taper) * lo_fac;
    // Register weight of the measured wound-bass decay law (params::bass_*):
    // full up to bass_law_t0, gone by bass_law_t1; the tenor, mid and
    // treble keep the calibrated law above.
    let bass_w = if p.bass_law_t1 > p.bass_law_t0 {
        ((p.bass_law_t1 - t) / (p.bass_law_t1 - p.bass_law_t0)).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // --- mode tables ----------------------------------------------------
    let f_limit = (0.44 * sample_rate).min(20000.0);
    let cap = mode_cap(idx);
    let mut modes_per_osc = 0;
    for n in 1..=cap {
        let fn_hz = n as f64 * f0 * (1.0 + b_coeff * (n * n) as f64).sqrt();
        if fn_hz >= f_limit {
            break;
        }
        modes_per_osc = n;
    }
    modes_per_osc = modes_per_osc.max(1);
    let modes_padded = pad8(modes_per_osc);
    let total_modes = n_osc * modes_padded;

    let mut cr_sus = vec![0.0f32; total_modes];
    let mut ci_sus = vec![0.0f32; total_modes];
    let mut damp_mul = vec![1.0f32; total_modes];
    let mut gin = vec![0.0f32; total_modes];
    let mut gout = vec![0.0f32; total_modes];
    let mut gout_re = vec![0.0f32; total_modes];
    // per partial: what the locked unison's drain adds to the prompt's
    // output (see the unison drain below); applied after the voicing trim
    let mut uni_lift = vec![1.0f64; modes_per_osc];
    let uni_w = 1.0 - ((t - p.unison_top_t) / 0.03).clamp(0.0, 1.0);

    let dt = 1.0 / sample_rate;
    let cents = 5.7779e-4; // fractional frequency per cent, small-angle
    for n in 1..=modes_per_osc {
        let nf = n as f64;
        let fn_hz = nf * f0 * (1.0 + b_coeff * nf * nf).sqrt();
        if fn_hz >= 0.499 * sample_rate {
            continue; // all slots stay zero (dead) modes
        }
        let fk = fn_hz / 1000.0;
        let f0k = f0 / 1000.0;
        let wound = (1.0 - t).powi(3);
        // wound-string law on the copper-wound keys, the plain-wire law
        // above the break (idx 24), blended over ~a fifth (params::a1_plain)
        let wg = if t > 0.28 { 1.0 / (1.0 + ((t - 0.28) / 0.05).powi(2)) } else { 1.0 };
        let a1w = wg * p.a1_wound + (1.0 - wg) * p.a1_plain;
        let a2w = wg * p.a2_wound;
        // intrinsic string loss (winding, viscous/air, quartic) — what
        // the aftersound decays at
        let sig_intr = (sigma_fund
            + a1w * wound * (fk - f0k)
            + (a2 + a2w * wound) * (fk * fk - f0k * f0k)
            + p.a4 * (fk.powi(4) - f0k.powi(4)))
        .max(0.15);
        // complex bridge admittance at this partial
        let (y_re, y_im) = crate::soundboard::bridge_admittance_c(fn_hz, p);
        let shape = p.bridge_couple_floor + (1.0 - p.bridge_couple_floor) * (y_re * y_re).min(2.0);
        // Wound-bass law (weight bass_w, see below): the aftersound decays
        // at the intrinsic string loss alone, and the prompt adds the weak-
        // coupling bridge loss, LINEAR in Re Y (sigma_b ~ 2 f0 Z Re Y,
        // Woodhouse 2021) under a smooth frequency envelope.
        let sig_after = p.bass_after_base + p.bass_after_slope * fk.powf(p.bass_after_pow) + p.bass_after_a2 * (1.0 - t).powi(6) * fk * fk + p.a4 * fk.powi(4);
        let g_bass = p.bass_couple * (p.bridge_couple_floor + (1.0 - p.bridge_couple_floor) * y_re.min(2.5))
            / (1.0 + (fn_hz / p.bass_couple_hz).powi(2))
            + p.bass_couple_lo / (1.0 + (fn_hz / p.bass_couple_lo_hz).powi(4));
        // Unison drain (Weinreich 1977). One string loses 2 f0 Z Re Y of
        // its amplitude per second to a bridge of conductance Re Y: Z Re Y
        // at each of its 2 f0 reflections there. The strings of a unison
        // that move in phase push the bridge together, and that motion
        // drains as many times faster as there are strings; it holds
        // together while the strings' mistuning at this partial (rad/s) is
        // under the drain itself. The mistuning grows with the partial
        // number and the drain does not, so this is the fundamental's law:
        // by the second partial about a third of it is left, by the third
        // a tenth. The lattice above is a proxy for the conductance's
        // peaks and valleys and its compass taper was set on the bass; in
        // the plain-wire middle it left a fundamental that sits between
        // two of its peaks with almost no bridge loss at all, 10-12 dB/s
        // at F#4, A4 and C5 beside 23-33 at C4 and D#4, where the
        // recordings' fundamentals fall 25, 28 and 34 dB/s at C4, A4 and C5
        // and stand no plateau. A real board's modes overlap there, and
        // its valleys do not reach down to nothing: the in-phase drain at
        // the mean conductance (params::unison_y) is the least a locked
        // partial loses.
        let g_lat = couple_amt * shape;
        let g_uni = if n_strings > 1 && uni_w > 0.0 {
            let full = n_strings as f64 * 2.0 * f0 * z_char * p.unison_y;
            let x = core::f64::consts::TAU * fn_hz * detune_cents * cents / full.max(1e-9);
            uni_w * full / (1.0 + x.powi(4))
        } else {
            0.0
        };
        // the lattice alone: what the reactive pull below and the level
        // the calibration's gains stand for were set with
        let g_v_lat = (1.0 - bass_w) * g_lat + bass_w * g_bass;
        let g_v = (1.0 - bass_w) * g_lat.max(g_uni) + bass_w * g_bass; // vertical coupling loss (1/s)
        let sig_intr = (1.0 - bass_w) * sig_intr + bass_w * sig_after;
        // reactive part: the bridge pulls a coupled partial's frequency.
        // Clamped to +-4 cents so the dispersion law stays recognisably a
        // piano's (strong drains on a real instrument wobble, they do not
        // transpose).
        // The mean conductance is a resistance: only the lattice pulls.
        let pull = (g_v_lat * (y_im / y_re.max(0.05)).clamp(-1.2, 1.2))
            .clamp(-fn_hz * 4.0 * cents * core::f64::consts::TAU, fn_hz * 4.0 * cents * core::f64::consts::TAU);
        let g_h = g_v * p.pol_couple;
        let d_v = C64::new(-sig_intr - g_v, -pull);
        let dw_pol = core::f64::consts::TAU * fn_hz * (pol_det_cents * cents);
        // pol_sig slows the aftersound only where the measured corpus
        // shows it (bass/mid: the real C4 holds ~1.4 dB/s late); at the
        // top the real aftersound is NOT slower than the single-decay law
        // (the real C7's second partial is -32 dB rel p1 by 300 ms), so
        // the factor ramps out over the last octave and a half.
        let pol_sig_t = p.pol_sig + (1.0 - p.pol_sig) * ((t - 0.6) / 0.25).clamp(0.0, 1.0);
        // the wound-bass law's intrinsic loss already IS the aftersound
        let pol_sig_t = (1.0 - bass_w) * pol_sig_t + bass_w;
        let d_h = C64::new(-sig_intr * pol_sig_t - g_h, dw_pol);
        // The polarisation leakage varies per partial on a real string
        // (termination asymmetry is frequency-dependent): the reference
        // fits show the prompt/aftersound amplitude split swinging tens of
        // dB partial to partial, so the drive share carries a bounded
        // deterministic jitter.
        let mid_w = ((idx as f64 - 24.0) / 3.0).clamp(0.0, 1.0) * (1.0 - ((t - p.pol_drive_upper_t) / 0.12).clamp(0.0, 1.0));
        let mid_mul = if n == 1 { 1.0 } else { 1.0 + mid_w * (p.pol_drive_upper - 1.0) / (1.0 + (fn_hz / p.pol_drive_upper_hz).powi(2)) };
        let pd = ((1.0 - bass_w) * p.pol_drive + bass_w * p.bass_pol_drive) * (1.0 + 0.6 * kj(idx * 31 + n, 12)) * mid_mul;
        let pair = polarisation_pair(d_v, d_h, C64::new(g_v, pull), p.pol_couple, p.pol_cross, pd, p.pol_rad);
        // strike comb (shared by all slots of this partial)
        let scomb = (nf * core::f64::consts::PI * strike_pos).sin();
        let filled = (scomb * scomb + p.comb_fill * p.comb_fill).sqrt();
        let gin_n = if scomb < 0.0 { -filled } else { filled };
        let sign = if n % 2 == 1 { 1.0 } else { -1.0 };
        let base = sign * tension * nf / (mu * length * length * fn_hz * sample_rate);
        let sigma_d = ((45.0 + fn_hz / 35.0) * damper_strength).min(2000.0);
        for (slot, (l, rres)) in pair.iter().enumerate() {
            // floor 0.1/s (0.9 dB/s, the slowest aftersound the recordings
            // show): with the calibration's 0.1 decay exponent any slower
            // pole rounds to |C| >= 1 in f32 at 192 kHz
            let sig = (-l.re).clamp(0.1, 400.0);
            if slot == 0 && g_v > g_v_lat {
                // A bridge that takes the string's energy faster radiates
                // it faster; it does not radiate less of it. The prompt
                // keeps the energy it gave the first `window` seconds at
                // the lattice's rate, which is the level the measured
                // keys' gains were fitted to.
                let window = crate::calibration::LEVEL_WINDOW.1;
                let seen = |s: f64| (1.0 - (-2.0 * s * window).exp()) / (2.0 * s * window);
                let before = (sig - (g_v - g_v_lat)).max(0.1);
                uni_lift[n - 1] = (seen(before) / seen(sig)).sqrt();
            }
            let r = (-sig * dt).exp();
            let th = core::f64::consts::TAU * fn_hz * dt + l.im * dt;
            let m = slot * modes_padded + (n - 1);
            cr_sus[m] = (r * th.cos()) as f32;
            ci_sus[m] = (r * th.sin()) as f32;
            damp_mul[m] = (-sigma_d * dt).exp() as f32;
            gin[m] = gin_n as f32;
            gout[m] = (base * rres.re) as f32;
            gout_re[m] = (base * rres.im) as f32;
        }
        if n_osc == 3 {
            // ANTI: mistuned anti-phase unison mode. Bridge-cancelling,
            // so it keeps nearly intrinsic decay and radiates only its
            // mistuning residue; it is what beats against the prompt line.
            let m = 2 * modes_padded + (n - 1);
            let sig = (sig_intr + p.anti_couple * g_v).clamp(0.1, 400.0);
            let r = (-sig * dt).exp();
            let fd = fn_hz * (1.0 + anti_sign * detune_cents * cents);
            let th = core::f64::consts::TAU * fd * dt;
            cr_sus[m] = (r * th.cos()) as f32;
            ci_sus[m] = (r * th.sin()) as f32;
            damp_mul[m] = (-sigma_d * dt).exp() as f32;
            gin[m] = gin_n as f32;
            gout[m] = (base * p.anti_gain) as f32;
        }
    }

    // --- voicing: even response across the compass ----------------------
    // Estimate the mezzo-forte force pulse this key's hammer produces (felt
    // compression from the impact energy, effective stiffness, half-sine
    // contact time bounded below by the string-impedance relaxation), weight
    // each mode by that pulse's spectrum, and normalise the resulting
    // excitation-weighted response. This equalises what is actually heard
    // across the compass — the same thing a technician does when voicing —
    // without touching the modal structure inside a note.
    let k_eff = felt_p * felt_k * u_mf.powf(felt_p - 1.0) * 2.0; // incl. lock-up onset

    let tau_felt = core::f64::consts::PI * (hammer_mass / k_eff).sqrt();
    let tau_z = hammer_mass / (2.0 * z_total);
    let tau = tau_felt.max(tau_z);
    let momentum = 2.0 * hammer_mass * v_mf;
    let mut nrm = 0.0f64;
    for osc in 0..n_osc {
        for n in 1..=modes_per_osc {
            let m = osc * modes_padded + (n - 1);
            let fn_hz = n as f64 * f0 * (1.0 + b_coeff * (n * n) as f64).sqrt();
            let pulse = momentum / (1.0 + (2.0 * fn_hz * tau).powi(2));
            let rad = crate::soundboard::radiativity(fn_hz, p); // R(f), see soundboard.rs
            let amp = (gout[m] as f64).hypot(gout_re[m] as f64);
            let g = gin[m] as f64 * amp * pulse * rad;
            nrm += g * g;
        }
    }
    // The modal state accumulates per-sample force, so grows with fs; the
    // 1/fs in the raw gout compensates, but normalising by nrm (itself
    // proportional to 1/fs) would cancel that again. Keep the response
    // sample-rate independent explicitly.
    // The top ~1.5 octaves radiate as short spikes whose peaks sat 10+ dB
    // above the rest of the compass at equal velocity (crest, not power) —
    // taper them so fortissimo treble stings without eating the whole
    // output headroom.
    let taper = 1.0 / (1.0 + p.top_taper * ((t - 0.75).max(0.0) / 0.25).powi(2));
    let trim = ((p.trim_ref * taper / nrm.sqrt().max(1e-18)) * (48000.0 / sample_rate)) as f32;
    for g in gout.iter_mut() {
        *g *= trim;
    }
    for g in gout_re.iter_mut() {
        *g *= trim;
    }
    for (m, lift) in uni_lift.iter().enumerate() {
        gout[m] = (gout[m] as f64 * lift) as f32;
        gout_re[m] = (gout_re[m] as f64 * lift) as f32;
    }

    // Phantom audibility is a WOUND-string phenomenon; on plain wire the
    // longitudinal series is a faint colour, not a voice (a forte C4/C5
    // bank once measured as one isolated inharmonic tone at ~6.3 kHz on
    // every note — "bell"). Gated down past the wound/plain transition
    // (idx 24, t~0.28): full on the wound bass, -20 dB by C4, and switched
    // off above that (its modal products are not computed there).
    let ph_wound = 1.0 / (1.0 + (((t - 0.28) / 0.10).max(0.0)).powi(2));
    let ph_level = ph_wound * ((1.0 - t) + 0.05).powf(p.ph_taper);

    // --- longitudinal / phantom bank ------------------------------------
    // Tension modulation couples the transverse partials into the
    // string's longitudinal modes (Bank & Sujbert, JASA 2005): mode k is
    // driven by the k-th spatial component of d/dx (y_x)^2, i.e. by the
    // modal products q_m q_n with |m - n| = k or m + n = k — sum and
    // difference frequencies of partial PAIRS, resonantly amplified near
    // the longitudinal frequencies (the phantom partials of the bass).
    // voice.rs forms those products from the vertical modal states.
    // Longitudinal wave speed: plain wire is bulk steel (~5100 m/s); on
    // wound strings the copper adds transverse mass but almost no
    // longitudinal stiffness, so c_long scales by sqrt(core/total mass).
    let mu_core = if idx < 24 { 0.010 } else { mu };
    let c_long = 5100.0 * (mu_core / mu).sqrt();
    let f_l1 = p.ph_ratio * f0 * c_long / c_wave;
    let mut ph_cr = vec![0.0f32; PH_MODES];
    let mut ph_ci = vec![0.0f32; PH_MODES];
    let mut ph_gin = vec![0.0f32; PH_MODES];
    let mut ph_gout = vec![0.0f32; PH_MODES];
    for m in 1..=PH_MODES {
        let f = m as f64 * f_l1;
        if f >= (0.45 * sample_rate).min(9500.0) {
            break;
        }
        let i = m - 1;
        let sigma = (p.ph_sigma + p.ph_sigma_slope * f / 1000.0).min(400.0);
        let r = (-sigma * dt).exp();
        let th = core::f64::consts::TAU * f * dt;
        ph_cr[i] = (r * th.cos()) as f32;
        ph_ci[i] = (r * th.sin()) as f32;
        // the spatial derivative of the k-th component: drive ~ k
        ph_gin[i] = m as f32;
        // Same output convention as the soundboard bank (sigma * 0.006):
        // resonant gain ~ sigma/(1-r) is normalised out, so ph_gain is a
        // plateau-comparable level, not a raw state gain.
        ph_gout[i] = ((1.0 / (m as f64).powf(p.ph_tilt)) * sigma * 0.006 * (48000.0 / sample_rate)) as f32;
    }
    // Slope amplitude of partial n per unit modal state: the state is
    // force-normalised (displacement ~ Im z / f_n), slope ~ n x displacement.
    let ph_parents = if ph_level >= 0.1 { modes_per_osc.min(PH_PARENTS) } else { 0 };
    let mut ph_slope = [0.0f32; PH_PARENTS];
    for (i, w) in ph_slope.iter_mut().enumerate().take(ph_parents) {
        let n = (i + 1) as f64;
        *w = (1.0 / (1.0 + b_coeff * n * n).sqrt()) as f32;
    }

    // --- attack complex parameters --------------------------------------
    let njit = 1.0 + 0.25 * sc * kj(idx, 5);
    let thump_amp = (p.thump_amp * njit) as f32;
    let click_amp = (p.click_amp * (1.0 + 0.25 * sc * kj(idx, 7))) as f32;
    let click_hz = p.click_hz * (1.0 + 0.2 * sc * kj(idx, 8));

    // --- sympathetic bank ----------------------------------------------
    // First partials of this key's strings, rung by bridge vibration when the
    // damper is off the string. More damped than the main voice (the bridge
    // coupling that feeds them also drains them) and injected/tapped near the
    // bridge rather than at the strike point.
    // Partial depth scales with register: the audible sympathetic content
    // of a bass string is high in its series; treble strings only have a
    // few partials in-band anyway.
    let sym_cap: usize = if idx < 24 { 24 } else if idx < 56 { 16 } else { 12 };
    let mut sym_count = 0;
    for n in 1..=sym_cap {
        let fn_hz = n as f64 * f0 * (1.0 + b_coeff * (n * n) as f64).sqrt();
        if fn_hz >= 18000.0f64.min(0.44 * sample_rate) {
            break;
        }
        sym_count = n;
    }
    sym_count = sym_count.max(1);
    let sym_modes = pad8(sym_count);
    let mut sym_cr = vec![0.0f32; sym_modes];
    let mut sym_ci = vec![0.0f32; sym_modes];
    let mut sym_damp_mul = vec![1.0f32; sym_modes];
    let mut sym_gin = vec![0.0f32; sym_modes];
    let mut sym_gout = vec![0.0f32; sym_modes];
    for n in 1..=sym_count {
        let m = n - 1;
        let fn_hz = n as f64 * f0 * (1.0 + b_coeff * (n * n) as f64).sqrt();
        if fn_hz >= 0.499 * sample_rate {
            continue;
        }
        let fk = fn_hz / 1000.0;
        let f0k = f0 / 1000.0;
        let wound = (1.0 - t).powi(3);
        // wound-string law on the copper-wound keys, the plain-wire law
        // above the break (idx 24), blended over ~a fifth (params::a1_plain)
        let wg = if t > 0.28 { 1.0 / (1.0 + ((t - 0.28) / 0.05).powi(2)) } else { 1.0 };
        let a1w = wg * p.a1_wound + (1.0 - wg) * p.a1_plain;
        let a2w = wg * p.a2_wound;
        let sigma = ((sigma_fund
            + a1w * wound * (fk - f0k)
            + (a2 + a2w * wound) * (fk * fk - f0k * f0k)
            + p.a4 * (fk.powi(4) - f0k.powi(4)))
        .max(0.15)
            * 1.2)
            .min(400.0);
        let r = (-sigma * dt).exp();
        let theta = core::f64::consts::TAU * fn_hz * dt;
        sym_cr[m] = (r * theta.cos()) as f32;
        sym_ci[m] = (r * theta.sin()) as f32;
        let sigma_d = ((45.0 + fn_hz / 35.0) * damper_strength).min(2000.0);
        sym_damp_mul[m] = (-sigma_d * dt).exp() as f32;
        sym_gin[m] = (n as f64 * core::f64::consts::PI * 0.12).sin() as f32;
        let sign = if n % 2 == 1 { 1.0 } else { -1.0 };
        sym_gout[m] = (sign * tension * n as f64 / (mu * length * length * fn_hz * sample_rate)) as f32;
    }

    KeyDesign {
        f0: f0 as f32,
        b_coeff: b_coeff as f32,
        n_strings,
        n_osc,
        unison_detune: if n_osc == 3 { detune_cents * cents } else { 0.0 },
        anti_lock: p.anti_lock,
        after_spread: p.after_spread,
        side_modes: 0,
        side_l_im: vec![0.0; (n_osc - 1) * SIDE_MODES],
        side_l_re: vec![0.0; (n_osc - 1) * SIDE_MODES],
        side_r_im: vec![0.0; (n_osc - 1) * SIDE_MODES],
        side_r_re: vec![0.0; (n_osc - 1) * SIDE_MODES],
        modes_per_osc,
        modes_padded,
        total_modes,
        hammer_mass,
        felt_k,
        felt_p,
        felt_u_lock,
        felt_lock_w,
        core_k,
        u_core,
        felt_lambda,
        z_total,
        t1_seconds,
        speed_q,
        pulse_tau: tau,
        speed_pivot: p.v_mf,
        undamped,
        pan: key_pan(idx),
        rough_depth: p.rough_depth as f32,
        img_fc_mul: p.img_fc_mul,
        img_g_base: p.img_g_base,
        img_g_slope: p.img_g_slope,
        thump_amp,
        thump_lp_c: (1.0 - (-core::f64::consts::TAU * p.thump_hz / sample_rate).exp()) as f32,
        thump_len: (p.thump_ms * 0.001 * sample_rate) as u32,
        thump_vpow: p.thump_vpow as f32,
        click_amp,
        click_lp_c: (1.0 - (-core::f64::consts::TAU * click_hz / sample_rate).exp()) as f32,
        click_len: (p.click_ms * 0.001 * sample_rate) as u32,
        click_vpow: p.click_vpow as f32,
        knock_amp: p.knock_amp as f32,
        uc_third: p.uc_third as f32,
        knock_hp_c: (1.0 - (-core::f64::consts::TAU * p.knock_hp / sample_rate).exp()) as f32,
        cs_amp: p.cs_amp as f32,
        cs_len: (p.cs_ms * 0.001 * sample_rate * ((1.0 - t) + 0.12).powf(p.cs_taper)).max(16.0) as u32,
        cs_c_hi: (1.0 - (-core::f64::consts::TAU * p.cs_hi / sample_rate).exp()) as f32,
        cs_c_lo: (1.0 - (-core::f64::consts::TAU * p.cs_lo / sample_rate).exp()) as f32,
        cs_c_tilt: (1.0 - (-core::f64::consts::TAU * p.cs_tilt / sample_rate).exp()) as f32,
        cs_vpow: ((1.0 - bass_ramp) * p.cs_vpow + bass_ramp * p.cs_vpow_bass) as f32,
        cr_sus,
        ci_sus,
        damp_mul,
        gin,
        gout,
        gout_re,
        ph_cr,
        ph_ci,
        ph_gin,
        ph_gout,
        ph_gain: if ph_parents > 0 { (p.ph_gain * ph_level) as f32 } else { 0.0 },
        ph_slope,
        ph_parents,
        // Drive normalisation: a partial's modal state accumulates force
        // per sample, so its scale is the momentum a mezzo-forte blow
        // delivers (2 m v) times the sample rate. Dividing it out makes
        // ph_norm dimensionless and the quadratic LAW the same on every key.
        ph_drive: (p.ph_norm / (2.0 * hammer_mass * p.v_mf * sample_rate)) as f32,
        sym_modes,
        sym_cr,
        sym_ci,
        sym_damp_mul,
        sym_gin,
        sym_gout,
    }
}

/// Unison locking (Weinreich, JASA 1977). The strings of a unison share one
/// bridge. A partial whose bridge loss is large against its mistuning locks
/// its strings together: they swing in phase, the hammer's blow goes almost
/// wholly into that bridge-pumping motion, and only (mistuning / loss)^2 of
/// it is left in the anti-phase motion that outlasts it. A partial the
/// bridge barely drains, or one whose strings are far apart, keeps its
/// strings independent and its full aftersound. So the anti-phase mode's
/// output is scaled per partial by x^2 / sqrt(1 + x^4), x = mistuning (rad/s)
/// over anti_lock times the in-phase mode's extra decay rate (1/s): x^2 when
/// locked, 1 when free.
///
/// The fixed share this replaces gave every partial the same aftersound, 15
/// dB under its prompt. Low partials of the tenor and bass, which the bridge
/// drains at 15-60 dB/s and whose strings are a fraction of a hertz apart,
/// then fell fast and held on a plateau: C3's fundamental stood 15 dB over
/// its partials 2-6 at 1-2 s against the recording, where it falls in one
/// stage.
///
/// What the anti-phase mode gives up goes to the in-phase mode: the blow
/// sets the strings' sum, and the two modes share it. The prompt rates were
/// fitted with the fixed plateau in place, so a locked partial's rate had
/// been pushed up to make room for it; the energy the partial radiates over
/// `window` seconds from the strike is what the fit matched, and it is
/// kept: the in-phase mode slows until the pair radiates what it did, one
/// slower stage for a fast one and a plateau.
///
/// Runs once per key, after the calibration's decay scales: the rates that
/// decide the lock are the ones the key ends up with.
pub(crate) fn lock_unisons(key: &mut KeyDesign, sample_rate: f64, window: f64) {
    if key.n_osc != 3 || key.anti_lock <= 0.0 {
        return;
    }
    let sigma = |cr: f32, ci: f32| -((cr as f64).hypot(ci as f64)).ln() * sample_rate;
    // mean over the window of exp(-2 s t)
    let seen = |s: f64| {
        let u = 2.0 * s * window;
        if u < 1e-9 { 1.0 } else { (1.0 - (-u).exp()) / u }
    };
    for m in 0..key.modes_per_osc {
        let a = 2 * key.modes_padded + m;
        let vert = (key.gout[m] as f64).hypot(key.gout_re[m] as f64);
        let anti = (key.gout[a] as f64).abs();
        if anti == 0.0 || vert == 0.0 {
            continue;
        }
        let n = (m + 1) as f64;
        let f = n * key.f0 as f64 * (1.0 + key.b_coeff as f64 * n * n).sqrt();
        let (fitted, after) = (sigma(key.cr_sus[m], key.ci_sus[m]), sigma(key.cr_sus[a], key.ci_sus[a]));
        if fitted <= after {
            continue;
        }
        // mean power over the window of the two modes sounding together
        // (they start in phase and are a fraction of a hertz apart)
        let power = |v: f64, sv: f64, w: f64| v * v * seen(sv) + w * w * seen(after) + 2.0 * v * w * seen(0.5 * (sv + after));
        let total = power(vert, fitted, anti);
        let detune = core::f64::consts::TAU * f * key.unison_detune;
        // The lock depends on the rate and the rate on the lock; both
        // settle in a few rounds (a slower prompt only loosens the lock).
        let (mut prompt, mut keep) = (fitted, 1.0);
        for _ in 0..6 {
            let x2 = (detune / (key.anti_lock * (prompt - after).max(1e-9))).powi(2);
            keep = x2 / (1.0 + x2 * x2).sqrt();
            let (v, w) = (vert + (1.0 - keep) * anti, keep * anti);
            let (mut lo, mut hi) = (after, fitted);
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                if power(v, mid, w) > total { lo = mid } else { hi = mid }
            }
            prompt = 0.5 * (lo + hi);
        }
        let radius = (key.cr_sus[m] as f64).hypot(key.ci_sus[m] as f64);
        let factor = (-prompt / sample_rate).exp() / radius;
        key.cr_sus[m] = (key.cr_sus[m] as f64 * factor) as f32;
        key.ci_sus[m] = (key.ci_sus[m] as f64 * factor) as f32;
        let lift = 1.0 + (1.0 - keep) * anti / vert;
        key.gout[m] = (key.gout[m] as f64 * lift) as f32;
        key.gout_re[m] = (key.gout_re[m] as f64 * lift) as f32;
        key.gout[a] = (key.gout[a] as f64 * keep) as f32;
    }
}

/// The aftersound radiates in its own pattern. The prompt is the strings
/// moving up and down together: they push the bridge into the board. The
/// aftersound is what the bridge does not drain: the strings' horizontal
/// motion and their anti-phase motion, which push the bridge sideways and
/// rock it. The board answers that with another mixture of its modes, so at
/// two listening points the aftersound is not a scaled copy of the prompt.
///
/// With one path for all three string modes it was: where a partial's
/// prompt fell to the level of its aftersound the two met with the same
/// phase in both channels, and since the aftersound sits a fraction of a
/// hertz off the prompt that phase was near opposite for most partials of
/// the middle. Partial n of C4 fell 10-25 dB into a hole about 3/n seconds
/// after the strike and came back 6-12 dB, in both channels at once: a comb
/// of holes sweeping down through every note, and at C4 a fundamental that
/// read 42 dB/s over its first second where the recording falls a steady 25.
///
/// So each slow mode keeps cos(a) of its output on the prompt's path and
/// sends sin(a) = `after_spread` in the pattern orthogonal to it: where the
/// prompt reaches the two points as (L, R), that share arrives as
/// (-conj R, conj L). Its level, the mean power of the two channels, is
/// what it was. The interference between prompt and aftersound is cos(a) of
/// what it was in that mean, and where one channel dips the other fills.
///
/// `paths(f)` gives the prompt's steady-state path at `f` Hz to the left
/// and right outputs and the response of the instant path, which the taps
/// are fed into. Runs once per key, after the calibration: the calibration
/// counts each mode's level, and this leaves it alone.
pub(crate) fn spread_aftersound(key: &mut KeyDesign, sample_rate: f64, paths: &dyn Fn(f64) -> (C64, C64, C64)) {
    let s = key.after_spread.clamp(0.0, 1.0);
    if s == 0.0 {
        return;
    }
    let c = (1.0 - s * s).sqrt();
    let mut modes = 0;
    for m in 0..key.modes_per_osc.min(SIDE_MODES) {
        let n = (m + 1) as f64;
        if n * key.f0 as f64 * (1.0 + key.b_coeff as f64 * n * n).sqrt() > SIDE_HZ && m >= 8 {
            break;
        }
        modes = m + 1;
    }
    key.side_modes = pad8(modes).min(key.modes_padded).min(SIDE_MODES);
    for slot in 1..key.n_osc {
        for m in 0..modes {
            let i = slot * key.modes_padded + m;
            let j = (slot - 1) * SIDE_MODES + m;
            let res = C64::new(key.gout[i] as f64, key.gout_re[i] as f64);
            let f = (key.ci_sus[i] as f64).atan2(key.cr_sus[i] as f64) * sample_rate / core::f64::consts::TAU;
            if res.abs2() == 0.0 || f <= 0.0 {
                continue;
            }
            let (l, r, instant) = paths(f);
            if instant.abs2() < 1e-12 * (l.abs2() + r.abs2()) || instant.abs2() == 0.0 {
                continue; // nothing to carry the taps: the mode stays as it is
            }
            let wl = r.conj().mul(res).div(instant).scale(-s);
            let wr = l.conj().mul(res).div(instant).scale(s);
            key.side_l_im[j] = wl.re as f32;
            key.side_l_re[j] = wl.im as f32;
            key.side_r_im[j] = wr.re as f32;
            key.side_r_re[j] = wr.im as f32;
            key.gout[i] = (res.re * c) as f32;
            key.gout_re[i] = (res.im * c) as f32;
        }
    }
}

/// Player-perspective pan of key index `idx` (bass left).
pub(crate) fn key_pan(idx: usize) -> f32 {
    (-0.55 + 1.1 * (idx as f64 / 87.0)) as f32
}

/// MIDI velocity (1..=127) to hammer speed in m/s: ~0.3 m/s pianissimo to
/// ~7 m/s fortissimo (Askenfelt-Jansson's measured range). Two constraints
/// shaped this curve, in order:
/// - the MEZZO point is pinned: velocity 50 lands at 1.59 m/s, exactly
///   where the level calibration was done (real performance MIDI has its
///   median at velocity 40-55; an earlier steeper curve put those at
///   pianissimo speeds and whole pieces rendered ~25 dB under commercial
///   listening level)
/// - CONTRAST around that point is as wide as the mezzo pin allows: a C4
///   render spans ~27 dB from velocity 30 to 127 (real performances span
///   roughly 25-40 dB; a flatter earlier curve with a high floor measured
///   ~24 dB and read as "everything at the same loudness")
pub fn velocity_to_speed(vel: u8) -> f64 {
    let v = vel.min(127) as f64 / 127.0;
    0.10 + 7.2 * v.powf(1.70)
}
