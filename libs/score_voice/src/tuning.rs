//! Intonation measurement: a precise pitch tracker and a per-note report of
//! how a sung line sits against its score, in cents.
//!
//! The tracker is the difference function of the YIN recipe, normalised per
//! lag by the energy of the two windows (so a lag's dip reads 0 for a
//! perfectly periodic signal), searched within a range around the expected
//! pitch (no octave errors), refined with a parabola through the dip and
//! then a golden-section search on the fractional lag with the second window
//! read by cubic interpolation. On a steady harmonic tone it is accurate to
//! well under a cent from 60 Hz to 1 kHz.
//!
//! The report reads each note's frames: the centre (mean pitch over the
//! steady part, whole vibrato cycles), the drift inside it (the centre of
//! its last third against its first), the error on arrival (the first 60 ms
//! of the vowel) and when the pitch settles within 15 cents of the note.

use crate::pitch::hz_to_midi;

/// One tracked frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchFrame {
    /// Centre (s).
    pub t: f32,
    /// Hz (0 when no periodic dip was found).
    pub hz: f32,
    /// The normalised difference at the dip: 0 perfectly periodic, 1 noise.
    pub aperiodicity: f32,
}

/// `x` at `i + f` (`f` in 0..1), Catmull-Rom. The index stays whole: a
/// sample position as one f32 is only half a sample fine by three minutes.
fn cubic(x: &[f32], i: usize, f: f32) -> f32 {
    let g = |k: isize| x.get((i as isize + k).max(0) as usize).copied().unwrap_or(0.0);
    let (p0, p1, p2, p3) = (g(-1), g(0), g(1), g(2));
    p1 + 0.5 * f * (p2 - p0 + f * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + f * (3.0 * (p1 - p2) + p3 - p0)))
}

/// Normalised squared difference of the window at `s` and the window a
/// (fractional) lag later.
fn nsd(x: &[f32], s: usize, w: usize, tau: f32) -> f32 {
    let (whole, frac) = (tau.floor() as usize, tau - tau.floor());
    let (mut d, mut m) = (0.0f64, 0.0f64);
    for j in 0..w {
        let a = x[s + j];
        let b = cubic(x, s + j + whole, frac);
        d += ((a - b) * (a - b)) as f64;
        m += (a * a + b * b) as f64;
    }
    if m > 0.0 { (d / m) as f32 } else { 1.0 }
}

/// The pitch of `x` (after [`prepare`]) around sample `center`, searched between `fmin` and
/// `fmax` Hz. `None` when the window runs off the signal.
pub fn pitch_at(x: &[f32], rate: f32, center: usize, fmin: f32, fmax: f32) -> Option<(f32, f32)> {
    let tmin = ((rate / fmax).floor() as usize).max(2);
    let tmax = (rate / fmin).ceil() as usize + 1;
    // At least 25 ms and three of the longest periods.
    let w = ((0.025 * rate) as usize).max(3 * tmax);
    let s = center.checked_sub((w + tmax) / 2)?;
    if s + w + tmax + 3 > x.len() || s == 0 {
        return None;
    }
    let mut d = vec![1.0f32; tmax + 2];
    for tau in tmin - 1..=tmax + 1 {
        let (mut e, mut m) = (0.0f32, 0.0f32);
        for j in 0..w {
            let (a, b) = (x[s + j], x[s + j + tau]);
            e += (a - b) * (a - b);
            m += a * a + b * b;
        }
        d[tau] = if m > 1e-12 { e / m } else { 1.0 };
    }
    // The deepest dip: the range is narrow enough (a fifth either side)
    // that neither an octave nor a subharmonic is in it, and the first dip
    // under a threshold can be a high formant's near-period.
    let tau = (tmin..=tmax).min_by(|a, b| d[*a].total_cmp(&d[*b])).unwrap();
    if tau == tmin || tau == tmax {
        return Some((0.0, d[tau]));
    }
    // Parabola through the dip, then a golden-section refinement.
    let (a, b, c) = (d[tau - 1], d[tau], d[tau + 1]);
    let den = a - 2.0 * b + c;
    let p = if den > 0.0 { 0.5 * (a - c) / den } else { 0.0 };
    let mut lo = tau as f32 + p - 0.5;
    let mut hi = tau as f32 + p + 0.5;
    let g = 0.618_034f32;
    let mut x1 = hi - g * (hi - lo);
    let mut x2 = lo + g * (hi - lo);
    let mut f1 = nsd(x, s, w, x1);
    let mut f2 = nsd(x, s, w, x2);
    for _ in 0..24 {
        if f1 < f2 {
            hi = x2;
            x2 = x1;
            f2 = f1;
            x1 = hi - g * (hi - lo);
            f1 = nsd(x, s, w, x1);
        } else {
            lo = x1;
            x1 = x2;
            f1 = f2;
            x2 = lo + g * (hi - lo);
            f2 = nsd(x, s, w, x2);
        }
    }
    let period = 0.5 * (lo + hi);
    Some((rate / period, f1.min(f2)))
}

/// `x` low-passed at 4 kHz, forwards and backwards (no phase shift): what
/// [`pitch_at`] should read. The fractional-lag refinement interpolates the
/// signal, and interpolation is exact only at whole samples; with air and
/// breath up near Nyquist those whole lags win and the pitch reads in steps
/// of a fraction of a sample. Below 4 kHz the interpolation is exact enough.
pub fn prepare(x: &[f32], rate: f32) -> Vec<f32> {
    let mut y: Vec<f64> = x.iter().map(|v| *v as f64).collect();
    // Two passes of two second-order Butterworth sections each way.
    let w0 = std::f64::consts::TAU * 4000.0 / rate as f64;
    let alpha = w0.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
    let a0 = 1.0 + alpha;
    let (b0, b1, a1, a2) = ((1.0 - w0.cos()) / 2.0 / a0, (1.0 - w0.cos()) / a0, -2.0 * w0.cos() / a0, (1.0 - alpha) / a0);
    let pass = |y: &mut Vec<f64>| {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for v in y.iter_mut() {
            let out = b0 * *v + b1 * x1 + b0 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = *v;
            y2 = y1;
            y1 = out;
            *v = out;
        }
    };
    for _ in 0..2 {
        pass(&mut y);
        y.reverse();
        pass(&mut y);
        y.reverse();
    }
    y.iter().map(|v| *v as f32).collect()
}

/// Track `x` (after [`prepare`]) every `hop` seconds between `fmin` and
/// `fmax` Hz.
pub fn track(x: &[f32], rate: f32, hop: f32, fmin: f32, fmax: f32) -> Vec<PitchFrame> {
    let step = ((hop * rate) as usize).max(1);
    (0..x.len())
        .step_by(step)
        .map(|c| match pitch_at(x, rate, c, fmin, fmax) {
            Some((hz, ap)) => PitchFrame { t: c as f32 / rate, hz, aperiodicity: ap },
            None => PitchFrame { t: c as f32 / rate, hz: 0.0, aperiodicity: 1.0 },
        })
        .collect()
}

/// A note of the score, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetNote {
    /// When the vowel (the pitch) starts, and ends.
    pub start: f32,
    pub end: f32,
    pub note: f32,
}

/// How one note was sung (cents against the note; NaN when not measured).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteReport {
    pub target: TargetNote,
    /// The mean over the steady part (after the arrival, whole vibrato
    /// cycles when there is vibrato).
    pub centre: f32,
    /// The centre of the last third against the first third.
    pub drift: f32,
    /// Mean error over the first 60 ms of the note.
    pub arrival: f32,
    /// Seconds from the start until the pitch stays within 15 cents (+ the
    /// vibrato depth), NaN if it never does.
    pub settle: f32,
    /// Voiced frames measured.
    pub frames: usize,
}

/// Track and report every note. `vibrato` is the peak depth in cents the
/// voice may swing (widens the settle band); frames are every 5 ms.
pub fn report(x: &[f32], rate: f32, notes: &[TargetNote], vibrato_cents: f32) -> Vec<NoteReport> {
    let hop = 0.005;
    let x = &prepare(x, rate)[..];
    notes
        .iter()
        .map(|n| {
            let hz = crate::pitch::midi_to_hz(n.note);
            // Search within a fifth either side.
            let (fmin, fmax) = (hz / 1.5, hz * 1.5);
            let mut frames = Vec::new();
            let mut t = n.start;
            while t < n.end {
                if let Some((f, ap)) = pitch_at(x, rate, (t * rate) as usize, fmin, fmax) {
                    if f > 0.0 && ap < 0.3 {
                        frames.push((t - n.start, (hz_to_midi(f) - n.note) * 100.0));
                    }
                }
                t += hop;
            }
            note_report(*n, &frames, vibrato_cents)
        })
        .collect()
}

/// A note's report from its (seconds from start, cents) frames.
pub fn note_report(target: TargetNote, frames: &[(f32, f32)], vibrato_cents: f32) -> NoteReport {
    let len = target.end - target.start;
    let mean = |a: f32, b: f32| {
        let v: Vec<f32> = frames.iter().filter(|(t, _)| *t >= a && *t < b).map(|(_, c)| *c).collect();
        if v.is_empty() { f32::NAN } else { v.iter().sum::<f32>() / v.len() as f32 }
    };
    let arrival = mean(0.0, 0.06);
    // Steady: from 120 ms (or a third in) to 30 ms before the end.
    let a = 0.12f32.min(len / 3.0);
    let b = (len - 0.03).max(a + 0.01);
    let centre = mean(a, b);
    let third = (b - a) / 3.0;
    let drift = if b - a >= 0.3 { mean(b - third, b) - mean(a, a + third) } else { f32::NAN };
    let band = 15.0 + vibrato_cents;
    let settle = match frames.iter().rposition(|(_, c)| c.abs() > band) {
        None => frames.first().map_or(f32::NAN, |f| f.0),
        Some(i) if i + 1 < frames.len() => frames[i + 1].0,
        Some(_) => f32::NAN,
    };
    NoteReport { target, centre, drift, arrival, settle, frames: frames.len() }
}

/// A summary of a report: (mean centre, mean |centre|, worst |centre|,
/// mean |drift|, median settle s) over the notes measured.
pub fn summary(reports: &[NoteReport]) -> (f32, f32, f32, f32, f32) {
    let c: Vec<f32> = reports.iter().map(|r| r.centre).filter(|v| v.is_finite()).collect();
    let d: Vec<f32> = reports.iter().map(|r| r.drift.abs()).filter(|v| v.is_finite()).collect();
    let mut s: Vec<f32> = reports.iter().map(|r| r.settle).filter(|v| v.is_finite()).collect();
    s.sort_by(f32::total_cmp);
    let avg = |v: &[f32]| if v.is_empty() { f32::NAN } else { v.iter().sum::<f32>() / v.len() as f32 };
    let abs: Vec<f32> = c.iter().map(|v| v.abs()).collect();
    (avg(&c), avg(&abs), abs.iter().cloned().fold(0.0, f32::max), avg(&d), s.get(s.len() / 2).copied().unwrap_or(f32::NAN))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tracker_reads_a_harmonic_tone_to_half_a_cent() {
        let rate = 48_000.0;
        // Late in a long signal too (a song's third minute), where a sample
        // position as one f32 is only half a sample fine.
        for at in [0usize, 9_000_000] {
            for hz in [65.4f32, 110.0, 196.0, 261.63, 440.0, 523.25, 880.0, 987.8] {
                let tone: Vec<f32> = (0..9600)
                    .map(|i| {
                        let p = std::f64::consts::TAU * hz as f64 * i as f64 / rate as f64;
                        (1..20).map(|k| ((k as f64 * p + k as f64 * 0.7).sin() / k as f64) as f32).sum::<f32>() * 0.2
                    })
                    .collect();
                let x = [vec![0.0; at], prepare(&tone, rate)].concat();
                let (f, ap) = pitch_at(&x, rate, at + 4800, hz / 1.5, hz * 1.5).unwrap();
                let cents = (hz_to_midi(f) - hz_to_midi(hz)) * 100.0;
                assert!(cents.abs() < 0.5 && ap < 0.01, "{hz} Hz at {at} read {f} ({cents:+.3} cents, ap {ap})");
            }
        }
    }
}
