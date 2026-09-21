// The kit: physical parameters of every voice. Measured against the
// Salamander Drumkit (Alexander Holm, CC BY-SA 3.0; overhead pair,
// 48 kHz/24-bit): an 18x18" kick, a 14x5" birch snare, a 12x7" rack tom, a
// 14x14" floor tom, 14" hi-hats, a 20" medium ride and an 18" medium crash.
// The 13" TomMid and 16" TomFloor have no reference sample: they are the two
// measured toms scaled by membrane physics (f ~ 1/diameter at like tension,
// air loading and radiation damping ~ diameter).
//
// Every number with an audible consequence was set against a measurement
// (scratchpad metrics.md holds the table); comments say which.

use crate::contact::Striker;
use crate::cymbal::{Bell, CymbalDesign};
use crate::membrane::{DampLaw, MembraneDesign, ResoHead};
use crate::rattle::{ClapDesign, RattleDesign};

// ---------------------------------------------------------------------------
// Strikers
// ---------------------------------------------------------------------------

/// Felt kick beater: ~45 g effective (head + rod share), 0.4-6 m/s (a
/// pedal cannot go much slower than 0.4 m/s and still hit). The contact
/// compliance is felt + the head's local dimple: a loose 18" head dents
/// ~10 mm under the beater, so K is far below a felt-on-steel value.
pub const KICK_BEATER: Striker = Striker {
    mass: 0.045,
    k: 1.5e5,
    p: 2.0,
    lambda: 0.4,
    r_point: 1.0e9, // two-way: the head is the modal bank
    v_min: 1.0,
    v_max: 6.0,
    v_curve: 2.0,
    f_max: 4000.0,
    timeout_ms: 30.0,
    relax_s: 0.0,
    retract: 8.0, // pedal return spring
};

/// Stick tip (acorn, hickory) on a Mylar head: ~12 g dynamic mass, 0.3-10
/// m/s. The compliance is the head's local dimple under the tip (the wood
/// is rigid by comparison): measured stick strokes deliver their impulse
/// over 2-5 ms at 50-200 N peak, which needs ~10 mm of local deflection at
/// fortissimo — K ~ 1e5-1e6 N/m^p, stiffer for the tighter snare head.
pub const STICK_HEAD: Striker = Striker {
    mass: 0.012,
    k: 6.0e5,
    p: 1.6,
    lambda: 0.25,
    r_point: 1.0e9,
    v_min: 0.35,
    v_max: 10.0,
    v_curve: 2.0,
    f_max: 4000.0,
    timeout_ms: 12.0,
    relax_s: 0.0,
    retract: 0.0,
};

pub const STICK_TOM: Striker = Striker { k: 1.0e6, ..STICK_HEAD };
pub const STICK_FLOOR: Striker = Striker { k: 6.0e5, v_max: 9.0, timeout_ms: 16.0, ..STICK_HEAD };

/// Stick shaft across the rim + head (side stick): heavier contact, wood on
/// metal, very stiff and short.
pub const STICK_RIM: Striker = Striker {
    mass: 0.030,
    k: 5.0e7,
    p: 1.5,
    lambda: 0.005,
    r_point: 1.0e9,
    v_min: 0.4,
    v_max: 6.0,
    v_curve: 2.0,
    f_max: 4000.0,
    timeout_ms: 4.0,
    relax_s: 0.0,
    retract: 0.0,
};

/// Stick tip on bronze: hard on hard, 0.1-0.3 ms (the tip's wood and the
/// plate's local dimple set K; a Hertz wood-on-bronze value would give a
/// one-sample pulse and peak forces in the tens of kN).
pub const STICK_CYMBAL: Striker = Striker {
    mass: 0.008,
    k: 3.0e9,
    p: 1.5,
    lambda: 0.005,
    r_point: 80.0, // 8 sqrt(D rho h) for 1 mm bronze
    v_min: 0.3,
    v_max: 9.0,
    v_curve: 2.0,
    f_max: 20000.0,
    timeout_ms: 4.0,
    relax_s: 0.0,
    retract: 0.0,
};

/// Stick shoulder into the crash edge: broader, softer contact.
pub const STICK_CRASH: Striker = Striker { mass: 0.02, k: 3.0e8, p: 1.5, v_max: 10.0, v_curve: 1.3, ..STICK_CYMBAL };

/// Hi-hat pedal: the whole top cymbal (0.6 kg) closes onto the bottom at
/// 0.3-1.5 m/s over a broad, compliant edge contact.
pub const HAT_PEDAL: Striker = Striker {
    mass: 0.15,
    k: 2.0e9,
    p: 1.5,
    lambda: 0.02,
    r_point: 80.0,
    v_min: 0.3,
    v_max: 1.5,
    v_curve: 1.0,
    f_max: 4000.0,
    timeout_ms: 20.0,
    relax_s: 0.0,
    retract: 0.0,
};

// ---------------------------------------------------------------------------
// Membranes
// ---------------------------------------------------------------------------

/// 18x18" kick, Remo Pinstripe batter (2 x 7 mil), coated Ambassador reso.
/// Reference: doublet 57 Hz (T60 ~0.1 s) over 42 Hz (T60 0.5-0.8 s), sub
/// band -2 dB re total in the first 50 ms, mid -21, high -23, air -41 (FF),
/// 12 ms attack.
pub const KICK: MembraneDesign = MembraneDesign {
    radius: 0.2286,
    depth: 0.457,
    f01: 43.0,
    sigma_area: 0.50,
    air_load: 1.2,
    n_modes: 40,
    damp: DampLaw { a: 7.0, b: 40.0, c: 0.8 },
    muffle: 3.0,
    strike_r: 0.12,
    strike_jitter: 0.04,
    tension_gamma: 0.0015,
    out_slope: 0.2,
    multipole: 0.15,
    ring: 0.8,
    out_gain: 0.257,
    reso: Some(ResoHead {
        ratio: 1.0,
        sigma_area: 0.25,
        damp: DampLaw { a: 4.5, b: 20.0, c: 0.6 },
        radiate: 0.25,
        air_spring: 0.075,
        sym_sig: 50.0,
        asym_sig: 0.0,
        n_pairs: 2,
        wire_inject: 0.0,
    }),
    // shell + high head modes carrying the beater's contact noise: the
    // reference 2-8 kHz band sits at -23 dB (0-50 ms) / -41 dB (50-200 ms)
    // re the whole hit, T60 0.2-0.3 s, air (>8 kHz) 18 dB under that.
    shell: &[
        (380.0, 0.30, 0.10),
        (560.0, 0.28, 0.12),
        (790.0, 0.26, 0.15),
        (1100.0, 0.25, 0.3),
        (1500.0, 0.2, 0.6),
        (2050.0, 0.15, 0.6),
        (2800.0, 0.15, 0.6),
        (3700.0, 0.15, 0.6),
        (4900.0, 0.15, 0.5),
        (6400.0, 0.14, 0.3),
        (8300.0, 0.13, 0.15),
        (10800.0, 0.12, 0.1),
    ],
    shell_drive: 1.0,
    shell_linear: 0.15,
    head_drive: 1.0,
    cavity: Some((58.0, 0.35, 0.10)),
    contact_noise: 0.12,
    striker: KICK_BEATER,
};

/// 14x5" birch snare, Evans Power Center Reverse Dot batter, thin snare-side
/// head tuned ~1.7x. Reference (snares off): 205 Hz (0,1) T60 0.4 s;
/// (snares on): low band -1 dB w0, mid -3 to -8, high -16 to -19, wires
/// add 14-25 dB above 300 Hz from 50 ms on.
pub const SNARE: MembraneDesign = MembraneDesign {
    radius: 0.1778,
    depth: 0.127,
    f01: 200.0,
    sigma_area: 0.30,
    air_load: 0.9,
    n_modes: 44,
    damp: DampLaw { a: 22.0, b: 30.0, c: 1.0 },
    muffle: 0.0,
    strike_r: 0.28,
    strike_jitter: 0.06,
    tension_gamma: 0.00004,
    out_slope: 0.15,
    multipole: 0.3,
    ring: 0.5,
    out_gain: 0.28103,
    reso: Some(ResoHead {
        ratio: 1.7,
        sigma_area: 0.10,
        damp: DampLaw { a: 2.0, b: 14.0, c: 0.6 },
        radiate: 0.3,
        air_spring: 0.12,
        sym_sig: 4.0,
        asym_sig: 0.0,
        n_pairs: 2,
        wire_inject: 0.1,
    }),
    shell: &[(560.0, 0.2, 0.05), (1130.0, 0.25, 0.06), (1780.0, 0.15, 0.06), (2600.0, 0.12, 0.06), (3900.0, 0.1, 0.06), (5800.0, 0.08, 0.05)],
    shell_drive: 1.0,
    shell_linear: 1.0,
    head_drive: 1.0,
    cavity: None,
    contact_noise: 0.02,
    striker: STICK_HEAD,
};

/// Wire/head formants excited by the wire impacts (Hz, T60 s, inject gain).
pub const SNARE_WIRES: &[(f64, f64, f64)] = &[
    (900.0, 0.08, 0.05),
    (1250.0, 0.07, 0.08),
    (1650.0, 0.06, 0.10),
    (2150.0, 0.06, 0.10),
    (2800.0, 0.05, 0.09),
    (3600.0, 0.05, 0.08),
    (4600.0, 0.045, 0.07),
    (5900.0, 0.04, 0.08),
    (7600.0, 0.035, 0.09),
    (9800.0, 0.03, 0.08),
    (12500.0, 0.025, 0.06),
];

pub const SNARE_RATTLE: RattleDesign = RattleDesign {
    threshold: 5.0e-7,
    scale: 250.0,
    attack_s: 0.025,
    release_s: 0.002,
    gain: 2200.0,
    max_ms: 0.0,
};

/// Side stick: the stick shaft laid across the rim, tip on the head. Almost
/// all shell/rim, little head. Reference: 1130 Hz dominant partial, mid band
/// -1 dB, centroid 1.4-2.2 kHz, T60 mid 0.2 s / high 0.4 s.
pub const SIDESTICK: MembraneDesign = MembraneDesign {
    n_modes: 24,
    strike_r: 0.6,
    strike_jitter: 0.05,
    tension_gamma: 0.0,
    out_gain: 0.027827,
    damp: DampLaw { a: 30.0, b: 20.0, c: 0.7 },
    shell: &[
        (560.0, 0.25, 0.002),
        (1130.0, 0.70, 0.040),
        (1420.0, 0.45, 0.013),
        (1780.0, 0.50, 0.020),
        (2350.0, 0.45, 0.040),
        (2900.0, 0.42, 0.045),
        (3700.0, 0.40, 0.050),
        (4800.0, 0.38, 0.045),
        (6200.0, 0.35, 0.040),
        (8100.0, 0.30, 0.030),
        (10500.0, 0.25, 0.020),
    ],
    shell_drive: 1.0,
    shell_linear: 1.0,
    head_drive: 0.20,
    contact_noise: 0.5,
    reso: Some(ResoHead { radiate: 0.3, ..SNARE.reso.unwrap() }),
    striker: STICK_RIM,
    ..SNARE
};

/// 12x7" rack tom. Reference: 137 Hz sustaining member (T60 1.9-2.0 s) with a
/// 142.6 Hz fast member; glide +5/+11/+15 Hz at P/F/FF at 5 ms; low band
/// -5 dB w0, mid -18..-21, high -29..-37.
pub const TOM_HIGH: MembraneDesign = MembraneDesign {
    radius: 0.1524,
    depth: 0.178,
    f01: 138.5,
    sigma_area: 0.35,
    air_load: 1.0,
    n_modes: 40,
    damp: DampLaw { a: 1.4, b: 25.0, c: 1.2 },
    muffle: 0.0,
    strike_r: 0.1,
    strike_jitter: 0.03,
    tension_gamma: 0.00012,
    out_slope: 0.2,
    multipole: 0.08,
    ring: 1.5,
    out_gain: 0.38128,
    reso: Some(ResoHead {
        ratio: 1.0,
        sigma_area: 0.25,
        damp: DampLaw { a: 0.5, b: 7.0, c: 0.8 },
        radiate: 0.5,
        air_spring: 0.025,
        sym_sig: 40.0,
        asym_sig: 0.0,
        n_pairs: 2,
        wire_inject: 0.0,
    }),
    shell: &[(760.0, 0.3, 0.05), (1240.0, 0.25, 0.08), (1900.0, 0.2, 0.1), (2800.0, 0.18, 0.12), (4200.0, 0.15, 0.12), (6300.0, 0.12, 0.09), (9000.0, 0.1, 0.06)],
    shell_drive: 1.0,
    shell_linear: 1.0,
    head_drive: 1.0,
    cavity: None,
    contact_noise: 0.6,
    striker: STICK_TOM,
};

/// 13x9" tom (interpolated between the measured 12" and 14").
pub const TOM_MID: MembraneDesign = MembraneDesign {
    radius: 0.1651,
    depth: 0.229,
    f01: 108.0,
    damp: DampLaw { a: 1.8, b: 25.0, c: 1.2 },
    tension_gamma: 0.00013,
    out_gain: 0.35745,
    reso: Some(ResoHead { damp: DampLaw { a: 0.7, b: 6.5, c: 0.8 }, sym_sig: 44.0, ..TOM_HIGH.reso.unwrap() }),
    shell: &[(660.0, 0.3, 0.05), (1080.0, 0.25, 0.08), (1650.0, 0.2, 0.1), (2500.0, 0.18, 0.12), (3800.0, 0.15, 0.12), (5700.0, 0.12, 0.09), (8200.0, 0.1, 0.06)],
    striker: Striker { r_point: 70.0, ..STICK_TOM },
    ..TOM_HIGH
};

/// 14x14" floor tom. Reference: 64-66 Hz sustaining (T60 sub 2.4-2.9 s),
/// glide +2/+6/+14 Hz, second partial 144-186 Hz; sub band -5..-7 dB w0,
/// low -9..-10, mid -14..-20, high -26..-32.
pub const TOM_LOW: MembraneDesign = MembraneDesign {
    radius: 0.1778,
    depth: 0.356,
    f01: 66.0,
    sigma_area: 0.35,
    air_load: 1.3,
    n_modes: 40,
    damp: DampLaw { a: 1.8, b: 25.0, c: 1.2 },
    muffle: 0.0,
    strike_r: 0.1,
    strike_jitter: 0.03,
    tension_gamma: 0.0016,
    out_slope: 0.2,
    multipole: 0.08,
    ring: 1.2,
    out_gain: 0.14617,
    reso: Some(ResoHead {
        ratio: 1.0,
        sigma_area: 0.25,
        damp: DampLaw { a: 0.7, b: 7.0, c: 0.8 },
        radiate: 0.3,
        air_spring: 0.04,
        sym_sig: 30.0,
        asym_sig: 0.0,
        n_pairs: 2,
        wire_inject: 0.0,
    }),
    shell: &[(520.0, 0.3, 0.12), (880.0, 0.25, 0.2), (1400.0, 0.2, 0.25), (2100.0, 0.18, 0.3), (3200.0, 0.15, 0.3), (4900.0, 0.12, 0.25), (7200.0, 0.1, 0.18)],
    shell_drive: 1.0,
    shell_linear: 1.0,
    head_drive: 1.0,
    cavity: None,
    contact_noise: 1.2,
    striker: STICK_FLOOR,
};

/// 16x16" floor tom (extrapolated from the 14": f ~ 1/D, more air loading).
pub const TOM_FLOOR: MembraneDesign = MembraneDesign {
    radius: 0.2032,
    depth: 0.406,
    f01: 56.0,
    air_load: 1.45,
    damp: DampLaw { a: 1.6, b: 25.0, c: 1.2 },
    tension_gamma: 0.0018,
    out_gain: 0.1426,
    reso: Some(ResoHead { damp: DampLaw { a: 0.6, b: 7.0, c: 0.8 }, sym_sig: 28.0, air_spring: 0.045, ..TOM_LOW.reso.unwrap() }),
    shell: &[(450.0, 0.3, 0.12), (760.0, 0.25, 0.2), (1200.0, 0.2, 0.25), (1800.0, 0.18, 0.3), (2800.0, 0.15, 0.3), (4300.0, 0.12, 0.25), (6500.0, 0.1, 0.18)],
    striker: Striker { r_point: 55.0, ..STICK_FLOOR },
    ..TOM_LOW
};

// ---------------------------------------------------------------------------
// Cymbals
// ---------------------------------------------------------------------------

/// 18" medium crash. Reference: T60 low 7-14 s, mid 4.4-6.6, high 4.8-6.4,
/// air 2.9-3.4; centroid 1.3 kHz (0-20 ms) -> 3.1 kHz -> 5.3 kHz (200-400 ms)
/// at FF; >4 kHz energy peaks at 155 ms, +12 dB late/early (FF), +5.6 (P).
pub const CRASH: CymbalDesign = CymbalDesign {
    n_modes: 136,
    f_low: 150.0,
    f_ring: 1600.0,
    f_top: 17000.0,
    damp: DampLaw { a: 0.8, b: 0.5, c: 0.6 },
    damp_spread: 0.3,
    clamp_sig: 0.0,
    clamp_f: 0.0,
    tier1_f: 1400.0,
    tier2_f: 3600.0,
    eps1: 1200.0,
    eps2: 6000.0,
    nl_damp: 1.5,
    disp_norm: 6000.0,
    out_slope: 0.35,
    out_gain: 0.016005,
    strike_f: 6000.0,
    bell: None,
    bell_strike: false,
    contact_noise: 0.25,
    direct_nl: 150.0,
    striker: STICK_CRASH,
    seed: 0x4c52_4153,
};

/// 20" medium ride, bow ping. Reference: T60 mid 13-20 s, high 4.5-7,
/// air 1.3-1.6; centroid 9 kHz (0-20 ms) -> 5-6.7 kHz -> 2.4-3.2 kHz.
pub const RIDE: CymbalDesign = CymbalDesign {
    n_modes: 124,
    f_low: 90.0,
    f_ring: 1300.0,
    f_top: 17000.0,
    damp: DampLaw { a: 0.22, b: 0.4, c: 1.15 },
    damp_spread: 0.3,
    clamp_sig: 0.0,
    clamp_f: 0.0,
    tier1_f: 1600.0,
    tier2_f: 5000.0,
    eps1: 0.05,
    eps2: 0.08,
    nl_damp: 0.3,
    disp_norm: 6000.0,
    out_slope: 0.4,
    out_gain: 0.012589,
    strike_f: 25000.0,
    bell: Some(Bell { f_lo: 900.0, f_hi: 9000.0, drive: 1.0, q_mult: 1.5, n_modes: 16 }),
    bell_strike: false,
    contact_noise: 0.5,
    direct_nl: 600.0,
    striker: STICK_CYMBAL,
    seed: 0x5249_4445,
};

/// Same ride struck on the bell. Reference: centroid 5.2 kHz (0-20 ms) ->
/// 2.7 kHz; T60 mid 9.5 s, high 4.1, air 1.3; low band -14 dB w0 (the bell
/// has real body around 900 Hz - 1.5 kHz).
pub const RIDE_BELL: CymbalDesign = CymbalDesign {
    bell_strike: true,
    strike_f: 9000.0,
    disp_norm: 20000.0,
    nl_damp: 0.0, // its E scale is 10x the ride's; the bell's low modes keep their Q
    out_gain: 0.01769,
    striker: Striker { v_max: 8.0, v_curve: 1.6, ..STICK_CYMBAL },
    ..RIDE
};

/// 14" hi-hat, top cymbal, open. Reference: t60 7.5-9 s; T60 mid 8-14 s,
/// high 5-7, air 1.5-3.5; air -5 dB w0, high -10..-13, mid -16.
pub const HAT_OPEN: CymbalDesign = CymbalDesign {
    n_modes: 104,
    f_low: 250.0,
    f_ring: 1200.0,
    f_top: 17500.0,
    damp: DampLaw { a: 0.4, b: 0.25, c: 1.1 },
    damp_spread: 0.3,
    clamp_sig: 0.0,
    clamp_f: 0.0,
    tier1_f: 2200.0,
    tier2_f: 6000.0,
    eps1: 0.2,
    eps2: 0.2,
    nl_damp: 0.4,
    disp_norm: 25000.0,
    out_slope: 0.15,
    out_gain: 0.29268,
    strike_f: 30000.0,
    bell: None,
    bell_strike: false,
    contact_noise: 3.0,
    direct_nl: 75.0,
    striker: STICK_CYMBAL,
    seed: 0x4841_5431,
};

/// Closed: pressed onto the bottom cymbal — long wavelengths clamped, every
/// band T60 0.2-0.3 s; centroid 12.7 kHz in the first 20 ms, air band -1 dB.
pub const HAT_CLOSED: CymbalDesign = CymbalDesign {
    damp: DampLaw { a: 10.0, b: 8.0, c: 0.5 },
    damp_spread: 0.0,
    clamp_sig: 20.0,
    clamp_f: 4000.0,
    out_slope: 0.9,
    eps1: 0.0,
    eps2: 0.0,
    nl_damp: 0.0,
    out_gain: 0.015217,
    strike_f: 30000.0,
    seed: 0x4841_5432,
    ..HAT_OPEN
};

pub const HAT_CHATTER: RattleDesign = RattleDesign {
    threshold: 0.15,
    scale: 1.0,
    attack_s: 0.0001,
    release_s: 0.0006,
    gain: 0.08,
    max_ms: 60.0,
};

/// Pedal ("chick"): the cymbals clap together. Reference: 12 ms attack,
/// centroid 10-11.5 kHz, T60 0.2 s, air -1.7 dB w0, t40 154 ms.
pub const HAT_PEDAL_D: CymbalDesign = CymbalDesign {
    damp: DampLaw { a: 14.0, b: 6.0, c: 0.6 },
    clamp_sig: 5.0,
    clamp_f: 5000.0,
    out_slope: 0.6,
    eps1: 0.0,
    eps2: 0.0,
    nl_damp: 0.0,
    out_gain: 0.018332,
    strike_f: 20000.0,
    striker: HAT_PEDAL,
    seed: 0x4841_5433,
    ..HAT_OPEN
};

pub const PEDAL_CHATTER: RattleDesign = RattleDesign {
    threshold: 0.15,
    scale: 2.0,
    attack_s: 0.0002,
    release_s: 0.002,
    gain: 1.2,
    max_ms: 80.0,
};

// ---------------------------------------------------------------------------
// Clap
// ---------------------------------------------------------------------------

pub const CLAP: ClapDesign = ClapDesign {
    burst_decay_s: 0.0025,
    burst_spacing_s: 0.0095,
    spacing_jitter_s: 0.0025,
    tail_level: 0.22,
    tail_decay_s: 0.045,
    gain: 2500.0,
};

/// Clap body formants (Hz, T60 s, inject gain): the cupped-hand cavity and
/// the hands themselves, 0.9-9 kHz, peaking 1.2-2.3 kHz.
pub const CLAP_BODY: &[(f64, f64, f64)] = &[
    (880.0, 0.035, 0.45),
    (1150.0, 0.045, 0.85),
    (1420.0, 0.05, 1.0),
    (1780.0, 0.05, 1.0),
    (2200.0, 0.045, 0.9),
    (2750.0, 0.04, 0.5),
    (3400.0, 0.035, 0.32),
    (4300.0, 0.03, 0.2),
    (5600.0, 0.025, 0.12),
    (7400.0, 0.02, 0.07),
    (9800.0, 0.018, 0.04),
];
pub const CLAP_OUT_GAIN: f32 = 0.004;

// ---------------------------------------------------------------------------
// Master and stereo placement (drummer's perspective; constant power)
// ---------------------------------------------------------------------------

/// Kit master. The per-voice out_gains are balanced like the reference
/// kit's sfz (snare fortissimo ~ -6 dBFS peak); this brings the whole kit
/// down so a full-velocity groove with coincident kick + snare + clap +
/// crash + hats + ride + bell + toms stays under 0 dBFS (the sound.rs groove
/// fires eight voices at once at velocity 1.0 and peaks -1 dBFS).
pub const MASTER: f32 = 0.2;

pub const PAN: [f32; 14] = [
    0.0,   // Kick
    0.0,   // Snare
    0.0,   // SideStick
    -0.32, // HiHatClosed
    -0.32, // HiHatOpen
    -0.32, // HiHatPedal
    -0.18, // TomHigh
    0.02,  // TomMid
    0.20,  // TomLow
    0.34,  // TomFloor
    0.36,  // Ride
    0.36,  // RideBell
    -0.24, // Crash
    0.10,  // Clap
];
