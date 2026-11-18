//! One key: its strings as delay loops, the felt hammer inside them, and the
//! bridge junction they share.
//!
//! A loop holds the round trip of a velocity wave. Position `s` (samples
//! travelled since the bridge) runs 1..=delay; the first half is the wave
//! going to the agraffe, the second half the wave coming back, stored
//! negated so the rigid agraffe needs no sign change. The lumped filters and
//! the junction sit where the wave reaches the bridge.

use crate::design::{design_dispersion, design_loss, refine_dispersion, tune, tune_at, LoopFilters, MAX_SECTIONS};
use crate::scale::KeyScale;

/// Loops of a key: up to three strings and one for their sideways motion.
pub const MAX_LOOPS: usize = 4;

#[derive(Clone)]
struct StringLoop {
    buf: Vec<f32>,
    mask: usize,
    pos: usize,
    delay: usize,
    /// Strike taps: the outgoing wave at the hammer, `tap_out` samples
    /// back, and the wave returning from the agraffe, a strike span
    /// further. The span falls between whole samples, so that wave is
    /// read and written over four samples from `tap_back` on, with
    /// `tap_weights` (4-point Lagrange): the strike point's comb then moves
    /// smoothly from key to key and alike at every sample rate. Rounded to
    /// a whole sample, its first null jumped by a kHz or more between
    /// neighbouring keys in the fifth and sixth octaves.
    tap_out: usize,
    tap_back: usize,
    tap_weights: [f64; 4],
    eta: f64,
    tune_x: f64,
    tune_y: f64,
    loss: f64,
    ap: [[f64; 2]; MAX_SECTIONS],
    /// The wave below a few partials, which a damper's felt lets through
    /// in part.
    damp_low: f64,
    /// Share of the hammer force this string takes.
    weight: f64,
    /// Share of the hammer force that goes into this loop without the felt
    /// feeling its motion (the sideways loop).
    side: f64,
    /// How much of the bridge's motion this loop sees and causes: 1 for a
    /// string moving up and down, small for sideways motion.
    attach: f64,
    /// Junction: this loop's share in the bridge velocity.
    junction: f64,
}

#[derive(Clone, Default)]
struct Hammer {
    active: bool,
    pos: f64,
    vel: f64,
    /// Mean displacement of the struck strings at the strike point.
    string_pos: f64,
    compression: f64,
    force: f64,
    samples: u32,
    /// Length of the first contact in samples, once it has ended: to the
    /// moment within its last sample that the felt left the string, so a
    /// contact voiced in seconds comes out alike at any rate.
    first_contact: f64,
    touching: bool,
    peak_force: f64,
}

/// Fewest samples a period of its note that a string loop runs with. A
/// loop places its strike taps, its strike span and its hammer's contact in
/// whole samples (the span by interpolation, but never under one sample),
/// and with too few samples a period it cannot hold them: at 44.1 kHz from
/// F7 up (under 17 samples a period, 2-6 samples of delay line besides the
/// filters) the strike's taps overran the delay line, the span was held at
/// one sample and the blow's length moved by a tenth of a period, so the
/// top octave's tone depended on the rate. A shorter loop runs at twice the
/// rate and comes back through a half-band low-pass ([`HalfBand`]): C#7-C8
/// at 44.1 kHz, D#7-C8 at 48 kHz, none at 96 kHz.
const MIN_PERIOD_SAMPLES: f64 = 20.0;

/// Odd taps either side of the half-band low-pass's centre (39 taps).
const HALF_PAIRS: usize = 10;
const HALF_RING: usize = 64;

/// Takes a loop run at twice the rate back to the output's: a half-band
/// low-pass (Blackman-windowed sinc, every other tap zero) read at every
/// second sample. At 44.1 kHz out it is flat within 0.01 dB to 16 kHz and
/// 0.2 dB at 18 kHz; what the loop makes above the output's half rate
/// folds back above 20 kHz, 16 dB down from 24 kHz and 70 dB from 28 kHz.
#[derive(Clone)]
struct HalfBand {
    /// Weights of the odd taps either side of the centre, nearest first.
    weights: [f64; HALF_PAIRS],
    history: [f64; HALF_RING],
    at: usize,
}

impl HalfBand {
    fn new() -> Self {
        let mut weights = [0.0; HALF_PAIRS];
        for (j, w) in weights.iter_mut().enumerate() {
            let k = (2 * j + 1) as f64;
            let x = std::f64::consts::PI * k / (2 * HALF_PAIRS) as f64;
            let window = 0.42 + 0.5 * x.cos() + 0.08 * (2.0 * x).cos();
            *w = (std::f64::consts::FRAC_PI_2 * k).sin() / (std::f64::consts::PI * k) * window;
        }
        // unity at DC: the centre's half and twice the odd taps' sum
        let sum: f64 = weights.iter().sum();
        for w in &mut weights {
            *w *= 0.25 / sum;
        }
        HalfBand { weights, history: [0.0; HALF_RING], at: 0 }
    }

    fn push(&mut self, x: f64) {
        self.history[self.at] = x;
        self.at = (self.at + 1) & (HALF_RING - 1);
    }

    /// The low-passed signal, 2 * HALF_PAIRS - 1 samples behind the newest.
    fn take(&self) -> f64 {
        let back = |d: usize| self.history[(self.at + HALF_RING - 1 - d) & (HALF_RING - 1)];
        let centre = 2 * HALF_PAIRS - 1;
        let mut y = 0.5 * back(centre);
        for (j, &w) in self.weights.iter().enumerate() {
            let k = 2 * j + 1;
            y += w * (back(centre - k) + back(centre + k));
        }
        y
    }

    fn clear(&mut self) {
        self.history = [0.0; HALF_RING];
    }
}

#[derive(Clone)]
pub struct Voice {
    strings: Vec<StringLoop>,
    filters: LoopFilters,
    /// Strings the hammer meets; a sideways loop may follow them.
    struck: usize,
    /// The bridge yields less to fast motion than to slow (a board is
    /// springy at its low resonances and heavy above them). The strings
    /// feel this share of their bridge's motion at high frequencies, all
    /// of it at low ones, with the corner's one-pole coefficient; `yield_gain`
    /// keeps the first partial's drain where the scale puts it.
    yield_high: f64,
    yield_coef: f64,
    yield_gain: f64,
    yield_low: f64,
    /// Below the board's lowest modes the bridge hardly yields at all: the
    /// strings feel their bridge's motion less by `stiff_depth` there, by
    /// a one-pole low-pass with coefficient `stiff_coef`, whose output is
    /// `stiff_low` (0 depth: not at all).
    stiff_depth: f64,
    stiff_coef: f64,
    stiff_low: f64,
    phantom: Phantom,
    inv_2z: f64,
    hammer: Hammer,
    hammer_inv_mass: f64,
    hammer_k: f64,
    hammer_p: f64,
    hysteresis: f64,
    unevenness: f64,
    soft: bool,
    /// Seconds per sample at the rate the loop runs.
    dt: f64,
    /// Samples the loop runs per output sample (see [`MIN_PERIOD_SAMPLES`]).
    oversample: usize,
    decimator: HalfBand,
    damper_gain: f64,
    damper_now: f64,
    damper_target: f64,
    damper_slew: f64,
    /// The slew as built.
    damper_slew_built: f64,
    /// One-pole coefficient of the band the felt lets through, and the
    /// share of that band that escapes the damper.
    damper_low: f64,
    damper_escape: f64,
    /// Largest wave written during the last block, m/s.
    pub peak: f32,
    pub quiet_blocks: u32,
}

/// Longitudinal motion of the strings. Stretching a string as it swings
/// modulates its tension at the sums of its partials' frequencies; that
/// force reaches the bridge too, strongest near the string's longitudinal
/// modes. It is quadratic in the motion, so it colours forte playing (the
/// metallic edge of a hard bass note) and vanishes in soft playing.
#[derive(Clone, Default)]
struct Phantom {
    /// Share of the strength this key's phantoms get (see [`phantom_fade`]).
    fade: f64,
    /// 1 / the bridge velocity at which the phantom equals the motion.
    gain: f64,
    /// Share of the squared motion that goes out as it is, beside the
    /// resonators.
    direct: f64,
    /// High-pass (two one-pole stages) that removes the difference tones.
    hp_coef: f64,
    hp: [f64; 2],
    /// Two resonators on the first two longitudinal modes: 2 r cos w, r^2,
    /// input gain, and two states each.
    res: [[f64; 5]; 2],
}

/// Above about C5 the strings' longitudinal modes leave the audible range:
/// a key keeps all its phantoms while its first longitudinal mode is below
/// 7 kHz and none from 8.8 kHz, fading between (C#5 to E5) as a raised
/// cosine in log frequency, at every sample rate. The second modes of the
/// keys that keep any then stay under 17.6 kHz, within 0.4 of the rate
/// from 44.1 kHz up.
fn phantom_fade(longitudinal: f64) -> f64 {
    const FULL: f64 = 7000.0;
    const NONE: f64 = 8800.0;
    let t = (longitudinal / FULL).ln() / (NONE / FULL).ln();
    if t >= 1.0 {
        0.0
    } else {
        (0.5 * std::f64::consts::PI * t.max(0.0)).cos().powi(2)
    }
}

impl Phantom {
    fn new(key: &KeyScale, fs: f64, strength: f64, direct: f64) -> Self {
        let fade = phantom_fade(key.longitudinal);
        if fade == 0.0 {
            return Phantom::default();
        }
        let mut res = [[0.0; 5]; 2];
        for (m, r) in res.iter_mut().enumerate() {
            let f = key.longitudinal * (m + 1) as f64;
            if f < 0.4 * fs {
                let radius = (-std::f64::consts::PI * f / (12.0 * fs)).exp();
                *r = [2.0 * radius * (std::f64::consts::TAU * f / fs).cos(), radius * radius, 0.5 * (1.0 - radius * radius), 0.0, 0.0];
            }
        }
        Phantom { fade, gain: fade * strength.max(0.0), direct, hp_coef: (-std::f64::consts::TAU * (6.0 * key.f1).max(150.0) / fs).exp(), hp: [0.0; 2], res }
    }

    #[inline]
    fn run(&mut self, bridge: f64) -> f64 {
        let mut x = self.gain * bridge * bridge;
        for low in &mut self.hp {
            *low = x + self.hp_coef * (*low - x);
            x -= *low;
        }
        let mut out = self.direct * x;
        for r in &mut self.res {
            // y[n] = b (x[n] - x[n-2]) + 2 r cos w y[n-1] - r^2 y[n-2], with
            // x[n-2] folded into the second state
            let y = r[2] * x + r[3];
            r[3] = r[0] * y + r[4];
            r[4] = -r[2] * x - r[1] * y;
            out += y;
        }
        out
    }

    fn clear(&mut self) {
        self.hp = [0.0; 2];
        for r in &mut self.res {
            r[3] = 0.0;
            r[4] = 0.0;
        }
    }
}

/// What a piano technician sets: how the hammers are voiced, how clean the
/// unisons are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voicing {
    /// Added to every key's felt exponent: how much the felt stiffens as it
    /// is driven harder, which is how far tone follows touch.
    pub felt_exponent: f64,
    /// Felt hysteresis (Hunt-Crossley), s/m: the felt pushes harder while it
    /// is being compressed than while it recovers.
    pub felt_hysteresis: f64,
    /// Scales every hammer's contact time: below 1 is a harder, brighter
    /// set of hammers.
    pub contact: f64,
    /// Typical mistuning between the strings of a unison, cents.
    pub unison_cents: f64,
    /// How unevenly a hammer meets its strings (0.07 = 7 %).
    pub strike_unevenness: f64,
    /// Share of a blow that sets the strings moving sideways (0 = none).
    pub sideways: f64,
    /// How much the bridge yields to sideways motion, against 1 for the
    /// up-and-down motion that drives the board.
    pub sideways_attach: f64,
    /// How much the bridge yields to the strings well above
    /// `bridge_yield_above`, against 1 below it, for keys up to middle C (1 = alike at all
    /// frequencies). Less yield means the upper partials of a low note
    /// drain more slowly than its fundamental.
    pub bridge_yield: f64,
    /// The frequency above which the bridge yields less, Hz.
    pub bridge_yield_above: f64,
    /// How much less the bridge yields to slow motion than to motion above
    /// `bridge_stiff_below`, in the lowest octave (fading out by C3): the
    /// board is stiff below its lowest modes, so a bass note's lowest
    /// partials drain slowly and its upper ones at the rate the scale gives.
    pub bridge_stiffness: f64,
    /// The frequency below which the bridge stiffens, Hz.
    pub bridge_stiff_below: f64,
    /// Strength of the strings' longitudinal (phantom) components: 1 over
    /// the bridge velocity in m/s at which they would equal the motion
    /// that causes them (0 = none).
    pub phantoms: f64,
    /// Share of the squared motion that reaches the bridge as it is, beside
    /// the two longitudinal resonators (0 = the resonators only).
    pub phantom_direct: f64,
    /// Scales every string's high-frequency loss (the b3 f^2 term): above 1
    /// the upper partials die sooner.
    pub string_loss_high: f64,
    /// No hammer's felt is more than this many times as hard (its force at
    /// 1 mm of compression) as the median felt of the keys within an octave
    /// of it. From about C#2 to A2 the blow outlasts the wave's first return
    /// from the agraffe, and the returns, not the felt, end it: the contact
    /// shortens by one sample for 15-40 % harder felt, so the contact time
    /// cannot choose the felt there, and chasing it ends on felt like stone
    /// (a sharp edge in the strings, a wiry ring that stays in soft playing).
    /// Those keys take felt like their neighbours' and the longer contact
    /// that comes with it. Infinite: no limit.
    pub felt_ceiling: f64,
    /// How far, in dB, a key's mezzo-forte level may stand out from its
    /// register before it is evened. A key is regulated to the board's
    /// response smoothed over a third of an octave (a register's trend, as
    /// a technician evens it by ear); where one of the board's modes falls
    /// on its fundamental and lifts or sinks it by more than this, the
    /// excess is evened too. 0 evens every key's own level exactly (its
    /// treble then moves by what its fundamental lacks); infinite leaves
    /// every accident of the board as level.
    pub regulation_tolerance: f64,
}

impl Default for Voicing {
    fn default() -> Self {
        Voicing { felt_exponent: 0.6, felt_hysteresis: 0.08, contact: 1.0, unison_cents: 0.15, strike_unevenness: 0.07, sideways: 0.22, sideways_attach: 0.45, bridge_yield: 0.25, bridge_yield_above: 1500.0, bridge_stiffness: 0.97, bridge_stiff_below: 250.0, phantoms: 80.0, phantom_direct: 1.0, string_loss_high: 1.0, felt_ceiling: 1.25, regulation_tolerance: 3.0 }
    }
}

/// The bridge's stiffness at low frequencies is whole up to this key and
/// gone from `STIFF_NONE` on.
const STIFF_FULL: f64 = 33.0;
const STIFF_NONE: f64 = 48.0;

/// Hammer speed of the mezzo-forte blow the hammers are voiced at, m/s.
pub const VOICING_SPEED: f64 = 2.7;

/// 4-point Lagrange weights for a delay of `d` samples from the first of
/// the four (best between the middle two, 1 <= d < 2).
fn lagrange(d: f64) -> [f64; 4] {
    [-(d - 1.0) * (d - 2.0) * (d - 3.0) / 6.0, d * (d - 2.0) * (d - 3.0) / 2.0, -d * (d - 1.0) * (d - 3.0) / 2.0, d * (d - 1.0) * (d - 2.0) / 6.0]
}

/// Deterministic per-key variation in [-1, 1].
pub fn jitter(key: u32, salt: u32) -> f64 {
    let mut x = (key as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (salt as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 52) as f64 - 1.0
}

/// The loop filters of a key: stretch for as many partials as the allpass
/// order of its register holds, then the string's own loss.
pub fn loop_filters(key: &KeyScale, fs: f64) -> LoopFilters {
    loop_filters_with(key, fs, 1.0)
}

/// [`loop_filters`] with the string's high-frequency loss scaled by
/// `loss_high`.
pub fn loop_filters_with(key: &KeyScale, fs: f64, loss_high: f64) -> LoopFilters {
    let max_sections = match key.strings {
        1 => 16,
        2 => 14,
        _ => 12,
    };
    let mut filters = design_dispersion(key.f1, key.b, fs, max_sections, 14_000.0);
    design_loss(&mut filters, key.f1, key.b1, key.b3 * loss_high, fs);
    refine_dispersion(&mut filters, key.f1, key.b, fs, 12_000.0);
    filters
}

impl Voice {
    pub fn new(key: &KeyScale, fs: f64, voicing: &Voicing) -> Self {
        // A short loop runs at twice the output's rate (see
        // [`MIN_PERIOD_SAMPLES`]); everything here is built for the rate it
        // runs at.
        let oversample = if fs / key.f1 < MIN_PERIOD_SAMPLES { 2 } else { 1 };
        let fs = fs * oversample as f64;
        let n = key.strings;
        let filters = loop_filters_with(key, fs, voicing.string_loss_high);
        // Unison mistuning in cents: what a good tuning leaves. Above C6 it
        // narrows to none at the top: there the strings' own loss soon
        // matches the bridge's, and a detuned unison would leave a null and a
        // late plateau that the reference AU's top octave does not have.
        let top = crate::scale::curve(&[(84.0, 1.0), (108.0, 0.0)], key.midi as f64);
        let spread = voicing.unison_cents * top * (1.0 + 0.5 * jitter(key.midi as u32, 1));
        let cents: [f64; 3] = match n {
            1 => [0.0, 0.0, 0.0],
            2 => [-0.5 * spread, 0.5 * spread, 0.0],
            _ => [-spread, 0.15 * spread * jitter(key.midi as u32, 2), spread * (0.8 + 0.2 * jitter(key.midi as u32, 3))],
        };
        // First null of the strike comb on the stretched partial 1/strike.
        let n_null = 1.0 / key.strike;
        let f_null = key.f1 / (1.0 + key.b).sqrt() * n_null * (1.0 + key.b * n_null * n_null).sqrt();
        // A string also moves sideways, parallel to the board. The bridge
        // hardly yields that way, so this motion is driven little, heard
        // little and lasts long: it is most of what remains of a note after
        // its first seconds. One loop carries it for the whole unison. The
        // blow drives it against the sense in which it moves the bridge:
        // then what it gives the bridge joins the fading first sound
        // smoothly. Driven the other way the two cancel where they cross,
        // and every partial dips 10-15 dB before the aftersound appears.
        let sideways = voicing.sideways > 0.0;
        let sum_attach2 = n as f64 + if sideways { voicing.sideways_attach.powi(2) } else { 0.0 };
        let junction = |attach: f64| 2.0 * attach * key.z / (key.bridge_r + key.z * sum_attach2);
        let stiff_depth = voicing.bridge_stiffness * crate::scale::curve(&[(STIFF_FULL, 1.0), (STIFF_NONE, 0.0)], key.midi as f64);
        let stiff_coef = (-std::f64::consts::TAU * voicing.bridge_stiff_below / fs).exp();
        // Where the bridge stiffens it also pulls the strings' partials flat
        // (the stiffness's phase adds to each round trip's lag). A tuner
        // tunes the partials as they sound, so the loops are tuned that
        // much sharp.
        let stiff_pull = |f: f64| -> f64 {
            let w = std::f64::consts::TAU * f / fs;
            let (dr, di) = (1.0 - stiff_coef * w.cos(), stiff_coef * w.sin());
            let lag = n as f64 * junction(1.0) * stiff_depth * (1.0 - stiff_coef) * di / (dr * dr + di * di);
            1.0 + lag / std::f64::consts::TAU
        };
        let whole = tune(&filters, key.f1 * stiff_pull(key.f1), fs).delay;
        let strings = (0..n + sideways as usize)
            .map(|i| {
                let (cents, attach, side) = if i < n { (cents[i], 1.0, 0.0) } else { (0.5 + 0.3 * jitter(key.midi as u32, 5), voicing.sideways_attach, -voicing.sideways) };
                let f = key.f1 * 2f64.powf(cents / 1200.0);
                let t = tune_at(&filters, f * stiff_pull(f), fs, whole);
                let size = (t.delay + 2).next_power_of_two();
                let span = (fs / f_null).clamp(1.0, (t.delay as f64 - 4.0).max(1.0));
                let whole = span.floor() as usize;
                let tap_out = ((t.delay - whole) / 2).min(t.delay.saturating_sub(whole + 2)).max(1);
                StringLoop {
                    buf: vec![0.0; size],
                    mask: size - 1,
                    pos: 0,
                    delay: t.delay,
                    tap_out,
                    tap_back: tap_out + whole - 1,
                    tap_weights: lagrange(1.0 + span - whole as f64),
                    eta: t.eta,
                    tune_x: 0.0,
                    tune_y: 0.0,
                    loss: 0.0,
                    ap: [[0.0; 2]; MAX_SECTIONS],
                    damp_low: 0.0,
                    weight: 0.0,
                    side,
                    attach,
                    junction: junction(attach),
                }
            })
            .collect();
        // Up to middle C the bridge yields a quarter as much well above
        // 1.5 kHz as below it; the difference fades out by C6, where the
        // recording's upper partials fall as fast as its fundamentals.
        let yield_high = voicing.bridge_yield + (1.0 - voicing.bridge_yield) * ((key.midi as f64 - 60.0) / 24.0).clamp(0.0, 1.0);
        let yield_coef = (-std::f64::consts::TAU * voicing.bridge_yield_above / fs).exp();
        let yield_gain = {
            // 1 / |H| at the first partial, H = high + (1 - high) (1 - c) / (1 - c z^-1)
            let w = std::f64::consts::TAU * key.f1 / fs;
            let (dr, di) = (1.0 - yield_coef * w.cos(), yield_coef * w.sin());
            let d = dr * dr + di * di;
            let (re, im) = (yield_high + (1.0 - yield_high) * (1.0 - yield_coef) * dr / d, -(1.0 - yield_high) * (1.0 - yield_coef) * di / d);
            1.0 / (re * re + im * im).sqrt()
        };
        let mut voice = Voice {
            strings,
            filters,
            struck: n,
            yield_high,
            yield_coef,
            yield_gain,
            yield_low: 0.0,
            stiff_depth,
            stiff_coef,
            stiff_low: 0.0,
            phantom: Phantom::new(key, fs, voicing.phantoms, voicing.phantom_direct),
            inv_2z: 0.5 / key.z,
            hammer: Hammer::default(),
            hammer_inv_mass: 1.0 / key.hammer_mass,
            hammer_k: 1e9,
            hammer_p: key.hammer_p + voicing.felt_exponent,
            hysteresis: voicing.felt_hysteresis,
            unevenness: voicing.strike_unevenness,
            soft: false,
            dt: 1.0 / fs,
            oversample,
            decimator: HalfBand::new(),
            damper_gain: if key.damper_t60 > 0.0 { (-6.908 / (key.damper_t60 * key.f1)).exp() } else { 1.0 },
            damper_now: 1.0,
            damper_target: 1.0,
            // Felt takes time to settle on a string: longest on the heavy
            // bass strings. And it stops the upper partials first; the low
            // ones of a heavy string push through it for a while.
            damper_slew: 1.0 / (crate::scale::curve(&[(21.0, 0.05), (60.0, 0.025), (91.0, 0.012)], key.midi as f64) * fs),
            damper_slew_built: 1.0 / (crate::scale::curve(&[(21.0, 0.05), (60.0, 0.025), (91.0, 0.012)], key.midi as f64) * fs),
            damper_low: (-std::f64::consts::TAU * 3.0 * key.f1 / fs).exp(),
            damper_escape: crate::scale::curve(&[(21.0, 0.5), (60.0, 0.3), (91.0, 0.15)], key.midi as f64),
            peak: 0.0,
            quiet_blocks: 0,
        };
        voice.set_soft(false, key.midi);
        voice.voice_hammer(key.contact * voicing.contact * fs);
        voice
    }

    /// Voice the hammer: choose the felt stiffness whose first contact at a
    /// mezzo-forte blow lasts `target` samples (harder felt leaves sooner).
    fn voice_hammer(&mut self, target: f64) {
        // Contact time is not monotonic in stiffness: felt that is too hard
        // stops dead on the string and waits for the wave to return from
        // the agraffe. So walk up from soft felt to the first stiffness that
        // is short enough, then refine between the last two steps.
        let (mut soft, mut hard) = (1e3f64.ln(), 1e15f64.ln());
        let mut k = soft;
        // (contact, stiffness) of the shortest contact on the way up
        let mut shortest = (f64::INFINITY, hard);
        let mut target = target;
        while k < hard {
            let contact = self.contact_at(k.exp(), target);
            if contact <= target {
                hard = k;
                break;
            }
            if contact < shortest.0 {
                shortest = (contact, k);
            } else if contact > 1.02 * shortest.0 {
                // Past the shortest contact this felt can give: harder only
                // stays on longer, and far harder stops dead and hits the
                // string like stone (a buzz of clicks in a short bass
                // string). Settle for felt just shy of that shortest contact.
                target = 1.02 * shortest.0;
                (soft, hard) = (shortest.1 - 0.4, shortest.1);
                break;
            }
            soft = k;
            k += 0.4;
        }
        for _ in 0..11 {
            let mid = 0.5 * (soft + hard);
            if self.contact_at(mid.exp(), target) > target {
                soft = mid;
            } else {
                hard = mid;
            }
        }
        self.hammer_k = hard.exp();
        self.clear();
    }

    /// Samples of first contact of a mezzo-forte blow with felt stiffness
    /// `k` (infinite if the felt is still on the string well past `target`).
    fn contact_at(&mut self, k: f64, target: f64) -> f64 {
        let mut scratch = [0.0f32; 32];
        self.hammer_k = k;
        self.clear();
        self.strike(VOICING_SPEED);
        let mut ran = 0.0;
        while self.hammer.active && self.hammer.first_contact == 0.0 && ran < 1.5 * target + 32.0 {
            self.process(None, &mut scratch, &mut [0.0; 32]);
            ran += (scratch.len() * self.oversample) as f64;
        }
        if self.hammer.first_contact == 0.0 { f64::INFINITY } else { self.hammer.first_contact }
    }

    /// Each string's partial `n`, Hz, where its loop puts it.
    #[cfg(test)]
    pub(crate) fn string_partials(&self, n: usize) -> Vec<f64> {
        let fs = 1.0 / self.dt;
        self.strings.iter().map(|s| crate::design::loop_partial(&self.filters, crate::design::Tuning { delay: s.delay, eta: s.eta }, n, fs)).collect()
    }

    /// The felt's force in N at 1 mm of compression: how hard it is.
    pub(crate) fn felt_at_mm(&self) -> f64 {
        self.hammer_k * 1e-3f64.powf(self.hammer_p)
    }

    /// Soften the felt to at most `force` N at 1 mm of compression.
    pub(crate) fn limit_felt(&mut self, force: f64) {
        if self.felt_at_mm() > force {
            self.hammer_k = force / 1e-3f64.powf(self.hammer_p);
            self.clear();
        }
    }

    /// (seconds of first contact, peak force in N) of the last blow.
    pub fn last_blow(&self) -> (f64, f64) {
        (self.hammer.first_contact * self.dt, self.hammer.peak_force)
    }

    /// Una corda: the hammer moves sideways, misses a string and meets
    /// softer, less worn felt.
    pub fn set_soft(&mut self, on: bool, midi: u8) {
        self.soft = on;
        let uneven = self.unevenness * (1.0 + 0.4 * jitter(midi as u32, 4));
        let w: [f64; 3] = match (self.struck, on) {
            (1, _) => [1.0, 0.0, 0.0],
            (2, false) => [0.5 + uneven, 0.5 - uneven, 0.0],
            (2, true) => [0.85, 0.15, 0.0],
            (_, false) => [1.0 / 3.0 + uneven, 1.0 / 3.0 - 0.2 * uneven, 1.0 / 3.0 - 0.8 * uneven],
            (_, true) => [0.5, 0.47, 0.03],
        };
        for (s, w) in self.strings.iter_mut().zip(w) {
            s.weight = w;
        }
    }

    /// The hammer leaves the jack at `speed` m/s; the strings keep whatever
    /// motion they have.
    pub fn strike(&mut self, speed: f64) {
        self.hammer = Hammer { active: true, vel: speed, ..Hammer::default() };
        self.quiet_blocks = 0;
    }

    /// Strength of the phantom partials (see [`Voicing::phantoms`]); keys
    /// whose longitudinal modes are out of range have none, and the keys
    /// where they leave it less.
    pub fn set_phantoms(&mut self, strength: f64) {
        self.phantom.gain = self.phantom.fade * strength.max(0.0);
    }

    /// Measurement aid: the damper takes `times` as long to settle as built.
    pub fn set_damper_slew(&mut self, times: f64) {
        self.damper_slew = self.damper_slew_built / times.max(1e-3);
    }

    /// 1 = damper fully on the strings, 0 = lifted.
    pub fn set_damper(&mut self, amount: f64) {
        // A damper that only brushes the strings takes far longer than one
        // resting on them, which is what half-pedalling plays with.
        self.damper_target = self.damper_gain.powf(amount.clamp(0.0, 1.0).powi(3));
    }

    pub fn hammer_active(&self) -> bool {
        self.hammer.active
    }

    pub fn clear(&mut self) {
        for s in &mut self.strings {
            s.buf.fill(0.0);
            s.tune_x = 0.0;
            s.tune_y = 0.0;
            s.loss = 0.0;
            s.damp_low = 0.0;
            s.ap = [[0.0; 2]; MAX_SECTIONS];
        }
        self.phantom.clear();
        self.decimator.clear();
        self.yield_low = 0.0;
        self.stiff_low = 0.0;
        // what the last blow measured stays readable
        self.hammer = Hammer { first_contact: self.hammer.first_contact, peak_force: self.hammer.peak_force, ..Hammer::default() };
        self.damper_now = self.damper_target;
        self.peak = 0.0;
        self.quiet_blocks = 0;
    }

    /// Felt force for the predicted compression `x0` when the force itself
    /// pushes string and hammer apart by `alpha` metres per newton.
    fn felt_force(&self, x0: f64, alpha: f64) -> f64 {
        if x0 <= 0.0 {
            return 0.0;
        }
        let k = self.hammer_k * if self.soft { 0.55 } else { 1.0 };
        let (p, prev, rate) = (self.hammer_p, self.hammer.compression, self.hysteresis / self.dt);
        let (mut lo, mut hi) = (0.0, x0 / alpha);
        let mut f = self.hammer.force.clamp(lo, hi);
        for _ in 0..12 {
            let x = x0 - alpha * f;
            let xp = k * x.max(0.0).powf(p);
            let h = (1.0 + rate * (x - prev)).max(0.0);
            let g = f - xp * h;
            if g > 0.0 {
                hi = f;
            } else {
                lo = f;
            }
            if g.abs() < 1e-9 * (1.0 + f) {
                break;
            }
            let slope = 1.0 + alpha * (if x > 0.0 { p * xp / x * h } else { 0.0 } + if h > 0.0 { xp * rate } else { 0.0 });
            let next = f - g / slope;
            f = if next > lo && next < hi { next } else { 0.5 * (lo + hi) };
        }
        f
    }

    fn hammer_step(&mut self) {
        let dt = self.dt;
        let (mut v_in, mut w2) = (0.0, 0.0);
        let before = self.hammer.compression;
        for s in &self.strings {
            let a = s.buf[s.pos.wrapping_sub(s.tap_out) & s.mask] as f64;
            let mut b = 0.0;
            for (k, &h) in s.tap_weights.iter().enumerate() {
                b += h * s.buf[s.pos.wrapping_sub(s.tap_back + k) & s.mask] as f64;
            }
            v_in += s.weight * (a - b);
            w2 += s.weight * s.weight;
        }
        let yield_ = w2 * self.inv_2z;
        let x0 = (self.hammer.pos + dt * self.hammer.vel) - (self.hammer.string_pos + dt * v_in);
        let alpha = dt * dt * self.hammer_inv_mass + dt * yield_;
        let force = self.felt_force(x0, alpha);
        let h = &mut self.hammer;
        h.vel -= dt * force * self.hammer_inv_mass;
        h.pos += dt * h.vel;
        h.string_pos += dt * (v_in + yield_ * force);
        h.compression = h.pos - h.string_pos;
        h.force = force;
        h.samples += 1;
        h.peak_force = h.peak_force.max(force);
        if force > 0.0 {
            h.touching = true;
        } else if h.touching && h.first_contact == 0.0 {
            // the felt's compression fell from `before` to `x0` (at most 0)
            // over this step: it left the string where that crossed zero
            let within = if before > 0.0 && x0 < before { (before / (before - x0)).clamp(0.0, 1.0) } else { 0.0 };
            h.first_contact = (h.samples - 1) as f64 + within;
        }
        if force > 0.0 {
            for s in &mut self.strings {
                let dv = force * (s.weight + s.side) * self.inv_2z;
                let i = s.pos.wrapping_sub(s.tap_out) & s.mask;
                s.buf[i] += dv as f32;
                for (k, &h) in s.tap_weights.iter().enumerate() {
                    let i = s.pos.wrapping_sub(s.tap_back + k) & s.mask;
                    s.buf[i] -= (h * dv) as f32;
                }
            }
        }
        // Done once the hammer has fallen away from the string, or after
        // 80 ms whatever happened.
        let h = &mut self.hammer;
        if (h.compression < -1.5e-3 && h.vel < 0.0) || h.samples as f64 * dt > 0.08 {
            h.active = false;
        }
    }

    /// Run the key for `out.len()` samples. `drive` is the bridge's own
    /// motion at this key (from the rest of the instrument), `out` receives
    /// the bridge velocity these strings cause, in m/s; the hammer's force
    /// in N is added to `blow` while it is on the strings.
    pub fn process(&mut self, drive: Option<&[f32]>, out: &mut [f32], blow: &mut [f32]) {
        match self.oversample {
            1 => self.run::<1>(drive, out, blow),
            _ => self.run::<2>(drive, out, blow),
        }
    }

    /// [`Voice::process`] with the loop run `TICKS` times per output sample.
    fn run<const TICKS: usize>(&mut self, drive: Option<&[f32]>, out: &mut [f32], blow: &mut [f32]) {
        let n = self.strings.len();
        let sections = self.filters.sections;
        let (c1, c2) = (self.filters.c1, self.filters.c2);
        let (g0, la) = (self.filters.loss_g0, self.filters.loss_a);
        let mut peak = 0.0f64;
        let phantoms = self.phantom.gain > 0.0;
        let stiff = self.stiff_depth > 0.0;
        let mut arrive = [0.0f64; MAX_LOOPS];
        for i in 0..out.len() {
            let push = drive.map_or(0.0, |drive| drive[i] as f64);
            let mut force = 0.0;
            for _ in 0..TICKS {
                if self.hammer.active {
                    self.hammer_step();
                }
                let mut sum = 0.0;
                for (s, arrive) in self.strings.iter_mut().zip(arrive.iter_mut()) {
                    let mut x = s.buf[s.pos.wrapping_sub(s.delay) & s.mask] as f64;
                    for (ap, (&c1, &c2)) in s.ap[..sections].iter_mut().zip(c1.iter().zip(c2.iter())) {
                        let y = c2 * x + ap[0];
                        ap[0] = c1 * (x - y) + ap[1];
                        ap[1] = x - c2 * y;
                        x = y;
                    }
                    s.loss = g0 * x + la * s.loss;
                    let y = s.eta * (s.loss - s.tune_y) + s.tune_x;
                    s.tune_x = s.loss;
                    s.tune_y = y;
                    *arrive = y;
                    sum += s.junction * y;
                }
                let caused = -sum;
                self.yield_low = caused + self.yield_coef * (self.yield_low - caused);
                let mut felt = self.yield_gain * (self.yield_high * caused + (1.0 - self.yield_high) * self.yield_low);
                if stiff {
                    self.stiff_low = felt + self.stiff_coef * (self.stiff_low - felt);
                    felt -= self.stiff_depth * self.stiff_low;
                }
                let bridge = felt + push;
                self.damper_now += (self.damper_target - self.damper_now).clamp(-self.damper_slew, self.damper_slew);
                let keep = self.damper_now;
                for (s, &arrive) in self.strings.iter_mut().zip(arrive[..n].iter()) {
                    let mut w = arrive + s.attach * bridge;
                    if keep < 1.0 {
                        s.damp_low = w + self.damper_low * (s.damp_low - w);
                        w -= (1.0 - keep) * (w - self.damper_escape * s.damp_low);
                    } else {
                        s.damp_low = w;
                    }
                    s.buf[s.pos] = w as f32;
                    s.pos = (s.pos + 1) & s.mask;
                    peak = peak.max(w.abs());
                }
                let sound = if phantoms { caused + self.phantom.run(caused) } else { caused };
                if TICKS == 1 {
                    out[i] = sound as f32;
                } else {
                    self.decimator.push(sound);
                }
                if self.hammer.active {
                    force += self.hammer.force;
                }
            }
            if TICKS > 1 {
                out[i] = self.decimator.take() as f32;
            }
            if force != 0.0 {
                blow[i] += (force / TICKS as f64) as f32;
            }
        }
        self.peak = peak as f32;
    }
}
