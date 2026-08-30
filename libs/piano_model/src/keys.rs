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
//   dispersion law, with B rising from ~4.5e-5 (A0, long wound strings) to
//   ~2e-2 (C8, short stiff wire) — the Railsback-curve regime of a real piano
// - octave stretch: a cubic Railsback-style tuning curve (+30/-37 cents at
//   the extremes) so the inharmonic partials of low notes line up with the
//   fundamentals of high ones the way a tuner lays a piano
// - losses: sigma_n anchored to a fundamental T60 that falls from ~19 s at
//   A0 to ~0.7 s at C8, plus a quadratic-in-frequency viscoelastic/air term
//   so high partials die much faster than the fundamental
// - unisons: 1 string A0..E1, 2 strings F1..E2, 3 strings F2..C8, detuned by
//   ~0.5-1.8 cents; single-string notes get the two polarisations of the one
//   string instead. Each unison member gets a different decay-rate multiplier
//   — the Weinreich normal-mode picture of bridge-coupled strings — which is
//   what produces the prompt-sound/aftersound double decay and the slow
//   unison beating.
// - the hammer strikes at x0/L ~ 0.132 (bass) to 0.082 (treble), giving
//   comb dips near the 8th-12th partials via g_in = sin(n pi x0/L); the
//   dips have a floor (comb_fill) because the felt contact is wide, the
//   contact point wanders during the blow, and the termination is not
//   rigid — measured piano spectra show shallow dips, never deep nulls
// - longitudinal modes: each string's longitudinal wave speed (steel core;
//   the copper winding adds transverse mass but little longitudinal
//   stiffness) puts free longitudinal modes at m * f0 * (c_long/c_trans),
//   a bank per key that the squared bridge signal drives (voice.rs) — the
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

/// Longitudinal/phantom bank size (padded kernel lanes).
pub const PH_MODES: usize = 8;

pub struct KeyDesign {
    pub f0: f32,
    pub b_coeff: f32,
    pub n_strings: usize,
    pub n_osc: usize,
    pub modes_per_osc: usize,        // real modes per oscillator
    pub modes_padded: usize,         // padded to 8
    pub total_modes: usize,          // n_osc * modes_padded
    pub hammer_mass: f64,
    pub felt_k: f64,
    pub felt_p: f64,
    pub felt_u_lock: f64,
    pub felt_lock_w: f64,
    pub felt_lambda: f64,
    pub z_total: f64,                // wave impedance seen by the hammer (n_strings * Z)
    pub t1_seconds: f64,             // agraffe reflection round trip 2 x0 / c
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
    pub gout: Vec<f32>,              // bridge force output weight
    // Longitudinal / phantom bank (all length PH_MODES; ph_gain 0 = off):
    pub ph_cr: Vec<f32>,
    pub ph_ci: Vec<f32>,
    pub ph_gin: Vec<f32>,
    pub ph_gout: Vec<f32>,
    pub ph_gain: f32,
    pub ph_direct: f32,
    pub ph_hp_c: f32,
    pub ph_pre_c: f32,
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
        0..=11 => 128,
        12..=23 => 104,
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

pub fn build_key(key: u8, sample_rate: f64, p: &DesignParams) -> KeyDesign {
    let idx = (key - FIRST_KEY) as usize;
    let t = idx as f64 / 87.0;
    let sc = p.scatter;

    // --- tuning ---------------------------------------------------------
    let stretch_cents = ((idx as f64 - 45.0) / 42.0).powi(3) * 30.0;
    let f0 = 440.0 * ((key as f64 - 69.0) / 12.0 + stretch_cents / 1200.0).exp2();

    // --- inharmonicity --------------------------------------------------
    let b_coeff = 10f64.powf(p.b_lo + p.b_span * t) * (1.0 + 0.15 * sc * kj(idx, 6));

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
    // Single-string notes still get two oscillators: the two transverse
    // polarisations of the one string, which real bass strings exchange
    // energy between (that is where their double decay comes from).
    let n_osc = n_strings.max(2);

    // --- hammer ---------------------------------------------------------
    // Head mass ~11.5 g (A0) falling to ~3.6 g (C8), curved so the mid keys
    // land near the published effective striking masses (Chaigne-Askenfelt's
    // C4 simulation uses 2.9 g on a 3.9 g string; Hall/Giordano give
    // ~10-12 g bass, ~3-6 g treble).
    let hammer_mass = p.hm_base + p.hm_span * (1.0 - t).powf(p.hm_pow);
    // Felt stiffness scaled so mf contact times land on the measured 4 ms
    // (bass) .. <1 ms (treble); voicing scatter moves individual hammers
    // the way real felt varies needle-to-needle.
    let felt_k = 10f64.powf(p.feltk_lo + p.feltk_span * t) * (2f64).powf(0.5 * sc * kj(idx, 1));
    let felt_p = p.feltp_lo + p.feltp_span * t;
    let felt_lambda = 1.0;
    // Mezzo-forte felt compression estimate; lock-up starts just below it,
    // and bites harder in the bass (thick, graded felt) than in the treble
    // (thin felt that is near its compacted state already).
    let v_mf = p.v_mf;
    let e_mf = 0.5 * hammer_mass * v_mf * v_mf;
    let u_mf = (e_mf * (felt_p + 1.0) / felt_k).powf(1.0 / (felt_p + 1.0));
    let felt_u_lock = p.lock_frac * u_mf;
    let felt_lock_w = p.lockw_lo + (p.lockw_hi - p.lockw_lo) * t;
    let strike_pos =
        (p.spos_lo - (p.spos_lo - p.spos_hi) * t.powf(p.spos_pow)) * (1.0 + 0.04 * sc * kj(idx, 2));
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

    // --- unison detail --------------------------------------------------
    let detune_cents = (p.det_lo + p.det_slope * t) * (1.0 + 0.25 * sc * kj(idx, 4));
    let detune_pattern: [f64; 3] = if n_osc == 2 { [-0.5, 0.55, 0.0] } else { [0.0, 1.0, -0.85] };
    // Weinreich decay split between unison normal modes; the split collapses
    // toward the treble where short stiff unisons lock together and the
    // aftersound effect is weak.
    let base_mult: [f64; 3] = if n_osc == 2 { [1.45, 0.62, 1.0] } else { [1.55, 0.85, 0.5] };
    let spread = (1.0 - p.wein_treble * t) * p.wein;
    // Clamped: a large split scale must never drive a multiplier to zero or
    // negative (negative sigma = a growing mode). The contrast exponent
    // steepens the split over the top octaves (measured treble notes have a
    // much larger prompt/aftersound ratio than mids).
    let top_x = 1.0 + (p.wein_top - 1.0) * ((t - 0.55) / 0.45).clamp(0.0, 1.0);
    let sigma_mult: [f64; 3] = [
        (1.0 + (base_mult[0] - 1.0) * spread).max(0.05).powf(top_x),
        (1.0 + (base_mult[1] - 1.0) * spread).max(0.05).powf(top_x),
        (1.0 + (base_mult[2] - 1.0) * spread).max(0.05).powf(top_x),
    ];
    let osc_level = 1.0 / n_osc as f64;
    // in-phase-biased drive weights (see params.wein_inw)
    let mut in_w = [1.0f64; 3];
    if p.wein_inw != 0.0 {
        let mut sum = 0.0;
        for j in 0..n_osc {
            in_w[j] = sigma_mult[j].powf(p.wein_inw);
            sum += in_w[j];
        }
        let norm = n_osc as f64 / sum;
        for w in in_w.iter_mut() {
            *w *= norm;
        }
    }

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

    let dt = 1.0 / sample_rate;
    for osc in 0..n_osc {
        let f0d = f0 * (detune_pattern[osc] * detune_cents / 1200.0).exp2();
        let smult = sigma_mult[osc];
        for n in 1..=modes_per_osc {
            let m = osc * modes_padded + (n - 1);
            let fn_hz = n as f64 * f0d * (1.0 + b_coeff * (n * n) as f64).sqrt();
            if fn_hz >= 0.499 * sample_rate {
                continue; // stays a zero (dead) mode
            }
            let sigma = ((sigma_fund + a2 * ((fn_hz / 1000.0).powi(2) - (f0 / 1000.0).powi(2))).max(0.15) * smult).min(400.0);
            let r = (-sigma * dt).exp();
            let theta = core::f64::consts::TAU * fn_hz * dt;
            cr_sus[m] = (r * theta.cos()) as f32;
            ci_sus[m] = (r * theta.sin()) as f32;
            let sigma_d = ((45.0 + fn_hz / 35.0) * damper_strength).min(2000.0);
            damp_mul[m] = (-sigma_d * dt).exp() as f32;
            let s = (n as f64 * core::f64::consts::PI * strike_pos).sin();
            // comb dips have a floor: wide felt contact, wandering contact
            // point, non-rigid termination (sign kept; +1 at exact zeros)
            let filled = (s * s + p.comb_fill * p.comb_fill).sqrt();
            gin[m] = (in_w[osc] * if s < 0.0 { -filled } else { filled }) as f32;
            let sign = if n % 2 == 1 { 1.0 } else { -1.0 };
            gout[m] = (sign * osc_level * tension * n as f64 / (mu * length * length * fn_hz * sample_rate)) as f32;
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
            let g = gin[m] as f64 * gout[m] as f64 * pulse * rad;
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

    // --- longitudinal / phantom bank ------------------------------------
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
        ph_gin[i] = 1.0;
        // Same output convention as the soundboard bank (sigma * 0.006):
        // resonant gain ~ sigma/(1-r) is normalised out, so ph_gain is a
        // plateau-comparable level, not a raw state gain.
        ph_gout[i] = ((1.0 / (m as f64).powf(p.ph_tilt)) * sigma * 0.006 * (48000.0 / sample_rate)) as f32;
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
        let sigma = ((sigma_fund + a2 * ((fn_hz / 1000.0).powi(2) - (f0 / 1000.0).powi(2))).max(0.15) * 1.2).min(400.0);
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
        modes_per_osc,
        modes_padded,
        total_modes,
        hammer_mass,
        felt_k,
        felt_p,
        felt_u_lock,
        felt_lock_w,
        felt_lambda,
        z_total,
        t1_seconds,
        undamped,
        pan: (-0.55 + 1.1 * t) as f32,
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
        cs_vpow: p.cs_vpow as f32,
        cr_sus,
        ci_sus,
        damp_mul,
        gin,
        gout,
        ph_cr,
        ph_ci,
        ph_gin,
        ph_gout,
        // Per-key drive normalisation: the tension-modulation drive is
        // quadratic in the string's own bridge amplitude, which is several
        // times larger in the bass than the treble at equal dynamic (more
        // partials, heavier strings). Normalising to the key's typical mf
        // bridge amplitude keeps the quadratic LAW per key while placing
        // the ff phantom level comparably across the compass.
        ph_gain: (p.ph_gain * (0.29 + 0.67 * (1.0 - t).powf(2.4))) as f32,
        ph_direct: p.ph_direct as f32,
        ph_hp_c: (1.0 - (-core::f64::consts::TAU * p.ph_hp / sample_rate).exp()) as f32,
        ph_pre_c: (1.0 - (-core::f64::consts::TAU * 5200.0 / sample_rate).exp()) as f32,
        ph_drive: (p.ph_norm / (0.29 + 0.67 * (1.0 - t).powf(2.4))) as f32,
        sym_modes,
        sym_cr,
        sym_ci,
        sym_damp_mul,
        sym_gin,
        sym_gout,
    }
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
