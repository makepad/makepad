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
//! Deliberately self-contained. The transform, the window, the decimator and
//! the profiles all live in this file, so a worker that has PCM and nothing
//! else can run it; it holds one frame of scratch plus twelve floats per
//! analysis window, and it is pure — same samples in, same key out, on every
//! machine and in every order.
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
    /// The margin over second place measured against the whole spread of the
    /// twenty-four candidates, so it says how much better the answer is than
    /// the next one rather than how well it fits in the abstract. A record
    /// whose relative minor scores nearly as well reads LOW here, which is
    /// the truth about that record and not a fault in the estimate.
    pub confidence: f32,
}

/// How a key's tonic is spelled when it is the root of a MAJOR key: the
/// flat side of the wheel, because nobody writes A# major.
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

// ---------------------------------------------------------------------------
// the analysis pass
// ---------------------------------------------------------------------------

/// The rate the chroma pass runs at. Everything key lives in is under 4 kHz,
/// so the top of the band is thrown away ONCE by the decimator instead of
/// being paid for in every transform.
const TARGET_RATE: f64 = 11_025.0;
/// Transform size and hop, in samples at the decimated rate.
///
/// 8192 is 0.74 seconds and 1.35 Hz per bin. That resolution is what sets
/// the bottom of the chroma band: a semitone at A2 is 6.2 Hz wide and this
/// window's main lobe is 5.4 Hz, so a note there still falls inside its own
/// class. At 4096 the lobe is 10.8 Hz and A2 smears across three classes,
/// which is why the cheaper size is not used.
const FRAME: usize = 8192;
/// Half-window overlap: enough that a chord change never falls entirely in
/// one window's taper, cheap enough that a long record is still one pass.
const HOP: usize = 4096;
/// The band folded onto the twelve classes, A2 to A7.
///
/// The floor is not the lowest note a record plays — it is the lowest note
/// this transform can tell from its neighbours. Below it the bass is heard
/// through its own harmonics, which land on the same pitch class an octave
/// up (and on the fifth, which the profiles already expect to be strong).
const CHROMA_LOW_HZ: f64 = 110.0;
const CHROMA_HIGH_HZ: f64 = 3_520.0;
/// Where the weight over the band is centred, and how wide it is in
/// octaves. Chords are voiced around the middle of the keyboard; without
/// this a bright master's cymbals contribute as much pitch-class evidence
/// as its piano, and cymbals have no pitch class.
const OCTAVE_CENTRE_HZ: f64 = 440.0;
const OCTAVE_WIDTH: f64 = 2.0;
/// The percentile of window loudness a track is judged against, rather than
/// its maximum, so one clipped bar cannot set the gate for the whole record.
const LOUDNESS_PERCENTILE: f64 = 0.90;
/// A window this far under that reference is the room between the tracks,
/// not the record: its chroma is noise, and normalizing it would give that
/// noise a full-scale vote.
const WINDOW_FLOOR: f64 = 0.04;
/// Shortest track worth an opinion. Under a second there is no harmony to
/// average over, only whatever chord happened to be sounding.
const MIN_TRACK_SECS: f64 = 1.0;
/// Below this, the chroma has no shape at all — silence that cleared the
/// gate, or broadband noise — and every candidate scores the same.
const FLAT_CHROMA: f64 = 1e-3;

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
    let spread = chroma.iter().map(|value| (value - mean) * (value - mean)).sum::<f64>();
    if mean <= 1e-12 || (spread / 12.0).sqrt() <= mean * FLAT_CHROMA {
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
    let mut worst = f64::INFINITY;
    for (index, score) in scores.iter().enumerate() {
        if index != best && *score > second {
            second = *score;
        }
        if *score < worst {
            worst = *score;
        }
    }
    let span = scores[best] - worst;
    let confidence = if span > 1e-9 {
        ((scores[best] - second) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some(KeyEstimate {
        tonic: (best / 2) as u8,
        minor: best % 2 == 1,
        confidence: confidence as f32,
    })
}

/// The track's pitch-class profile: twelve numbers, one per semitone of the
/// octave, summed over every analysis window that carried music.
///
/// Each window is normalized against its own loudest class BEFORE it is
/// added, so a drop contributes exactly as much evidence as the breakdown
/// before it. A key is what a record spends its bars in, not what it spends
/// its decibels on, and without this the loudest thirty seconds would decide
/// the whole thing.
fn track_chroma(frames: &[[i16; 2]], sample_rate: u32) -> Option<[f64; 12]> {
    if sample_rate == 0 {
        return None;
    }
    let rate = sample_rate as f64;
    if frames.len() as f64 / rate < MIN_TRACK_SECS {
        return None;
    }
    let (mono, rate_out) = decimate_mono(frames, rate);
    if mono.len() < FRAME || rate_out < 1.0 {
        return None;
    }

    // Bin geometry, and the fold from bins onto pitch classes. Computed once
    // for the whole track: it is the same for every window, and it is the
    // expensive part of the inner loop if it is not.
    let bin_hz = rate_out / FRAME as f64;
    let low = CHROMA_LOW_HZ.max(bin_hz * 2.0);
    let high = CHROMA_HIGH_HZ.min(rate_out * 0.45);
    if high <= low * 2.0 {
        return None;
    }
    let bins = FRAME / 2 + 1;
    let mut fold: Vec<Option<(usize, f64)>> = Vec::with_capacity(bins);
    for bin in 0..bins {
        let frequency = bin as f64 * bin_hz;
        if frequency < low || frequency > high {
            fold.push(None);
            continue;
        }
        let midi = 69.0 + 12.0 * (frequency / 440.0).log2();
        let nearest = midi.round();
        let class = (nearest as i64).rem_euclid(12) as usize;
        // A raised cosine across the semitone, one at the note and zero at
        // the boundary with its neighbour: window leakage lands in the bins
        // between two notes, and this is what stops it voting for either.
        let centred = (PI * (midi - nearest)).cos().powi(2);
        let octaves = (frequency / OCTAVE_CENTRE_HZ).log2() / OCTAVE_WIDTH;
        fold.push(Some((class, centred * (-0.5 * octaves * octaves).exp())));
    }

    let transform = Fft::new(FRAME);
    let window: Vec<f64> = (0..FRAME)
        .map(|index| 0.5 - 0.5 * (2.0 * PI * index as f64 / FRAME as f64).cos())
        .collect();
    let mut real = vec![0.0f64; FRAME];
    let mut imaginary = vec![0.0f64; FRAME];
    let mut windows: Vec<[f64; 12]> = Vec::with_capacity(mono.len() / HOP + 1);
    let mut loudness: Vec<f64> = Vec::with_capacity(mono.len() / HOP + 1);
    let mut at = 0usize;
    while at + FRAME <= mono.len() {
        for index in 0..FRAME {
            real[index] = mono[at + index] as f64 * window[index];
            imaginary[index] = 0.0;
        }
        transform.forward(&mut real, &mut imaginary);
        let mut chroma = [0.0f64; 12];
        let mut total = 0.0f64;
        for (bin, folded) in fold.iter().enumerate() {
            let Some((class, weight)) = folded else { continue };
            // Magnitude, not power: a chord's loudest partial is already the
            // one the ear names, and squaring it would let it name the bar.
            let magnitude =
                (real[bin] * real[bin] + imaginary[bin] * imaginary[bin]).sqrt();
            let value = magnitude * weight;
            chroma[*class] += value;
            total += value;
        }
        windows.push(chroma);
        loudness.push(total);
        at += HOP;
    }
    if windows.is_empty() {
        return None;
    }

    let mut ranked = loudness.clone();
    ranked.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let index = ((ranked.len() as f64 * LOUDNESS_PERCENTILE) as usize)
        .min(ranked.len().saturating_sub(1));
    let reference = ranked.get(index).copied().unwrap_or(0.0);
    if reference <= 1e-9 {
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

/// Down-mix to mono and decimate to about [`TARGET_RATE`], returning the
/// signal and the rate it actually came out at.
///
/// The anti-alias filter is two moving averages of `factor` samples run back
/// to back, sampled every `factor`th sample — a triangular decimator whose
/// response is `sinc(pi f / rate_out)^2`. It is chosen over a pole cascade
/// because it is FLAT where it matters: under 3 dB of tilt across the whole
/// chroma band, against 12 dB for four one-poles steep enough to be worth
/// having, and a tilt is not harmless here — it would weigh the classes that
/// happen to sit high in the band against the ones that sit low. What folds
/// back into the band arrives about 16 dB down, and it lands on a null at
/// the output rate itself.
fn decimate_mono(frames: &[[i16; 2]], rate: f64) -> (Vec<f32>, f64) {
    let factor = ((rate / TARGET_RATE).round() as usize).max(1);
    if factor == 1 {
        let mono = frames
            .iter()
            .map(|frame| (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0)
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
        let mono = (frame[0] as f64 + frame[1] as f64) * 0.5 / 32768.0;
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
    fn new(size: usize) -> Fft {
        let size = size.next_power_of_two().max(2);
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
    /// signal; a slice of the wrong length is left untouched rather than
    /// indexed past its end.
    fn forward(&self, real: &mut [f64], imaginary: &mut [f64]) {
        if real.len() != self.size || imaginary.len() != self.size {
            return;
        }
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
    use super::*;

    /// Semitones above C4 for the twelve pitch classes, as frequencies.
    fn pitch(class: usize, octave: i32) -> f64 {
        // MIDI 60 is C4; class 0 is C.
        let midi = 12 * (octave + 1) + class as i32;
        440.0 * 2.0f64.powf((midi as f64 - 69.0) / 12.0)
    }

    /// A chord progression as PCM: every chord is a set of pure tones, held
    /// for an equal share of `seconds`. Pure tones and not a synthesized
    /// instrument on purpose — a partial that is not there cannot be the
    /// reason the estimate is right.
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
                // A per-voice phase offset so the tones do not all start at
                // zero together and make one broadband click at t = 0.
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

    /// I - IV - V - I in C: the cadence that names a key, and the only
    /// chord that both frames is the tonic.
    fn c_major_cadence(rate: u32) -> Vec<[i16; 2]> {
        let (one, four, five) = (major(0, 4), major(5, 3), major(7, 3));
        progression(rate, 8.0, &[&one, &four, &five, &one])
    }

    /// i - iv - V - i in A minor. The dominant is MAJOR, as it is played,
    /// which puts a G# in the profile and is exactly what separates A minor
    /// from its relative C major.
    fn a_minor_cadence(rate: u32) -> Vec<[i16; 2]> {
        let (one, four, five) = (minor(9, 3), minor(2, 4), major(4, 4));
        progression(rate, 8.0, &[&one, &four, &five, &one])
    }

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

    /// The same cadence in all twelve keys, both modes. A detector can get
    /// C right by accident — every constant in it was chosen while looking
    /// at C — and only the transpositions say whether it heard the music or
    /// the arithmetic.
    #[test]
    fn every_key_is_found_where_it_was_put() {
        for root in 0..12usize {
            let (one, four, five) =
                (major(root, 3), major((root + 5) % 12, 3), major((root + 7) % 12, 3));
            let track = progression(44_100, 8.0, &[&one, &four, &five, &one]);
            let key = estimate_key(&track, 44_100).expect("a key");
            assert_eq!((key.tonic as usize, key.minor), (root, false), "major on {root}");

            let (one, four, five) =
                (minor(root, 3), minor((root + 5) % 12, 3), major((root + 7) % 12, 3));
            let track = progression(44_100, 8.0, &[&one, &four, &five, &one]);
            let key = estimate_key(&track, 44_100).expect("a key");
            assert_eq!((key.tonic as usize, key.minor), (root, true), "minor on {root}");
        }
    }

    /// The same music at three rates has to give the same answer: the
    /// decimator picks a different factor for each, and a key that moves
    /// with the sample rate is a bug in that filter, not in the profiles.
    #[test]
    fn the_rate_the_file_was_made_at_does_not_change_the_key() {
        for rate in [22_050u32, 44_100, 48_000] {
            let key = estimate_key(&c_major_cadence(rate), rate).expect("a key");
            assert_eq!(key.camelot(), "8B", "{rate} Hz gave {key:?}");
        }
    }

    /// One channel silent is a real file, not a broken one: the down-mix
    /// halves the level and nothing else about the answer may move.
    #[test]
    fn one_dead_channel_is_still_a_key() {
        let mut frames = c_major_cadence(44_100);
        for frame in frames.iter_mut() {
            frame[1] = 0;
        }
        let key = estimate_key(&frames, 44_100).expect("a key");
        assert_eq!(key.camelot(), "8B", "{key:?}");
    }

    /// The whole wheel, by hand, against the chart a DJ reads off the
    /// screen. Every neighbour on it shares its notes with the last, which
    /// is the only property of this mapping anyone actually uses.
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
        let name = |tonic: u8, minor: bool| {
            KeyEstimate { tonic, minor, confidence: 0.0 }.name()
        };
        assert_eq!(name(0, false), "C");
        assert_eq!(name(9, true), "Am");
        assert_eq!(name(7, false), "G");
        assert_eq!(name(4, true), "Em");
        assert_eq!(name(6, false), "F#");
        assert_eq!(name(3, false), "Eb");
        assert_eq!(name(10, true), "Bbm");
        assert_eq!(name(8, true), "G#m");
    }

    /// The estimate has to survive the things a library is full of: files
    /// that are ten samples long, files that are silent, and headers that
    /// claim a rate no decoder would produce.
    #[test]
    fn nothing_to_judge_is_answered_with_none_and_not_a_panic() {
        assert_eq!(estimate_key(&[], 44_100), None);
        assert_eq!(estimate_key(&[[0, 0]; 10], 44_100), None);
        assert_eq!(estimate_key(&[[1_234, -4_321]; 10], 44_100), None);
        // Ten minutes of digital black.
        assert_eq!(estimate_key(&vec![[0, 0]; 44_100 * 4], 44_100), None);
        // A rate of zero, and a rate so low the band has no octave in it.
        assert_eq!(estimate_key(&c_major_cadence(44_100), 0), None);
        assert_eq!(estimate_key(&vec![[100, 100]; 8_000], 100), None);
    }

    /// A track that is only nearly silent must not be normalized up into an
    /// opinion — but one real bar in an otherwise quiet file must still be
    /// heard, which is the same gate seen from the other side.
    #[test]
    fn a_quiet_bar_in_a_silent_file_is_what_gets_judged() {
        let mut frames = vec![[0i16, 0]; 44_100 * 12];
        let chord = c_major_cadence(44_100);
        for (index, frame) in chord.iter().enumerate().take(44_100 * 8) {
            frames[44_100 * 2 + index] = *frame;
        }
        let key = estimate_key(&frames, 44_100).expect("a key");
        assert_eq!(key.camelot(), "8B", "{key:?}");
    }

    #[test]
    fn a_sine_at_a440_lands_in_the_a_bin() {
        let tone = progression(44_100, 4.0, &[&[440.0]]);
        let chroma = track_chroma(&tone, 44_100).expect("a chroma");
        let mut best = 0usize;
        for class in 1..12 {
            if chroma[class] > chroma[best] {
                best = class;
            }
        }
        assert_eq!(best, 9, "{chroma:?}");
        // And it is not a near thing: a pure tone belongs to ONE class.
        let rest = chroma.iter().enumerate().filter(|(class, _)| *class != 9);
        let runner_up = rest.fold(0.0f64, |most, (_, value)| most.max(*value));
        assert!(chroma[9] > runner_up * 4.0, "{chroma:?}");
    }

    /// The octave a chord is voiced in is not part of its name.
    #[test]
    fn the_same_chord_an_octave_up_is_the_same_key() {
        let low = progression(44_100, 6.0, &[&major(0, 3)]);
        let high = progression(44_100, 6.0, &[&major(0, 5)]);
        let low = estimate_key(&low, 44_100).expect("a key");
        let high = estimate_key(&high, 44_100).expect("a key");
        assert_eq!(low.camelot(), high.camelot(), "{low:?} vs {high:?}");
    }

    /// The profiles themselves, guarded by what they mean. A table typed one
    /// degree out still looks like a plausible list of numbers, and what it
    /// silently does is move the fifth's weight onto the tritone — which
    /// reads every record as the key a third away and never once looks like
    /// a crash.
    #[test]
    fn the_profiles_rank_their_own_degrees() {
        for profile in [MAJOR_PROFILE, MINOR_PROFILE] {
            let strongest = (0..12).fold(0, |best: usize, degree| {
                if profile[degree] > profile[best] { degree } else { best }
            });
            assert_eq!(strongest, 0, "the tonic is not the strongest degree");
            assert!(profile[7] > profile[6], "the fifth is under the tritone");
            assert!(profile[7] > profile[8], "the fifth is under the sixth");
        }
        // The third is what the two modes disagree about, and nothing else.
        assert!(MAJOR_PROFILE[4] > MAJOR_PROFILE[3]);
        assert!(MINOR_PROFILE[3] > MINOR_PROFILE[4]);
        // The fifth is second only to the tonic in major; in minor the
        // third takes that place, which is the whole character of the mode.
        assert!(MAJOR_PROFILE[7] > MAJOR_PROFILE[3] && MAJOR_PROFILE[7] > MAJOR_PROFILE[4]);
        assert!(MINOR_PROFILE[3] > MINOR_PROFILE[7]);
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
    }

    /// The transform against the definition, on a signal with a known
    /// answer: a single bin's worth of cosine has to come back as a single
    /// bin.
    #[test]
    fn the_transform_transforms() {
        let size = 64usize;
        let transform = Fft::new(size);
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
        // A slice of the wrong length is refused rather than indexed.
        let mut short = vec![0.0f64; 3];
        let mut also = vec![0.0f64; 3];
        transform.forward(&mut short, &mut also);
        assert_eq!(short, vec![0.0; 3]);
    }
}
