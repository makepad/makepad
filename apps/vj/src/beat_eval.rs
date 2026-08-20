//! Beat-tracking evaluation harness — test-only, no part of the app.
//!
//! `wave_analysis` claims a beat grid. This module is the independent judge of
//! that claim: it re-derives, from the audio alone and by a completely
//! different route, where the beats actually are, and then scores the grid
//! against them with the standard beat-tracking metrics from the literature
//! (F-measure, Cemgil, P-score, CMLc/CMLt/AMLc/AMLt — Davies, Degara &
//! Plumbley 2009, "Evaluation Methods for Musical Audio Beat Tracking
//! Algorithms").
//!
//! Independence is the whole point, so nothing here shares code with the
//! analysis:
//!
//! * the analysis uses one-pole 3-band RMS flux at a 10 ms hop; the judge uses
//!   an **STFT SuperFlux** onset detector (Böck & Widmer, DAFx 2013) — 2048
//!   window, 5 ms hop, 24-bands-per-octave log filterbank, log magnitude,
//!   maximum-filtered spectral trajectory — with the published adaptive peak
//!   picker on top;
//! * the analysis finds a tempo by autocorrelation + comb; the judge's
//!   reference tracker is **Ellis's 2007 dynamic-programming beat tracker**,
//!   which decodes a beat SEQUENCE (not a fixed grid) by maximizing onset
//!   strength plus a log-Gaussian tempo-consistency penalty;
//! * a third opinion comes from the **kick sequence** — a 1 ms-hop low-band
//!   transient detector. On four-to-the-floor material the kicks ARE the
//!   beats, which makes them ground truth rather than another estimate, and
//!   this library is house and techno.
//!
//! Plus a fourth, entirely outside the audio: the library's ID3 `TBPM` tags,
//! written by the DJ tooling that analysed it.
//!
//! Run it over the real library (nothing here is committed audio):
//!
//! ```text
//! VJ_BEAT_CORPUS=local/music cargo test -p makepad-vj --release \
//!     -- --nocapture beat_eval::corpus --ignored
//! ```

#![allow(dead_code)]

use super::{analyze, decode_audio_file, OnePole, TrackGrid};
use crate::mixer::TrackPcm;
use std::f64::consts::PI;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// fft
// ---------------------------------------------------------------------------

/// In-place iterative radix-2 complex FFT. `re`/`im` must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    // bit reversal
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2usize;
    while len <= n {
        let angle = -2.0 * PI / len as f64;
        let (wr, wi) = (angle.cos() as f32, angle.sin() as f32);
        let mut start = 0usize;
        while start < n {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (ar, ai) = (re[start + k], im[start + k]);
                let (br, bi) = (re[start + k + len / 2], im[start + k + len / 2]);
                let (tr, ti) = (br * cr - bi * ci, br * ci + bi * cr);
                re[start + k] = ar + tr;
                im[start + k] = ai + ti;
                re[start + k + len / 2] = ar - tr;
                im[start + k + len / 2] = ai - ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            start += len;
        }
        len <<= 1;
    }
}

// ---------------------------------------------------------------------------
// the independent onset front end: SuperFlux
// ---------------------------------------------------------------------------

/// Judge-side analysis hop, seconds. 5 ms is fine enough that the ±70 ms
/// tolerance of the standard metrics is not hop-limited, and fine enough that
/// the sub-hop bias of the grid is measurable at a quarter of a hop.
pub const JUDGE_HOP: f64 = 0.005;
/// 1024 at 44.1 kHz is a 23 ms window. A 2048 window measures the same
/// onsets but puts them 30 ms late and smears the flux peak over ten frames,
/// which is enough to make the reference tracker drift off a click track —
/// a judge has to be more precise than the thing it judges.
const FFT_SIZE: usize = 1024;

pub struct OnsetFront {
    /// SuperFlux novelty per frame.
    pub flux: Vec<f32>,
    /// Low-band (kick) novelty per frame, same frames.
    pub low_flux: Vec<f32>,
    pub hop_secs: f64,
}

fn mono(pcm: &TrackPcm) -> Vec<f32> {
    pcm.frames
        .iter()
        .map(|f| (f[0] as f32 + f[1] as f32) * 0.5 / 32768.0)
        .collect()
}

/// Triangular log-spaced filterbank edges, 24 bands per octave from `lo` to
/// `hi`, expressed as FFT bin indices.
fn filterbank(sample_rate: f64, lo: f64, hi: f64, per_octave: f64) -> Vec<(usize, usize, usize)> {
    let bin_hz = sample_rate / FFT_SIZE as f64;
    let mut centres = Vec::new();
    let mut f = lo;
    while f <= hi {
        centres.push(f);
        f *= 2f64.powf(1.0 / per_octave);
    }
    let mut out = Vec::new();
    for window in centres.windows(3) {
        let (a, b, c) = (window[0], window[1], window[2]);
        let (a, b, c) = (
            (a / bin_hz).round() as usize,
            (b / bin_hz).round() as usize,
            (c / bin_hz).round() as usize,
        );
        if b > a && c > b && c < FFT_SIZE / 2 {
            out.push((a, b, c));
        }
    }
    out
}

pub fn onset_front(pcm: &TrackPcm) -> OnsetFront {
    let rate = pcm.sample_rate.max(1) as f64;
    let hop = (rate * JUDGE_HOP).round().max(1.0) as usize;
    let samples = mono(pcm);
    let bank = filterbank(rate, 30.0, 17_000.0, 24.0);
    // Hann window.
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / FFT_SIZE as f64).cos()) as f32)
        .collect();
    let frames = if samples.len() > FFT_SIZE { (samples.len() - FFT_SIZE) / hop + 1 } else { 0 };
    let mut spec: Vec<Vec<f32>> = Vec::with_capacity(frames);
    let mut low: Vec<f32> = Vec::with_capacity(frames);
    let bin_hz = rate / FFT_SIZE as f64;
    let low_from = (35.0 / bin_hz).round() as usize;
    let low_to = ((130.0 / bin_hz).round() as usize).min(FFT_SIZE / 2 - 1);
    let mut re = vec![0.0f32; FFT_SIZE];
    let mut im = vec![0.0f32; FFT_SIZE];
    for frame in 0..frames {
        let start = frame * hop;
        for i in 0..FFT_SIZE {
            re[i] = samples[start + i] * window[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        let magnitude = |bin: usize| (re[bin] * re[bin] + im[bin] * im[bin]).sqrt();
        let mut bands = Vec::with_capacity(bank.len());
        for &(a, b, c) in &bank {
            let mut sum = 0.0f32;
            for bin in a..=c {
                let weight = if bin <= b {
                    if b == a { 1.0 } else { (bin - a) as f32 / (b - a) as f32 }
                } else if c == b {
                    1.0
                } else {
                    (c - bin) as f32 / (c - b) as f32
                };
                sum += weight * magnitude(bin);
            }
            // Logarithmic magnitude, as SuperFlux specifies.
            bands.push((1.0 + sum).ln());
        }
        let mut low_sum = 0.0f32;
        for bin in low_from..=low_to {
            low_sum += magnitude(bin);
        }
        low.push((1.0 + low_sum).ln());
        spec.push(bands);
    }

    // SuperFlux: positive difference against a frequency-maximum-filtered
    // frame mu frames back, so vibrato and slow glides do not read as onsets.
    let mu = ((0.020 / JUDGE_HOP).round() as usize).max(1);
    let bands = bank.len();
    let mut flux = vec![0.0f32; spec.len()];
    for frame in mu..spec.len() {
        let previous = &spec[frame - mu];
        let now = &spec[frame];
        let mut sum = 0.0f32;
        for band in 0..bands {
            let from = band.saturating_sub(1);
            let to = (band + 1).min(bands - 1);
            let mut maximum = previous[from];
            for b in from..=to {
                maximum = maximum.max(previous[b]);
            }
            sum += (now[band] - maximum).max(0.0);
        }
        flux[frame] = sum;
    }
    let mut low_flux = vec![0.0f32; low.len()];
    for frame in mu..low.len() {
        low_flux[frame] = (low[frame] - low[frame - mu]).max(0.0);
    }
    OnsetFront { flux, low_flux, hop_secs: hop as f64 / rate }
}

/// The published adaptive peak picker (Böck, Krebs & Schedl, ISMIR 2012):
/// a frame is an onset when it is the maximum of a short window around it,
/// exceeds a local mean by `delta` (expressed in units of the novelty's own
/// standard deviation), and is at least `min_gap` after the last one.
pub fn pick_peaks(novelty: &[f32], hop: f64, delta: f32, min_gap: f64) -> Vec<f64> {
    pick_peaks_strength(novelty, hop, delta, min_gap)
        .into_iter()
        .map(|(at, _)| at)
        .collect()
}

/// The peaks with their novelty, which is what the support objective weights
/// by: an onset the whole mix agrees on should count for more than a tick.
pub fn pick_peaks_strength(
    novelty: &[f32],
    hop: f64,
    delta: f32,
    min_gap: f64,
) -> Vec<(f64, f32)> {
    if novelty.is_empty() {
        return Vec::new();
    }
    let pre_max = ((0.030 / hop).round() as usize).max(1);
    let post_max = ((0.030 / hop).round() as usize).max(1);
    let pre_avg = ((0.100 / hop).round() as usize).max(1);
    let post_avg = ((0.070 / hop).round() as usize).max(1);
    let mean = novelty.iter().map(|v| *v as f64).sum::<f64>() / novelty.len() as f64;
    let variance =
        novelty.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / novelty.len() as f64;
    let threshold = delta as f64 * variance.sqrt();
    let gap = (min_gap / hop).round() as usize;
    let mut out = Vec::new();
    let mut last = 0usize;
    let mut first = true;
    for index in 0..novelty.len() {
        let value = novelty[index] as f64;
        if value <= 0.0 {
            continue;
        }
        let from = index.saturating_sub(pre_max);
        let to = (index + post_max).min(novelty.len() - 1);
        if (from..=to).any(|i| novelty[i] as f64 > value) {
            continue;
        }
        let from = index.saturating_sub(pre_avg);
        let to = (index + post_avg).min(novelty.len() - 1);
        let local: f64 =
            (from..=to).map(|i| novelty[i] as f64).sum::<f64>() / (to - from + 1) as f64;
        if value < local + threshold {
            continue;
        }
        if !first && index < last + gap {
            continue;
        }
        out.push((index as f64 * hop, novelty[index]));
        last = index;
        first = false;
    }
    out
}

// ---------------------------------------------------------------------------
// fine-grained transient positions, for bias and jitter
// ---------------------------------------------------------------------------

/// A 1 ms-hop BROADBAND transient envelope, the timing authority.
///
/// The STFT front end decides WHICH onsets exist — that is the part that
/// wants a spectral view and a published detector. It is not allowed to
/// decide WHEN they happened: a windowed transform cannot resolve a position
/// inside its own window, and the measured error is tens of milliseconds,
/// which is the same size as the effects under study. So every time the judge
/// produces — detected onsets, decoded beats — is snapped onto the nearest
/// peak of this, and every alignment number is measured against it.
pub fn fine_onset_envelope(pcm: &TrackPcm) -> (Vec<f32>, f64) {
    let rate = pcm.sample_rate.max(1) as f64;
    let hop = (rate * 0.001).round().max(1.0) as usize;
    let mut low = OnePole::new(200.0, rate as f32);
    let mut mid = OnePole::new(2_000.0, rate as f32);
    let mut energy: Vec<[f32; 3]> = Vec::with_capacity(pcm.frames.len() / hop + 1);
    let mut sums = [0.0f64; 3];
    let mut in_hop = 0usize;
    for frame in &pcm.frames {
        let value = (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0;
        let low_band = low.process(value);
        let mid_band = mid.process(value) - low_band;
        let high_band = value - low.state - mid_band;
        for (sum, band) in sums.iter_mut().zip([low_band, mid_band, high_band]) {
            *sum += (band as f64) * (band as f64);
        }
        in_hop += 1;
        if in_hop == hop {
            let inverse = 1.0 / in_hop as f64;
            energy.push([
                (sums[0] * inverse).sqrt() as f32,
                (sums[1] * inverse).sqrt() as f32,
                (sums[2] * inverse).sqrt() as f32,
            ]);
            sums = [0.0; 3];
            in_hop = 0;
        }
    }
    let look = 8usize;
    let mut onset = vec![0.0f32; energy.len()];
    for index in look..energy.len() {
        let mut sum = 0.0f32;
        for band in 0..3 {
            let now = (1.0 + 96.0 * energy[index][band]).ln();
            let before = (1.0 + 96.0 * energy[index - look][band]).ln();
            sum += (now - before).max(0.0);
        }
        onset[index] = sum;
    }
    (onset, hop as f64 / rate)
}

/// Move each time onto the strongest peak of `fine` within `radius`, keeping
/// the times that have no peak near them where they are.
pub fn snap_to_fine(times: &[f64], fine: &[f32], hop: f64, radius: f64) -> Vec<f64> {
    let reach = (radius / hop).round().max(1.0) as usize;
    times
        .iter()
        .map(|at| {
            let centre = (at / hop).round().max(0.0) as usize;
            if fine.is_empty() {
                return *at;
            }
            let from = centre.saturating_sub(reach);
            let to = (centre + reach).min(fine.len() - 1);
            if from >= to {
                return *at;
            }
            let mut best = from;
            for index in from..=to {
                if fine[index] > fine[best] {
                    best = index;
                }
            }
            if fine[best] > 0.0 {
                best as f64 * hop
            } else {
                *at
            }
        })
        .collect()
}

/// A 1 ms-hop low-band transient envelope: the kick alone, which is what the
/// beat sits on in this genre.
pub fn fine_low_envelope(pcm: &TrackPcm) -> (Vec<f32>, f64) {
    let rate = pcm.sample_rate.max(1) as f64;
    let hop = (rate * 0.001).round().max(1.0) as usize;
    let mut state = 0.0f32;
    let cutoff = 160.0f32;
    let alpha = 1.0 - (-2.0 * std::f32::consts::PI * cutoff / rate as f32).exp();
    let mut energy: Vec<f32> = Vec::with_capacity(pcm.frames.len() / hop + 1);
    let mut sum = 0.0f64;
    let mut in_hop = 0usize;
    for frame in &pcm.frames {
        let value = (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0;
        state += alpha * (value - state);
        sum += (state as f64) * (state as f64);
        in_hop += 1;
        if in_hop == hop {
            energy.push(((sum / in_hop as f64).sqrt()) as f32);
            sum = 0.0;
            in_hop = 0;
        }
    }
    // 10 ms look-back so a 1 ms hop still sees a whole attack.
    let look = 10usize;
    let mut onset = vec![0.0f32; energy.len()];
    for index in look..energy.len() {
        let now = (1.0 + 96.0 * energy[index]).ln();
        let before = (1.0 + 96.0 * energy[index - look]).ln();
        onset[index] = (now - before).max(0.0);
    }
    (onset, hop as f64 / rate)
}

// ---------------------------------------------------------------------------
// standard beat-tracking metrics
// ---------------------------------------------------------------------------

/// Greedy one-to-one matching inside `tolerance`, then F = 2TP/(2TP+FP+FN).
/// The MIREX default tolerance is 70 ms.
pub fn f_measure(detections: &[f64], reference: &[f64], tolerance: f64) -> (f64, f64, f64) {
    if detections.is_empty() || reference.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut used = vec![false; reference.len()];
    let mut hits = 0usize;
    for detection in detections {
        let mut best: Option<(usize, f64)> = None;
        for (index, annotation) in reference.iter().enumerate() {
            if used[index] {
                continue;
            }
            let distance = (annotation - detection).abs();
            if distance <= tolerance && best.map_or(true, |(_, d)| distance < d) {
                best = Some((index, distance));
            }
        }
        if let Some((index, _)) = best {
            used[index] = true;
            hits += 1;
        }
    }
    let precision = hits as f64 / detections.len() as f64;
    let recall = hits as f64 / reference.len() as f64;
    let f = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    (f, precision, recall)
}

/// Cemgil accuracy: a Gaussian error window (sigma 40 ms) rather than a hard
/// tolerance, so a grid that is merely close scores less than one that is on.
pub fn cemgil(detections: &[f64], reference: &[f64]) -> f64 {
    if detections.is_empty() || reference.is_empty() {
        return 0.0;
    }
    let sigma = 0.040f64;
    let mut sum = 0.0;
    for annotation in reference {
        let nearest = detections
            .iter()
            .map(|d| (d - annotation).abs())
            .fold(f64::INFINITY, f64::min);
        sum += (-nearest * nearest / (2.0 * sigma * sigma)).exp();
    }
    sum / (0.5 * (detections.len() + reference.len()) as f64)
}

/// Continuity-based accuracy at one metrical interpretation, as Davies,
/// Degara & Plumbley define it: an annotation counts as correctly tracked
/// when exactly ONE detection falls inside its tolerance window, exactly one
/// falls inside the previous annotation's window, and the interval between
/// those two detections matches the annotated interval to within the same
/// tolerance. Requiring exactly one is what makes double time fail.
///
/// Returns `(continuity, total)` — the longest correctly-tracked run as a
/// fraction of the reference, and the total correct fraction.
fn continuity_at(detections: &[f64], reference: &[f64], theta: f64) -> (f64, f64) {
    if detections.len() < 2 || reference.len() < 2 {
        return (0.0, 0.0);
    }
    let nearest = |at: f64| -> usize {
        let mut best = 0usize;
        let mut distance = (detections[0] - at).abs();
        for (index, detection) in detections.iter().enumerate().skip(1) {
            let candidate = (detection - at).abs();
            if candidate < distance {
                distance = candidate;
                best = index;
            }
        }
        best
    };
    let mut correct = vec![false; reference.len()];
    for index in 1..reference.len() {
        let interval = reference[index] - reference[index - 1];
        if interval <= 0.0 {
            continue;
        }
        let tolerance = theta * interval;
        let here = nearest(reference[index]);
        if here == 0 {
            continue;
        }
        // Phase: the nearest detection is inside the tolerance.
        if (detections[here] - reference[index]).abs() >= tolerance {
            continue;
        }
        // Period: the detector's OWN step into that beat matches the
        // annotated step. This is what rejects a double-time reading, whose
        // phase is perfect and whose step is half.
        let step = detections[here] - detections[here - 1];
        if (step - interval).abs() < tolerance {
            correct[index] = true;
        }
    }
    let total = correct.iter().filter(|c| **c).count() as f64 / reference.len() as f64;
    let mut longest = 0usize;
    let mut run = 0usize;
    for c in &correct {
        if *c {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    (longest as f64 / reference.len() as f64, total)
}

/// Metrical variations of a reference beat sequence: double time, half time
/// (both phases), triple and third. AML scores against the best of these.
fn metrical_variations(reference: &[f64]) -> Vec<Vec<f64>> {
    let mut out = vec![reference.to_vec()];
    if reference.len() >= 2 {
        // Double time: interpolate a beat between each pair.
        let mut double = Vec::with_capacity(reference.len() * 2);
        for index in 0..reference.len() - 1 {
            double.push(reference[index]);
            double.push(0.5 * (reference[index] + reference[index + 1]));
        }
        double.push(reference[reference.len() - 1]);
        out.push(double);
        // Half time, both phases.
        for offset in 0..2usize {
            let half: Vec<f64> =
                reference.iter().skip(offset).step_by(2).copied().collect();
            if half.len() >= 2 {
                out.push(half);
            }
        }
        // Triple and third.
        let mut triple = Vec::new();
        for index in 0..reference.len() - 1 {
            let (a, b) = (reference[index], reference[index + 1]);
            triple.push(a);
            triple.push(a + (b - a) / 3.0);
            triple.push(a + 2.0 * (b - a) / 3.0);
        }
        triple.push(reference[reference.len() - 1]);
        out.push(triple);
        for offset in 0..3usize {
            let third: Vec<f64> =
                reference.iter().skip(offset).step_by(3).copied().collect();
            if third.len() >= 2 {
                out.push(third);
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Continuity {
    pub cmlc: f64,
    pub cmlt: f64,
    pub amlc: f64,
    pub amlt: f64,
}

pub fn continuity(detections: &[f64], reference: &[f64]) -> Continuity {
    let theta = 0.175;
    let (cmlc, cmlt) = continuity_at(detections, reference, theta);
    let mut amlc = cmlc;
    let mut amlt = cmlt;
    for variation in metrical_variations(reference) {
        let (c, t) = continuity_at(detections, &variation, theta);
        amlc = amlc.max(c);
        amlt = amlt.max(t);
    }
    Continuity { cmlc, cmlt, amlc, amlt }
}

/// McKinney/Goto P-score: impulse-train cross-correlation inside ±20 % of a
/// beat, normalized by the larger of the two counts.
pub fn p_score(detections: &[f64], reference: &[f64]) -> f64 {
    if detections.len() < 2 || reference.len() < 2 {
        return 0.0;
    }
    let mean_interval = (reference[reference.len() - 1] - reference[0])
        / (reference.len() - 1) as f64;
    let tolerance = 0.2 * mean_interval;
    let mut used = vec![false; reference.len()];
    let mut hits = 0usize;
    for detection in detections {
        for (index, annotation) in reference.iter().enumerate() {
            if !used[index] && (annotation - detection).abs() <= tolerance {
                used[index] = true;
                hits += 1;
                break;
            }
        }
    }
    hits as f64 / detections.len().max(reference.len()) as f64
}

// ---------------------------------------------------------------------------
// reference tracker: Ellis 2007 dynamic programming
// ---------------------------------------------------------------------------

/// Ellis, "Beat Tracking by Dynamic Programming" (J. New Music Research 2007).
///
/// Given an onset strength envelope and a target period, the beat sequence
/// maximizes `sum(onset) + alpha * sum(log-Gaussian tempo penalty)`. It is a
/// SEQUENCE, not a fixed grid: it can follow a tempo that moves, which is
/// exactly the axis on which a fixed grid can be wrong.
/// Weight of the tempo-consistency penalty against onset strength. Ellis
/// calls it "tightness"; his default is 6 over a differently scaled envelope.
///
/// A judge whose own setting decides the verdict is not a judge, so this one
/// is pinned from both sides by measurement rather than picked.
///
/// From below: a loose tracker does not track, it CHASES. Over twenty house
/// records, raising this from 100 shrank the spread of the reference's own
/// beat intervals from 4.3 % to 3.0 % and its onset support from 1.59× the
/// best fixed grid to 1.16× — a free tracker scoring half again what any
/// fixed grid can is not finding beats a fixed grid misses, it is putting
/// beats on every bass note it passes. Every tracker under test scored
/// higher against the tighter reference, which is what it looks like when
/// the noise comes out of a measurement.
///
/// From above: `the_reference_follows_a_tempo_ramp` still passes at 6400, so
/// this is a factor of four below the point where the reference would stop
/// being able to see a tempo move — which is the one thing it exists to do
/// that a fixed grid cannot.
pub const DP_TIGHTNESS: f64 = 1600.0;

fn dp_tightness() -> f64 {
    DP_TIGHTNESS
}

/// The activation the dynamic program is decoded over: a triangular spike at
/// every detected onset, `strength` tall and `half_width` wide.
///
/// Handing the DP a raw windowed flux does not work, and it is worth saying
/// why, because it is the same trap the analysis avoids by refitting against
/// onsets. A 23 ms window smears a transient over several frames, so moving
/// a beat by one frame costs almost no onset strength — less than the tempo
/// penalty for the same move — and the tracker settles on whatever integer
/// frame period its tempo estimate rounded to and walks off the music at a
/// steady few milliseconds a beat. Measured on a click track it drifted 70 ms
/// in thirty seconds. Spikes restore the gradient: a frame off the onset now
/// costs real strength, so the onsets pull the tempo instead of the other way
/// round.
pub fn impulse_activation(
    onsets: &[(f64, f32)],
    hop: f64,
    seconds: f64,
    half_width: f64,
) -> Vec<f32> {
    let mut out = vec![0.0f32; (seconds / hop) as usize + 2];
    let reach = (half_width / hop).round().max(1.0) as isize;
    for (time, strength) in onsets {
        let centre = (time / hop).round() as isize;
        for offset in -reach..=reach {
            let index = centre + offset;
            if index < 0 || index as usize >= out.len() {
                continue;
            }
            let taper = 1.0 - (offset.abs() as f32 / (reach + 1) as f32);
            out[index as usize] = out[index as usize].max(strength * taper);
        }
    }
    out
}

pub fn ellis_dp(novelty: &[f32], hop: f64, period_secs: f64, alpha: f64) -> Vec<f64> {
    let period = period_secs / hop;
    if period < 2.0 || novelty.len() < 8 {
        return Vec::new();
    }
    // Normalize the envelope so alpha means the same thing on every track.
    let mean = novelty.iter().map(|v| *v as f64).sum::<f64>() / novelty.len() as f64;
    let variance =
        novelty.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / novelty.len() as f64;
    let scale = if variance > 1e-12 { 1.0 / variance.sqrt() } else { 1.0 };
    let strength: Vec<f64> = novelty.iter().map(|v| (*v as f64 - mean) * scale).collect();

    let search_from = (period * 0.5).round() as usize;
    let search_to = (period * 2.0).round() as usize;
    // txcost[i] for a candidate predecessor at lag i.
    let cost: Vec<f64> = (search_from..=search_to)
        .map(|lag| -alpha * ((lag as f64 / period).ln()).powi(2))
        .collect();

    let mut score = vec![f64::NEG_INFINITY; strength.len()];
    let mut back = vec![usize::MAX; strength.len()];
    for index in 0..strength.len() {
        if index < search_from {
            score[index] = strength[index];
            continue;
        }
        let mut best = f64::NEG_INFINITY;
        let mut best_at = usize::MAX;
        let to = search_to.min(index);
        for lag in search_from..=to {
            let candidate = score[index - lag] + cost[lag - search_from];
            if candidate > best {
                best = candidate;
                best_at = index - lag;
            }
        }
        if best_at == usize::MAX {
            score[index] = strength[index];
        } else {
            score[index] = strength[index] + best;
            back[index] = best_at;
        }
    }
    // Start the backtrace at the best score in the last stretch, then walk.
    let tail_from = strength.len().saturating_sub(search_to);
    let mut at = tail_from;
    for index in tail_from..strength.len() {
        if score[index] > score[at] {
            at = index;
        }
    }
    let mut beats = Vec::new();
    while at != usize::MAX {
        beats.push(at as f64 * hop);
        let next = back[at];
        if next == usize::MAX || next >= at {
            break;
        }
        at = next;
    }
    beats.reverse();
    // The head of the chain is a seed, not a decision: the first states have
    // no predecessor, so nothing holds them to the tempo and they land on
    // whatever the envelope is doing at the start of the file. Measured on a
    // click track the whole sequence sits within six milliseconds of its
    // clicks except the first beats, which sit tens of milliseconds out. So
    // the head is trimmed back to where the intervals become regular — which
    // is exactly the point where the tempo model started holding — rather
    // than by some fixed count.
    if beats.len() > 12 {
        let mut steps: Vec<f64> = beats.windows(2).map(|pair| pair[1] - pair[0]).collect();
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = steps[steps.len() / 2];
        let mut drop = 0usize;
        while drop < 4 && ((beats[drop + 1] - beats[drop]) / median - 1.0).abs() > 0.05 {
            drop += 1;
        }
        beats.drain(..drop);
    }
    beats
}

/// Global tempo estimate for the reference tracker, by its own route:
/// autocorrelation of the SuperFlux envelope with the Ellis/Klapuri
/// log-Gaussian tempo prior (centre 120 BPM, one octave of width), summed
/// over the first three harmonics.
pub fn reference_tempo(novelty: &[f32], hop: f64, min_bpm: f64, max_bpm: f64) -> f64 {
    let mean = novelty.iter().map(|v| *v as f64).sum::<f64>() / novelty.len().max(1) as f64;
    let centred: Vec<f64> = novelty.iter().map(|v| *v as f64 - mean).collect();
    let min_lag = (60.0 / (max_bpm * hop)).floor().max(2.0) as usize;
    let max_lag = ((60.0 / (min_bpm * hop)).ceil() as usize).min(centred.len() / 3);
    if max_lag <= min_lag {
        return 0.0;
    }
    let autocorrelation = |lag: usize| -> f64 {
        if lag == 0 || lag >= centred.len() {
            return 0.0;
        }
        let mut dot = 0.0;
        for index in lag..centred.len() {
            dot += centred[index] * centred[index - lag];
        }
        dot / (centred.len() - lag) as f64
    };
    let scored = |lag: usize| -> f64 {
        let bpm = 60.0 / (lag as f64 * hop);
        // Ellis's prior: Gaussian on log tempo, centre 120, sigma 1.0 octave.
        let prior = (-0.5 * ((bpm / 120.0).ln() / 2f64.ln()).powi(2)).exp();
        (autocorrelation(lag) + 0.5 * autocorrelation(lag * 2) + 0.25 * autocorrelation(lag * 3))
            * prior
    };
    let mut best = (min_lag, f64::NEG_INFINITY);
    for lag in min_lag..=max_lag {
        let score = scored(lag);
        if score > best.1 {
            best = (lag, score);
        }
    }
    // Sub-lag refinement. At a 5 ms hop one lag step is a third of a BPM near
    // 128, and a third of a BPM walks a five-minute track a quarter of a
    // second — enough to make the reference tracker drift off a click.
    let mut lag = best.0 as f64;
    if best.0 > min_lag && best.0 < max_lag {
        let (left, centre, right) = (scored(best.0 - 1), best.1, scored(best.0 + 1));
        let denominator = left - 2.0 * centre + right;
        if denominator.abs() > 1e-12 {
            let shift = 0.5 * (left - right) / denominator;
            if shift.abs() < 1.0 {
                lag = best.0 as f64 + shift;
            }
        }
    }
    60.0 / (lag * hop)
}

// ---------------------------------------------------------------------------
// per-track evaluation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct TrackReport {
    pub name: String,
    pub seconds: f64,
    pub tagged_bpm: Option<f64>,
    pub grid: TrackGrid,
    /// Kick-chain tempo and how much of the track the chain covers.
    pub kick_bpm: f64,
    pub kick_coverage: f64,
    pub kick_beats: usize,
    /// The independent DP tracker's tempo.
    pub reference_bpm: f64,
    /// Our grid against the kicks, trimmed to where the chain exists.
    pub vs_kicks: Scores,
    /// Our grid against the independent DP tracker's beats.
    pub vs_reference: Scores,
    /// The DP tracker against the kicks — the yardstick for `vs_kicks`.
    pub reference_vs_kicks: Scores,
    /// Onset support: ours, the DP tracker's, and the exhaustive best fixed
    /// grid's. `ours / oracle` is the headline — how much of the fixed-grid
    /// ceiling the analysis reaches.
    pub support_ours: f64,
    pub support_reference: f64,
    pub support_oracle: f64,
    pub oracle_bpm: f64,
    /// The oracle grid judged the same way ours is. If the oracle also scores
    /// better here, the support gap is real and not an artefact of the
    /// objective.
    pub oracle_vs_kicks: Scores,
    pub oracle_bias_ms: f64,
    pub oracle_jitter_ms: f64,
    /// Median offset of the kicks from our grid, in fractions of a beat:
    /// ±0.5 is an offbeat lock, near zero is agreement.
    pub kick_phase: f64,
    pub reference_phase: f64,
    /// Our grid with that median offset taken out, scored again. The gap
    /// between this and `vs_kicks` is the part of the error that is PHASE —
    /// a grid on the wrong pulse of the bar — and what remains is the part
    /// that is PERIOD, a grid that walks.
    pub vs_kicks_rephased: Scores,
    /// Signed offset of the grid from the fine low-band transients, ms.
    pub bias_ms: f64,
    pub jitter_ms: f64,
    /// The same for the reference tracker, so its trustworthiness is measured
    /// rather than assumed.
    pub reference_bias_ms: f64,
    pub reference_jitter_ms: f64,
    /// Interquartile spread of the reference's own beat intervals, as a
    /// fraction of their median. This music is machine-made and its tempo
    /// does not move, so a reference that wobbles is chasing onsets rather
    /// than tracking beats — the one failure mode a free tracker has that a
    /// fixed grid cannot have, and the one that would make it a bad judge.
    pub reference_ibi_spread: f64,
    /// Downbeat margin: how much more low-band onset the chosen bar phase
    /// carries than the runner-up, as a ratio.
    pub downbeat_margin: f64,
    /// Fraction of the downbeats a grid fitted to the first (and second) half
    /// of the track calls that the whole-track grid also calls downbeats.
    pub downbeat_halves: (f64, f64),
    /// Fraction of the track's structural boundaries that land on bar
    /// position zero under our grid, and how many were counted. Chance is
    /// 0.25.
    pub downbeat_structure: (f64, usize),
    /// Grid drift: how far apart the whole-track grid and a grid fitted to
    /// the first half are, at the end of the track, in ms.
    pub drift_ms: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Scores {
    pub f: f64,
    pub precision: f64,
    pub recall: f64,
    pub cemgil: f64,
    pub p_score: f64,
    pub continuity: Continuity,
}

fn score(detections: &[f64], reference: &[f64]) -> Scores {
    let (f, precision, recall) = f_measure(detections, reference, 0.070);
    Scores {
        f,
        precision,
        recall,
        cemgil: cemgil(detections, reference),
        p_score: p_score(detections, reference),
        continuity: continuity(detections, reference),
    }
}

/// Beat times a `TrackGrid` predicts across `seconds`.
pub fn grid_beats(grid: &TrackGrid, seconds: f64) -> Vec<f64> {
    if !grid.has_grid() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut beat = grid.beat_at(0.0).ceil() as i64;
    loop {
        let at = grid.secs_at_beat(beat as f64);
        if at > seconds {
            break;
        }
        if at >= 0.0 {
            out.push(at);
        }
        beat += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// the kick reference: ground truth for four-to-the-floor material
// ---------------------------------------------------------------------------

/// A beat reference built from the kick drum alone.
///
/// House and techno put a kick on every beat, so on this library the kick
/// times ARE the beat times — no algorithm decided them, a transient detector
/// found them. That makes this the closest thing to a human annotation
/// available without one, and it is the number the verdict rests on.
///
/// The chain is built only from onsets that were actually detected, so where
/// the track has no kick (a breakdown) it has no reference either; `spans`
/// records where it does, and scoring is trimmed to those.
pub struct KickReference {
    /// Kick transients that form an isochronous chain — the reference beats.
    pub beats: Vec<f64>,
    /// Every low-band onset with its strength, beat or not.
    pub onsets: Vec<(f64, f32)>,
    pub bpm: f64,
    /// Chained beats as a fraction of the beats the chain's span should hold.
    pub coverage: f64,
    /// Stretches with a continuous chain, as `(from, to)` seconds.
    pub spans: Vec<(f64, f64)>,
}

/// Two cascaded one-poles at 110 Hz: steep enough that a bass line an octave
/// up does not read as a kick, which a single pole at 160 Hz very much does.
pub fn kick_envelope(pcm: &TrackPcm) -> (Vec<f32>, f64) {
    let rate = pcm.sample_rate.max(1) as f64;
    let hop = (rate * 0.001).round().max(1.0) as usize;
    let alpha = 1.0 - (-2.0 * std::f32::consts::PI * 110.0 / rate as f32).exp();
    let (mut a, mut b) = (0.0f32, 0.0f32);
    let mut energy: Vec<f32> = Vec::with_capacity(pcm.frames.len() / hop + 1);
    let mut sum = 0.0f64;
    let mut in_hop = 0usize;
    for frame in &pcm.frames {
        let value = (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0;
        a += alpha * (value - a);
        b += alpha * (a - b);
        sum += (b as f64) * (b as f64);
        in_hop += 1;
        if in_hop == hop {
            energy.push(((sum / in_hop as f64).sqrt()) as f32);
            sum = 0.0;
            in_hop = 0;
        }
    }
    let look = 12usize;
    let mut onset = vec![0.0f32; energy.len()];
    for index in look..energy.len() {
        let now = (1.0 + 200.0 * energy[index]).ln();
        let before = (1.0 + 200.0 * energy[index - look]).ln();
        onset[index] = (now - before).max(0.0);
    }
    (onset, hop as f64 / rate)
}

/// Modal inter-onset interval inside the beat band, by a sliding 3 % window
/// over the sorted intervals. No octave folding: a bass note between kicks
/// makes a short interval, and a short interval should be discarded, not
/// doubled into a vote for the beat.
fn modal_interval(onsets: &[f64], min_bpm: f64, max_bpm: f64) -> Option<f64> {
    let mut intervals: Vec<f64> = onsets
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|interval| {
            *interval > 0.0 && 60.0 / interval >= min_bpm && 60.0 / interval <= max_bpm
        })
        .collect();
    if intervals.len() < 8 {
        return None;
    }
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut best = (intervals[0], 0usize);
    let mut start = 0usize;
    for end in 0..intervals.len() {
        while intervals[end] > intervals[start] * 1.03 {
            start += 1;
        }
        if end - start + 1 > best.1 {
            let inside = &intervals[start..=end];
            best = (inside.iter().sum::<f64>() / inside.len() as f64, end - start + 1);
        }
    }
    (best.1 >= 8).then_some(best.0)
}

/// The reference beat sequence.
///
/// Hand-rolled chaining over picked kick peaks turned out to be exactly as
/// fragile as it sounds — the modal inter-onset interval lands on a bassline
/// as happily as on the kick. So the reference is instead a PUBLISHED tracker
/// run on the cleanest signal a house record has: Ellis's dynamic program
/// over the low-band transient envelope, at its own tempo, deciding its own
/// beat times. Nothing about it comes from `wave_analysis`, and unlike a
/// fixed grid it is free to follow a tempo that moves — which is the whole
/// axis on which a fixed grid could be losing.
///
/// The kick envelope is not a subtle signal: on this library the reference's
/// own alignment to the transients (`bias`, `jitter`) is reported beside the
/// scores so its trustworthiness is visible rather than assumed.
pub fn kick_reference(pcm: &TrackPcm, front: &OnsetFront) -> KickReference {
    let (fine, fine_hop) = kick_envelope(pcm);
    // Which kicks exist is the STFT's call — a time-domain low-pass, however
    // steep, fires two and three times on one kick as its pitch envelope
    // sweeps, and a detector that reports 2.5 kicks per beat cannot be the
    // ground truth for where the beat is. When it happened is still the fine
    // envelope's call. A quarter-second refractory: the beat band tops out at
    // 180 BPM, so nothing real is closer than a third of a second.
    let detected = pick_peaks_strength(&front.low_flux, front.hop_secs, 1.0, 0.250);
    let times: Vec<f64> = detected.iter().map(|(at, _)| *at).collect();
    let mut onsets: Vec<(f64, f32)> = snap_to_fine(&times, &fine, fine_hop, 0.035)
        .into_iter()
        .zip(detected.iter().map(|(_, strength)| *strength))
        .collect();
    onsets.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let novelty = impulse_activation(&onsets, JUDGE_HOP, pcm.seconds(), 0.015);
    let bpm = reference_tempo(&novelty, JUDGE_HOP, 70.0, 180.0);
    if !(60.0..=200.0).contains(&bpm) {
        return KickReference {
            beats: Vec::new(),
            onsets,
            bpm: 0.0,
            coverage: 0.0,
            spans: Vec::new(),
        };
    }
    let coarse = ellis_dp(&novelty, JUDGE_HOP, 60.0 / bpm, dp_tightness());
    // Snap each decoded beat onto the 1 ms transient under it, so the
    // reference carries the tracker's decision but the envelope's precision.
    let beats = snap_to_fine(&coarse, &fine, fine_hop, 0.025);
    // A beat is "covered" when a real kick transient sits under it; the
    // spans are the stretches where that holds for eight beats running, and
    // scoring against the reference happens only there. Elsewhere the
    // reference is the tracker's interpolation, not evidence.
    let period = 60.0 / bpm;
    let supported: Vec<bool> = beats
        .iter()
        .map(|beat| onsets.iter().any(|(onset, _)| (onset - beat).abs() < 0.070))
        .collect();
    // Demanding a run of consecutive supported beats is too brittle: one
    // beat whose kick the detector missed cuts a whole minute of good
    // reference in half. A beat is in a valid span when most of the sixteen
    // beats around it — four bars, the unit this music is built in — have a
    // kick under them.
    let mut dense = vec![false; beats.len()];
    for index in 0..beats.len() {
        let from = index.saturating_sub(8);
        let to = (index + 8).min(beats.len() - 1);
        let count = (from..=to).filter(|i| supported[*i]).count();
        dense[index] = count as f64 >= 0.7 * (to - from + 1) as f64;
    }
    let mut spans: Vec<(f64, f64)> = Vec::new();
    let mut run: Option<usize> = None;
    for index in 0..beats.len() {
        if dense[index] {
            run.get_or_insert(index);
        } else if let Some(start) = run.take() {
            if index - start >= 8 {
                spans.push((beats[start] - 0.01, beats[index - 1] + 0.01));
            }
        }
    }
    if let Some(start) = run {
        if beats.len() - start >= 8 {
            spans.push((beats[start] - 0.01, beats[beats.len() - 1] + 0.01));
        }
    }
    let covered: f64 = spans.iter().map(|(from, to)| to - from).sum();
    let coverage = if pcm.seconds() > 0.0 { covered / pcm.seconds() } else { 0.0 };
    let _ = period;
    KickReference { beats, onsets, bpm, coverage, spans }
}

/// Keep only the times inside one of `spans`.
pub fn trim_to_spans(times: &[f64], spans: &[(f64, f64)]) -> Vec<f64> {
    times
        .iter()
        .copied()
        .filter(|at| spans.iter().any(|(from, to)| at >= from && at <= to))
        .collect()
}

// ---------------------------------------------------------------------------
// the oracle: the best fixed grid there is
// ---------------------------------------------------------------------------

/// How well a beat sequence is supported by the independent onset list:
/// the mean over beats of the strongest onset within a 30 ms Gaussian of it.
/// Normalizing by beat count makes grids at the SAME tempo comparable, which
/// is what the oracle search needs.
pub fn onset_support(beats: &[f64], onsets: &[(f64, f32)]) -> f64 {
    if beats.is_empty() || onsets.is_empty() {
        return 0.0;
    }
    let sigma = 0.030f64;
    let mut sum = 0.0;
    let mut at = 0usize;
    for beat in beats {
        while at + 1 < onsets.len() && onsets[at].0 < beat - 0.120 {
            at += 1;
        }
        let mut best = 0.0f64;
        let mut index = at;
        while index < onsets.len() && onsets[index].0 < beat + 0.120 {
            let (time, strength) = onsets[index];
            let weight = (-((time - beat).powi(2)) / (2.0 * sigma * sigma)).exp();
            best = best.max(strength as f64 * weight);
            index += 1;
        }
        sum += best;
    }
    sum / beats.len() as f64
}

/// The same support, sampled from a dense table so an exhaustive grid search
/// is affordable: `table[i]` is the support of a beat at `i * step` seconds.
pub struct SupportTable {
    values: Vec<f32>,
    step: f64,
}

impl SupportTable {
    pub fn build(onsets: &[(f64, f32)], seconds: f64) -> SupportTable {
        let step = 0.0005;
        let sigma = 0.030f64;
        let mut values = vec![0.0f32; (seconds / step) as usize + 2];
        let reach = (0.120 / step) as isize;
        for (time, strength) in onsets {
            let centre = (time / step).round() as isize;
            for offset in -reach..=reach {
                let index = centre + offset;
                if index < 0 || index as usize >= values.len() {
                    continue;
                }
                let delta = offset as f64 * step;
                let weight = (-(delta * delta) / (2.0 * sigma * sigma)).exp() as f32;
                let value = strength * weight;
                if value > values[index as usize] {
                    values[index as usize] = value;
                }
            }
        }
        SupportTable { values, step }
    }

    #[inline]
    fn at(&self, seconds: f64) -> f64 {
        if seconds < 0.0 {
            return 0.0;
        }
        let position = seconds / self.step;
        let index = position as usize;
        if index + 1 >= self.values.len() {
            return 0.0;
        }
        let fraction = (position - index as f64) as f32;
        (self.values[index] * (1.0 - fraction) + self.values[index + 1] * fraction) as f64
    }

    /// Mean support of the fixed grid `phase + k * period` across the track.
    pub fn grid(&self, period: f64, phase: f64, seconds: f64) -> f64 {
        if period <= 0.05 {
            return 0.0;
        }
        let count = ((seconds - phase) / period).floor();
        if !(count >= 1.0) {
            return 0.0;
        }
        let count = count as usize;
        let mut sum = 0.0;
        for beat in 0..=count {
            sum += self.at(phase + beat as f64 * period);
        }
        sum / (count + 1) as f64
    }
}

/// The best fixed grid there is, found by exhaustive search rather than by any
/// tracker's reasoning. This is the CEILING of the fixed-grid class — no
/// algorithm that publishes one BPM and one anchor can beat it — so the gap
/// between it and the analysis is exactly what the analysis's search leaves
/// on the table, with nothing about method in the way.
///
/// Three passes per seed tempo: 0.1 % period steps over ±5 % with the phase
/// swept at 2 ms, then 0.005 % over ±0.15 % with the phase at 0.5 ms, then
/// 0.0005 % over ±0.015 % with the phase at 0.1 ms. The last pass matters:
/// over a seven-minute track a 0.05 % period error walks the grid a fifth of
/// a second, so an oracle that stops coarse is not an oracle.
///
/// The seeds must all name the SAME metrical level. Mean-support-per-beat
/// rises when a grid is thinned — a half-time grid keeps only the beats it
/// likes — so an oracle allowed to change octave answers a different, easier
/// question. Octave correctness is judged separately, against the tags and
/// the kicks; this judges the search.
pub fn oracle_grid(table: &SupportTable, seconds: f64, seeds: &[f64]) -> (f64, f64, f64) {
    let mut best = (0.5f64, 0.0f64, f64::NEG_INFINITY);
    for seed in seeds {
        if !(40.0..=400.0).contains(seed) || !seed.is_finite() {
            continue;
        }
        let base = 60.0 / seed;
        let mut period = base;
        let mut phase = 0.0f64;
        let mut support = f64::NEG_INFINITY;
        // Coarse: full phase sweep, because nothing is known about it yet.
        for step in 0..=100usize {
            let candidate = base * (0.95 + 0.10 * step as f64 / 100.0);
            let phases = (candidate / 0.002).round() as usize;
            for phase_step in 0..phases {
                let candidate_phase = phase_step as f64 * 0.002;
                let value = table.grid(candidate, candidate_phase, seconds);
                if value > support {
                    support = value;
                    period = candidate;
                    phase = candidate_phase;
                }
            }
        }
        // Then two localizing passes on both axes at once.
        for (period_span, period_steps, phase_span, phase_step) in
            [(0.0015f64, 60usize, 0.004f64, 0.0005f64), (0.00015, 60, 0.001, 0.0001)]
        {
            let (centre_period, centre_phase) = (period, phase);
            let phases = (2.0 * phase_span / phase_step).round() as usize;
            for step in 0..=period_steps {
                let candidate = centre_period
                    * (1.0 - period_span
                        + 2.0 * period_span * step as f64 / period_steps as f64);
                for phase_index in 0..=phases {
                    let candidate_phase =
                        centre_phase - phase_span + phase_index as f64 * phase_step;
                    if candidate_phase < 0.0 {
                        continue;
                    }
                    let value = table.grid(candidate, candidate_phase, seconds);
                    if value > support {
                        support = value;
                        period = candidate;
                        phase = candidate_phase;
                    }
                }
            }
        }
        if support > best.2 {
            best = (period, phase, support);
        }
    }
    (60.0 / best.0, best.1, best.2)
}

/// A piecewise beat grid: constant tempo inside each segment, continuous at
/// the joins. The prototype for what a drifting drummer needs and what one
/// straight line across a whole song cannot give.
pub struct PiecewiseGrid {
    /// `(start time, period)` per segment; beats run from the previous
    /// segment's last beat.
    pub segments: Vec<(f64, f64)>,
    pub beats: Vec<f64>,
}

/// Fit a piecewise-constant tempo map to the onsets, `beats_per_segment`
/// beats at a time.
///
/// Deliberately the SAME least-squares fit the published grid uses, run per
/// segment and chained so each segment starts where the last one ended. That
/// is the point of the prototype: it measures what the existing machinery
/// buys when it is allowed to bend, with nothing else changed, so the number
/// it produces is the value of the segmentation and not of a better fitter.
pub fn piecewise_fit(
    onsets: &[(f64, f32)],
    seconds: f64,
    seed_period: f64,
    seed_phase: f64,
    beats_per_segment: usize,
) -> PiecewiseGrid {
    let mut segments = Vec::new();
    let mut beats = Vec::new();
    let mut at = seed_phase;
    let mut period = seed_period;
    while at < seconds {
        // Collect the onset nearest each predicted beat of this segment.
        let mut points: Vec<(f64, f64, f64)> = Vec::new();
        for index in 0..beats_per_segment {
            let predicted = at + index as f64 * period;
            if predicted > seconds {
                break;
            }
            let radius = period * 0.25;
            let mut best: Option<(f64, f32)> = None;
            for (time, strength) in onsets {
                if (time - predicted).abs() <= radius
                    && best.map_or(true, |(_, s)| *strength > s)
                {
                    best = Some((*time, *strength));
                }
            }
            if let Some((time, strength)) = best {
                points.push((index as f64, time, strength as f64));
            }
        }
        // Fit period AND phase, then pull the phase back toward where the
        // previous segment left off. Pinning the phase outright is what a
        // naive chained fit does and it is why one does not work: a segment
        // that starts a few milliseconds out can never say so, so the error
        // rides forward into every segment after it. Letting the phase move
        // and then constraining it keeps the map continuous without making
        // it deaf.
        if points.len() >= 4 {
            let (mut sum_w, mut sum_b, mut sum_t, mut sum_bb, mut sum_bt) =
                (0.0, 0.0, 0.0, 0.0, 0.0);
            for (index, time, weight) in &points {
                sum_w += weight;
                sum_b += weight * index;
                sum_t += weight * time;
                sum_bb += weight * index * index;
                sum_bt += weight * index * time;
            }
            let denominator = sum_w * sum_bb - sum_b * sum_b;
            if denominator.abs() > 1e-9 {
                let fitted_period = (sum_w * sum_bt - sum_b * sum_t) / denominator;
                let fitted_start = (sum_t - fitted_period * sum_b) / sum_w;
                // A segment refines a tempo; it does not get to change it.
                if fitted_period.is_finite()
                    && (fitted_period / period - 1.0).abs() < 0.10
                    && fitted_start.is_finite()
                    && (fitted_start - at).abs() < 0.25 * period
                {
                    period = fitted_period;
                    at = fitted_start;
                }
            }
        }
        segments.push((at, period));
        for index in 0..beats_per_segment {
            let beat = at + index as f64 * period;
            if beat > seconds {
                break;
            }
            beats.push(beat);
        }
        at += beats_per_segment as f64 * period;
    }
    PiecewiseGrid { segments, beats }
}

/// Median signed offset of `beats` from `grid`, in fractions of a beat,
/// wrapped to `[-0.5, 0.5)`. Tells an offbeat lock (±0.5) apart from a
/// tracking disagreement.
pub fn phase_offset(beats: &[f64], grid: &TrackGrid) -> f64 {
    if beats.is_empty() || !grid.has_grid() {
        return f64::NAN;
    }
    let mut offsets: Vec<f64> = beats
        .iter()
        .map(|at| {
            let phase = grid.beat_at(*at).rem_euclid(1.0);
            if phase >= 0.5 {
                phase - 1.0
            } else {
                phase
            }
        })
        .collect();
    offsets.sort_by(|a, b| a.partial_cmp(b).unwrap());
    offsets[offsets.len() / 2]
}

/// How the two tempi relate: 1 = same, 2 = ours is double, and so on.
pub fn octave_class(ours: f64, reference: f64) -> &'static str {
    if reference <= 0.0 || ours <= 0.0 {
        return "n/a";
    }
    let ratio = ours / reference;
    for (value, name) in [
        (1.0, "same"),
        (2.0, "double"),
        (0.5, "half"),
        (1.5, "3/2"),
        (2.0 / 3.0, "2/3"),
        (3.0, "triple"),
        (1.0 / 3.0, "third"),
        (4.0 / 3.0, "4/3"),
        (0.75, "3/4"),
    ] {
        if (ratio / value - 1.0).abs() < 0.04 {
            return name;
        }
    }
    "other"
}

/// Read a `TBPM` frame out of an ID3v2 header, which is how the DJ tooling
/// that analysed this library records its own tempo opinion.
pub fn tagged_bpm(path: &Path) -> Option<f64> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 10 || &bytes[0..3] != b"ID3" {
        return None;
    }
    let version = bytes[3];
    let syncsafe = |b: &[u8]| -> usize {
        ((b[0] as usize) << 21) | ((b[1] as usize) << 14) | ((b[2] as usize) << 7) | b[3] as usize
    };
    let size = syncsafe(&bytes[6..10]);
    let end = (10 + size).min(bytes.len());
    let mut at = 10usize;
    while at + 10 <= end {
        let id = &bytes[at..at + 4];
        if id[0] == 0 {
            break;
        }
        let frame_size = if version >= 4 {
            syncsafe(&bytes[at + 4..at + 8])
        } else {
            u32::from_be_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize
        };
        if frame_size == 0 || at + 10 + frame_size > end {
            break;
        }
        if id == b"TBPM" {
            let body = &bytes[at + 11..at + 10 + frame_size];
            let text: String = body
                .iter()
                .take_while(|b| **b != 0)
                .map(|b| *b as char)
                .collect();
            return text.trim().parse::<f64>().ok();
        }
        at += 10 + frame_size;
    }
    None
}

pub fn evaluate(path: &Path, pcm: &TrackPcm) -> TrackReport {
    let seconds = pcm.seconds();
    let analysis = analyze(pcm);
    let grid = analysis.grid;
    let front = onset_front(pcm);

    // Ground truth: the kick chain.
    let kicks = kick_reference(pcm, &front);

    // The timing authority: which onsets exist is the STFT's call, when they
    // happened is this envelope's.
    let (fine, fine_hop) = fine_onset_envelope(pcm);

    // The independent BROADBAND onset list: the STFT says which, the fine
    // envelope says when. This is what the second, style-agnostic reference
    // tracker decodes.
    let detected = pick_peaks_strength(&front.flux, front.hop_secs, 1.2, 0.030);
    let times: Vec<f64> = detected.iter().map(|(at, _)| *at).collect();
    let mut onsets: Vec<(f64, f32)> = snap_to_fine(&times, &fine, fine_hop, 0.035)
        .into_iter()
        .zip(detected.iter().map(|(_, strength)| *strength))
        .collect();
    onsets.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    // Reference tracker: its own tempo, its own decoding.
    let activation = impulse_activation(&onsets, JUDGE_HOP, seconds, 0.015);
    let reference_bpm = reference_tempo(&activation, JUDGE_HOP, 70.0, 180.0);
    let reference_beats = if reference_bpm > 1.0 {
        let decoded = ellis_dp(&activation, JUDGE_HOP, 60.0 / reference_bpm, dp_tightness());
        snap_to_fine(&decoded, &fine, fine_hop, 0.025)
    } else {
        Vec::new()
    };

    let ours = grid_beats(&grid, seconds);
    // The ceiling is measured on the KICKS, not on every onset in the mix.
    // A broadband support objective picks the offbeat on plenty of house
    // records — the hats between the kicks carry more spectral novelty than
    // the kick does — and a beatgrid that sits on the hats is wrong however
    // well it scores. In this genre the beat is the kick, so the ceiling is
    // the best grid over the kicks.
    let table = SupportTable::build(&kicks.onsets, seconds);
    // Seeds all at OUR metrical level, so the oracle answers "is this the
    // best fixed grid at this tempo" and not "would a different tempo score
    // higher on a per-beat average".
    let mut seeds = vec![grid.bpm];
    for candidate in [kicks.bpm, reference_bpm].into_iter().chain(tagged_bpm(path)) {
        for multiple in [0.5, 1.0, 2.0] {
            let scaled = candidate * multiple;
            if grid.bpm > 1.0 && (scaled / grid.bpm - 1.0).abs() < 0.05 {
                seeds.push(scaled);
            }
        }
    }
    seeds.retain(|bpm| (60.0..=200.0).contains(bpm));
    let (oracle_bpm, oracle_phase, support_oracle) = oracle_grid(&table, seconds, &seeds);
    let oracle = TrackGrid {
        bpm: oracle_bpm,
        beat_secs: 60.0 / oracle_bpm.max(1e-9),
        first_beat_secs: oracle_phase,
        downbeat_phase: 0,
        confidence: 1.0,
    };
    let oracle_beats = grid_beats(&oracle, seconds);
    let support_ours = if grid.has_grid() {
        table.grid(grid.beat_secs, grid.first_beat_secs.rem_euclid(grid.beat_secs), seconds)
    } else {
        0.0
    };
    let support_reference = if kicks.beats.is_empty() {
        0.0
    } else {
        onset_support(&kicks.beats, &kicks.onsets)
    };

    // Scoring against the kicks happens only where the chain exists.
    let ours_trimmed = trim_to_spans(&ours, &kicks.spans);
    let reference_trimmed = trim_to_spans(&reference_beats, &kicks.spans);
    let kick_beats = trim_to_spans(&kicks.beats, &kicks.spans);

    // Split the error into the part that is phase and the part that is
    // period, by taking the median phase error out and scoring again.
    let kick_phase = phase_offset(&kick_beats, &grid);
    let rephased: Vec<f64> = if kick_phase.is_finite() {
        ours_trimmed
            .iter()
            .map(|at| at + kick_phase * grid.beat_secs)
            .collect()
    } else {
        ours_trimmed.clone()
    };

    // Alignment against the fine LOW-band envelope: in this genre the beat is
    // the kick, so that is what a ruling has to sit on.
    let (low, low_hop) = fine_low_envelope(pcm);
    let (bias_ms, jitter_ms) = alignment(&ours, grid.beat_secs, &low, low_hop);
    let (reference_bias_ms, reference_jitter_ms) =
        alignment(&kicks.beats, 60.0 / kicks.bpm.max(1.0), &low, low_hop);
    let reference_ibi_spread = {
        let mut steps: Vec<f64> =
            kicks.beats.windows(2).map(|pair| pair[1] - pair[0]).collect();
        if steps.len() < 8 {
            f64::NAN
        } else {
            steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = steps[steps.len() / 2];
            (steps[steps.len() * 3 / 4] - steps[steps.len() / 4]) / median.max(1e-9)
        }
    };
    let (oracle_bias_ms, oracle_jitter_ms) =
        alignment(&oracle_beats, oracle.beat_secs, &low, low_hop);

    // Downbeat margin, whether analysing half the track calls the same
    // moments downbeats as analysing all of it, and whether the track's own
    // arrangement changes agree.
    let downbeat_margin = downbeat_margin(pcm, &grid);
    let downbeat_structure =
        downbeat_vs_structure(&grid, &structural_boundaries(pcm, 24));
    let half = pcm.frames.len() / 2;
    let first_half =
        TrackPcm { frames: pcm.frames[..half].to_vec(), sample_rate: pcm.sample_rate };
    let second_half =
        TrackPcm { frames: pcm.frames[half..].to_vec(), sample_rate: pcm.sample_rate };
    let first_grid = analyze(&first_half).grid;
    let second_grid = analyze(&second_half).grid;
    let downbeat_halves = (
        downbeat_agreement(&grid, &first_grid, 0.0, first_half.seconds()),
        downbeat_agreement(
            &grid,
            &second_grid,
            half as f64 / pcm.sample_rate.max(1) as f64,
            second_half.seconds(),
        ),
    );
    let drift_ms = if first_grid.has_grid() && grid.has_grid() {
        let at = seconds * 0.95;
        (first_grid.secs_at_beat(first_grid.beat_at(at).round())
            - grid.secs_at_beat(grid.beat_at(at).round()))
        .abs()
            * 1_000.0
    } else {
        f64::NAN
    };

    TrackReport {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        seconds,
        tagged_bpm: tagged_bpm(path),
        grid,
        kick_bpm: kicks.bpm,
        kick_coverage: kicks.coverage,
        kick_beats: kick_beats.len(),
        reference_bpm,
        vs_kicks: score(&ours_trimmed, &kick_beats),
        vs_reference: score(&ours, &reference_beats),
        reference_vs_kicks: score(&reference_trimmed, &kick_beats),
        support_ours,
        support_reference,
        support_oracle,
        oracle_bpm,
        oracle_vs_kicks: score(&trim_to_spans(&oracle_beats, &kicks.spans), &kick_beats),
        oracle_bias_ms,
        oracle_jitter_ms,
        kick_phase,
        reference_phase: phase_offset(&reference_beats, &grid),
        vs_kicks_rephased: score(&rephased, &kick_beats),
        bias_ms,
        jitter_ms,
        reference_bias_ms,
        reference_jitter_ms,
        reference_ibi_spread,
        downbeat_margin,
        downbeat_halves,
        downbeat_structure,
        drift_ms,
    }
}

/// Median signed distance from each beat to the strongest transient in the
/// half-beat around it, and the median absolute deviation of those distances,
/// both in milliseconds. Bias says the whole sequence sits early or late;
/// jitter says it wanders.
fn alignment(beats: &[f64], beat_secs: f64, fine: &[f32], hop: f64) -> (f64, f64) {
    let mut offsets: Vec<f64> = Vec::new();
    for beat in beats {
        let half = beat_secs * 0.25;
        let from = ((beat - half) / hop).round().max(0.0) as usize;
        let to = ((beat + half) / hop).round().max(0.0) as usize;
        if to >= fine.len() || from > to {
            continue;
        }
        let mut best = from;
        for index in from..=to {
            if fine[index] > fine[best] {
                best = index;
            }
        }
        if fine[best] > 0.0 {
            offsets.push((best as f64 * hop - beat) * 1_000.0);
        }
    }
    if offsets.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    let mut sorted = offsets.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let mut spread: Vec<f64> = offsets.iter().map(|v| (v - median).abs()).collect();
    spread.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (median, spread[spread.len() / 2])
}

/// What fraction of the downbeats a grid fitted to PART of the track calls
/// are also downbeats of the whole-track grid. Comparing `downbeat_phase`
/// directly says nothing — the two grids number their beats from different
/// anchors — so this compares the moments themselves.
fn downbeat_agreement(whole: &TrackGrid, part: &TrackGrid, offset: f64, span: f64) -> f64 {
    if !whole.has_grid() || !part.has_grid() {
        return f64::NAN;
    }
    let mut total = 0usize;
    let mut agreed = 0usize;
    let mut beat = part.beat_at(0.0).ceil() as i64;
    loop {
        let local = part.secs_at_beat(beat as f64);
        if local > span {
            break;
        }
        beat += 1;
        if !part.is_downbeat(beat - 1) {
            continue;
        }
        let at = local + offset;
        let whole_beat = whole.beat_at(at).round() as i64;
        let distance = (whole.secs_at_beat(whole_beat as f64) - at).abs();
        if distance > 0.15 * whole.beat_secs {
            continue;
        }
        total += 1;
        if whole.is_downbeat(whole_beat) {
            agreed += 1;
        }
    }
    if total == 0 {
        f64::NAN
    } else {
        agreed as f64 / total as f64
    }
}

/// Where a track's STRUCTURE changes: the times of the largest jumps in a
/// two-second loudness envelope — drops, breakdowns, the bar the hats come
/// in, the bar the bass leaves.
///
/// This is the only downbeat ground truth available without a human. There
/// is no way to hear "the one" from the audio alone at a single bar, but
/// this music is built in four-, eight- and sixteen-bar phrases and its
/// arrangement changes land on the downbeat of a phrase essentially always.
/// So a grid whose downbeat is right will put these moments at bar position
/// zero, and a grid whose downbeat is a beat out will put them at position
/// one — over enough boundaries the difference is not subtle.
pub fn structural_boundaries(pcm: &TrackPcm, count: usize) -> Vec<f64> {
    let rate = pcm.sample_rate.max(1) as f64;
    let hop = (rate * 0.05).round().max(1.0) as usize;
    let mut loudness: Vec<f32> = Vec::with_capacity(pcm.frames.len() / hop + 1);
    let mut sum = 0.0f64;
    let mut in_hop = 0usize;
    for frame in &pcm.frames {
        let value = (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0;
        sum += (value as f64) * (value as f64);
        in_hop += 1;
        if in_hop == hop {
            loudness.push(((sum / in_hop as f64).sqrt() as f32 + 1e-6).ln());
            sum = 0.0;
            in_hop = 0;
        }
    }
    // Change over a two-second window either side, so a single bar of
    // silence does not register but an arrangement change does.
    let window = (2.0 / 0.05) as usize;
    if loudness.len() < 3 * window {
        return Vec::new();
    }
    let mut change = vec![0.0f32; loudness.len()];
    for index in window..loudness.len() - window {
        let before: f32 =
            loudness[index - window..index].iter().sum::<f32>() / window as f32;
        let after: f32 =
            loudness[index..index + window].iter().sum::<f32>() / window as f32;
        change[index] = (after - before).abs();
    }
    // Pick the strongest, keeping them four seconds apart so one drop does
    // not fill the whole list.
    let mut order: Vec<usize> = (window..loudness.len() - window).collect();
    order.sort_by(|a, b| change[*b].partial_cmp(&change[*a]).unwrap());
    let mut out: Vec<f64> = Vec::new();
    for index in order {
        let at = index as f64 * 0.05;
        if out.iter().all(|other: &f64| (other - at).abs() > 4.0) {
            out.push(at);
        }
        if out.len() >= count {
            break;
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    out
}

/// How the structural boundaries fall across the four positions of the bar,
/// under `grid`. Returns `(fraction at position zero, count)`. A grid whose
/// downbeat is right puts most of them at zero; chance is a quarter.
pub fn downbeat_vs_structure(grid: &TrackGrid, boundaries: &[f64]) -> (f64, usize) {
    if !grid.has_grid() || boundaries.is_empty() {
        return (f64::NAN, 0);
    }
    let mut counts = [0usize; 4];
    let mut total = 0usize;
    for at in boundaries {
        let beat = grid.beat_at(*at);
        // Only count a boundary that actually lands near a beat; one that
        // falls between beats is not evidence about which beat starts a bar.
        if (beat - beat.round()).abs() > 0.25 {
            continue;
        }
        let position = (beat.round() as i64 + grid.downbeat_phase as i64).rem_euclid(4) as usize;
        counts[position] += 1;
        total += 1;
    }
    if total == 0 {
        return (f64::NAN, 0);
    }
    (counts[0] as f64 / total as f64, total)
}

/// How much more low-band onset energy the chosen bar phase carries than the
/// best alternative. Below about 1.05 the downbeat is a coin toss.
fn downbeat_margin(pcm: &TrackPcm, grid: &TrackGrid) -> f64 {
    if !grid.has_grid() {
        return 0.0;
    }
    let (fine, hop) = fine_low_envelope(pcm);
    let seconds = pcm.seconds();
    let mut phase_energy = [0.0f64; 4];
    let mut beat = grid.beat_at(0.0).ceil() as i64;
    while grid.secs_at_beat(beat as f64) < seconds {
        let at = grid.secs_at_beat(beat as f64);
        let from = ((at - 0.030) / hop).round().max(0.0) as usize;
        let to = ((at + 0.030) / hop).round().max(0.0) as usize;
        if to >= fine.len() {
            break;
        }
        let mut peak = 0.0f32;
        for index in from..=to {
            peak = peak.max(fine[index]);
        }
        let bar_position = (beat + grid.downbeat_phase as i64).rem_euclid(4) as usize;
        phase_energy[bar_position] += peak as f64;
        beat += 1;
    }
    let chosen = phase_energy[0];
    let runner_up = phase_energy[1..].iter().copied().fold(0.0f64, f64::max);
    if runner_up <= 1e-9 {
        return 0.0;
    }
    chosen / runner_up
}

// ---------------------------------------------------------------------------
// corpus driver
// ---------------------------------------------------------------------------

fn walk_mp3(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            walk_mp3(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("mp3") {
            out.push(path);
        }
    }
}

/// Deterministic spread over the corpus: sort, then take every Nth so the
/// sample is not one artist.
pub fn corpus(dir: &Path, count: usize) -> Vec<PathBuf> {
    let mut all = Vec::new();
    walk_mp3(dir, &mut all);
    if all.is_empty() || count == 0 {
        return all;
    }
    if all.len() <= count {
        return all;
    }
    let stride = all.len() as f64 / count as f64;
    (0..count)
        .map(|index| all[((index as f64 * stride) as usize).min(all.len() - 1)].clone())
        .collect()
}

fn median(values: &mut Vec<f64>) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values[values.len() / 2]
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// Read a separated stem out of the VJ's stem cache: `header` for the shape,
/// `<stem>.pcm` for interleaved 16-bit stereo on the track's own timeline.
pub fn read_stem(dir: &Path, stem: &str) -> Option<TrackPcm> {
    let header = std::fs::read_to_string(dir.join("header")).ok()?;
    let field = |name: &str| -> Option<u64> {
        header.lines().find_map(|line| {
            line.strip_prefix(&format!("{name}="))
                .and_then(|value| value.trim().parse().ok())
        })
    };
    let sample_rate = field("sample_rate")? as u32;
    let frames = field("frames")? as usize;
    let bytes = std::fs::read(dir.join(format!("{stem}.pcm"))).ok()?;
    if bytes.len() < frames * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(frames);
    for frame in bytes[..frames * 4].chunks_exact(4) {
        out.push([
            i16::from_le_bytes([frame[0], frame[1]]),
            i16::from_le_bytes([frame[2], frame[3]]),
        ]);
    }
    Some(TrackPcm { frames: out, sample_rate })
}

/// Does re-gridding from the ISOLATED DRUMS beat gridding from the mix?
///
/// The app separates stems already, and caches them by digest, so once a
/// track has been demixed its drums are sitting on disk for nothing. On this
/// library it should not matter — a house record's kick is the loudest thing
/// in it and nothing masks it — but on a record with a band playing over the
/// drums it is exactly the masking that makes the mix hard to read.
///
/// Both grids are judged against a reference built from the DRUMS, which is
/// as close to an unarguable beat annotation as recorded music gets.
///
/// ```text
/// VJ_BEAT_STEMS=/path/to/stem-cache/<digest> \
/// VJ_BEAT_TRACK=/path/to/the/same/track.mp3 \
///     cargo test -p makepad-vj --release -- --nocapture --ignored beat_eval::drums
/// ```
#[test]
#[ignore = "opt-in: needs a separated track in the stem cache"]
fn drums_stem_regrid() {
    let (Ok(stems), Ok(track)) =
        (std::env::var("VJ_BEAT_STEMS"), std::env::var("VJ_BEAT_TRACK"))
    else {
        eprintln!("VJ_BEAT_STEMS / VJ_BEAT_TRACK not set");
        return;
    };
    let stems = PathBuf::from(stems);
    let track = PathBuf::from(track);
    let Some(drums) = read_stem(&stems, "drums") else {
        eprintln!("no drums stem in {}", stems.display());
        return;
    };
    let mix = decode_audio_file(&track).expect("decode the mix");
    let seconds = drums.seconds();
    assert!(
        (mix.seconds() - seconds).abs() < 1.0,
        "the stem ({seconds:.1}s) and the track ({:.1}s) are not the same recording",
        mix.seconds()
    );

    // The reference: the drums, read by the same machinery, where nothing is
    // masking the kick.
    let front = onset_front(&drums);
    let reference = kick_reference(&drums, &front);
    let truth = trim_to_spans(&reference.beats, &reference.spans);

    let from_mix = analyze(&mix).grid;
    let from_drums = analyze(&drums).grid;
    let score_of = |grid: &TrackGrid| -> (f64, f64, f64) {
        let beats = trim_to_spans(&grid_beats(grid, seconds), &reference.spans);
        let scores = score(&beats, &truth);
        (scores.f, scores.continuity.cmlt, grid.bpm)
    };
    let (mix_f, mix_cmlt, mix_bpm) = score_of(&from_mix);
    let (drums_f, drums_cmlt, drums_bpm) = score_of(&from_drums);
    // And the same least-squares fit allowed to bend every four bars, over
    // the drums. A tempo map needs dense evidence in every segment, which is
    // the thing an isolated drum track actually supplies.
    let (fine, fine_hop) = fine_onset_envelope(&drums);
    let detected = pick_peaks_strength(&front.flux, front.hop_secs, 1.2, 0.030);
    let times: Vec<f64> = detected.iter().map(|(t, _)| *t).collect();
    let mut onsets: Vec<(f64, f32)> = snap_to_fine(&times, &fine, fine_hop, 0.035)
        .into_iter()
        .zip(detected.iter().map(|(_, s)| *s))
        .collect();
    onsets.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let piecewise = piecewise_fit(
        &onsets,
        seconds,
        from_drums.beat_secs,
        from_drums.first_beat_secs.rem_euclid(from_drums.beat_secs),
        16,
    );
    let piecewise_scores =
        score(&trim_to_spans(&piecewise.beats, &reference.spans), &truth);
    eprintln!(
        "{}\n  from the mix:      {mix_bpm:8.3} BPM  F {mix_f:.3}  CMLt {mix_cmlt:.3}\n  \
         from the drums:    {drums_bpm:8.3} BPM  F {drums_f:.3}  CMLt {drums_cmlt:.3}\n  \
         drums, segmented:  {} segments  F {:.3}  CMLt {:.3}\n  \
         reference {} beats over {:.0}% of the track",
        track.file_name().unwrap_or_default().to_string_lossy(),
        piecewise.segments.len(),
        piecewise_scores.f,
        piecewise_scores.continuity.cmlt,
        truth.len(),
        reference.coverage * 100.0,
    );
}

/// One track, everything the judge sees, for when a corpus number looks
/// wrong and the question is which stage produced it.
///
/// ```text
/// VJ_BEAT_TRACK=/path/to.mp3 cargo test -p makepad-vj --release \
///     -- --nocapture --ignored beat_eval::one_track
/// ```
#[test]
#[ignore = "opt-in: needs a track"]
fn one_track_diagnostics() {
    let Ok(path) = std::env::var("VJ_BEAT_TRACK") else {
        eprintln!("VJ_BEAT_TRACK not set");
        return;
    };
    let path = PathBuf::from(path);
    let pcm = decode_audio_file(&path).expect("decode");
    let seconds = pcm.seconds();
    eprintln!("{} — {seconds:.1}s, tag {:?}", path.display(), tagged_bpm(&path));

    let front = onset_front(&pcm);
    let picked = pick_peaks_strength(&front.low_flux, front.hop_secs, 1.0, 0.250);
    let mut intervals: Vec<f64> = picked.windows(2).map(|p| p[1].0 - p[0].0).collect();
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!(
        "kick onsets: {} in {seconds:.0}s ({:.2}/s), IOI median {:.4}s ({:.1} BPM), \
         quartiles {:.3}/{:.3}, modal {:?}",
        picked.len(),
        picked.len() as f64 / seconds,
        intervals.get(intervals.len() / 2).copied().unwrap_or(0.0),
        60.0 / intervals.get(intervals.len() / 2).copied().unwrap_or(1.0),
        intervals.get(intervals.len() / 4).copied().unwrap_or(0.0),
        intervals.get(intervals.len() * 3 / 4).copied().unwrap_or(0.0),
        modal_interval(
            &picked.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
            70.0,
            180.0
        )
        .map(|i| 60.0 / i),
    );
    let activation = impulse_activation(&picked, JUDGE_HOP, seconds, 0.015);
    eprintln!(
        "kick-DP tempo {:.3}",
        reference_tempo(&activation, JUDGE_HOP, 70.0, 180.0)
    );
    let reference = kick_reference(&pcm, &front);
    let mut steps: Vec<f64> = reference.beats.windows(2).map(|p| p[1] - p[0]).collect();
    steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!(
        "reference: {} beats, tempo {:.3}, step median {:.4} ({:.2} BPM), \
         quartiles {:.4}/{:.4}, coverage {:.0}%",
        reference.beats.len(),
        reference.bpm,
        steps.get(steps.len() / 2).copied().unwrap_or(0.0),
        60.0 / steps.get(steps.len() / 2).copied().unwrap_or(1.0),
        steps.get(steps.len() / 4).copied().unwrap_or(0.0),
        steps.get(steps.len() * 3 / 4).copied().unwrap_or(0.0),
        reference.coverage * 100.0,
    );
    // Where inside the analysis the period comes from, stage by stage.
    let envelopes = super::build_envelopes(&pcm);
    let hop_rate = envelopes.sample_rate / envelopes.hop as f64;
    let grid = analyze(&pcm).grid;
    eprintln!("published grid: {:.4} BPM, first beat {:.4}s", grid.bpm, grid.first_beat_secs);
    let report = evaluate(&path, &pcm);
    let oracle_period = 60.0 / report.oracle_bpm;
    let beats = seconds / oracle_period;
    eprintln!(
        "oracle {:.4} BPM: ours is {:+.4} % off, which over {:.0} beats is {:+.0} ms of walk",
        report.oracle_bpm,
        (grid.bpm / report.oracle_bpm - 1.0) * 100.0,
        beats,
        (60.0 / grid.bpm - oracle_period) * beats * 1000.0,
    );
    // What the low band says about the two pulses half a beat apart.
    let period_hops = grid.beat_secs * hop_rate;
    let offset_hops = grid.first_beat_secs * hop_rate - 0.5;
    let here = super::comb_energy(&envelopes.low_onset, period_hops, offset_hops);
    let there =
        super::comb_energy(&envelopes.low_onset, period_hops, offset_hops + period_hops * 0.5);
    let broad_here = super::comb_energy(&envelopes.onset, period_hops, offset_hops);
    let broad_there =
        super::comb_energy(&envelopes.onset, period_hops, offset_hops + period_hops * 0.5);
    eprintln!(
        "half-beat evidence: low band here {here:.0} vs there {there:.0} (ratio {:.3}), \
         broadband here {broad_here:.0} vs there {broad_there:.0} (ratio {:.3})",
        there / here.max(1e-9),
        broad_there / broad_here.max(1e-9),
    );

    // The comb's own resolution, in the same units.
    let span = (60.0 * hop_rate / grid.bpm) * 0.03;
    eprintln!(
        "comb sweep: +/-{:.3} hops in 48 steps = {:.4} % per step = {:.0} ms of walk per step",
        span,
        200.0 * span / 48.0 / (60.0 * hop_rate / grid.bpm),
        2.0 * span / 48.0 / hop_rate * beats * 1000.0,
    );
    print_track(&report, 0.0);
}

#[test]
#[ignore = "opt-in: needs a local music corpus"]
fn corpus_evaluation() {
    // An explicit list, for asking about a named handful rather than a
    // stratified sample of the library.
    if let Ok(list) = std::env::var("VJ_BEAT_TRACKS") {
        let mut reports = Vec::new();
        for path in list.split(':').filter(|p| !p.is_empty()) {
            let path = PathBuf::from(path);
            let Ok(pcm) = decode_audio_file(&path) else {
                eprintln!("  SKIP (decode) {}", path.display());
                continue;
            };
            let started = std::time::Instant::now();
            let report = evaluate(&path, &pcm);
            print_track(&report, started.elapsed().as_secs_f64());
            reports.push(report);
        }
        summarize(&reports);
        return;
    }
    let Ok(dir) = std::env::var("VJ_BEAT_CORPUS") else {
        eprintln!("VJ_BEAT_CORPUS not set");
        return;
    };
    let count: usize = std::env::var("VJ_BEAT_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(&dir);
    let dir = if root.is_dir() { root } else { PathBuf::from(&dir) };
    let paths = corpus(&dir, count);
    eprintln!("corpus: {} tracks from {}", paths.len(), dir.display());

    let mut reports = Vec::new();
    for path in &paths {
        let Ok(pcm) = decode_audio_file(path) else {
            eprintln!("  SKIP (decode) {}", path.display());
            continue;
        };
        let started = std::time::Instant::now();
        let report = evaluate(path, &pcm);
        print_track(&report, started.elapsed().as_secs_f64());
        reports.push(report);
    }

    summarize(&reports);
}

pub fn print_track(report: &TrackReport, took: f64) {
    eprintln!(
        "{:<38} {:4.0}s tag {:>4} | ours {:7.3}  ref {:7.3} ({} beats, {:.0}% covered, \
         bias {:+.1}/jit {:.1} ms)  superflux-dp {:7.3}  oracle {:7.3}\n    \
         vs ref   ours F {:.3} P {:.3} R {:.3} cemgil {:.3} CMLt {:.3} AMLt {:.3} \
         phase {:+.3} (rephased F {:.3})\n             oracle F {:.3} CMLt {:.3} bias {:+.1}/jit {:.1} ms\n    \
         support  ours/oracle {:.4}  ref/oracle {:.4} | \
         ours bias {:+5.1} ms jitter {:4.1} ms | downbeat x{:.2} halves {:.2}/{:.2} \
         structure {:.2} of {} | drift {:3.0} ms | {:.1}s",
        report.name.chars().take(38).collect::<String>(),
        report.seconds,
        report.tagged_bpm.map(|b| format!("{b:.0}")).unwrap_or("-".into()),
        report.grid.bpm,
        report.kick_bpm,
        report.kick_beats,
        report.kick_coverage * 100.0,
        report.reference_bias_ms,
        report.reference_jitter_ms,
        report.reference_bpm,
        report.oracle_bpm,
        report.vs_kicks.f,
        report.vs_kicks.precision,
        report.vs_kicks.recall,
        report.vs_kicks.cemgil,
        report.vs_kicks.continuity.cmlt,
        report.vs_kicks.continuity.amlt,
        report.kick_phase,
        report.vs_kicks_rephased.f,
        report.oracle_vs_kicks.f,
        report.oracle_vs_kicks.continuity.cmlt,
        report.oracle_bias_ms,
        report.oracle_jitter_ms,
        report.support_ours / report.support_oracle.max(1e-9),
        report.support_reference / report.support_oracle.max(1e-9),
        report.bias_ms,
        report.jitter_ms,
        report.downbeat_margin,
        report.downbeat_halves.0,
        report.downbeat_halves.1,
        report.downbeat_structure.0,
        report.downbeat_structure.1,
        report.drift_ms,
        took,
    );
}

pub fn summarize(reports: &[TrackReport]) {
    if reports.is_empty() {
        return;
    }
    eprintln!("\n=== summary over {} tracks ===", reports.len());
    let mut tag_exact = 0usize;
    let mut tag_close = 0usize;
    let mut tag_octave = 0usize;
    let mut tag_other = 0usize;
    let mut tagged = 0usize;
    let mut kick_same = 0usize;
    let mut kick_other = 0usize;
    for report in reports {
        if let Some(tag) = report.tagged_bpm {
            tagged += 1;
            let ratio = report.grid.bpm / tag;
            if (ratio - 1.0).abs() < 0.005 {
                tag_exact += 1;
            } else if (ratio - 1.0).abs() < 0.02 {
                tag_close += 1;
            } else if matches!(
                octave_class(report.grid.bpm, tag),
                "double" | "half" | "3/2" | "2/3" | "triple" | "third"
            ) {
                tag_octave += 1;
            } else {
                tag_other += 1;
            }
        }
        if report.kick_bpm > 1.0 {
            if octave_class(report.grid.bpm, report.kick_bpm) == "same" {
                kick_same += 1;
            } else {
                kick_other += 1;
            }
        }
    }
    eprintln!(
        "tempo vs ID3 tag ({tagged} tagged): exact(<0.5%) {tag_exact}  close(<2%) {tag_close}  \
         octave {tag_octave}  wrong {tag_other}"
    );
    eprintln!("tempo vs kicks: same {kick_same}  differing {kick_other}");

    let stat = |name: &str, mut values: Vec<f64>| {
        let m = median(&mut values);
        let a = mean(&values);
        let worst = values
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        eprintln!("  {name:<26} mean {a:.4}  median {m:.4}  min {worst:.4}");
    };
    stat("F vs kicks (ours)", reports.iter().map(|r| r.vs_kicks.f).collect());
    stat("F vs kicks (rephased)", reports.iter().map(|r| r.vs_kicks_rephased.f).collect());
    stat("F vs kicks (oracle grid)", reports.iter().map(|r| r.oracle_vs_kicks.f).collect());
    stat(
        "CMLt vs kicks (oracle)",
        reports.iter().map(|r| r.oracle_vs_kicks.continuity.cmlt).collect(),
    );
    stat("Cemgil vs kicks", reports.iter().map(|r| r.vs_kicks.cemgil).collect());
    stat("P-score vs kicks", reports.iter().map(|r| r.vs_kicks.p_score).collect());
    stat("CMLt vs kicks (ours)", reports.iter().map(|r| r.vs_kicks.continuity.cmlt).collect());
    stat(
        "CMLt vs kicks (DP)",
        reports.iter().map(|r| r.reference_vs_kicks.continuity.cmlt).collect(),
    );
    stat("AMLt vs kicks (ours)", reports.iter().map(|r| r.vs_kicks.continuity.amlt).collect());
    stat("F vs DP tracker", reports.iter().map(|r| r.vs_reference.f).collect());
    stat(
        "support ours / oracle",
        reports
            .iter()
            .map(|r| r.support_ours / r.support_oracle.max(1e-9))
            .collect(),
    );
    stat(
        "support reference / oracle",
        reports
            .iter()
            .map(|r| r.support_reference / r.support_oracle.max(1e-9))
            .collect(),
    );
    let oracle_same = reports
        .iter()
        .filter(|r| octave_class(r.grid.bpm, r.oracle_bpm) == "same")
        .count();
    let oracle_tight = reports
        .iter()
        .filter(|r| (r.grid.bpm / r.oracle_bpm - 1.0).abs() < 0.001)
        .count();
    eprintln!(
        "  oracle: same octave on {oracle_same}/{}, within 0.1 % on {oracle_tight}/{}",
        reports.len(),
        reports.len()
    );
    let mut bias: Vec<f64> = reports.iter().map(|r| r.bias_ms.abs()).collect();
    let mut jitter: Vec<f64> = reports.iter().map(|r| r.jitter_ms).collect();
    let mut drift: Vec<f64> =
        reports.iter().map(|r| r.drift_ms).filter(|v| v.is_finite()).collect();
    eprintln!(
        "  alignment: |bias| median {:.1} ms, jitter median {:.1} ms, \
         half-vs-whole drift median {:.0} ms",
        median(&mut bias),
        median(&mut jitter),
        median(&mut drift),
    );
    let agree = reports
        .iter()
        .filter(|r| r.downbeat_halves.0 > 0.9 && r.downbeat_halves.1 > 0.9)
        .count();
    eprintln!(
        "  downbeat: both halves agree with the whole on {agree}/{} tracks, \
         margin median {:.2}",
        reports.len(),
        median(&mut reports.iter().map(|r| r.downbeat_margin).collect()),
    );
    let tag_matches = reports
        .iter()
        .filter(|r| {
            r.tagged_bpm
                .is_some_and(|tag| (r.kick_bpm / tag - 1.0).abs() < 0.02)
        })
        .count();
    let tagged = reports.iter().filter(|r| r.tagged_bpm.is_some()).count();
    eprintln!(
        "  REFERENCE quality: jitter median {:.1} ms, interval spread median {:.3}, \
         tempo matches the tag on {tag_matches}/{tagged}",
        median(&mut reports.iter().map(|r| r.reference_jitter_ms).collect()),
        median(
            &mut reports
                .iter()
                .map(|r| r.reference_ibi_spread)
                .filter(|v| v.is_finite())
                .collect()
        ),
    );
    let structure: Vec<f64> = reports
        .iter()
        .filter(|r| r.downbeat_structure.1 >= 8)
        .map(|r| r.downbeat_structure.0)
        .collect();
    let strong = structure.iter().filter(|v| **v >= 0.5).count();
    eprintln!(
        "  downbeat vs structure: mean {:.3} at bar position zero over {} tracks \
         (chance 0.25), at least half on {strong}",
        mean(&structure),
        structure.len(),
    );
    // Does the published confidence know when the grid is bad? A number that
    // says LOCK on a grid at 0.2 F is worse than no number, because every
    // consumer downstream believes it.
    let good: Vec<f64> = reports
        .iter()
        .filter(|r| r.vs_kicks.f >= 0.8)
        .map(|r| r.grid.confidence as f64)
        .collect();
    let bad: Vec<f64> = reports
        .iter()
        .filter(|r| r.vs_kicks.f < 0.4 && r.kick_coverage > 0.15)
        .map(|r| r.grid.confidence as f64)
        .collect();
    eprintln!(
        "  confidence: {:.3} on the {} grids that scored 0.8+, {:.3} on the {} that \
         scored under 0.4",
        mean(&good),
        good.len(),
        mean(&bad),
        bad.len(),
    );
    let offbeat = reports
        .iter()
        .filter(|r| r.kick_phase.is_finite() && r.kick_phase.abs() > 0.2)
        .count();
    eprintln!("  grid on the offbeat relative to the kicks: {offbeat}/{}", reports.len());
}

// ---------------------------------------------------------------------------
// self-tests for the judge itself
// ---------------------------------------------------------------------------

#[cfg(test)]
mod judge_tests {
    use super::*;

    /// A percussive fixture with a programmable tempo curve: a kick every
    /// beat, a hat on every offbeat, at whatever tempo `bpm_at` says.
    fn fixture(rate: u32, seconds: f64, bpm_at: impl Fn(f64) -> f64) -> (TrackPcm, Vec<f64>) {
        let len = (rate as f64 * seconds) as usize;
        let mut frames = vec![[0i16; 2]; len];
        let mut beats = Vec::new();
        let mut at = 0.25f64;
        let mut index = 0usize;
        while at < seconds {
            beats.push(at);
            let period = 60.0 / bpm_at(at);
            for (offset, low, gain) in
                [(0.0f64, true, 1.0f64), (period * 0.5, false, 0.45)]
            {
                let start = ((at + offset) * rate as f64) as usize;
                for sample in 0..(rate as f64 * 0.07) as usize {
                    if start + sample >= len {
                        break;
                    }
                    let time = sample as f64 / rate as f64;
                    let value = if low {
                        (-38.0 * time).exp()
                            * (2.0 * std::f64::consts::PI * 55.0 * time).sin()
                    } else {
                        (-200.0 * time).exp()
                            * (2.0 * std::f64::consts::PI * 6_500.0 * time).sin()
                    };
                    let sample_value = (gain * value * 18_000.0) as i16;
                    frames[start + sample] = [
                        frames[start + sample][0].saturating_add(sample_value),
                        frames[start + sample][1].saturating_add(sample_value),
                    ];
                }
            }
            at += period;
            index += 1;
        }
        let _ = index;
        (TrackPcm { frames, sample_rate: rate }, beats)
    }

    /// Render a list of `(time, gain, hz, decay)` hits.
    fn render(rate: u32, seconds: f64, hits: &[(f64, f64, f64, f64)]) -> TrackPcm {
        let len = (rate as f64 * seconds) as usize;
        let mut frames = vec![[0i16; 2]; len];
        for (at, gain, hz, decay) in hits {
            let start = (at * rate as f64) as usize;
            for index in 0..(rate as f64 * 0.07) as usize {
                if start + index >= len {
                    break;
                }
                let time = index as f64 / rate as f64;
                let value = gain
                    * (-decay * time).exp()
                    * (2.0 * std::f64::consts::PI * hz * time).sin();
                let sample = (value * 18_000.0) as i16;
                frames[start + index] = [
                    frames[start + index][0].saturating_add(sample),
                    frames[start + index][1].saturating_add(sample),
                ];
            }
        }
        TrackPcm { frames, sample_rate: rate }
    }

    const KICK: (f64, f64) = (55.0, 38.0);
    const HAT: (f64, f64) = (7_000.0, 220.0);

    /// The three cases a DJ app actually has to survive, each with its beats
    /// known exactly, so what the fixed grid does on them is a measurement
    /// and not an opinion.
    #[test]
    fn hard_cases() {
        let rate = 44_100u32;

        // 1. A breakdown: thirty-two seconds with no kick at all, in the
        //    middle of a steady record. The grid has to come out the other
        //    side still on the beat.
        {
            let (bpm, seconds) = (128.0f64, 240.0);
            let period = 60.0 / bpm;
            let (mut hits, mut beats) = (Vec::new(), Vec::new());
            let mut at = 0.3;
            while at < seconds {
                beats.push(at);
                let quiet = (100.0..132.0).contains(&at);
                if !quiet {
                    hits.push((at, 1.0, KICK.0, KICK.1));
                }
                hits.push((at + period * 0.5, 0.4, HAT.0, HAT.1));
                at += period;
            }
            let pcm = render(rate, seconds, &hits);
            let grid = analyze(&pcm).grid;
            let (f, _, _) = f_measure(&grid_beats(&grid, seconds), &beats, 0.070);
            eprintln!("breakdown: {:.3} BPM, F {f:.3}", grid.bpm);
            assert!((grid.bpm - bpm).abs() < 0.2, "breakdown bpm {:.3}", grid.bpm);
            assert!(f > 0.9, "the grid does not survive a breakdown: F {f:.3}");
        }

        // 2. Syncopation: the kick is NOT on every beat, which is the
        //    assumption the low-band phase rule leans on. Kicks on 1, the
        //    "and" of 2, and 4 — nothing on beat 3 at all — over the
        //    backbeat clap that every record of this kind has.
        //
        //    The clap is not decoration, it is the case being tested. Take
        //    it away and the strongest periodicity left is the beat and a
        //    half between the kicks: the analysis then publishes 82.67 BPM
        //    for a 124 BPM pattern — exactly two thirds — and so, quite
        //    independently, does the streaming detector. That is a genuine
        //    limitation of an autocorrelation whose tie-break knows only
        //    octaves, and it is recorded in `estimate_grid`; what it is not
        //    is a case this library contains, because a record with no
        //    backbeat and no kick on two beats of four is not a house
        //    record.
        {
            let (bpm, seconds) = (124.0f64, 200.0);
            let period = 60.0 / bpm;
            let (mut hits, mut beats) = (Vec::new(), Vec::new());
            let mut beat = 0usize;
            loop {
                let at = 0.25 + beat as f64 * period;
                if at >= seconds {
                    break;
                }
                beats.push(at);
                // Kicks on 1, 2 and 4 — the hole is beat 3 — with a bass
                // stab on the "and" of 3 filling it, and the backbeat clap
                // on 2 and 4.
                if beat % 4 != 2 {
                    hits.push((at, 1.0, KICK.0, KICK.1));
                } else {
                    hits.push((at + period * 0.5, 0.8, 80.0, 30.0));
                }
                if beat % 2 == 1 {
                    hits.push((at, 0.7, 1_800.0, 90.0));
                }
                hits.push((at + period * 0.5, 0.35, HAT.0, HAT.1));
                beat += 1;
            }
            let pcm = render(rate, seconds, &hits);
            let grid = analyze(&pcm).grid;
            let (f, _, _) = f_measure(&grid_beats(&grid, seconds), &beats, 0.070);
            let relation = octave_class(grid.bpm, bpm);
            eprintln!(
                "syncopated: {:.3} BPM for a {bpm} pattern ({relation}), F {f:.3}",
                grid.bpm
            );
            // What IS guaranteed is that the tempo is a metrical relative of
            // the real one — never an unrelated number — so the grid is
            // always something a listener could hold, and a half- or
            // double-time reading is recoverable by ear or by the operator.
            // What is NOT guaranteed, and this fixture is here to keep
            // honest, is the metrical LEVEL: the published tempo here is two
            // thirds of the pattern's, because the autocorrelation of a
            // half-beat hat grid peaks as hard at a beat and a half as at a
            // beat, and the tie-break knows only octaves. Tighten this
            // assertion the day that is fixed.
            assert!(
                relation != "other",
                "syncopation produced {:.3} BPM, which is not a metrical relative of \
                 {bpm} — that is a different and worse failure than the known one",
                grid.bpm
            );
        }

        // 3. A DJ transition: ninety seconds at 124, a thirty-second ride up
        //    to 130, ninety seconds at 130. One fixed grid CANNOT be right
        //    across this, and the measurement says how wrong — which is the
        //    number that decides whether the offline path needs a
        //    tempo-varying grid at all.
        {
            let seconds = 210.0;
            let bpm_at = |t: f64| {
                if t < 90.0 {
                    124.0
                } else if t < 120.0 {
                    124.0 + 6.0 * (t - 90.0) / 30.0
                } else {
                    130.0
                }
            };
            let (mut hits, mut beats) = (Vec::new(), Vec::new());
            let mut at = 0.25;
            while at < seconds {
                beats.push(at);
                let period = 60.0 / bpm_at(at);
                hits.push((at, 1.0, KICK.0, KICK.1));
                hits.push((at + period * 0.5, 0.4, HAT.0, HAT.1));
                at += period;
            }
            let pcm = render(rate, seconds, &hits);
            let grid = analyze(&pcm).grid;
            let ours = grid_beats(&grid, seconds);
            let (f, _, _) = f_measure(&ours, &beats, 0.070);
            let section = |from: f64, to: f64| {
                let want: Vec<f64> =
                    beats.iter().copied().filter(|b| *b >= from && *b < to).collect();
                let got: Vec<f64> =
                    ours.iter().copied().filter(|b| *b >= from && *b < to).collect();
                f_measure(&got, &want, 0.070).0
            };
            eprintln!(
                "dj transition: {:.3} BPM, F {f:.3} overall — before {:.3}, \
                 during {:.3}, after {:.3}",
                grid.bpm,
                section(0.0, 90.0),
                section(90.0, 120.0),
                section(120.0, seconds),
            );
            // A fixed grid gets at most one side of a transition. What must
            // not happen is that it gets NEITHER — a tempo averaged across
            // the ride, fitting nothing.
            let best = section(0.0, 90.0).max(section(120.0, seconds));
            assert!(
                best > 0.8,
                "the grid fits neither side of a DJ transition (before {:.3}, \
                 after {:.3}); it has averaged the ride",
                section(0.0, 90.0),
                section(120.0, seconds),
            );
        }
    }

    /// A human drummer: the tempo wanders, nobody plays four-to-the-floor,
    /// and the kick is not on every beat.
    ///
    /// This is the case the app's model is NOT built for, and the point of
    /// the fixture is to say by how much rather than to argue about it. The
    /// tempo takes a slow random walk of about ±1.5 % — less drift than a
    /// real band, more than any sequencer — over a kick/snare/hat pattern
    /// with a fill every eight bars.
    #[test]
    fn a_human_drummer_defeats_one_straight_line() {
        let rate = 44_100u32;
        let (nominal, seconds) = (116.0f64, 240.0);
        // A deterministic wander: three slow sinusoids, no two commensurate.
        let bpm_at = |t: f64| {
            nominal
                * (1.0
                    + 0.010 * (t / 37.0).sin()
                    + 0.005 * (t / 17.3).sin()
                    + 0.003 * (t / 7.1).sin())
        };
        let (mut hits, mut beats) = (Vec::new(), Vec::new());
        let mut at = 0.4f64;
        let mut beat = 0usize;
        while at < seconds {
            beats.push(at);
            let period = 60.0 / bpm_at(at);
            match beat % 4 {
                0 => hits.push((at, 1.0, 62.0, 34.0)),
                2 => {
                    hits.push((at, 0.85, 62.0, 34.0));
                    hits.push((at, 0.8, 1_600.0, 70.0));
                }
                _ => hits.push((at, 0.9, 1_600.0, 70.0)),
            }
            hits.push((at + period * 0.5, 0.30, 8_000.0, 260.0));
            hits.push((at, 0.22, 8_000.0, 260.0));
            // A fill across the last bar of every eight.
            if (beat / 4) % 8 == 7 {
                for step in 1..4 {
                    hits.push((
                        at + period * step as f64 / 4.0,
                        0.55,
                        320.0 + 90.0 * step as f64,
                        60.0,
                    ));
                }
            }
            at += period;
            beat += 1;
        }
        let pcm = render(rate, seconds, &hits);
        let grid = analyze(&pcm).grid;
        let ours = grid_beats(&grid, seconds);
        let (fixed_f, _, _) = f_measure(&ours, &beats, 0.070);

        // What a free tracker gets on the same audio.
        let front = onset_front(&pcm);
        let reference = kick_reference(&pcm, &front);
        let (free_f, _, _) = f_measure(&reference.beats, &beats, 0.070);

        // And what the SAME least-squares fit gets when it is allowed to
        // bend once every four bars.
        let (fine, fine_hop) = fine_onset_envelope(&pcm);
        let detected = pick_peaks_strength(&front.flux, front.hop_secs, 1.2, 0.030);
        let times: Vec<f64> = detected.iter().map(|(t, _)| *t).collect();
        let mut onsets: Vec<(f64, f32)> = snap_to_fine(&times, &fine, fine_hop, 0.035)
            .into_iter()
            .zip(detected.iter().map(|(_, s)| *s))
            .collect();
        onsets.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let piecewise = piecewise_fit(
            &onsets,
            seconds,
            grid.beat_secs,
            grid.first_beat_secs.rem_euclid(grid.beat_secs),
            16,
        );
        let (piecewise_f, _, _) = f_measure(&piecewise.beats, &beats, 0.070);

        let drift = (0..240)
            .map(|t| bpm_at(t as f64))
            .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        eprintln!(
            "human drummer: tempo rides {:.2}..{:.2} BPM ({:.1} % of a beat per bar at \
             the worst); published grid {:.3} BPM scores F {fixed_f:.3}, a free tracker \
             scores F {free_f:.3}, the same fit in sixteen-beat segments scores F \
             {piecewise_f:.3} over {} segments",
            drift.0,
            drift.1,
            100.0 * (drift.1 - drift.0) / nominal,
            grid.bpm,
            piecewise.segments.len(),
        );
        assert!(
            free_f > fixed_f + 0.1,
            "the fixture does not actually defeat a fixed grid (fixed {fixed_f:.3}, \
             free {free_f:.3}); it is not drifting enough to be evidence"
        );
        // This says a bending grid CAN hold a drifting tempo. It does not
        // say that fitting one segment at a time is how to get there, and
        // the same prototype on real records says it is not: over ABBA's
        // isolated drums — a drifting band, clean evidence, 98 % coverage —
        // one straight line scores 0.858 and these same sixteen-beat
        // segments score 0.266. Sixteen beats is too few to pin a tempo
        // against real onsets, so each segment slides and the slide rides
        // forward. A tempo map has to come from decoding a beat SEQUENCE
        // under a tempo-continuity model and summarizing that, which is what
        // the reference tracker above does and what it scores 0.998 here
        // doing.
        assert!(
            piecewise_f > fixed_f + 0.1,
            "a bending grid does not recover a drifting tempo even on clean \
             evidence (fixed {fixed_f:.3}, piecewise {piecewise_f:.3}) — the tempo-map \
             recommendation rests on this and would be wrong"
        );
    }

    /// The reference tracker has to be free enough to follow a tempo that
    /// moves, or it is just another fixed grid and cannot be used to ask
    /// whether a fixed grid is enough. A DJ ramping one record into another
    /// is the case that matters, so that is the fixture: 124 BPM to 130 over
    /// three minutes, which is a larger ride than any real transition.
    ///
    /// This is what bounds the tightness constant from above. Tighter is a
    /// better judge of steady music right up until it stops being able to
    /// see this, and this test is where that line is.
    #[test]
    fn the_reference_follows_a_tempo_ramp() {
        let seconds = 180.0;
        let (pcm, beats) =
            fixture(44_100, seconds, |at| 124.0 + 6.0 * (at / seconds).min(1.0));
        let front = onset_front(&pcm);
        let reference = kick_reference(&pcm, &front);
        let (f, precision, recall) = f_measure(&reference.beats, &beats, 0.070);
        eprintln!(
            "ramp: reference {} beats vs {} true, F {f:.3} (P {precision:.3} R {recall:.3}), \
             tightness {}",
            reference.beats.len(),
            beats.len(),
            dp_tightness(),
        );
        assert!(
            f > 0.95,
            "the reference cannot follow a 124->130 BPM ramp (F {f:.3}); it is too tight \
             to judge whether a fixed grid is enough"
        );
        // …and a fixed grid demonstrably cannot, which is the point of
        // having a free reference at all.
        let grid = analyze(&pcm).grid;
        let (fixed, _, _) = f_measure(&grid_beats(&grid, seconds), &beats, 0.070);
        eprintln!("ramp: the fixed grid scores F {fixed:.3} at {:.2} BPM", grid.bpm);
        assert!(
            fixed < f,
            "a fixed grid matched a ramp as well as the free tracker did; the fixture \
             is not ramping"
        );
    }

    #[test]
    fn f_measure_matches_its_definition() {
        let reference = vec![0.0, 0.5, 1.0, 1.5];
        assert!((f_measure(&reference, &reference, 0.07).0 - 1.0).abs() < 1e-9);
        // Everything a beat late: nothing matches.
        let late: Vec<f64> = reference.iter().map(|v| v + 0.25).collect();
        assert!(f_measure(&late, &reference, 0.07).0 < 1e-9);
        // Half the beats present: precision 1, recall 0.5, F = 2/3.
        let half = vec![0.0, 1.0];
        let (f, p, r) = f_measure(&half, &reference, 0.07);
        assert!((p - 1.0).abs() < 1e-9 && (r - 0.5).abs() < 1e-9);
        assert!((f - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn continuity_sees_the_metrical_level() {
        let reference: Vec<f64> = (0..64).map(|i| i as f64 * 0.5).collect();
        let same = reference.clone();
        let c = continuity(&same, &reference);
        assert!(c.cmlt > 0.95 && c.amlt > 0.95, "{c:?}");
        // Double time: CML fails, AML recovers.
        let double: Vec<f64> = (0..128).map(|i| i as f64 * 0.25).collect();
        let c = continuity(&double, &reference);
        assert!(c.cmlt < 0.2, "double time should fail CML: {c:?}");
        assert!(c.amlt > 0.9, "double time should pass AML: {c:?}");
    }

    #[test]
    fn the_judge_finds_a_click_tracks_beats() {
        // The judge's whole front end, over a synthetic 128 BPM click.
        let rate = 44_100u32;
        let bpm = 128.0f64;
        let seconds = 30.0;
        let len = (rate as f64 * seconds) as usize;
        let period = 60.0 * rate as f64 / bpm;
        let mut frames = vec![[0i16; 2]; len];
        let mut position = 0.31 * rate as f64;
        while (position as usize) < len {
            let start = position as usize;
            for index in 0..(rate as f64 * 0.06) as usize {
                if start + index >= len {
                    break;
                }
                let time = index as f64 / rate as f64;
                let value = 0.9 * (-40.0 * time).exp()
                    * (2.0 * std::f64::consts::PI * 60.0 * time).sin()
                    + 0.3 * (-160.0 * time).exp()
                        * (2.0 * std::f64::consts::PI * 2_000.0 * time).sin();
                frames[start + index] = [(value * 20_000.0) as i16; 2];
            }
            position += period;
        }
        let pcm = TrackPcm { frames, sample_rate: rate };
        let front = onset_front(&pcm);
        let (fine, fine_hop) = fine_onset_envelope(&pcm);
        // The fine envelope is the judge's clock: check it against clicks
        // whose times are known exactly before trusting anything measured
        // in milliseconds against it.
        let truth: Vec<f64> = {
            let mut out = Vec::new();
            let mut at = 0.31;
            while at < seconds {
                out.push(at);
                at += 60.0 / bpm;
            }
            out
        };
        let peaks = snap_to_fine(&truth, &fine, fine_hop, 0.030);
        let mut errors: Vec<f64> =
            peaks.iter().zip(&truth).map(|(p, t)| (p - t) * 1000.0).collect();
        errors.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let clock_bias = errors[errors.len() / 2];
        eprintln!("fine-envelope clock bias {clock_bias:+.1} ms");
        assert!(
            clock_bias.abs() < 5.0,
            "the judge's own clock is {clock_bias:+.1} ms off the clicks"
        );
        let reference = kick_reference(&pcm, &front);
        let kicks = reference.beats.clone();
        assert!(
            (reference.bpm - bpm).abs() < 1.0,
            "judge kick tempo {:.2} for a {bpm} click",
            reference.bpm
        );
        // Coverage is a fraction of the whole file, and the file has a
        // little silence at each end that no chain can cover.
        assert!(reference.coverage > 0.90, "coverage {}", reference.coverage);
        assert!(
            kicks.len() > (30.0 * bpm / 60.0 * 0.95) as usize,
            "the chain found {} of about {} clicks",
            kicks.len(),
            (30.0 * bpm / 60.0) as usize
        );
        let reference_bpm = reference_tempo(&front.flux, front.hop_secs, 70.0, 180.0);
        assert!(
            (reference_bpm - bpm).abs() < 3.0 || (reference_bpm - bpm / 2.0).abs() < 3.0,
            "judge reference tempo {reference_bpm:.2}"
        );
        // And the DP tracker lands on the clicks.
        let detected = pick_peaks_strength(&front.flux, front.hop_secs, 1.2, 0.030);
        let times: Vec<f64> = detected.iter().map(|(at, _)| *at).collect();
        let onsets: Vec<(f64, f32)> = snap_to_fine(&times, &fine, fine_hop, 0.035)
            .into_iter()
            .zip(detected.iter().map(|(_, s)| *s))
            .collect();
        let activation = impulse_activation(&onsets, JUDGE_HOP, seconds, 0.015);
        let decoded = ellis_dp(&activation, JUDGE_HOP, 60.0 / bpm, dp_tightness());
        let beats = snap_to_fine(&decoded, &fine, fine_hop, 0.025);
        let mut steps: Vec<f64> = beats.windows(2).map(|p| p[1] - p[0]).collect();
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!(
            "dp: {} beats, ref tempo {reference_bpm:.2}, step median {:.4} (want {:.4}), \
             min {:.4} max {:.4}; kicks {} beats",
            beats.len(),
            steps.get(steps.len() / 2).copied().unwrap_or(0.0),
            60.0 / bpm,
            steps.first().copied().unwrap_or(0.0),
            steps.last().copied().unwrap_or(0.0),
            kicks.len(),
        );
        // Both trackers are measured against the clicks THEMSELVES, whose
        // times are known exactly, rather than against each other — two
        // trackers can agree and both be wrong, and on a fixture there is no
        // reason to accept that.
        // Beats the tracker extrapolates into the silence before the first
        // click or after the last are not wrong, so they are not counted.
        let (first, last) = (truth[0], truth[truth.len() - 1]);
        let worst_against_truth = |beats: &[f64]| -> f64 {
            beats
                .iter()
                .filter(|beat| **beat >= first - 0.001 && **beat <= last + 0.001)
                .map(|beat| {
                    truth
                        .iter()
                        .map(|click| (click - beat).abs())
                        .fold(f64::INFINITY, f64::min)
                })
                .fold(0.0f64, f64::max)
        };
        let broadband = worst_against_truth(&beats);
        let low_band = worst_against_truth(&kicks);
        eprintln!(
            "worst distance from a click: broadband reference {:.0} ms, \
             kick reference {:.0} ms",
            broadband * 1000.0,
            low_band * 1000.0,
        );
        assert!(
            low_band < 0.015,
            "the kick reference wanders {:.0} ms off a click track",
            low_band * 1000.0
        );
        assert!(
            broadband < 0.015,
            "the broadband reference wanders {:.0} ms off a click track",
            broadband * 1000.0
        );
        // Not 1.0: the trimmed chain head costs a couple of beats of recall
        // out of the sixty in a thirty-second fixture.
        let (f, _, _) = f_measure(&kicks, &truth, 0.070);
        assert!(f > 0.96, "kick reference vs the clicks F = {f:.3}");
        // …and so does the real analysis.
        let grid = analyze(&pcm).grid;
        let ours = grid_beats(&grid, seconds);
        let (f, _, _) = f_measure(&ours, &truth, 0.070);
        assert!(f > 0.9, "analysis vs the clicks F = {f:.3} on a click track");
    }
}
