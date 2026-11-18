//! The instrument in SI units: per key, the string, the hammer and the
//! bridge it ends on. Lengths, diameters and masses follow smooth curves
//! through a few anchor keys of a concert grand; tension, wave impedance and
//! inharmonicity are derived from them, never set by hand.

pub const FIRST_KEY: u8 = 21;
pub const LAST_KEY: u8 = 108;
pub const KEYS: usize = 88;

/// Young's modulus and density of steel music wire.
const E_STEEL: f64 = 2.0e11;
const RHO_STEEL: f64 = 7850.0;

#[derive(Clone, Debug)]
pub struct KeyScale {
    pub midi: u8,
    /// Frequency of the first partial after stretch tuning, Hz.
    pub f1: f64,
    /// Inharmonicity coefficient: f_n = n f0 sqrt(1 + B n^2).
    pub b: f64,
    /// Wave impedance of one string, kg/s.
    pub z: f64,
    /// Strings the hammer meets (1, 2 or 3).
    pub strings: usize,
    /// Speaking length, m.
    pub length: f64,
    /// Strike point as a fraction of the length from the agraffe.
    pub strike: f64,
    pub hammer_mass: f64,
    /// Exponent p of the felt law F = K x^p. K is not listed: every hammer
    /// is voiced (K chosen) for `contact` at a mezzo-forte blow.
    pub hammer_p: f64,
    /// Time the felt stays on the string at mezzo forte, s.
    pub contact: f64,
    /// Bridge resistance at this key's bridge point, kg/s.
    pub bridge_r: f64,
    /// String's own loss: sigma(f) = b1 + b3 f^2, in 1/s.
    pub b1: f64,
    pub b3: f64,
    /// Time for the damper to take 60 dB off; 0 where there is no damper.
    pub damper_t60: f64,
    /// First longitudinal mode of the string, Hz.
    pub longitudinal: f64,
}

/// Piecewise-linear through `(x, y)` anchors, clamped at the ends.
pub fn curve(points: &[(f64, f64)], x: f64) -> f64 {
    if x <= points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        if x <= w[1].0 {
            let t = (x - w[0].0) / (w[1].0 - w[0].0);
            return w[0].1 + t * (w[1].1 - w[0].1);
        }
    }
    points[points.len() - 1].1
}

fn log_curve(points: &[(f64, f64)], x: f64) -> f64 {
    let mut logs = [(0.0, 0.0); 8];
    for (i, p) in points.iter().enumerate() {
        logs[i] = (p.0, p.1.ln());
    }
    curve(&logs[..points.len()], x).exp()
}

/// Number of strings of a key: 8 single wound strings, 12 wound pairs,
/// plain trichords above.
pub fn strings_of(midi: u8) -> usize {
    match midi {
        0..=28 => 1,
        29..=40 => 2,
        _ => 3,
    }
}

fn is_wound(midi: u8) -> bool {
    midi <= 40
}

/// (length m, stiffness diameter m, mass per length kg/m) for a key at
/// equal-tempered `f0`.
fn string_of(midi: u8, f0: f64) -> (f64, f64, f64) {
    let m = midi as f64;
    if is_wound(midi) {
        let t = (m - 21.0) / 19.0;
        let length = 2.02 - 0.42 * t;
        let core = (1.6 - 0.6 * t) * 1e-3;
        // The winding is chosen for the tension, as a scale designer does.
        let tension = 1100.0 - 150.0 * t;
        let c = 2.0 * length * f0;
        (length, core, tension / (c * c))
    } else {
        let length = (0.052 * 2f64.powf(0.94 * (108.0 - m) / 12.0)).min(1.92);
        let d = (0.775 + 0.45 * (108.0 - m) / 67.0) * 1e-3;
        (length, d, RHO_STEEL * std::f64::consts::PI * d * d / 4.0)
    }
}

fn equal_tempered(midi: u8) -> f64 {
    440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0)
}

/// Prompt decay of the note in dB/s that the bridge is sized for (the
/// register trend of measured grands), where the board yields freely: in
/// the lowest octave the board is stiff below its lowest modes (see
/// `Voicing::bridge_stiffness`), and there only the partials above about
/// 250 Hz drain this fast. Through the tenor a note's lowest partials ring
/// on at 9-13 dB/s, as the reference AU's do, where the trend once rose to
/// 24 by middle C; above it the curve follows the reference AU's partial 1
/// register by register, 10-14 dB/s around middle C and about 20 in the
/// fifth octave. From G5 up it is the first of two stages, as the reference
/// AU's partial 1 falls over its first half second: 36 dB/s at G5, 54 at
/// C6, 62-82 above; once a note has fallen some 20 dB the sideways loop
/// carries its slower ring. Sized to one slope across both stages, 14-28
/// dB/s, a treble note hung on under its neighbours.
pub(crate) fn prompt_decay_db(midi: f64) -> f64 {
    curve(&[(21.0, 7.0), (28.0, 8.5), (36.0, 9.0), (48.0, 9.5), (54.0, 10.5), (59.0, 13.0), (61.0, 10.0), (64.0, 14.0), (69.0, 17.0), (72.0, 20.0), (77.0, 22.0), (79.0, 36.0), (82.0, 40.0), (84.0, 54.0), (86.0, 62.0), (96.0, 66.0), (99.0, 82.0), (104.0, 78.0), (108.0, 68.0)], midi)
}

/// The longest first contact a key is voiced to, in periods of its note.
/// The felt of the top octave stiffens little (a total exponent near 1), so
/// its blow is close to a half-sine, whose spectrum falls to nulls at 1.5
/// and 2.5 cycles a contact: from about 1.1 periods partial 2 neared its
/// null and partial 1 neared its own at 1.5 (A#7-C8 at 1.5-1.74), and
/// neighbouring keys' partial 2 against partial 1 at the bridge jumped by up
/// to 12.7 dB. Held at one period (from D#7 up) partial 2 sits on the
/// pulse's next lobe instead, -9 to -11 dB from C7 to C8.
const TOP_CONTACT_PERIODS: f64 = 1.0;

/// How far the octaves above the temperament octave lean from 4:2 toward
/// 2:1 (0: 4:2 octaves, 1: 2:1 octaves).
const TREBLE_TWO_ONE: f64 = 0.7;

/// The bass octaves widen from 6:3 at A#1 to 10:5 at D1 and below.
const BASS_TEN_FIVE_FROM: f64 = 34.0;
const BASS_TEN_FIVE: f64 = 26.0;

pub fn build_scale() -> Vec<KeyScale> {
    // Pass 1: strings at equal temperament.
    let mut keys: Vec<KeyScale> = (FIRST_KEY..=LAST_KEY)
        .map(|midi| {
            let m = midi as f64;
            let f0 = equal_tempered(midi);
            let (length, d, mu) = string_of(midi, f0);
            let c = 2.0 * length * f0;
            let tension = mu * c * c;
            let b = std::f64::consts::PI.powi(3) * E_STEEL * d.powi(4) / (64.0 * tension * length * length);
            let z = mu * c;
            let strings = strings_of(midi);
            // The thin strings of the top octave lose more of their own: what
            // is left of a note once the bridge has taken its share goes on
            // falling at some 20-40 dB/s, as the reference AU's top octave
            // does, instead of hanging on.
            let b1 = curve(&[(21.0, 0.12), (28.0, 0.14), (48.0, 0.25), (84.0, 0.45), (96.0, 0.83), (98.0, 1.5), (101.0, 2.8), (104.0, 3.6), (108.0, 2.9)], m);
            // The single wound strings of the lowest octave lose their upper
            // partials faster: the heavy winding rubs on its core.
            let b3 = log_curve(&[(21.0, 2.4e-7), (28.0, 8e-8), (36.0, 2.5e-8), (60.0, 3e-8), (72.0, 6e-8), (84.0, 1e-7), (96.0, 7e-8), (108.0, 3e-8)], m);
            let sigma = prompt_decay_db(m) / 8.686;
            // The bridge takes what the strings' own loss at partial 1 leaves
            // of the prompt decay: high in the treble, that loss is most of it.
            let total_z = strings as f64 * z;
            let bridge_r = 2.0 * total_z * f0 / (sigma - b1 - b3 * f0 * f0).max(0.05) - total_z;
            // Longitudinal waves: 5100 m/s in plain steel; the winding adds
            // mass but no stiffness, so the wound strings are slower.
            let c_long = if is_wound(midi) { 5100.0 * (RHO_STEEL * std::f64::consts::PI * d * d / 4.0 / mu).sqrt() } else { 5100.0 };
            KeyScale {
                midi,
                f1: f0,
                b,
                z,
                strings,
                length,
                strike: curve(&[(21.0, 0.122), (50.0, 0.122), (108.0, 0.07)], m),
                hammer_mass: log_curve(&[(21.0, 6.7e-3), (36.0, 4.9e-3), (60.0, 2.97e-3), (96.0, 2.2e-3), (108.0, 2.0e-3)], m),
                // How fast the felt stiffens as it is driven harder (with
                // `Voicing::felt_exponent` on top). From middle C up the
                // blow lasts one or two periods of the note, so felt that
                // stiffens fast shortens its contact on a hard blow and
                // lifts the low partials, not only the top: the treble grew
                // far louder than its speed. These exponents keep its growth
                // within some 2-5 dB of the reference AU's and its
                // brightening near it (13 dB from v40 to v110 at C4, 1-3 dB
                // from C6 up); one law cannot give both exactly.
                hammer_p: curve(&[(21.0, 2.2), (36.0, 2.3), (48.0, 2.4), (55.0, 2.45), (60.0, 2.0), (64.0, 1.4), (72.0, 1.2), (84.0, 0.85), (96.0, 0.7), (108.0, 0.5)], m),
                // First contact at mezzo forte, which the felt is voiced to.
                // A pair of strings (F1 up) takes twice the load off the
                // hammer: voiced to the singles' contact its felt came out a
                // quarter as hard and its attack 8 dB duller, so the pairs
                // are voiced shorter and the step goes unheard, as in the
                // reference AU. From C2 to B2 the contact cannot choose the
                // felt (see `Voicing::felt_ceiling`): asked for there is the
                // contact that felt like the neighbours' gives, so the pairs'
                // harder felt does not harden those keys with it. From C5
                // up the contact is shorter than before the felt above was
                // softened, for the attack and its growth with touch; its
                // shape around F5-F#5 keeps their attacks even where the
                // board colours them.
                contact: (1e-3 * curve(&[(21.0, 2.2), (28.0, 2.2), (29.0, 1.6), (34.0, 1.65), (35.0, 2.12), (36.0, 3.45), (40.0, 3.06), (41.0, 2.88), (45.0, 2.49), (47.0, 2.36), (48.0, 2.35), (60.0, 1.8), (67.0, 1.9), (72.0, 1.6), (79.0, 1.25), (84.0, 0.9), (91.0, 0.55), (96.0, 0.44), (103.0, 0.37), (108.0, 0.4)], m))
                    .min(TOP_CONTACT_PERIODS / f0),
                bridge_r,
                b1,
                b3,
                damper_t60: if midi >= 92 { 0.0 } else { curve(&[(21.0, 0.5), (48.0, 0.22), (60.0, 0.14), (91.0, 0.08)], m) },
                longitudinal: c_long / (2.0 * length),
            }
        })
        .collect();

    // Pass 2: stretch, the way a tuner sets octaves on coinciding partials.
    // `cents[k]` moves the first partial away from equal temperament.
    let ratio = |n_low: f64, b_low: f64, n_high: f64, b_high: f64| -> f64 {
        // cents by which the upper note's partial 1 must rise so that its
        // partial n_high meets partial n_low of the lower note
        let low = n_low * ((1.0 + n_low * n_low * b_low) / (1.0 + b_low)).sqrt();
        let high = n_high * ((1.0 + n_high * n_high * b_high) / (1.0 + b_high)).sqrt();
        1200.0 * (low / (2.0 * high)).log2()
    };
    let idx = |midi: u8| (midi - FIRST_KEY) as usize;
    let mut cents = [0.0f64; KEYS];
    // Temperament octave A3..A4: the 4:2 octave A3-A4 spread evenly.
    let a3 = -ratio(4.0, keys[idx(57)].b, 2.0, keys[idx(69)].b);
    for midi in 57..=69u8 {
        cents[idx(midi)] = a3 * (69 - midi) as f64 / 12.0;
    }
    // Above it a tuner narrows the octaves from 4:2 toward 2:1: up there the
    // lower note's 4th partial is faint and far sharp, and 4:2 octaves chained
    // up the treble put the top octave some 50 cents sharp, which a listener
    // hears as sharp, not as bright.
    for midi in 70..=LAST_KEY {
        let low = idx(midi - 12);
        let (b_low, b_high) = (keys[low].b, keys[idx(midi)].b);
        cents[idx(midi)] = cents[low] + (1.0 - TREBLE_TWO_ONE) * ratio(4.0, b_low, 2.0, b_high) + TREBLE_TWO_ONE * ratio(2.0, b_low, 1.0, b_high);
    }
    for midi in (FIRST_KEY..=56).rev() {
        let high = idx(midi + 12);
        // 4:2 octaves in the tenor, 6:3 in the bass, and in the lowest
        // octave the tuner listens higher still, to 10:5 from D1 down: there
        // the low partials are faint, and a 6:3 octave leaves the 10th
        // partial beating against the octave above's 5th.
        let n = if midi < 45 { 6.0 + 4.0 * ((BASS_TEN_FIVE_FROM - midi as f64) / (BASS_TEN_FIVE_FROM - BASS_TEN_FIVE)).clamp(0.0, 1.0) } else { 4.0 };
        cents[idx(midi)] = cents[high] - ratio(n, keys[idx(midi)].b, n / 2.0, keys[high].b);
    }
    for (k, key) in keys.iter_mut().enumerate() {
        key.f1 *= 2f64.powf(cents[k] / 1200.0);
    }
    keys
}
