//! Musical key of a whole track, for the explorer's KEY column and for the
//! harmonic-mixing hints that read off it.
//!
//! One pass over the decoded PCM builds a twelve-bin pitch-class profile — a
//! chroma — and the key is whichever of the twenty-four major and minor
//! candidates that profile correlates with best. The correlation is against
//! the Krumhansl-Kessler probe-tone profiles: what listeners actually report
//! about how strongly each degree belongs to a key, rather than what a scale
//! spelling says ought to belong. That is what makes the chords a record
//! spends its bars on count for more than the passing notes over them.
//!
//! Between the transform and the twelve classes there is a SEMITONE array,
//! not a direct bin-to-class fold. Three things need that middle step and
//! none of them is optional:
//!
//! - **Neutrality.** Transform bins are linearly spaced and pitch is
//!   logarithmic, so a semitone up at the top of the band owns twenty times
//!   the bins a semitone down at the bottom does. Folding bins straight onto
//!   classes made a FLAT spectrum come out with a deterministic shape —
//!   measured per-class mass ran from Bb at -14.8% to G# at +14.3%, sd/mean
//!   0.0883 — and white noise read as F minor with more confidence than any
//!   correct answer scored. Each semitone is now the MEAN weighted magnitude
//!   over its own band, and each class is divided by the mass a flat
//!   spectrum puts in it, which drops that figure to 0.0000.
//! - **Harmonics.** A pitch is not just its fundamental. Harmonic 3 is the
//!   fifth, harmonic 5 is the MAJOR THIRD, and a bass-forward mix used to
//!   read every minor chord as major because of the second of those: a
//!   sawtooth bass under a minor triad put more weight on the major third
//!   than the chord's own minor third carried. The semitone array is walked
//!   upward and each note subtracts its expected share from its own
//!   harmonics before anything votes.
//! - **Tilt.** The class weighting is a Gaussian in octaves over a band five
//!   octaves wide, so it is cut off well up its own sides, and a truncated
//!   window is not offset-invariant: a spectrum with a smooth slope and no
//!   notes in it at all still folded to a shape, and pink noise came back as
//!   Bb minor. Each semitone is divided by the running mean of the octave
//!   either side of it, so a broadband source of any slope reads flat and
//!   only the PEAKS — the notes — survive into the chroma.
//!
//! And the thing it still cannot do: a chord voiced entirely below
//! [`CHROMA_LOW_HZ`] is heard through its partials, and the first of those
//! to clear the floor is often a fifth. Such a record reads a perfect fifth
//! sharp, before and after everything above; see [`CHROMA_LOW_HZ`] for what
//! was tried and what it cost.
//!
//! Deliberately self-contained. The transform, the window, the decimator and
//! the profiles all live in this file, so a worker that has PCM and nothing
//! else can run it. It is not cheap in memory: the decimated mono signal is
//! held whole, which is four bytes per sample at about 11 kHz — roughly
//! 16 MB for a six-minute record — plus twelve doubles per analysis window,
//! about 93 KB over the same six minutes.
//!
//! Deterministic for a given build: no threading, no clock, no allocation
//! order dependence, same samples in and same key out. NOT bit-identical
//! across platforms — `cos`, `sin`, `exp`, `log2` and `powf` come from the
//! system math library and the last bits of a correlation move with it. Two
//! machines can disagree on a record whose top two candidates are inside
//! 1e-15 of each other, which is a record with no answer anyway.
//!
//! What it does not pretend to do: a record that modulates gets ONE key here,
//! the one it spends most of its bars in. Every estimate carries a
//! CONFIDENCE, and on most records the runner-up is the relative major or
//! minor — six of seven notes shared — so a modest margin between them is
//! honest rather than a failure.

use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// results
// ---------------------------------------------------------------------------

/// A detected musical key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyEstimate {
    /// Tonic as a pitch class, 0 = C, 1 = C#, ... 11 = B.
    pub tonic: u8,
    /// True for minor (Aeolian), false for major (Ionian).
    pub minor: bool,
    /// How far ahead of the runner-up the winner scored, 0.0..=1.0.
    ///
    /// The margin over second place, in correlation points, against a fixed
    /// yardstick — [`CONFIDENCE_MARGIN`], the margin a clean cadence with no
    /// competing key opens up. It is deliberately NOT divided by the spread
    /// of the twenty-four candidates: that divisor is dominated by the WORST
    /// candidate, which is a number about the record's brightness and not
    /// about how sure the answer is, and it squeezed every honest estimate
    /// into 0.076..=0.191 while noise scored 0.200.
    ///
    /// A record whose relative minor scores nearly as well reads LOW here,
    /// which is the truth about that record and not a fault in the estimate.
    pub confidence: f32,
}

/// How a key's tonic is spelled when it is the root of a MAJOR key: flats
/// everywhere the choice is free, because nobody writes A# major. The one
/// exception is the tritone at index 6, which is written F# and not Gb —
/// that is what the wheel prints and what a DJ reads off the screen.
const MAJOR_NAMES: [&str; 12] =
    ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
/// The same twelve as MINOR roots, where the sharp side is the convention
/// for three of them (C#, F#, G#) and the flat side for the rest.
const MINOR_NAMES: [&str; 12] =
    ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B"];

impl KeyEstimate {
    /// Camelot wheel notation, e.g. "8A" (A minor) or "8B" (C major).
    ///
    /// The wheel is the circle of fifths numbered 1..12 with C major nailed
    /// at 8, and a minor key wears the number of the major key a minor third
    /// above it — which is exactly what puts A minor and C major both at 8,
    /// so a mix between neighbours on the wheel is a mix between keys that
    /// share their notes.
    pub fn camelot(&self) -> String {
        let tonic = (self.tonic % 12) as u32;
        let root = if self.minor { (tonic + 3) % 12 } else { tonic };
        // Seven semitones is one step around the circle of fifths; the +7
        // then lands C (step 0) on 8 rather than on 1.
        let number = ((root * 7) % 12 + 7) % 12 + 1;
        let letter = if self.minor { "A" } else { "B" };
        format!("{number}{letter}")
    }

    /// Where this key sits on the wheel, for putting a list in an order
    /// a DJ can read down: neighbours on the wheel are neighbours in the
    /// list, and a key sits beside its relative.
    ///
    /// Sorting the printed label instead puts 10A before 2A, which is
    /// alphabetical order and is no order at all.
    pub fn wheel_order(&self) -> u8 {
        let tonic = (self.tonic % 12) as u32;
        let root = if self.minor { (tonic + 3) % 12 } else { tonic };
        let number = ((root * 7) % 12 + 7) % 12 + 1;
        // The mode is the tie-break, so a key and its relative sit
        // together rather than in two separate runs -- minor first, which
        // is the order the wheel is always printed in.
        ((number - 1) * 2 + u32::from(!self.minor)) as u8
    }

    /// The name in whichever notation the operator reads.
    pub fn label(&self, notation: KeyNotation) -> String {
        match notation {
            KeyNotation::Wheel => self.camelot(),
            KeyNotation::Open => self.open_key(),
            KeyNotation::Traditional => self.name(),
        }
    }

    /// The other numbered wheel: the same twelve positions, turned so that
    /// the natural minor is 1, with `m` and `d` for the two rings.
    pub fn open_key(&self) -> String {
        let camelot = self.camelot();
        let number: i32 = camelot
            .trim_end_matches(['A', 'B'])
            .parse()
            .unwrap_or(1);
        let letter = if self.minor { "m" } else { "d" };
        format!("{}{letter}", (number - 8).rem_euclid(12) + 1)
    }

    /// Traditional name, e.g. "Am" / "C".
    pub fn name(&self) -> String {
        let tonic = (self.tonic % 12) as usize;
        if self.minor {
            format!("{}m", MINOR_NAMES[tonic])
        } else {
            MAJOR_NAMES[tonic].to_string()
        }
    }
}

/// How a key is written on screen.
///
/// One formatter, because the notation is a reading habit and not three
/// different facts: whichever is chosen, it is the same estimate and the
/// same wheel underneath, and the sort never changes with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyNotation {
    /// The numbered wheel with the major ring lettered B, C major at 8.
    #[default]
    Wheel,
    /// The other numbered wheel: minor ring `m`, major ring `d`, turned so
    /// A minor is 1.
    Open,
    /// The names themselves: "Am", "C".
    Traditional,
}

impl KeyNotation {
    pub fn index(self) -> usize {
        match self {
            KeyNotation::Wheel => 0,
            KeyNotation::Open => 1,
            KeyNotation::Traditional => 2,
        }
    }

    pub fn from_index(index: usize) -> KeyNotation {
        match index {
            1 => KeyNotation::Open,
            2 => KeyNotation::Traditional,
            _ => KeyNotation::Wheel,
        }
    }
}

// ---------------------------------------------------------------------------
// how two keys sit together
// ---------------------------------------------------------------------------

/// A diatonic scale has seven notes, and one step around the circle of
/// fifths trades exactly one of them. That is the whole of the arithmetic
/// below: the fit between two keys is how many notes they still share.
const SCALE_NOTES: f32 = 7.0;

/// How far apart two keys sit on the wheel, 0..=6 steps.
///
/// The wheel is the circle of fifths, so a step is a fifth, and the far
/// side is the tritone six steps away in either direction.
fn wheel_distance(a: KeyEstimate, b: KeyEstimate) -> u8 {
    let position = |key: KeyEstimate| {
        let tonic = (key.tonic % 12) as u32;
        // A minor key sits with the major a minor third above it: that is
        // what puts a key and its relative on the same spot.
        let root = if key.minor { (tonic + 3) % 12 } else { tonic };
        (root * 7) % 12
    };
    let raw = (position(a) as i32 - position(b) as i32).rem_euclid(12);
    raw.min(12 - raw) as u8
}

/// How well two keys sit together: 1.0 for the same key, falling away as
/// they share fewer notes, never below zero.
///
/// Graded rather than allowed-or-not on purpose. A picker that only ever
/// took perfect matches would refuse most of a library, and a DJ trades a
/// step around the wheel for a better record all night. The caller weighs
/// this against tempo and energy, and against how sure the detector was.
pub fn key_fit(a: KeyEstimate, b: KeyEstimate) -> f32 {
    let shared = SCALE_NOTES - wheel_distance(a, b) as f32;
    let fit = (shared / SCALE_NOTES).max(0.0);
    if a.minor == b.minor {
        return fit;
    }
    // Same notes, different home. A relative pair shares all seven and is
    // the classic move, but the tonal centre does shift under the mix, so
    // it does not score as an exact match; a mode change further round the
    // wheel is a little harder still.
    fit * MODE_CHANGE
}

/// What a change of mode costs, as a share of the note-count fit.
const MODE_CHANGE: f32 = 0.94;

/// The semitone shift that makes `b` sit best against `a`, and the fit it
/// buys. Zero when the pair is already at its best.
///
/// One semitone is seven steps around the wheel, so a record that clashes
/// is often a semitone from agreeing. Kept to a semitone either way: real
/// mixes almost never move a record further, and a stretcher asked for more
/// starts to be heard doing it.
pub fn key_shift_to_fit(a: KeyEstimate, b: KeyEstimate) -> (i8, f32) {
    let mut best = (0i8, key_fit(a, b));
    for shift in [-1i8, 1] {
        let moved = KeyEstimate {
            tonic: ((b.tonic as i32 + shift as i32).rem_euclid(12)) as u8,
            ..b
        };
        let fit = key_fit(a, moved);
        if fit > best.1 {
            best = (shift, fit);
        }
    }
    best
}

// ---------------------------------------------------------------------------
// the analysis pass
// ---------------------------------------------------------------------------

/// The rate the chroma pass aims at. Everything key lives in is under 4 kHz,
/// so the top of the band is thrown away ONCE by the decimator instead of
/// being paid for in every transform.
const TARGET_RATE: f64 = 11_025.0;
/// The rate the chroma pass may not exceed, whatever the source rate.
///
/// [`FRAME`] is fixed, so the resolution guarantee below is really a
/// statement about the OUTPUT rate: a Hann main lobe is `4 * rate / FRAME`
/// wide and a semitone at [`CHROMA_LOW_HZ`] is 6.541 Hz, which caps the
/// output at 13_395 Hz. Aiming at [`TARGET_RATE`] alone did not enforce
/// that — a rounded `rate / 11025` stays at 1 all the way to 16_537 Hz, so a
/// 16 kHz decoder (a real rate, not a hypothetical one) ran the transform at
/// 16 kHz with a 7.81 Hz lobe and smeared the bottom octave across three
/// classes. The factor is now whichever of the two demands is stricter, so
/// 44.1 and 48 kHz decimate exactly as they always did and only the 12.5 to
/// 16.5 kHz gap moves.
const MAX_RATE: f64 = 13_394.0;
/// Transform size and hop, in samples at the decimated rate.
///
/// 8192 is 0.74 seconds and 1.35 Hz per bin. That resolution is what sets
/// the bottom of the chroma band: a semitone at A2 is 6.54 Hz wide and this
/// window's main lobe is 5.38 Hz, so a note there still falls inside its own
/// class. At 4096 the lobe is 10.8 Hz and A2 smears across three classes,
/// which is why the cheaper size is not used.
const FRAME: usize = 8192;
/// Half-window overlap: enough that a chord change never falls entirely in
/// one window's taper, cheap enough that a long record is still one pass.
const HOP: usize = 4096;
/// The band folded onto the twelve classes, A2 to A7.
///
/// The floor is not the lowest note a record plays — it is the lowest note
/// this transform can tell from its neighbours. Below it a bass note is
/// heard only through its partials, and those partials are not the note: in
/// a rich voicing the first one to clear 110 Hz is often harmonic 3, which
/// is a FIFTH.
///
/// That is a KNOWN LIMIT and not a solved problem. Six-partial triads
/// voiced at octave 3 — fundamentals from 131 Hz up, all inside the band —
/// read 12/24 wrong before [`HARMONIC_SUBTRACT`] and 0/24 after. At octave
/// 2, half the fundamentals fall under the floor: 21/24 wrong before,
/// 18/24 after. At octave 1 nothing is in band at all and the subtraction
/// has no fundamental to charge from, so every one of the 24 comes back
/// exactly a perfect fifth sharp, before and after. Fixing that needs a
/// fundamental estimated from its partials alone, which is a pitch tracker
/// and not a fold, and the versions of it tried here — a harmonic sum over
/// sub-band pitches — bought the sub-bass case by making a flat spectrum
/// look like a bass note and wrecking everything else.
const CHROMA_LOW_HZ: f64 = 110.0;
const CHROMA_HIGH_HZ: f64 = 3_520.0;
/// Where the weight over the band is centred, and how wide it is in
/// octaves. Chords are voiced around the middle of the keyboard; without
/// this a bright master's cymbals contribute as much pitch-class evidence
/// as its piano, and cymbals have no pitch class.
const OCTAVE_CENTRE_HZ: f64 = 440.0;
const OCTAVE_WIDTH: f64 = 2.0;
/// How many harmonics of a semitone are accounted for, and how much of a
/// note's own weight is charged to each of them.
///
/// The subtraction is `HARMONIC_SUBTRACT / h` of the note's remaining
/// weight, taken off harmonic `h` — a sawtooth's own 1/h roll-off, scaled
/// down. It is deliberately UNDER a full subtraction: the sum of the charge
/// a note collects from its seven possible parents is 1.72, so at 1.0 a
/// flat spectrum drives the top of the band to zero and a chord voiced in
/// octaves loses its upper voice.
///
/// Swept, with everything else fixed. At 0.0 a sawtooth bass five times
/// the level of the minor triad over it reads 12/12 chords as MAJOR —
/// harmonic 5 of the bass IS the major third, and that is the whole defect.
/// At 0.5, 0/12. At 0.8 it is still 0/12 but pink noise folds to a class
/// profile with sd/mean 0.071, over [`FLAT_CHROMA`], so noise starts coming
/// back as a key again. 0.5 also takes six-partial triads at octave 3 from
/// 6/24 wrong to 0/24.
const HARMONICS: usize = 8;
const HARMONIC_SUBTRACT: f64 = 0.5;
/// Half-width, in semitones, of the running mean each semitone is measured
/// against before it votes.
///
/// The class weights are a Gaussian in octaves over a band that is only
/// five octaves wide, so the Gaussian is CUT at 0.61 of its peak at the
/// bottom and 0.33 at the top. A truncated window is not offset-invariant,
/// and the consequence is measurable: a spectrum with a smooth 1/f tilt and
/// no notes in it at all folded to a class profile with sd/mean 0.182 —
/// pink noise came back as Bb minor. Dividing each semitone by the mean of
/// the octave either side of it removes any smooth tilt before the fold
/// sees it, and a broadband source of any slope reads flat.
///
/// One octave each way, swept: at half an octave the window sits between
/// the notes of a chord and flattens the chord itself — pink noise rises to
/// 0.049 and the mean confidence over the forty-eight cadences falls from
/// 0.730 to 0.490. At an octave and a half the window reaches past the
/// bass and a sawtooth bass starts winning again, 1/12 wrong. At one
/// octave: pink 0.032, cadences 48/48, bass 0/12.
const ENVELOPE_HALF: usize = 12;
/// How far under the window's own mean semitone level the running mean is
/// allowed to go before it stops dividing.
///
/// Flattening against a purely local mean is flattening against nothing
/// where there IS nothing: a mix with no content over 2 kHz has a
/// quantization floor up there, and dividing that floor by itself promoted
/// it to a full-strength vote. Measured on a chromatic cluster of pure
/// tones between 131 and 1975 Hz — a source with no key in it and no energy
/// in the top octave of the band — the empty octave's dither decided the
/// answer. -26 dB is forty decibels over a sixteen-bit floor and well under
/// any real spectral tilt: a 1/f slope across the five-octave band only
/// reaches -21 dB of its own mean at the top.
const ENVELOPE_FLOOR: f64 = 0.05;
/// The percentile of window loudness a track is judged against, rather than
/// its maximum, so one clipped bar cannot set the gate for the whole record.
const LOUDNESS_PERCENTILE: f64 = 0.90;
/// A window this far under that reference is the room between the tracks,
/// not the record: its chroma is noise, and normalizing it would give that
/// noise a full-scale vote.
const WINDOW_FLOOR: f64 = 0.04;
/// The ABSOLUTE floor, as an RMS over the loudest half second, in full-scale
/// units: four sixteen-bit steps, -78.3 dBFS.
///
/// [`WINDOW_FLOOR`] is relative to the track's own loudness, so it can never
/// fire on a track that is quiet all the way through — and a six-second file
/// whose loudest sample was ONE step used to come back as a key with a
/// confidence in the normal range, as did undithered hiss. Four steps is
/// eighteen decibels under the quietest passage of real music anyone would
/// hand a deck (-60 dBFS is 32.8 steps of RMS) and twelve decibels over
/// one-step dither, which is the loudest thing this gate has to refuse.
/// Half a second rather than a sample, so a single click cannot open it.
const TRACK_FLOOR: f64 = 4.0 / 32_768.0;
/// Shortest track worth an opinion. Under a second there is no harmony to
/// average over, only whatever chord happened to be sounding.
const MIN_TRACK_SECS: f64 = 1.0;
/// Below this relative spread the chroma has no shape at all and every
/// candidate is scoring the same accident.
///
/// The number is empirical and it only means anything because the fold is
/// class-neutral. With the old bin-to-class fold the geometry alone put a
/// hard floor of 0.088 under sd/mean — thirty times the 1e-3 gate that was
/// supposed to catch a flat profile — so that gate could not fire on any
/// input at all, and white noise came back as F minor.
///
/// Both populations, measured at 44.1 kHz. NOISE, over 33 probes (white at
/// six seeds and four lengths, pink at six seeds, white at three other
/// sample rates, one-step dither): worst 0.056, and 0.032 for the
/// ten-second cases. MUSIC, over 14 probes: worst 0.120, which is a C major
/// cadence buried under four times its own level of white noise and still
/// read as C major. A cadence in the clear is 0.95, a held triad 1.75.
///
/// 0.08 sits between the two, 1.4x over the worst noise and 1.5x under the
/// worst music. The populations are NOT decades apart and the value is a
/// real trade: raise it and a percussive record loses its key, lower it and
/// a noise floor gets one.
///
/// Recorded negative result: this gate cannot catch SHORT noise. It works
/// because a flat profile averages flatter over many windows, and a track
/// at [`MIN_TRACK_SECS`] has one or two. Measured over eight seeds, white
/// noise reads 0.063 at 1.5 s (caught), 0.070 at 1.2 s (3 of 8 get through)
/// and 0.120 at 1.0 s (all 8 get through). A one-second full-scale noise
/// burst gets a key, and no threshold here can stop it.
const FLAT_CHROMA: f64 = 0.08;
/// The correlation margin over second place that reads as certainty.
///
/// Measured over forty-eight cadences — twelve roots, both modes, at 44.1
/// and 48 kHz: raw margin min 0.209, mean 0.256, max 0.293. Against 0.35
/// those come out at 0.597 to 0.837, mean 0.730, which is the shape the
/// documented range asks for: a cadence that names its key reads high, and
/// the remaining headroom is for a record that leaves NO room for its
/// relative.
///
/// What it replaced: dividing by `best - worst` put every one of those
/// forty-eight between 0.076 and 0.191 while white noise scored 0.199 —
/// noise ranked above every correct answer the detector had ever given.
const CONFIDENCE_MARGIN: f64 = 0.35;

/// The Krumhansl-Kessler probe-tone profiles, tonic first: how strongly
/// each of the twelve degrees is felt to belong to the key. A candidate is
/// scored by lining degree `i` up with pitch class `(tonic + i) % 12`.
const MAJOR_PROFILE: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Estimate the key of a whole decoded track.
///
/// `frames` is interleaved stereo as the decoders hand it over; `None` comes
/// back when the audio is too short, too quiet or too featureless to judge,
/// which is a different answer from a key with low confidence and is meant
/// to be shown as an empty cell rather than as a guess.
pub fn estimate_key(frames: &[[i16; 2]], sample_rate: u32) -> Option<KeyEstimate> {
    let chroma = track_chroma(frames, sample_rate)?;
    // A profile with no shape cannot pick a winner, and the arbitrary one it
    // would pick would wear whatever confidence the ties happened to leave.
    let mean = chroma.iter().sum::<f64>() / 12.0;
    if mean <= 0.0 || !mean.is_finite() {
        return None;
    }
    let spread = chroma.iter().map(|value| (value - mean) * (value - mean)).sum::<f64>();
    if (spread / 12.0).sqrt() <= mean * FLAT_CHROMA {
        return None;
    }
    // Twenty-four candidates in one pass: even index major, odd index minor.
    let mut scores = [0.0f64; 24];
    let mut rotated = [0.0f64; 12];
    for tonic in 0..12 {
        for degree in 0..12 {
            rotated[degree] = chroma[(tonic + degree) % 12];
        }
        scores[tonic * 2] = pearson(&rotated, &MAJOR_PROFILE);
        scores[tonic * 2 + 1] = pearson(&rotated, &MINOR_PROFILE);
    }
    let mut best = 0usize;
    for index in 1..24 {
        if scores[index] > scores[best] {
            best = index;
        }
    }
    let mut second = f64::NEG_INFINITY;
    for (index, score) in scores.iter().enumerate() {
        if index != best && *score > second {
            second = *score;
        }
    }
    let confidence = ((scores[best] - second) / CONFIDENCE_MARGIN).clamp(0.0, 1.0);
    Some(KeyEstimate {
        tonic: (best / 2) as u8,
        minor: best % 2 == 1,
        confidence: confidence as f32,
    })
}

/// The track's pitch-class profile: twelve numbers, one per semitone of the
/// octave, summed over every analysis window that carried music.
///
/// Every window that carried music gets one vote of the same size: its
/// chroma is divided by its own loudest class before it is added. A key is
/// what a record spends its bars in, not what it spends its decibels on,
/// and without this a drone whose chroma is a single spike counts for more
/// than a chord whose chroma is spread over seven classes — measured, an
/// eight-second F# drone in front of sixteen seconds of a C major cadence
/// takes the record.
///
/// Which windows carry music is two separate questions and both are asked
/// here: [`TRACK_FLOOR`] for whether the FILE has anything in it, and
/// [`WINDOW_FLOOR`] against the record's own 90th percentile for whether
/// this bar does.
fn track_chroma(frames: &[[i16; 2]], sample_rate: u32) -> Option<[f64; 12]> {
    if sample_rate == 0 {
        return None;
    }
    let rate = sample_rate as f64;
    if frames.len() as f64 / rate < MIN_TRACK_SECS {
        return None;
    }
    if loudest_block_rms(frames, rate) < TRACK_FLOOR {
        return None;
    }
    let (mono, rate_out) = decimate_mono(frames, rate);
    if mono.len() < FRAME || rate_out < 1.0 {
        return None;
    }
    let geometry = Geometry::new(rate_out)?;

    let transform = Fft::new(FRAME);
    let window: Vec<f64> = (0..FRAME)
        .map(|index| crate::dsp_math::hann_f64(index, FRAME))
        .collect();
    let mut real = vec![0.0f64; FRAME];
    let mut imaginary = vec![0.0f64; FRAME];
    let mut notes = vec![0.0f64; geometry.notes()];
    let mut envelope = vec![0.0f64; geometry.notes()];
    let mut windows: Vec<[f64; 12]> = Vec::with_capacity(mono.len() / HOP + 1);
    let mut loudness: Vec<f64> = Vec::with_capacity(mono.len() / HOP + 1);
    let mut at = 0usize;
    while at + FRAME <= mono.len() {
        for index in 0..FRAME {
            real[index] = mono[at + index] as f64 * window[index];
            imaginary[index] = 0.0;
        }
        transform.forward(&mut real, &mut imaginary);
        geometry.gather(&real, &imaginary, &mut notes);
        loudness.push(geometry.level(&notes));
        windows.push(geometry.fold(&mut notes, &mut envelope));
        at += HOP;
    }
    if windows.is_empty() {
        return None;
    }

    let mut ranked = loudness.clone();
    ranked.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Nearest rank over the zero-based positions. `len * 0.90` handed back
    // the LAST position for every count up to ten, which gated any track
    // under 4.1 seconds against its own maximum — the one thing the
    // percentile exists to avoid.
    let index = ((ranked.len() - 1) as f64 * LOUDNESS_PERCENTILE) as usize;
    let reference = ranked.get(index).copied().unwrap_or(0.0);
    if reference <= 1e-12 {
        return None;
    }
    let floor = reference * WINDOW_FLOOR;
    let mut chroma = [0.0f64; 12];
    let mut kept = 0usize;
    for (values, level) in windows.iter().zip(&loudness) {
        if *level <= floor {
            continue;
        }
        let peak = values.iter().copied().fold(0.0f64, f64::max);
        if peak <= 1e-12 {
            continue;
        }
        for (out, value) in chroma.iter_mut().zip(values) {
            *out += value / peak;
        }
        kept += 1;
    }
    if kept == 0 {
        return None;
    }
    for value in chroma.iter_mut() {
        *value /= kept as f64;
    }
    Some(chroma)
}

/// RMS of the loudest half second of the down-mix, in full-scale units.
///
/// Half a second is long enough that one sample cannot set it and short
/// enough that a single real bar inside a silent file still shows up, which
/// is the pair of things [`TRACK_FLOOR`] has to be able to tell apart.
fn loudest_block_rms(frames: &[[i16; 2]], rate: f64) -> f64 {
    let block = ((rate * 0.5) as usize).max(1);
    let mut loudest = 0.0f64;
    let mut sum = 0.0f64;
    let mut count = 0usize;
    let mut full = false;
    for frame in frames {
        let mono = (frame[0] as f64 + frame[1] as f64) * 0.5 / 32_768.0;
        sum += mono * mono;
        count += 1;
        if count == block {
            loudest = loudest.max(sum / count as f64);
            full = true;
            sum = 0.0;
            count = 0;
        }
    }
    if !full && count > 0 {
        loudest = loudest.max(sum / count as f64);
    }
    loudest.sqrt()
}

// ---------------------------------------------------------------------------
// bins -> semitones -> classes
// ---------------------------------------------------------------------------

/// Everything about the fold that depends only on the rate: which bins feed
/// which semitone, what each semitone is worth, where its harmonics land,
/// and what it takes to make the twelve classes weigh the same.
///
/// Built once per track. It is the expensive part of the inner loop if it
/// is not.
struct Geometry {
    /// Per transform bin: the semitone it feeds and the weight it feeds at.
    bin: Vec<Option<(usize, f64)>>,
    /// Reciprocal of the total bin weight each semitone collects, so a
    /// semitone is the MEAN of its band and not the sum. Without this a
    /// semitone at the top of the band is worth twenty of one at the
    /// bottom purely because linear bins are denser up there.
    scale: Vec<f64>,
    /// Per semitone: pitch class, and the octave weight at its frequency.
    /// The weight is zero outside the voting band, whose semitones are only
    /// measured to give the running mean and the harmonic pass some
    /// context.
    class: Vec<usize>,
    weight: Vec<f64>,
    /// First semitone in the voting band. Nothing under it is allowed to
    /// charge its harmonics: below the resolution limit a note smears over
    /// three or four semitones, each of which would charge the full series,
    /// and a bass note that pays four times over gouges the chord above it.
    /// Measured, with sub-band notes charging: a sawtooth bass under a
    /// minor triad went from 0/12 wrong to 9/12 wrong.
    first_voting: usize,
    /// Per semitone and harmonic 2..=[`HARMONICS`]: the semitone the
    /// harmonic lands on and how far past it, so the charge can be split
    /// between that semitone and the next. Harmonics 5 and 7 are 0.14 and
    /// 0.31 of a semitone off the grid and rounding them would put a
    /// bass note's major third on the wrong class.
    harmonic: Vec<Option<(usize, f64)>>,
    /// Per class: what it takes to make a flat spectrum fold flat.
    normal: [f64; 12],
}

impl Geometry {
    fn new(rate_out: f64) -> Option<Geometry> {
        let bin_hz = rate_out / FRAME as f64;
        if !bin_hz.is_finite() || bin_hz <= 0.0 {
            return None;
        }
        // The two bins nearest DC are the window's own skirt, never a note.
        let usable_low = bin_hz * 2.0;
        let usable_high = rate_out * 0.45;
        let low = CHROMA_LOW_HZ.max(usable_low);
        let high = CHROMA_HIGH_HZ.min(usable_high);
        if high <= low {
            return None;
        }
        // The semitones that VOTE.
        let voting_low = midi_of(low).ceil() as i32;
        let voting_high = midi_of(high).floor() as i32;
        if voting_high - voting_low < 12 {
            return None;
        }
        // The semitones that are MEASURED. An octave of context past each
        // end of the voting band, as far as the transform can reach: the
        // running mean below wants real neighbours rather than a truncated
        // window, and the harmonic pass wants the bass that sits under the
        // band to be able to charge the partials it puts inside it.
        let margin = ENVELOPE_HALF as i32;
        let lowest = midi_of(usable_low.max(hz_of((voting_low - margin) as f64))).ceil() as i32;
        let highest =
            midi_of(usable_high.min(hz_of((voting_high + margin) as f64))).floor() as i32;
        if highest < voting_high || lowest > voting_low {
            return None;
        }
        let count = (highest - lowest + 1) as usize;

        let bins = FRAME / 2 + 1;
        let mut bin: Vec<Option<(usize, f64)>> = Vec::with_capacity(bins);
        let mut collected = vec![0.0f64; count];
        for index in 0..bins {
            let frequency = index as f64 * bin_hz;
            if frequency < bin_hz * 2.0 || frequency > rate_out * 0.45 {
                bin.push(None);
                continue;
            }
            let midi = midi_of(frequency);
            let nearest = midi.round();
            if nearest < lowest as f64 || nearest > highest as f64 {
                bin.push(None);
                continue;
            }
            // A raised cosine across the semitone, one at the note and zero
            // at the boundary with its neighbour: window leakage lands in
            // the bins between two notes, and this is what stops it voting
            // for either.
            let centred = (PI * (midi - nearest)).cos().powi(2);
            let note = (nearest as i32 - lowest) as usize;
            collected[note] += centred;
            bin.push(Some((note, centred)));
        }

        let mut scale = Vec::with_capacity(count);
        let mut class = Vec::with_capacity(count);
        let mut weight = Vec::with_capacity(count);
        for (note, total) in collected.iter().enumerate() {
            let semitone = lowest + note as i32;
            // A semitone narrower than the bins that are supposed to
            // resolve it has no reading of its own; it is left out rather
            // than amplified up from whatever fell in it.
            scale.push(if *total > 1e-9 { 1.0 / *total } else { 0.0 });
            class.push(semitone.rem_euclid(12) as usize);
            let voting = semitone >= voting_low && semitone <= voting_high;
            weight.push(if voting { octave_weight(hz_of(semitone as f64)) } else { 0.0 });
        }

        let mut harmonic = Vec::with_capacity(count * (HARMONICS - 1));
        for note in 0..count {
            for step in 2..=HARMONICS {
                let target = note as f64 + 12.0 * (step as f64).log2();
                let floor = target.floor();
                if floor < 0.0 || floor >= count as f64 {
                    harmonic.push(None);
                } else {
                    harmonic.push(Some((floor as usize, target - floor)));
                }
            }
        }

        let mut geometry = Geometry {
            bin,
            scale,
            class,
            weight,
            first_voting: (voting_low - lowest) as usize,
            harmonic,
            normal: [1.0; 12],
        };
        // A flat spectrum reads one at every usable semitone, so pushing
        // ones through the whole chain — subtraction included — measures
        // exactly what each class collects when the music says nothing.
        // Dividing that out is what makes noise come back shapeless
        // instead of coming back in F minor.
        let mut flat: Vec<f64> = (0..count)
            .map(|note| if geometry.scale[note] > 0.0 { 1.0 } else { 0.0 })
            .collect();
        let mut envelope = vec![0.0f64; count];
        let mass = geometry.fold(&mut flat, &mut envelope);
        for value in mass.iter() {
            if !value.is_finite() || *value <= 1e-12 {
                return None;
            }
        }
        for (normal, collected) in geometry.normal.iter_mut().zip(&mass) {
            *normal = 1.0 / collected;
        }
        Some(geometry)
    }

    fn notes(&self) -> usize {
        self.class.len()
    }

    /// How loud this window is, in the units the semitone array is in and
    /// over the band that votes. It has to be read BEFORE `fold`: flattening
    /// against the running mean throws level away by design, and a chroma
    /// summed after it carries no information about how loud the window was
    /// at all — [`WINDOW_FLOOR`] read off the folded chroma let a bar at 2%
    /// of full scale outvote a bar at full scale.
    fn level(&self, notes: &[f64]) -> f64 {
        notes.iter().zip(&self.weight).map(|(value, weight)| value * weight).sum()
    }

    /// Mean weighted magnitude per semitone, from one transformed window.
    ///
    /// Magnitude and not power: a chord's loudest partial is already the one
    /// the ear names, and squaring it would let it name the bar. Two tones a
    /// minor third apart at 1.0 and 0.5 come out of here at 2.2 to 1, which
    /// is the amplitude ratio; squared they would be 4 to 1.
    fn gather(&self, real: &[f64], imaginary: &[f64], notes: &mut [f64]) {
        for value in notes.iter_mut() {
            *value = 0.0;
        }
        for (index, entry) in self.bin.iter().enumerate() {
            let Some((note, weight)) = entry else { continue };
            let magnitude =
                (real[index] * real[index] + imaginary[index] * imaginary[index]).sqrt();
            notes[*note] += magnitude * weight;
        }
        for (value, scale) in notes.iter_mut().zip(&self.scale) {
            *value *= scale;
        }
    }

    /// Charge every semitone's harmonics against it, flatten what is left
    /// against its own neighbourhood, and fold that onto the twelve
    /// classes. `notes` and `envelope` are both consumed in place.
    ///
    /// The harmonic pass runs upward, so a note is only ever charged by
    /// notes BELOW it and is itself already clean when its own harmonics
    /// are taken off. That order is the whole trick: the bass is settled
    /// before the chord sitting over it is read.
    fn fold(&self, notes: &mut [f64], envelope: &mut [f64]) -> [f64; 12] {
        let count = self.class.len();
        for note in self.first_voting..count {
            let value = notes[note];
            if value <= 0.0 {
                continue;
            }
            for step in 2..=HARMONICS {
                let Some((target, split)) = self.harmonic[note * (HARMONICS - 1) + step - 2]
                else {
                    continue;
                };
                let charge = HARMONIC_SUBTRACT / step as f64 * value;
                notes[target] = (notes[target] - charge * (1.0 - split)).max(0.0);
                if target + 1 < count {
                    notes[target + 1] = (notes[target + 1] - charge * split).max(0.0);
                }
            }
        }
        let floor =
            ENVELOPE_FLOOR * notes.iter().sum::<f64>() / count as f64;
        let mut high = ENVELOPE_HALF.min(count - 1);
        let mut low = 0usize;
        let mut sum: f64 = notes[0..=high].iter().sum();
        for (note, slot) in envelope.iter_mut().enumerate().take(count) {
            let want_high = (note + ENVELOPE_HALF).min(count - 1);
            let want_low = note.saturating_sub(ENVELOPE_HALF);
            while high < want_high {
                high += 1;
                sum += notes[high];
            }
            while low < want_low {
                sum -= notes[low];
                low += 1;
            }
            *slot = (sum / (want_high - want_low + 1) as f64).max(floor);
        }
        let mut chroma = [0.0f64; 12];
        for note in 0..count {
            if envelope[note] <= 0.0 {
                continue;
            }
            chroma[self.class[note]] += notes[note] / envelope[note] * self.weight[note];
        }
        for (value, normal) in chroma.iter_mut().zip(&self.normal) {
            *value *= normal;
        }
        chroma
    }
}

/// MIDI number of a frequency, fractional. 69 is A440.
fn midi_of(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

fn hz_of(midi: f64) -> f64 {
    440.0 * ((midi - 69.0) / 12.0).exp2()
}

/// How much a semitone at `hz` counts, as a Gaussian in octaves around
/// [`OCTAVE_CENTRE_HZ`].
fn octave_weight(hz: f64) -> f64 {
    let octaves = (hz / OCTAVE_CENTRE_HZ).log2() / OCTAVE_WIDTH;
    (-0.5 * octaves * octaves).exp()
}

/// Down-mix to mono and decimate to about [`TARGET_RATE`], returning the
/// signal and the rate it actually came out at.
///
/// The anti-alias filter is two moving averages of `factor` samples run back
/// to back, sampled every `factor`th sample — a triangular decimator whose
/// response is `sinc(pi f / rate_out)^2`. It is chosen over a pole cascade
/// because it is FLAT where it matters. Measured through this function at
/// 44.1 kHz: 0.9996 at 110 Hz against 0.7215 at 3520 Hz, a tilt of 2.83 dB
/// across the whole chroma band, where four one-poles steep enough to be
/// worth having cost 12 dB. The tilt no longer moves a pitch class —
/// `Geometry` makes the classes weigh the same by construction — but it
/// still shapes which OCTAVE of a class is heard, and a filter that quietly
/// rewrites the octave weighting is a filter nobody can reason about.
///
/// Also measured, by sweeping everything above the new Nyquist and looking
/// for where it lands: the worst thing that folds back into the chroma band
/// arrives 15.4 dB down, at 7513 Hz. The response has an exact null at the
/// output rate itself and at every multiple of it.
fn decimate_mono(frames: &[[i16; 2]], rate: f64) -> (Vec<f32>, f64) {
    let aim = (rate / TARGET_RATE).round();
    let cap = (rate / MAX_RATE).ceil();
    let factor = (aim.max(cap).max(1.0) as usize).max(1);
    if factor == 1 {
        let mono = frames
            .iter()
            .map(|frame| crate::dsp_math::mono(*frame))
            .collect();
        return (mono, rate);
    }
    let mut out = Vec::with_capacity(frames.len() / factor + 1);
    let mut first = vec![0.0f64; factor];
    let mut second = vec![0.0f64; factor];
    let mut first_sum = 0.0f64;
    let mut second_sum = 0.0f64;
    let mut slot = 0usize;
    let mut phase = 0usize;
    let inverse = 1.0 / (factor as f64 * factor as f64);
    for frame in frames {
        let mono = crate::dsp_math::mono_f64(*frame);
        first_sum += mono - first[slot];
        first[slot] = mono;
        second_sum += first_sum - second[slot];
        second[slot] = first_sum;
        slot += 1;
        if slot == factor {
            slot = 0;
        }
        phase += 1;
        if phase == factor {
            phase = 0;
            out.push((second_sum * inverse) as f32);
        }
    }
    (out, rate / factor as f64)
}

/// A fixed-size forward transform. The twiddle table and the bit-reversal
/// permutation are built once and every window of the track reuses them;
/// evaluating the twiddles inside the butterfly instead costs two
/// transcendentals per butterfly, which on a six-minute record is a hundred
/// million of them.
///
/// Radix-2 and in place. The real signal goes through the full complex
/// transform rather than the half-length real trick: at this rate a whole
/// record is a fraction of a second either way, and the honest version is
/// the one that can be read.
struct Fft {
    size: usize,
    /// `(cos, sin)` of `-2 pi k / size` for `k` in `0..size/2`.
    twiddle: Vec<(f64, f64)>,
    reversal: Vec<u32>,
}

impl Fft {
    /// `size` must be a power of two, and at least two.
    ///
    /// It used to be rounded up quietly, which meant `Fft::new(100)` built a
    /// 128-point transform and then [`Fft::forward`] handed a 100-element
    /// slice straight back untouched — a caller with an off-by-a-bit size
    /// got zeros for a spectrum and no complaint. Both ends now say so.
    fn new(size: usize) -> Fft {
        assert!(
            size >= 2 && size.is_power_of_two(),
            "transform size {size} is not a power of two"
        );
        let mut twiddle = Vec::with_capacity(size / 2);
        for step in 0..size / 2 {
            let angle = -2.0 * PI * step as f64 / size as f64;
            twiddle.push((angle.cos(), angle.sin()));
        }
        let bits = size.trailing_zeros();
        let mut reversal = Vec::with_capacity(size);
        for index in 0..size {
            reversal.push((index as u32).reverse_bits() >> (32 - bits));
        }
        Fft { size, twiddle, reversal }
    }

    /// Forward transform in place. `imaginary` starts at zero for a real
    /// signal.
    fn forward(&self, real: &mut [f64], imaginary: &mut [f64]) {
        assert!(
            real.len() == self.size && imaginary.len() == self.size,
            "transform of {} wants {} samples, not {} and {}",
            self.size,
            self.size,
            real.len(),
            imaginary.len()
        );
        for index in 0..self.size {
            let target = self.reversal[index] as usize;
            if target > index {
                real.swap(index, target);
                imaginary.swap(index, target);
            }
        }
        let mut length = 2usize;
        while length <= self.size {
            let stride = self.size / length;
            let half = length / 2;
            let mut start = 0usize;
            while start < self.size {
                for step in 0..half {
                    let (cosine, sine) = self.twiddle[step * stride];
                    let from = start + step;
                    let to = from + half;
                    let real_part = real[to] * cosine - imaginary[to] * sine;
                    let imaginary_part = real[to] * sine + imaginary[to] * cosine;
                    real[to] = real[from] - real_part;
                    imaginary[to] = imaginary[from] - imaginary_part;
                    real[from] += real_part;
                    imaginary[from] += imaginary_part;
                }
                start += length;
            }
            length <<= 1;
        }
    }
}

/// Pearson correlation of two twelve-vectors.
///
/// The correlation, not a dot product: it subtracts each side's mean, so a
/// candidate is judged on the SHAPE of the profile it asks for rather than
/// on how much energy the track has. Without that, every key would score in
/// proportion to how loud the record is, and the flattest profile would win
/// every time. Zero when either side is constant, which is a refusal rather
/// than an answer.
fn pearson(left: &[f64; 12], right: &[f64; 12]) -> f64 {
    let left_mean = left.iter().sum::<f64>() / 12.0;
    let right_mean = right.iter().sum::<f64>() / 12.0;
    let mut covariance = 0.0f64;
    let mut left_spread = 0.0f64;
    let mut right_spread = 0.0f64;
    for (a, b) in left.iter().zip(right.iter()) {
        let (a, b) = (a - left_mean, b - right_mean);
        covariance += a * b;
        left_spread += a * a;
        right_spread += b * b;
    }
    let denominator = (left_spread * right_spread).sqrt();
    if denominator <= 1e-12 {
        return 0.0;
    }
    covariance / denominator
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    /// Three notations, one wheel underneath.
    #[test]
    fn a_key_is_written_three_ways_and_means_the_same_thing() {
        let a_minor = key(9, true);
        let c_major = key(0, false);
        assert_eq!(a_minor.camelot(), "8A");
        assert_eq!(c_major.camelot(), "8B");
        // The other wheel is turned so the natural minor is one.
        assert_eq!(a_minor.open_key(), "1m");
        assert_eq!(c_major.open_key(), "1d");
        assert_eq!(key(4, true).camelot(), "9A", "E minor is a step round");
        assert_eq!(key(4, true).open_key(), "2m");
        assert_eq!(key(2, true).camelot(), "7A", "D minor the other way");
        assert_eq!(key(2, true).open_key(), "12m");
        assert_eq!(a_minor.name(), "Am");
        assert_eq!(c_major.name(), "C");
        // And one formatter answers for all three.
        assert_eq!(a_minor.label(KeyNotation::Wheel), "8A");
        assert_eq!(a_minor.label(KeyNotation::Open), "1m");
        assert_eq!(a_minor.label(KeyNotation::Traditional), "Am");
    }

    /// The order a list is read down is the wheel's, not the alphabet's.
    #[test]
    fn keys_sort_around_the_wheel_with_relatives_together() {
        let mut keys = vec![
            key(2, true),   // 7A
            key(9, true),   // 8A
            key(0, false),  // 8B
            key(4, true),   // 9A
            key(11, true),  // 10A
            key(10, true),  // 3A
        ];
        keys.sort_by_key(|k| k.wheel_order());
        let round: Vec<String> = keys.iter().map(|k| k.camelot()).collect();
        assert_eq!(round, ["3A", "7A", "8A", "8B", "9A", "10A"]);
        // Which is exactly what sorting the printed label does NOT give.
        let mut text: Vec<String> = round.clone();
        text.sort();
        assert_ne!(text, round, "10A before 2A is no order at all");
    }

    use super::*;

    fn key(tonic: u8, minor: bool) -> KeyEstimate {
        KeyEstimate { tonic, minor, confidence: 1.0 }
    }

    #[test]
    fn a_key_fits_itself_best_and_a_tritone_away_worst() {
        // The wheel IS the circle of fifths, and one step around it changes
        // exactly one note of the seven. So the fit is a count of shared
        // notes, and it has to fall away in order as the keys separate.
        let c_major = key(0, false);
        let g_major = key(7, false); // one step
        let d_major = key(2, false); // two steps
        let fs_major = key(6, false); // six steps, the far side

        let same = key_fit(c_major, c_major);
        let one = key_fit(c_major, g_major);
        let two = key_fit(c_major, d_major);
        let far = key_fit(c_major, fs_major);

        assert!((same - 1.0).abs() < 1e-6, "a key fits itself: {same}");
        assert!(one < same && two < one && far < two, "{same} {one} {two} {far}");
        assert!(far >= 0.0, "a clash is still a number, not a negative: {far}");
        // Around the wheel is symmetric: a step down is a step.
        let f_major = key(5, false);
        assert!((key_fit(c_major, f_major) - one).abs() < 1e-6);
    }

    #[test]
    fn a_relative_minor_shares_every_note_and_nearly_every_point() {
        // A minor and C major sit on the same wheel position and share all
        // seven notes, so they must score close behind an exact match and
        // clearly ahead of a neighbouring key that has given one up.
        let c_major = key(0, false);
        let a_minor = key(9, true);
        let g_major = key(7, false);

        let relative = key_fit(c_major, a_minor);
        assert!(relative < 1.0, "a different tonal centre is not a free ride");
        assert!(
            relative > key_fit(c_major, g_major),
            "sharing all seven notes beats sharing six: {relative}"
        );
    }

    #[test]
    fn the_fit_does_not_care_which_record_is_asked_about_first() {
        for (a, b) in [((0, false), (7, false)), ((9, true), (2, false))] {
            let (a, b) = (key(a.0, a.1), key(b.0, b.1));
            assert!((key_fit(a, b) - key_fit(b, a)).abs() < 1e-6);
        }
    }

    #[test]
    fn a_semitone_shift_is_offered_when_it_buys_a_better_fit() {
        // Seven steps around the wheel is one semitone, so a pair that sits
        // badly can often be rescued by moving one. The shift is reported
        // signed, and it has to actually improve the fit it claims.
        let a = key(0, false);
        let b = key(1, false); // five steps away: a poor pair
        let (shift, fitted) = key_shift_to_fit(a, b);
        assert!(shift != 0, "this pair needs help");
        assert!(shift.abs() <= 2, "a DJ moves a record a semitone, not a fifth");
        assert!(fitted > key_fit(a, b), "the shift has to earn its keep");

        // A pair that already fits is left alone.
        let (none, _) = key_shift_to_fit(a, a);
        assert_eq!(none, 0);
    }

    // ---------------------------------------------------------------------------
    // signals
    // ---------------------------------------------------------------------------

    /// Frequency of a pitch class in an octave. MIDI 60 is C4; class 0 is C.
    fn pitch(class: usize, octave: i32) -> f64 {
        let midi = 12 * (octave + 1) + class as i32;
        440.0 * 2.0f64.powf((midi as f64 - 69.0) / 12.0)
    }

    /// Additive tones at fixed amplitudes, held for `seconds`. Pure tones and
    /// not a synthesized instrument on purpose — a partial that is not there
    /// cannot be the reason an estimate is right.
    fn tones(rate: u32, seconds: f64, voices: &[(f64, f64)]) -> Vec<[i16; 2]> {
        let count = (rate as f64 * seconds) as usize;
        let mut out = Vec::with_capacity(count);
        for index in 0..count {
            let at = index as f64 / rate as f64;
            let mut value = 0.0f64;
            for (voice, (frequency, amplitude)) in voices.iter().enumerate() {
                // A per-voice phase offset so the tones do not all start at zero
                // together and make one broadband click at t = 0.
                value += amplitude * (2.0 * PI * frequency * at + voice as f64 * 0.7).sin();
            }
            let sample = (value * 12_000.0).clamp(-32_000.0, 32_000.0) as i16;
            out.push([sample, sample]);
        }
        out
    }

    /// A chord progression as PCM: every chord is a set of pure tones, held for
    /// an equal share of `seconds`.
    fn progression(rate: u32, seconds: f64, chords: &[&[f64]]) -> Vec<[i16; 2]> {
        let count = (rate as f64 * seconds) as usize;
        let mut out = Vec::with_capacity(count);
        let per_chord = (count / chords.len().max(1)).max(1);
        for index in 0..count {
            let chord = chords[(index / per_chord).min(chords.len() - 1)];
            let at = index as f64 / rate as f64;
            let amplitude = 0.8 / chord.len().max(1) as f64;
            let mut value = 0.0f64;
            for (voice, frequency) in chord.iter().enumerate() {
                let phase = 2.0 * PI * frequency * at + voice as f64 * 0.7;
                value += amplitude * phase.sin();
            }
            let sample = (value * 32_000.0).clamp(-32_000.0, 32_000.0) as i16;
            out.push([sample, sample]);
        }
        out
    }

    fn triad(root: usize, octave: i32, third: usize) -> Vec<f64> {
        vec![
            pitch(root, octave),
            pitch((root + third) % 12, octave + ((root + third) / 12) as i32),
            pitch((root + 7) % 12, octave + ((root + 7) / 12) as i32),
        ]
    }

    fn major(root: usize, octave: i32) -> Vec<f64> {
        triad(root, octave, 4)
    }

    fn minor(root: usize, octave: i32) -> Vec<f64> {
        triad(root, octave, 3)
    }

    /// I - IV - V - I in C: the cadence that names a key, and the only chord
    /// that both frames is the tonic.
    fn c_major_cadence(rate: u32) -> Vec<[i16; 2]> {
        let (one, four, five) = (major(0, 4), major(5, 3), major(7, 3));
        progression(rate, 8.0, &[&one, &four, &five, &one])
    }

    /// i - iv - V - i in A minor. The dominant is MAJOR, as it is played, which
    /// puts a G# in the profile and is exactly what separates A minor from its
    /// relative C major.
    fn a_minor_cadence(rate: u32) -> Vec<[i16; 2]> {
        let (one, four, five) = (minor(9, 3), minor(2, 4), major(4, 4));
        progression(rate, 8.0, &[&one, &four, &five, &one])
    }

    /// The same cadence in any key and either mode, one octave lower, so the
    /// transposition tests and the loudness tests share a generator.
    fn cadence(rate: u32, root: usize, is_minor: bool) -> Vec<[i16; 2]> {
        let third = if is_minor { 3 } else { 4 };
        let (one, four, five) = (
            triad(root, 3, third),
            triad((root + 5) % 12, 3, third),
            major((root + 7) % 12, 3),
        );
        progression(rate, 8.0, &[&one, &four, &five, &one])
    }

    /// Deterministic broadband noise. A fixed xorshift and not a crate, so this
    /// suite says the same thing on every machine and in every release.
    fn noise(rate: u32, seconds: f64, amplitude: f64, seed: u64) -> Vec<[i16; 2]> {
        let mut state = seed | 1;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        let count = (rate as f64 * seconds) as usize;
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let left = (next() * amplitude * 32_000.0).clamp(-32_000.0, 32_000.0) as i16;
            let right = (next() * amplitude * 32_000.0).clamp(-32_000.0, 32_000.0) as i16;
            out.push([left, right]);
        }
        out
    }

    fn quieter(frames: &[[i16; 2]], gain: f64) -> Vec<[i16; 2]> {
        frames
            .iter()
            .map(|frame| {
                [(frame[0] as f64 * gain) as i16, (frame[1] as f64 * gain) as i16]
            })
            .collect()
    }

    fn then(first: &[[i16; 2]], second: &[[i16; 2]]) -> Vec<[i16; 2]> {
        let mut out = first.to_vec();
        out.extend_from_slice(second);
        out
    }

    fn over(first: &[[i16; 2]], second: &[[i16; 2]]) -> Vec<[i16; 2]> {
        let count = first.len().min(second.len());
        (0..count)
            .map(|index| {
                let left = (first[index][0] as f64 + second[index][0] as f64)
                    .clamp(-32_000.0, 32_000.0) as i16;
                let right = (first[index][1] as f64 + second[index][1] as f64)
                    .clamp(-32_000.0, 32_000.0) as i16;
                [left, right]
            })
            .collect()
    }

    /// Relative spread of a chroma: the quantity [`FLAT_CHROMA`] gates on.
    fn shape(chroma: &[f64; 12]) -> f64 {
        let mean = chroma.iter().sum::<f64>() / 12.0;
        let spread =
            chroma.iter().map(|value| (value - mean) * (value - mean)).sum::<f64>() / 12.0;
        spread.sqrt() / mean
    }

    fn strongest(chroma: &[f64; 12]) -> usize {
        (1..12).fold(0usize, |best, class| {
            if chroma[class] > chroma[best] {
                class
            } else {
                best
            }
        })
    }

    // ---------------------------------------------------------------------------
    // the answers
    // ---------------------------------------------------------------------------

    #[test]
    fn the_c_major_cadence_reads_as_c_major() {
        let key = estimate_key(&c_major_cadence(44_100), 44_100).expect("a key");
        assert_eq!(key.name(), "C", "{key:?}");
        assert_eq!(key.tonic, 0);
        assert!(!key.minor);
        assert_eq!(key.camelot(), "8B");
        assert!(key.confidence > 0.0 && key.confidence <= 1.0, "{key:?}");
    }

    #[test]
    fn the_a_minor_cadence_reads_as_a_minor() {
        let key = estimate_key(&a_minor_cadence(44_100), 44_100).expect("a key");
        assert_eq!(key.name(), "Am", "{key:?}");
        assert_eq!(key.tonic, 9);
        assert!(key.minor);
        assert_eq!(key.camelot(), "8A");
    }

    /// The same cadence in all twelve keys, both modes. A detector can get C
    /// right by accident — every constant in it was chosen while looking at C —
    /// and only the transpositions say whether it heard the music or the
    /// arithmetic.
    #[test]
    fn every_key_is_found_where_it_was_put() {
        for root in 0..12usize {
            for is_minor in [false, true] {
                let track = cadence(44_100, root, is_minor);
                let key = estimate_key(&track, 44_100).expect("a key");
                assert_eq!(
                    (key.tonic as usize, key.minor),
                    (root, is_minor),
                    "root {root} minor {is_minor} read as {key:?}"
                );
            }
        }
    }

    /// A cadence that names its key has to read as CERTAIN, not as a coin toss.
    ///
    /// The old confidence divided the margin by the whole spread of the
    /// twenty-four candidates, and that divisor is set by the WORST candidate;
    /// every one of these forty-eight came out between 0.076 and 0.191 while
    /// white noise scored 0.199. Measured now: 0.597 to 0.837, mean 0.730.
    #[test]
    fn a_cadence_that_names_its_key_says_so() {
        let mut lowest = 1.0f32;
        let mut total = 0.0f64;
        for root in 0..12usize {
            for is_minor in [false, true] {
                let key = estimate_key(&cadence(44_100, root, is_minor), 44_100).expect("a key");
                assert!(
                    key.confidence >= 0.0 && key.confidence <= 1.0,
                    "confidence out of range: {key:?}"
                );
                lowest = lowest.min(key.confidence);
                total += key.confidence as f64;
            }
        }
        assert!(lowest > 0.45, "weakest cadence read {lowest}");
        assert!(total / 24.0 > 0.6, "mean confidence {}", total / 24.0);
    }

    /// The same music at four rates has to give the same answer: the decimator
    /// picks a different factor for each. 16 kHz is in the band a rounded
    /// `rate / TARGET_RATE` used to leave undecimated, where the transform ran
    /// too coarse to resolve the bottom octave.
    #[test]
    fn the_rate_the_file_was_made_at_does_not_change_the_key() {
        for rate in [16_000u32, 22_050, 44_100, 48_000] {
            let key = estimate_key(&c_major_cadence(rate), rate).expect("a key");
            assert_eq!(key.camelot(), "8B", "{rate} Hz gave {key:?}");
        }
    }

    /// The transform has to be able to tell a semitone from its neighbour at
    /// the bottom of the band, at every rate a decoder produces. A Hann main
    /// lobe is `4 * rate / FRAME` wide; the semitone at [`CHROMA_LOW_HZ`] is
    /// 6.541 Hz. The 12.5 to 16.5 kHz gap is the one that used to fail.
    #[test]
    fn the_decimator_never_outruns_the_transform() {
        let semitone = CHROMA_LOW_HZ * (2.0f64.powf(1.0 / 12.0) - 1.0);
        for rate in [
            8_000u32, 11_025, 12_000, 12_500, 14_000, 16_000, 16_537, 22_050, 32_000, 44_100,
            48_000, 88_200, 96_000, 192_000, 384_000,
        ] {
            let (_, out) = decimate_mono(&[[0i16, 0]; 8], rate as f64);
            let lobe = 4.0 * out / FRAME as f64;
            assert!(
                lobe <= semitone,
                "{rate} Hz decimates to {out} Hz: a {lobe:.3} Hz lobe over a {semitone:.3} Hz semitone"
            );
            assert!(out >= 1.0, "{rate} Hz decimated to {out}");
        }
    }

    /// One channel silent is a real file, not a broken one. BOTH directions,
    /// because a down-mix that reads only one channel passes the other.
    #[test]
    fn either_dead_channel_is_still_a_key() {
        for dead in [0usize, 1] {
            let mut frames = c_major_cadence(44_100);
            for frame in frames.iter_mut() {
                frame[dead] = 0;
            }
            let key = estimate_key(&frames, 44_100).expect("a key");
            assert_eq!(key.camelot(), "8B", "channel {dead} silenced gave {key:?}");
        }
        // And the down-mix is a SUM, not a pick: a file whose two channels carry
        // different music is judged on both of them.
        let left = c_major_cadence(44_100);
        let right = cadence(44_100, 6, false);
        let split: Vec<[i16; 2]> = left
            .iter()
            .zip(right.iter())
            .map(|(a, b)| [a[0], b[1]])
            .collect();
        let both = estimate_key(&split, 44_100).expect("a key");
        let only_left = estimate_key(&left, 44_100).expect("a key");
        let only_right = estimate_key(&right, 44_100).expect("a key");
        assert_ne!(only_left.camelot(), only_right.camelot());
        assert!(
            both.confidence < only_left.confidence && both.confidence < only_right.confidence,
            "two keys at once read as confidently as one: {both:?} vs {only_left:?} / {only_right:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // the wheel and the names
    // ---------------------------------------------------------------------------

    /// The whole wheel, by hand, against the chart a DJ reads off the screen.
    /// Every neighbour on it shares its notes with the last, which is the only
    /// property of this mapping anyone actually uses.
    #[test]
    fn the_camelot_wheel_is_the_camelot_wheel() {
        // (tonic, minor) -> the number and letter it is known by.
        let wheel: [(u8, bool, &str); 24] = [
            (8, true, "1A"),   // G# / Ab minor
            (11, false, "1B"), // B major
            (3, true, "2A"),   // Eb minor
            (6, false, "2B"),  // F# major
            (10, true, "3A"),  // Bb minor
            (1, false, "3B"),  // Db major
            (5, true, "4A"),   // F minor
            (8, false, "4B"),  // Ab major
            (0, true, "5A"),   // C minor
            (3, false, "5B"),  // Eb major
            (7, true, "6A"),   // G minor
            (10, false, "6B"), // Bb major
            (2, true, "7A"),   // D minor
            (5, false, "7B"),  // F major
            (9, true, "8A"),   // A minor
            (0, false, "8B"),  // C major
            (4, true, "9A"),   // E minor
            (7, false, "9B"),  // G major
            (11, true, "10A"), // B minor
            (2, false, "10B"), // D major
            (6, true, "11A"),  // F# minor
            (9, false, "11B"), // A major
            (1, true, "12A"),  // C# minor
            (4, false, "12B"), // E major
        ];
        let mut seen: Vec<String> = Vec::new();
        for (tonic, minor, expected) in wheel {
            let key = KeyEstimate { tonic, minor, confidence: 0.0 };
            assert_eq!(key.camelot(), expected, "{tonic} minor={minor}");
            seen.push(key.camelot());
        }
        // Twenty-four keys, twenty-four addresses: no two keys share one.
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 24);
    }

    #[test]
    fn keys_are_spelled_the_way_they_are_written() {
        let name = |tonic: u8, minor: bool| KeyEstimate { tonic, minor, confidence: 0.0 }.name();
        assert_eq!(name(0, false), "C");
        assert_eq!(name(9, true), "Am");
        assert_eq!(name(7, false), "G");
        assert_eq!(name(4, true), "Em");
        // The tritone is written sharp on both sides of the wheel; everything
        // else that has a choice is written flat.
        assert_eq!(name(6, false), "F#");
        assert_eq!(name(6, true), "F#m");
        assert_eq!(name(3, false), "Eb");
        assert_eq!(name(10, true), "Bbm");
        assert_eq!(name(8, true), "G#m");
        assert_eq!(name(8, false), "Ab");
    }

    // ---------------------------------------------------------------------------
    // the refusals
    // ---------------------------------------------------------------------------

    /// The estimate has to survive the things a library is full of, and each of
    /// these dies at a DIFFERENT gate.
    #[test]
    fn nothing_to_judge_is_answered_with_none_and_not_a_panic() {
        // No samples at all.
        assert_eq!(estimate_key(&[], 44_100), None);
        // Shorter than MIN_TRACK_SECS.
        assert_eq!(estimate_key(&[[1_234, -4_321]; 10], 44_100), None);
        // Four seconds of digital black: long enough, loud enough to reach the
        // absolute floor and stopped by it.
        assert_eq!(estimate_key(&vec![[0, 0]; 44_100 * 4], 44_100), None);
        // A header that claims a rate no decoder produces.
        assert_eq!(estimate_key(&c_major_cadence(44_100), 0), None);
        // A rate whose usable band is under an octave wide: 400 Hz gives a
        // Nyquist-limited ceiling of 180 Hz over a 110 Hz floor, which is eight
        // semitones. Long enough and loud enough to reach that check.
        let low_rate = tones(400, 40.0, &[(150.0, 0.9)]);
        assert!(low_rate.len() > FRAME, "the band check has to be reachable");
        assert_eq!(estimate_key(&low_rate, 400), None);
    }

    /// A track that is quiet in ABSOLUTE terms is not a track.
    ///
    /// [`WINDOW_FLOOR`] is relative to the record's own loudness and can never
    /// fire on a record that is quiet throughout, so each of these used to come
    /// back as a key with a confidence in the normal range: one-step dither read
    /// F minor at 0.201, a one-step tone read A major at 0.096, three-step hiss
    /// read F minor.
    #[test]
    fn a_file_with_nothing_but_a_noise_floor_in_it_has_no_key() {
        // One-step dither.
        let mut dither = Vec::new();
        let mut state = 0x9E3779B97F4A7C15u64;
        for _ in 0..44_100 * 6 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = if state & 1 == 0 { 1i16 } else { -1 };
            dither.push([value, value]);
        }
        assert_eq!(estimate_key(&dither, 44_100), None, "one-step dither got a key");
        // A tone whose peak is one step.
        let tone: Vec<[i16; 2]> = (0..44_100 * 6)
            .map(|index| {
                let value = ((2.0 * PI * 220.0 * index as f64 / 44_100.0).sin() * 1.4) as i16;
                [value, value]
            })
            .collect();
        assert_eq!(estimate_key(&tone, 44_100), None, "a one-step tone got a key");
        // Three-step hiss.
        assert_eq!(estimate_key(&noise(44_100, 6.0, 3.0 / 32_000.0, 5_150), 44_100), None);
        // And the other side of the same gate: real music at -60 dBFS, which is
        // eighteen decibels over the floor, still gets judged.
        let quiet_music = quieter(&c_major_cadence(44_100), 0.001);
        let key = estimate_key(&quiet_music, 44_100).expect("quiet music is still music");
        assert_eq!(key.camelot(), "8B", "{key:?}");
    }

    /// Broadband noise has no key and must not be given one.
    ///
    /// This is the gate that could not fire at all before: the old bin-to-class
    /// fold put a hard floor of 0.088 under the relative spread of ANY chroma,
    /// eighty-eight times the 1e-3 threshold that was supposed to catch a flat
    /// profile, and white noise came back as F minor at a higher confidence
    /// than any correct answer ever scored.
    #[test]
    fn noise_has_no_key() {
        for seed in [1u64, 2, 3, 12_345, 24_680] {
            let white = noise(44_100, 10.0, 0.5, seed);
            assert_eq!(estimate_key(&white, 44_100), None, "white noise seed {seed} got a key");
            let chroma = track_chroma(&white, 44_100).expect("a chroma");
            assert!(shape(&chroma) < FLAT_CHROMA, "seed {seed} spread {}", shape(&chroma));
        }
        // Noise at a quarter of full scale, so the absolute floor is not what is
        // doing the refusing.
        assert_eq!(estimate_key(&noise(44_100, 10.0, 0.25, 7), 44_100), None);
        // And the other side: music does not trip it, even under four times its
        // own level in noise. Measured spread there is 0.120 against 0.95 for
        // the cadence in the clear.
        let buried = over(&c_major_cadence(44_100), &noise(44_100, 8.0, 0.5, 4_242));
        let key = estimate_key(&buried, 44_100).expect("music under noise is still music");
        assert_eq!(key.camelot(), "8B", "{key:?}");
        let chroma = track_chroma(&buried, 44_100).expect("a chroma");
        assert!(shape(&chroma) > FLAT_CHROMA, "buried cadence spread {}", shape(&chroma));
    }

    /// Under a second there is no harmony to average over. The length guard is
    /// the ONLY thing that refuses this one: at 11025 Hz a 0.9 second file is
    /// 9922 samples, more than one transform's worth.
    #[test]
    fn a_track_too_short_to_have_a_key_does_not_get_one() {
        let short = progression(11_025, 0.9, &[&major(0, 4)]);
        assert!(short.len() > FRAME, "the length guard has to be the gate that fires");
        assert_eq!(estimate_key(&short, 11_025), None);
        // Just over the line, the same music does get an answer.
        let long_enough = progression(11_025, 1.4, &[&major(0, 4)]);
        assert!(estimate_key(&long_enough, 11_025).is_some());
    }

    // ---------------------------------------------------------------------------
    // what the chroma is made of
    // ---------------------------------------------------------------------------

    #[test]
    fn a_sine_at_a440_lands_in_the_a_bin() {
        let chroma = track_chroma(&tones(44_100, 4.0, &[(440.0, 1.0)]), 44_100).expect("a chroma");
        assert_eq!(strongest(&chroma), 9, "{chroma:?}");
        // And it is not a near thing: a pure tone belongs to ONE class.
        let runner_up = (0..12)
            .filter(|class| *class != 9)
            .fold(0.0f64, |most, class| most.max(chroma[class]));
        assert!(chroma[9] > runner_up * 20.0, "{chroma:?}");
    }

    /// The octave a chord is voiced in is not part of its name.
    ///
    /// Three voicings and a whole cadence rather than one held triad: a single
    /// triad leaves its relative minor within a fraction of a percent, and a
    /// test that turns on the last bit of a correlation is a test that reports
    /// the weather.
    #[test]
    fn the_same_chord_an_octave_up_is_the_same_key() {
        let mut confidences = Vec::new();
        for octave in [3i32, 4, 5] {
            let (one, four, five) = (
                major(0, octave),
                major(5, octave - 1),
                major(7, octave - 1),
            );
            let track = progression(44_100, 8.0, &[&one, &four, &five, &one]);
            let key = estimate_key(&track, 44_100).expect("a key");
            assert_eq!(key.camelot(), "8B", "octave {octave} gave {key:?}");
            assert!(key.confidence > 0.2, "octave {octave} gave {key:?}");
            confidences.push(key.confidence);
        }
        // The three readings agree on the key AND roughly on how sure they are.
        let spread = confidences.iter().cloned().fold(0.0f32, f32::max)
            - confidences.iter().cloned().fold(1.0f32, f32::min);
        assert!(spread < 0.5, "the octave moved the confidence by {spread}: {confidences:?}");
    }

    /// Magnitude, not power. Two tones a minor third apart, one at half the
    /// other's amplitude: the chroma has to read the AMPLITUDE ratio. Squaring
    /// would read 4:1 and let the loudest partial in a chord name the bar.
    #[test]
    fn the_chroma_reads_amplitude_and_not_energy() {
        let chroma =
            track_chroma(&tones(44_100, 6.0, &[(440.0, 1.0), (523.251, 0.5)]), 44_100)
                .expect("a chroma");
        let ratio = chroma[9] / chroma[0];
        assert!((1.7..2.9).contains(&ratio), "A/C came out {ratio}: {chroma:?}");
        let chroma =
            track_chroma(&tones(44_100, 6.0, &[(440.0, 1.0), (523.251, 0.25)]), 44_100)
                .expect("a chroma");
        let ratio = chroma[9] / chroma[0];
        assert!((3.2..5.6).contains(&ratio), "A/C at a quarter came out {ratio}: {chroma:?}");
    }

    /// The raised cosine across each semitone: a partial that sits BETWEEN two
    /// notes is not evidence for either of them.
    ///
    /// The same interferer at three times the level of an A440, once on a note
    /// and once a quarter-tone off it. On the note it takes the chroma over; off
    /// it, it is thrown away and the quiet A still wins. Weighting every bin in
    /// the band equally instead loses that, and a detuned synth or a bent string
    /// votes at full strength for whichever neighbour it happens to round to.
    #[test]
    fn a_partial_between_two_notes_votes_for_neither() {
        let on_note = track_chroma(
            &tones(44_100, 6.0, &[(440.0, 1.0), (pitch(2, 5), 3.0)]),
            44_100,
        )
        .expect("a chroma");
        assert_eq!(strongest(&on_note), 2, "a loud D did not win: {on_note:?}");

        let quarter_sharp = pitch(2, 5) * 2.0f64.powf(0.5 / 12.0);
        let between = track_chroma(&tones(44_100, 6.0, &[(440.0, 1.0), (quarter_sharp, 3.0)]), 44_100)
            .expect("a chroma");
        assert_eq!(strongest(&between), 9, "a quarter-tone outvoted the note: {between:?}");
        assert!(
            between[9] > between[2] * 4.0 && between[9] > between[3] * 4.0,
            "the quarter-tone still voted: {between:?}"
        );
    }

    /// The band has a bottom and it is a resolution limit, not a taste.
    ///
    /// A 55 Hz tone is a real A, and the transform cannot say so: at that
    /// frequency a semitone is 3.3 Hz and this window's main lobe is 5.4 Hz. It
    /// must therefore NOT be read as an A. The same tone an octave up, at the
    /// floor, must be.
    #[test]
    fn a_pitch_under_the_floor_is_not_read_as_a_pitch() {
        let under = track_chroma(&tones(44_100, 6.0, &[(55.0, 1.0)]), 44_100).expect("a chroma");
        assert_ne!(strongest(&under), 9, "55 Hz was read as an A: {under:?}");
        let at_floor =
            track_chroma(&tones(44_100, 6.0, &[(110.0, 1.0)]), 44_100).expect("a chroma");
        assert_eq!(strongest(&at_floor), 9, "110 Hz was not read as an A: {at_floor:?}");
    }

    /// The band has a top too, and chords get voiced up there.
    ///
    /// A whole cadence with every voice between 1046 and 2349 Hz. Nothing in it
    /// is under a kilohertz, so a ceiling put anywhere near the fundamentals a
    /// piano spends its time on throws the entire record away.
    #[test]
    fn a_chord_voiced_high_still_has_a_key() {
        let (one, four, five) = (major(0, 6), major(5, 6), major(7, 6));
        let track = progression(44_100, 8.0, &[&one, &four, &five, &one]);
        let key = estimate_key(&track, 44_100).expect("a key");
        assert_eq!(key.tonic, 0, "{key:?}");
        assert!(!key.minor, "{key:?}");
    }

    /// The octave weighting is a weighting: the middle of the keyboard counts
    /// for more than either end of the band.
    #[test]
    fn the_middle_of_the_keyboard_counts_for_most() {
        // The shape of the window itself.
        assert!(octave_weight(440.0) > octave_weight(110.0) * 1.3);
        assert!(octave_weight(440.0) > octave_weight(3_520.0) * 1.3);
        assert!(octave_weight(220.0) > octave_weight(3_520.0));

        // And what it does. Two tones of equal amplitude, one in the middle of
        // the keyboard and one near an edge of the band: the middle one has to
        // come out ahead.
        let high = track_chroma(&tones(44_100, 6.0, &[(440.0, 1.0), (pitch(6, 7), 1.0)]), 44_100)
            .expect("a chroma");
        assert!(high[9] > high[6] * 1.8, "a cymbal-register tone weighed as much: {high:?}");
        let low = track_chroma(&tones(44_100, 6.0, &[(440.0, 1.0), (pitch(0, 3), 1.0)]), 44_100)
            .expect("a chroma");
        assert!(low[9] > low[0], "the bottom of the band outweighed the middle: {low:?}");
    }

    /// A flat spectrum has to fold to a flat chroma, exactly, at every rate.
    ///
    /// This is the property the whole semitone array exists for. Folding bins
    /// straight onto classes gave a flat spectrum a permanent shape — Bb at
    /// -14.8%, G# at +14.3%, sd/mean 0.0883 — which is why noise had a key.
    #[test]
    fn a_flat_spectrum_folds_flat() {
        for rate in [8_000.0f64, 11_025.0, 12_000.0, 13_000.0] {
            let geometry = Geometry::new(rate).expect("geometry at {rate}");
            let count = geometry.notes();
            let mut flat = vec![1.0f64; count];
            let mut envelope = vec![0.0f64; count];
            let mass = geometry.fold(&mut flat, &mut envelope);
            let deviation = shape(&mass);
            assert!(deviation < 1e-9, "{rate} Hz folds flat to sd/mean {deviation}: {mass:?}");
        }
    }

    /// A bass note pays for its own harmonics before the chord over it is read.
    ///
    /// Harmonic 5 of a fundamental is a MAJOR THIRD, and a bass-forward mix used
    /// to read every minor chord as major because of it: a sawtooth bass five
    /// times the level of the triad over it read 12/12 minor chords as major.
    /// The control is the same signal with harmonic 5 taken out, which was right
    /// all along and stays right.
    #[test]
    fn a_loud_bass_does_not_turn_a_minor_chord_major() {
        for skip_the_third in [false, true] {
            for root in [0usize, 4, 7, 10] {
                let mut voices: Vec<(f64, f64)> = triad(root, 4, 3)
                    .into_iter()
                    .map(|frequency| (frequency, 1.0))
                    .collect();
                let bass = pitch(root, 2);
                for harmonic in 1..=12usize {
                    if skip_the_third && harmonic == 5 {
                        continue;
                    }
                    let frequency = bass * harmonic as f64;
                    if frequency > 44_100.0 * 0.45 {
                        break;
                    }
                    voices.push((frequency, 5.0 / harmonic as f64));
                }
                let key = estimate_key(&tones(44_100, 6.0, &voices), 44_100).expect("a key");
                assert_eq!(
                    (key.tonic as usize, key.minor),
                    (root, true),
                    "minor triad on {root} over a sawtooth bass (harmonic 5 {}) read {key:?}",
                    if skip_the_third { "removed" } else { "present" }
                );
            }
        }
    }

    // ---------------------------------------------------------------------------
    // which windows get a vote, and how much of one
    // ---------------------------------------------------------------------------

    /// Every window that carried music gets ONE vote, whatever it weighed.
    ///
    /// Eight seconds of a C major cadence at full scale against twenty-four
    /// seconds of an F# major cadence at fifteen percent. Three times as many
    /// bars in F#, a fifth of the level: normalizing each window against its own
    /// loudest class is what makes the bars decide instead of the decibels.
    #[test]
    fn a_long_quiet_passage_outvotes_a_short_loud_one() {
        let loud = c_major_cadence(44_100);
        let mut quiet = Vec::new();
        for _ in 0..3 {
            quiet.extend_from_slice(&cadence(44_100, 6, false));
        }
        let track = then(&loud, &quieter(&quiet, 0.15));
        let key = estimate_key(&track, 44_100).expect("a key");
        assert_eq!(key.tonic, 6, "{key:?}");
        assert!(!key.minor, "{key:?}");
    }

    /// A window far enough under the record's own level is the room between the
    /// tracks, and it does not get a vote at all.
    ///
    /// The same shape as the test above, with the long passage at two percent
    /// instead of fifteen. Without the floor its noise is normalized up to full
    /// strength and thirty-two bars of nearly nothing decide the record.
    #[test]
    fn a_passage_under_the_floor_gets_no_vote() {
        let loud = c_major_cadence(44_100);
        let mut quiet = Vec::new();
        for _ in 0..3 {
            quiet.extend_from_slice(&cadence(44_100, 6, false));
        }
        let track = then(&loud, &quieter(&quiet, 0.02));
        let key = estimate_key(&track, 44_100).expect("a key");
        assert_eq!(key.camelot(), "8B", "{key:?}");
    }

    /// The loudness reference is a percentile, and a percentile has to be able
    /// to name something other than the maximum.
    ///
    /// A three-second track is nine windows, and `len * 0.90` rounds to nine —
    /// the last index — so every track under about four seconds used to be
    /// gated against its own peak. Here that peak is a single very loud bar in
    /// another key; gate on it and the whole cadence falls under the floor and
    /// the loud bar is the only thing left to judge.
    #[test]
    fn a_short_track_is_not_gated_against_its_own_peak() {
        let (one, four) = (major(0, 4), major(5, 3));
        let quiet = quieter(&progression(44_100, 3.0, &[&one, &four, &one]), 0.02);
        let burst = progression(44_100, 0.45, &[&major(6, 4)]);
        let mut track = quiet.clone();
        let at = track.len() - burst.len();
        track[at..].copy_from_slice(&burst);
        let key = estimate_key(&track, 44_100).expect("a key");
        assert_eq!(key.tonic, 0, "the loud bar decided a short track: {key:?}");
    }

    /// Half-window overlap. A chord that lands on the boundary between two
    /// windows falls in BOTH of their tapers; at half the hop there is always a
    /// window centred on it instead.
    ///
    /// The signal is a held C-and-G — a fifth, which names no mode — with short
    /// Eb bursts that do: the record is C minor and nothing but those bursts
    /// says so. Every burst straddles a boundary between non-overlapping
    /// windows. At a full hop they land where the taper is near zero, the Eb
    /// never arrives, and the fifth reads as C major instead. Measured: C minor
    /// at 0.823 with the half hop, C major at 0.239 with the full one.
    #[test]
    fn a_chord_on_the_boundary_is_still_heard() {
        let rate = 44_100u32;
        let (_, decimated) = decimate_mono(&[[0i16, 0]; 8], rate as f64);
        let factor = (rate as f64 / decimated).round() as usize;
        // One non-overlapped window, in source samples.
        let boundary = FRAME * factor;
        let burst = boundary / 12;
        let held = [pitch(0, 3), pitch(7, 3), pitch(0, 4), pitch(7, 4)];
        let says_minor = [pitch(3, 4), pitch(3, 5)];
        let count = (rate as f64 * 16.0) as usize;
        let mut out = Vec::with_capacity(count);
        for index in 0..count {
            let at = index as f64 / rate as f64;
            let mut value = 0.0f64;
            for (voice, frequency) in held.iter().enumerate() {
                value += 0.25 * (2.0 * PI * frequency * at + voice as f64 * 0.7).sin();
            }
            let offset = index % boundary;
            if offset < burst / 2 || offset > boundary - burst / 2 {
                for (voice, frequency) in says_minor.iter().enumerate() {
                    value += 2.0 * (2.0 * PI * frequency * at + voice as f64 * 0.31).sin();
                }
            }
            let sample = (value * 20_000.0).clamp(-32_000.0, 32_000.0) as i16;
            out.push([sample, sample]);
        }
        // The bursts are the only thing in the file that names a mode.
        let without = estimate_key(&tones(rate, 8.0, &held.map(|f| (f, 1.0))), rate).expect("a key");
        assert!(!without.minor, "the held fifth already picked a mode: {without:?}");
        let key = estimate_key(&out, rate).expect("a key");
        assert_eq!(key.camelot(), "5A", "the boundary bursts were not heard: {key:?}");
    }

    /// A window that is all one note does not get to outvote a window that is a
    /// chord, and neither gets to outvote the other by being longer than it is.
    ///
    /// Eight seconds of an F# drone — three octaves of one pitch class, so its
    /// chroma is a single spike — against sixteen seconds of a C major cadence,
    /// whose chroma is spread over seven. Normalizing each window against its
    /// own loudest class is what makes those thirty-two bars of C outweigh the
    /// sixteen bars of F#; summing the chroma raw instead lets the spike win,
    /// measured, and the record comes back as F# minor.
    #[test]
    fn a_drone_does_not_outvote_a_chord() {
        let drone: Vec<(f64, f64)> = [3i32, 4, 5]
            .iter()
            .map(|octave| (pitch(6, *octave), 1.0))
            .collect();
        let mut track = tones(44_100, 8.0, &drone);
        for _ in 0..2 {
            track.extend_from_slice(&c_major_cadence(44_100));
        }
        let key = estimate_key(&track, 44_100).expect("a key");
        assert_eq!(key.tonic, 0, "the drone decided the record: {key:?}");
        assert!(!key.minor, "{key:?}");
    }

    /// The decimator's filter, from the other side: content above the decimated
    /// Nyquist must not fold back into the band and vote.
    ///
    /// A C major cadence with a loud 10_285 Hz tone over it. Sub-sample by four
    /// without filtering and that tone lands on 740 Hz — an F#, a tritone from
    /// the key and the one note that ruins it. The filter puts it 22 dB down and
    /// the cadence is unmoved.
    #[test]
    fn what_is_above_the_new_nyquist_does_not_come_back_inside_the_band() {
        let alias = 44_100.0 / 4.0 - pitch(6, 5);
        assert!((alias - 10_285.0).abs() < 1.0, "the probe tone moved: {alias}");
        let track = over(&c_major_cadence(44_100), &tones(44_100, 8.0, &[(alias, 2.0)]));
        let key = estimate_key(&track, 44_100).expect("a key");
        assert_eq!(key.camelot(), "8B", "the aliased tone was heard: {key:?}");
        let chroma = track_chroma(&track, 44_100).expect("a chroma");
        assert!(chroma[6] < chroma[0], "an F# arrived from above Nyquist: {chroma:?}");
    }

    // ---------------------------------------------------------------------------
    // the pieces, on their own
    // ---------------------------------------------------------------------------

    /// The profiles themselves, guarded by what they mean. A table typed one
    /// degree out still looks like a plausible list of numbers, and what it
    /// silently does is move the fifth's weight onto the tritone — which reads
    /// every record as the key a third away and never once looks like a crash.
    #[test]
    fn the_profiles_rank_their_own_degrees() {
        // Through a function, so these are comparisons the test runs and not
        // constants the compiler folds before it ever gets there.
        fn stronger(profile: &[f64; 12], degree: usize, than: usize) -> bool {
            profile[degree] > profile[than]
        }
        for profile in [&MAJOR_PROFILE, &MINOR_PROFILE] {
            let strongest = (0..12).fold(0usize, |best, degree| {
                if profile[degree] > profile[best] {
                    degree
                } else {
                    best
                }
            });
            assert_eq!(strongest, 0, "the tonic is not the strongest degree");
            assert!(stronger(profile, 7, 6), "the fifth is under the tritone");
            assert!(stronger(profile, 7, 8), "the fifth is under the sixth");
        }
        // The third is what the two modes disagree about, and nothing else.
        assert!(stronger(&MAJOR_PROFILE, 4, 3));
        assert!(stronger(&MINOR_PROFILE, 3, 4));
        // The fifth is second only to the tonic in major; in minor the third
        // takes that place, which is the whole character of the mode.
        assert!(stronger(&MAJOR_PROFILE, 7, 3) && stronger(&MAJOR_PROFILE, 7, 4));
        assert!(stronger(&MINOR_PROFILE, 3, 7));
    }

    /// The correlation, on its own: a profile that IS the major template
    /// correlates perfectly with it and worse with everything else.
    #[test]
    fn the_correlation_prefers_the_profile_it_was_given() {
        assert!((pearson(&MAJOR_PROFILE, &MAJOR_PROFILE) - 1.0).abs() < 1e-12);
        assert!(pearson(&MAJOR_PROFILE, &MINOR_PROFILE) < 0.9);
        assert_eq!(pearson(&[1.0; 12], &MAJOR_PROFILE), 0.0);
        let mut rotated = [0.0f64; 12];
        for degree in 0..12 {
            rotated[degree] = MAJOR_PROFILE[(degree + 5) % 12];
        }
        assert!(pearson(&rotated, &MAJOR_PROFILE) < 0.9);
        // Affine invariance: the shape is what is scored, not the level.
        let mut scaled = [0.0f64; 12];
        for degree in 0..12 {
            scaled[degree] = MAJOR_PROFILE[degree] * 17.0 + 4.0;
        }
        assert!((pearson(&scaled, &MAJOR_PROFILE) - 1.0).abs() < 1e-12);
    }

    /// The transform against the definition, magnitude AND phase.
    ///
    /// A cosine alone cannot catch a sign error in the twiddle table: it is
    /// real and even, so its transform is real and a conjugated transform is
    /// the same transform. A sine can, and a signal with no symmetry at all
    /// checked against the naive sum can catch anything.
    #[test]
    fn the_transform_transforms() {
        let size = 64usize;
        let transform = Fft::new(size);

        // A single bin's worth of cosine comes back as a single bin.
        let mut real = vec![0.0f64; size];
        let mut imaginary = vec![0.0f64; size];
        for (index, value) in real.iter_mut().enumerate() {
            *value = (2.0 * PI * 5.0 * index as f64 / size as f64).cos();
        }
        transform.forward(&mut real, &mut imaginary);
        for bin in 0..size / 2 + 1 {
            let magnitude = (real[bin] * real[bin] + imaginary[bin] * imaginary[bin]).sqrt();
            if bin == 5 {
                assert!((magnitude - size as f64 / 2.0).abs() < 1e-9, "bin 5 {magnitude}");
            } else {
                assert!(magnitude < 1e-9, "bin {bin} {magnitude}");
            }
        }

        // A sine, where the ANSWER IS IMAGINARY and its sign is the sign of the
        // twiddles: X[k] = -i N/2 at k, +i N/2 at N - k.
        let mut real = vec![0.0f64; size];
        let mut imaginary = vec![0.0f64; size];
        for (index, value) in real.iter_mut().enumerate() {
            *value = (2.0 * PI * 5.0 * index as f64 / size as f64).sin();
        }
        transform.forward(&mut real, &mut imaginary);
        assert!(real[5].abs() < 1e-9, "a sine has no real part at its own bin: {}", real[5]);
        assert!(
            (imaginary[5] + size as f64 / 2.0).abs() < 1e-9,
            "bin 5 imaginary {} (a sign flip in the twiddles reads +32 here)",
            imaginary[5]
        );
        assert!((imaginary[size - 5] - size as f64 / 2.0).abs() < 1e-9);

        // And against the definition, on a signal with no symmetry to hide in.
        let signal: Vec<f64> = (0..size)
            .map(|index| ((index * index * 37 + index * 11) % 101) as f64 / 50.0 - 1.0)
            .collect();
        let mut real = signal.clone();
        let mut imaginary = vec![0.0f64; size];
        transform.forward(&mut real, &mut imaginary);
        for bin in 0..size {
            let (mut want_real, mut want_imaginary) = (0.0f64, 0.0f64);
            for (index, value) in signal.iter().enumerate() {
                let angle = -2.0 * PI * bin as f64 * index as f64 / size as f64;
                want_real += value * angle.cos();
                want_imaginary += value * angle.sin();
            }
            assert!((real[bin] - want_real).abs() < 1e-9, "bin {bin} real");
            assert!((imaginary[bin] - want_imaginary).abs() < 1e-9, "bin {bin} imaginary");
        }
    }

    /// A size that is not a power of two used to be rounded up in silence, and
    /// then every transform of a buffer the caller had sized to what they ASKED
    /// for came back untouched — a spectrum of zeros, with no complaint.
    #[test]
    #[should_panic(expected = "not a power of two")]
    fn a_transform_size_that_is_not_a_power_of_two_is_refused() {
        Fft::new(100);
    }

    #[test]
    #[should_panic(expected = "wants")]
    fn a_buffer_of_the_wrong_length_is_refused() {
        let transform = Fft::new(64);
        let mut real = vec![0.0f64; 63];
        let mut imaginary = vec![0.0f64; 63];
        transform.forward(&mut real, &mut imaginary);
    }

    /// Rates and lengths a decoder can hand over, including the ones that make
    /// no sense, against seven waveforms: silence, hard-panned full scale, both
    /// channels pinned, a sawtooth, an out-of-phase sine, one-step dither and a
    /// hashed pseudo-random fill. 840 cases. Nothing here may panic and no
    /// confidence may leave 0.0..=1.0.
    #[test]
    fn no_input_makes_it_panic() {
        let rates = [0u32, 1, 2, 7, 100, 489, 500, 8_000, 11_025, 44_100, 192_000, 384_000];
        let lengths = [0usize, 1, 2, 10, 4_095, 8_191, 8_192, 8_193, 16_384, 100_000];
        let mut cases = 0usize;
        for rate in rates {
            for length in lengths {
                let shapes: [Vec<[i16; 2]>; 7] = [
                    vec![[0, 0]; length],
                    vec![[i16::MIN, i16::MAX]; length],
                    vec![[i16::MAX, i16::MAX]; length],
                    (0..length).map(|i| [(i % 4_001) as i16 - 2_000, -((i % 331) as i16)]).collect(),
                    (0..length)
                        .map(|i| {
                            let value = ((i as f64 * 0.031).sin() * 30_000.0) as i16;
                            [value, -value]
                        })
                        .collect(),
                    (0..length)
                        .map(|i| {
                            let value = if i % 2 == 0 { 1i16 } else { -1 };
                            [value, value]
                        })
                        .collect(),
                    (0..length)
                        .map(|i| {
                            let mut state =
                                (i as u64).wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                            state ^= state >> 33;
                            let value = ((state % 60_001) as i32 - 30_000) as i16;
                            [value, value.wrapping_neg()]
                        })
                        .collect(),
                ];
                for frames in shapes {
                    cases += 1;
                    if let Some(key) = estimate_key(&frames, rate) {
                        assert!(key.tonic < 12, "{rate} {length} {key:?}");
                        assert!(
                            key.confidence.is_finite() && (0.0..=1.0).contains(&key.confidence),
                            "{rate} {length} {key:?}"
                        );
                        assert!(!key.camelot().is_empty() && !key.name().is_empty());
                    }
                }
            }
        }
        assert_eq!(cases, 840);
    }
}
