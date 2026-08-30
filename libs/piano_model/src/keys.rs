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
// tuned per key:
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
// - the hammer strikes at x0/L ~ 0.132 (bass) to 0.082 (treble), giving the
//   familiar comb nulls near the 8th-12th partials via g_in = sin(n pi x0/L)

use crate::modal::pad8;

pub const FIRST_KEY: u8 = 21; // A0
pub const LAST_KEY: u8 = 108; // C8
pub const NUM_KEYS: usize = 88;
/// Keys above this MIDI number have no damper on a real instrument.
pub const TOP_DAMPED_KEY: u8 = 88; // E6 is the last dampered key here

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
    // Flat per-mode tables, osc-major, each of length total_modes:
    pub cr_sus: Vec<f32>,            // sustain rotation, real
    pub ci_sus: Vec<f32>,            // sustain rotation, imag
    pub damp_mul: Vec<f32>,          // extra radius factor at full damper contact
    pub gin: Vec<f32>,               // hammer force injection weight
    pub gout: Vec<f32>,              // bridge force output weight
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

pub fn build_key(key: u8, sample_rate: f64) -> KeyDesign {
    let idx = (key - FIRST_KEY) as usize;
    let t = idx as f64 / 87.0;

    // --- tuning ---------------------------------------------------------
    let stretch_cents = ((idx as f64 - 45.0) / 42.0).powi(3) * 30.0;
    let f0 = 440.0 * ((key as f64 - 69.0) / 12.0 + stretch_cents / 1200.0).exp2();

    // --- inharmonicity --------------------------------------------------
    let b_coeff = 10f64.powf(-4.35 + 2.7 * t);

    // --- string scaling -------------------------------------------------
    // Real grand scales run ~1500 N on the wound bass singles down to
    // ~700-850 N across the plain-wire compass (Chaigne-Askenfelt's C4:
    // T = 670 N, mu = 6.3 g/m, L = 0.62 m). The old linear 1500-800t law
    // held 1100+ N through the mid keys, which (with the heavier wire
    // below) put the wave impedance the hammer works against at ~1.5x the
    // published values: the string could not yield and throw the hammer
    // back, so mid-key force pulses were long flat pedestals (dark, deep
    // 1 kHz sidelobe null) instead of early-peaked decaying humps.
    let tension = 700.0 + 800.0 * (1.0 - t).powi(4); // N
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
    // ~10-12 g bass, ~3-6 g treble). The old linear law left C4 at 8.1 g —
    // heavier than its own string, which pins the hammer in the "massive/
    // slow" limit: a -12 dB/oct excitation at every dynamic, i.e. dull, and
    // structurally unable to brighten toward the ff "light/fast" limit.
    let hammer_mass = 0.0036 + 0.0079 * (1.0 - t).powf(2.2);
    // Felt stiffness scaled so mf contact times land on the measured 4 ms
    // (bass) .. <1 ms (treble) and the C4 constant sits near the published
    // 4.5e9 N/m^p (ours: 4.9e9).
    let felt_k = 10f64.powf(7.9 + 4.0 * t);
    let felt_p = 2.4 + 1.0 * t;
    let felt_lambda = 1.0;
    // Mezzo-forte felt compression estimate; lock-up starts just below it,
    // and bites harder in the bass (thick, graded felt) than in the treble
    // (thin felt that is near its compacted state already).
    let v_mf = 2.0f64;
    let e_mf = 0.5 * hammer_mass * v_mf * v_mf;
    let u_mf = (e_mf * (felt_p + 1.0) / felt_k).powf(1.0 / (felt_p + 1.0));
    let felt_u_lock = 0.85 * u_mf;
    let felt_lock_w = 3.0 - 2.4 * t;
    let strike_pos = 0.132 - 0.05 * t.powf(1.3); // x0 / L
    let t1_seconds = 2.0 * strike_pos * length / c_wave;
    let z_total = z_char * n_strings as f64;

    // --- losses ---------------------------------------------------------
    let t60_fund = 20.0 * (1.0 - t).powf(1.4) + 0.5;
    let sigma_fund = 6.91 / t60_fund;
    // Quadratic-in-frequency viscoelastic/air loss. Measured against real
    // instruments: partials near 2 kHz on a mid key hold T60 ~1-2 s; the old
    // 0.9+0.7t killed them in ~0.7 s, which read as "damped/rubbery".
    let a2 = 0.42 + 0.33 * t;
    let damper_strength = 0.55 + 0.75 * t;
    let undamped = key > TOP_DAMPED_KEY;

    // --- unison detail --------------------------------------------------
    let detune_cents = 0.5 + 1.3 * t;
    let detune_pattern: [f64; 3] = if n_osc == 2 { [-0.5, 0.55, 0.0] } else { [0.0, 1.0, -0.85] };
    // Weinreich decay split between unison normal modes; the split collapses
    // toward the treble where short stiff unisons lock together and the
    // aftersound effect is weak.
    let base_mult: [f64; 3] = if n_osc == 2 { [1.45, 0.62, 1.0] } else { [1.55, 0.85, 0.5] };
    let spread = 1.0 - 0.7 * t;
    let sigma_mult: [f64; 3] = [
        1.0 + (base_mult[0] - 1.0) * spread,
        1.0 + (base_mult[1] - 1.0) * spread,
        1.0 + (base_mult[2] - 1.0) * spread,
    ];
    let osc_level = 1.0 / n_osc as f64;

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
            gin[m] = (n as f64 * core::f64::consts::PI * strike_pos).sin() as f32;
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
            let rad = crate::soundboard::radiativity(fn_hz); // R(f), see soundboard.rs
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
    let top_taper = 1.0 / (1.0 + 1.8 * ((t - 0.75).max(0.0) / 0.25).powi(2));
    let trim = ((1.4e-6 * top_taper / nrm.sqrt().max(1e-18)) * (48000.0 / sample_rate)) as f32;
    for g in gout.iter_mut() {
        *g *= trim;
    }

    // --- sympathetic bank ----------------------------------------------
    // First partials of this key's strings, rung by bridge vibration when the
    // damper is off the string. More damped than the main voice (the bridge
    // coupling that feeds them also drains them) and injected/tapped near the
    // bridge rather than at the strike point.
    let mut sym_count = 0;
    for n in 1..=12usize {
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
        cr_sus,
        ci_sus,
        damp_mul,
        gin,
        gout,
        sym_modes,
        sym_cr,
        sym_ci,
        sym_damp_mul,
        sym_gin,
        sym_gout,
    }
}

/// MIDI velocity (1..=127) to hammer speed in m/s: ~0.6 m/s pianissimo to
/// ~6 m/s fortissimo. Calibrated against real performance MIDI, where the
/// musically important range is velocity ~25-70 (medians 40-55 across a
/// classical corpus): those must land at mezzo hammer speeds (~1.5-2.5 m/s,
/// the Askenfelt-Jansson range), not down at pianissimo. The old 1.7
/// exponent put velocity 40 at 1.0 m/s and whole pieces rendered ~25 dB
/// under commercial listening level.
pub fn velocity_to_speed(vel: u8) -> f64 {
    let v = vel.min(127) as f64 / 127.0;
    0.16 + 5.9 * v.powf(1.52)
}
