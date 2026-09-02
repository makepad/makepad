//! Small, deterministic DSP transcribers for one loop-splat cell.
//!
//! Drums: log-mel front end (64 bands, 2048/256), superflux onsets, a
//! semi-supervised KL-NMF with analytic kick/snare/hat/tom/crash/ride
//! templates that adapt to the loop within a bounded drift plus two free
//! components for bleed, per-class peak picking snapped to the full-band
//! onset, then physical plausibility rules on the onset transient and its
//! tail (what still rings, measured away from other hits). Pitched lines:
//! YIN. The `#[ignore]`d tests render a real stem's transcription back
//! through the kit and time a four-bar loop.

use makepad_ai_stems::stft::Stft;
use makepad_score_view::build::{DrumHit, DrumVoice, PitchedNote};
use std::cmp::Ordering;

const WINDOW: usize = 2048;
const HOP: usize = 256;
const MEL_BANDS: usize = 64;
const DRUM_COMPONENTS: usize = 6;
const NMF_COMPONENTS: usize = 8;
const NMF_ITERATIONS: usize = 40;
const NMF_EPSILON: f32 = 1.0e-8;
const ONSET_MEDIAN_SECS: f64 = 0.35;
const CLASS_GAP_SECS: f64 = 0.045;
const MERGE_SECS: f64 = 0.030;
const SNAP_SECS: f64 = 0.012;
const ACTIVATION_BACKTRACK_SECS: f64 = 0.060;
/// A class onset must at least double the activation that was sounding
/// 17-58 ms earlier (peak >= 2x the preceding level); a crash's 4-6 Hz shimmer
/// swings by ~25 %, stick hits in the acceptance patterns land at 0.6-1.0.
const RISE_MIN: f32 = 0.5;
/// Adapted templates may not drift further than this factor from their
/// analytic prior in any band, so a lone crash cannot turn the hat template
/// into a crash template.
const TEMPLATE_DRIFT: f32 = 3.0;
const YIN_MIN_HZ: f64 = 40.0;
const YIN_MAX_HZ: f64 = 400.0;
const YIN_APERIODICITY: f64 = 0.15;

#[derive(Clone, Copy, Debug)]
pub struct LoopClock {
    pub bpm: f64,
    pub bars: u32,
    pub beats_per_bar: u32,
}

#[derive(Clone, Copy)]
struct Onset {
    frame: usize,
    strength: f32,
    level: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DrumClass {
    Kick,
    Snare,
    Hat,
    Tom,
    Crash,
    Ride,
}

impl DrumClass {
    const ALL: [Self; DRUM_COMPONENTS] = [
        Self::Kick,
        Self::Snare,
        Self::Hat,
        Self::Tom,
        Self::Crash,
        Self::Ride,
    ];
    const CYMBALS: [usize; 2] = [4, 5];

    /// The shortest interval at which the instrument can be struck again;
    /// activation peaks closer than this are one hit (the kick's pitch sweep
    /// makes its activation peak ~60 ms after the stick).
    fn merge_seconds(self) -> f64 {
        match self {
            Self::Kick | Self::Tom | Self::Crash | Self::Ride => 0.070,
            Self::Snare | Self::Hat => MERGE_SECS,
        }
    }
}

struct MelSpectrogram {
    /// Linear magnitude, mel-major: `mel * frames + frame`.
    magnitude: Vec<f32>,
    log_magnitude: Vec<f32>,
    centers_hz: [f32; MEL_BANDS],
    frames: usize,
    /// Frames within -10..+60 ms of any full-band onset: another hit is
    /// sounding there, so they say nothing about the tail of this one.
    onset_mask: Vec<bool>,
}

/// Transcribe percussion using a loop-adapted, semi-supervised KL-NMF.
pub fn transcribe_drums(mono: &[f32], sample_rate: u32, clock: &LoopClock) -> Vec<DrumHit> {
    if mono.len() < 2 || sample_rate == 0 || !clock.bpm.is_finite() || clock.bpm <= 0.0 {
        return Vec::new();
    }
    let stft = Stft::new(WINDOW, HOP, WINDOW);
    let (spectrum, frames) = stft.forward(mono);
    if frames < 2 {
        return Vec::new();
    }
    let mut mel = make_mel_spectrogram(&spectrum, frames, sample_rate, stft.bins());
    let peak_magnitude = mel.magnitude.iter().copied().fold(0.0f32, f32::max);
    if peak_magnitude <= 1.0e-7 {
        return Vec::new();
    }

    let full_flux = superflux(&mel);
    let mut scratch = Vec::new();
    let mut full_onsets = pick_peaks(
        &full_flux,
        None,
        sample_rate,
        0.020,
        0.020,
        &mut scratch,
    );
    let broadband_onsets = full_onsets.clone();
    let high_flux = superflux_range(&mel, 6_000.0, 16_001.0);
    let high_onsets = pick_peaks(
        &high_flux,
        None,
        sample_rate,
        CLASS_GAP_SECS,
        0.015,
        &mut scratch,
    );
    full_onsets.extend(high_onsets.iter().copied());
    let cymbal_flux = superflux_range(&mel, 2_000.0, 6_000.0);
    let cymbal_onsets = pick_peaks(
        &cymbal_flux,
        None,
        sample_rate,
        CLASS_GAP_SECS,
        0.010,
        &mut scratch,
    );
    full_onsets.extend(cymbal_onsets.iter().copied());
    merge_onsets(&mut full_onsets, sample_rate, 0.040, MergeAttack::EarliestStrong);
    mel.mask_onsets(&full_onsets, sample_rate);
    let (_templates, activations) = factorize_drums(&mel);
    // Crash and ride templates trade a cymbal's energy back and forth as its
    // spectrum shifts; onsets in either class are judged on their combined row.
    let cymbal_row: Vec<f32> = (0..frames)
        .map(|frame| {
            DrumClass::CYMBALS
                .iter()
                .map(|&component| activations[component * frames + frame])
                .fold(0.0f32, f32::max)
        })
        .collect();
    let snap_frames = ((SNAP_SECS * sample_rate as f64) / HOP as f64).floor().max(1.0) as usize;
    let backtrack_frames =
        ((ACTIVATION_BACKTRACK_SECS * sample_rate as f64) / HOP as f64).round() as usize;
    let mut class_onsets: [Vec<Onset>; DRUM_COMPONENTS] = std::array::from_fn(|component| {
        let row = &activations[component * frames..(component + 1) * frames];
        let rise_row: &[f32] = if DrumClass::CYMBALS.contains(&component) { &cymbal_row } else { row };
        let differences = positive_difference(row);
        let floor_ratio = match DrumClass::ALL[component] {
            DrumClass::Kick => 0.040,
            DrumClass::Snare => 0.020,
            DrumClass::Hat => 0.008,
            DrumClass::Tom => 0.020,
            DrumClass::Crash | DrumClass::Ride => 0.0005,
        };
        let onsets = pick_peaks(
            &differences,
            Some(row),
            sample_rate,
            CLASS_GAP_SECS,
            floor_ratio,
            &mut scratch,
        );
        let mut onsets: Vec<Onset> = onsets
            .into_iter()
            .filter_map(|mut onset| {
                onset.frame = backtrack_activation(row, onset.frame, sample_rate);
                let full = nearest_onset(onset.frame, &full_onsets, snap_frames)
                    .or_else(|| nearest_onset(onset.frame, &full_onsets, backtrack_frames))?;
                onset.frame = full.frame;
                (relative_rise(rise_row, onset.frame, sample_rate) >= RISE_MIN).then_some(onset)
            })
            .collect();
        merge_onsets(
            &mut onsets,
            sample_rate,
            DrumClass::ALL[component].merge_seconds(),
            MergeAttack::Earliest,
        );
        onsets
    });
    add_spectral_anchors(
        &mut class_onsets[2],
        &high_onsets,
        &activations[2 * frames..3 * frames],
        None,
        sample_rate,
        None,
        0.012,
        0.0,
    );
    for component in DrumClass::CYMBALS {
        let row = &activations[component * frames..(component + 1) * frames];
        add_spectral_anchors(
            &mut class_onsets[component],
            &broadband_onsets,
            row,
            Some(&cymbal_row),
            sample_rate,
            None,
            0.003,
            0.005,
        );
        add_spectral_anchors(
            &mut class_onsets[component],
            &cymbal_onsets,
            row,
            Some(&cymbal_row),
            sample_rate,
            Some(&broadband_onsets),
            0.003,
            0.030,
        );
    }
    suppress_implausible_onsets(&mut class_onsets, &mel, &activations, sample_rate);
    let hat_spacing = suppress_implausible_hats(&mut class_onsets, &mel, sample_rate);
    suppress_snare_hat_bleed(&mut class_onsets, &activations, &mel, frames, hat_spacing, sample_rate);
    suppress_cymbals_on_snare(&mut class_onsets, sample_rate);
    suppress_hats_under_cymbals(&mut class_onsets, &mel, hat_spacing, sample_rate);

    let mut cymbals: Vec<Onset> = class_onsets[4].iter().chain(&class_onsets[5]).copied().collect();
    for onset in &mut cymbals {
        onset.level = cymbal_level(&activations, onset.frame, frames);
    }
    merge_onsets(&mut cymbals, sample_rate, MERGE_SECS, MergeAttack::Earliest);
    let groups: [(&[Onset], DrumClass); 5] = [
        (&class_onsets[0], DrumClass::Kick),
        (&class_onsets[1], DrumClass::Snare),
        (&class_onsets[2], DrumClass::Hat),
        (&class_onsets[3], DrumClass::Tom),
        (&cymbals, DrumClass::Crash),
    ];
    let mut hits = Vec::new();
    let total_beats = f64::from(clock.bars) * f64::from(clock.beats_per_bar);
    for (onsets, class) in groups {
        let reference = percentile(
            &mut onsets.iter().map(|onset| onset.level).collect::<Vec<_>>(),
            0.95,
        )
        .max(NMF_EPSILON);
        for onset in onsets {
            let voice = match class {
                DrumClass::Kick => DrumVoice::Kick,
                DrumClass::Snare => DrumVoice::Snare,
                DrumClass::Hat => {
                    classify_hat(onset, &activations[2 * frames..3 * frames], sample_rate)
                }
                DrumClass::Tom => classify_tom(onset.frame, &mel, sample_rate),
                DrumClass::Crash | DrumClass::Ride => classify_cymbal(onset, &mel, sample_rate),
            };
            let velocity = (0.15 + 0.85 * (onset.level / reference).min(1.0)).clamp(0.15, 1.0);
            let onset_sample = onset.frame * HOP;
            let time_beats = onset_sample as f64 / sample_rate as f64 * clock.bpm / 60.0;
            if time_beats >= total_beats {
                continue;
            }
            hits.push(DrumHit {
                time_beats,
                voice,
                velocity,
            });
        }
    }
    hits.sort_by(|a, b| {
        a.time_beats
            .partial_cmp(&b.time_beats)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.voice.gm_note().cmp(&b.voice.gm_note()))
    });
    hits
}

fn make_mel_spectrogram(
    spectrum: &[f32],
    frames: usize,
    sample_rate: u32,
    bins: usize,
) -> MelSpectrogram {
    let nyquist = sample_rate as f32 * 0.5;
    let high_hz = 16_000.0f32.min(nyquist);
    let low_mel = hz_to_mel(20.0);
    let high_mel = hz_to_mel(high_hz.max(21.0));
    let mut edges_hz = [0.0f32; MEL_BANDS + 2];
    for (index, edge) in edges_hz.iter_mut().enumerate() {
        let amount = index as f32 / (MEL_BANDS + 1) as f32;
        *edge = mel_to_hz(low_mel + (high_mel - low_mel) * amount);
    }
    let centers_hz = std::array::from_fn(|index| edges_hz[index + 1]);
    let mut magnitude = vec![0.0f32; MEL_BANDS * frames];
    let fft_scale = 2.0 / WINDOW as f32;
    for mel_band in 0..MEL_BANDS {
        let left = edges_hz[mel_band];
        let center = edges_hz[mel_band + 1];
        let right = edges_hz[mel_band + 2];
        let mut weight_sum = 0.0f32;
        for bin in 0..bins {
            let hz = bin as f32 * sample_rate as f32 / WINDOW as f32;
            let weight = if hz < left || hz > right {
                0.0
            } else if hz <= center {
                (hz - left) / (center - left).max(f32::EPSILON)
            } else {
                (right - hz) / (right - center).max(f32::EPSILON)
            };
            if weight <= 0.0 {
                continue;
            }
            weight_sum += weight;
            for frame in 0..frames {
                magnitude[mel_band * frames + frame] +=
                    weight * magnitude_at(spectrum, frames, bin, frame) * fft_scale;
            }
        }
        if weight_sum > 0.0 {
            for value in &mut magnitude[mel_band * frames..(mel_band + 1) * frames] {
                *value /= weight_sum;
            }
        }
    }
    let log_magnitude = magnitude.iter().map(|value| (1.0 + 100.0 * value).ln()).collect();
    MelSpectrogram {
        magnitude,
        log_magnitude,
        centers_hz,
        frames,
        onset_mask: vec![false; frames],
    }
}

impl MelSpectrogram {
    /// Mask the frames around every full-band onset that is a real attack:
    /// one where the total magnitude at least doubles what was sounding
    /// before it. Flux bumps in a decaying cymbal do not qualify, so they do
    /// not hide the cymbal's own tail.
    fn mask_onsets(&mut self, onsets: &[Onset], sample_rate: u32) {
        let total: Vec<f32> = (0..self.frames)
            .map(|frame| (0..MEL_BANDS).map(|band| self.magnitude[band * self.frames + frame]).sum())
            .collect();
        let frames_per_ms = sample_rate as f64 / HOP as f64 / 1000.0;
        let before = (10.0 * frames_per_ms).round() as usize;
        let after = (60.0 * frames_per_ms).round() as usize;
        for onset in onsets {
            if relative_rise(&total, onset.frame, sample_rate) < RISE_MIN {
                continue;
            }
            let start = onset.frame.saturating_sub(before);
            let end = (onset.frame + after).min(self.frames - 1);
            for masked in &mut self.onset_mask[start..=end] {
                *masked = true;
            }
        }
    }
}

#[inline]
fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

#[inline]
fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0f32.powf(mel / 2595.0) - 1.0)
}

fn superflux(mel: &MelSpectrogram) -> Vec<f32> {
    superflux_range(mel, 20.0, 16_001.0)
}

fn superflux_range(mel: &MelSpectrogram, low_hz: f32, high_hz: f32) -> Vec<f32> {
    let mut flux = vec![0.0f32; mel.frames];
    let mut selected_bands = 0usize;
    for frame in 0..mel.frames {
        let mut sum = 0.0f32;
        for band in 0..MEL_BANDS {
            if mel.centers_hz[band] < low_hz || mel.centers_hz[band] >= high_hz {
                continue;
            }
            if frame == 0 {
                selected_bands += 1;
            }
            let current = mel.log_magnitude[band * mel.frames + frame];
            let previous = if frame == 0 {
                0.0
            } else {
                let start = band.saturating_sub(1);
                let end = (band + 2).min(MEL_BANDS);
                (start..end)
                    .map(|nearby| mel.log_magnitude[nearby * mel.frames + frame - 1])
                    .fold(0.0f32, f32::max)
            };
            sum += (current - previous).max(0.0);
        }
        flux[frame] = sum;
    }
    if selected_bands != 0 {
        for value in &mut flux {
            *value /= selected_bands as f32;
        }
    }
    flux
}

fn factorize_drums(mel: &MelSpectrogram) -> (Vec<f32>, Vec<f32>) {
    let mut templates = analytic_templates(&mel.centers_hz);
    let priors = templates.clone();
    let mut activations = vec![NMF_EPSILON; NMF_COMPONENTS * mel.frames];
    for component in 0..NMF_COMPONENTS {
        let norm = (0..MEL_BANDS)
            .map(|band| templates[band * NMF_COMPONENTS + component].powi(2))
            .sum::<f32>()
            .max(NMF_EPSILON);
        for frame in 0..mel.frames {
            let projection = (0..MEL_BANDS)
                .map(|band| {
                    templates[band * NMF_COMPONENTS + component]
                        * mel.magnitude[band * mel.frames + frame]
                })
                .sum::<f32>();
            activations[component * mel.frames + frame] = (projection / norm).max(NMF_EPSILON);
        }
    }

    let mut reconstruction = vec![0.0f32; MEL_BANDS * mel.frames];
    for _ in 0..NMF_ITERATIONS {
        reconstruct(&templates, &activations, mel.frames, &mut reconstruction);
        for component in 0..NMF_COMPONENTS {
            let denominator = (0..MEL_BANDS)
                .map(|band| templates[band * NMF_COMPONENTS + component])
                .sum::<f32>()
                .max(NMF_EPSILON);
            for frame in 0..mel.frames {
                let numerator = (0..MEL_BANDS)
                    .map(|band| {
                        let index = band * mel.frames + frame;
                        templates[band * NMF_COMPONENTS + component]
                            * mel.magnitude[index]
                            / reconstruction[index].max(NMF_EPSILON)
                    })
                    .sum::<f32>();
                let at = component * mel.frames + frame;
                activations[at] =
                    (activations[at] * numerator / denominator).max(NMF_EPSILON);
            }
        }

        reconstruct(&templates, &activations, mel.frames, &mut reconstruction);
        for component in 0..NMF_COMPONENTS {
            let denominator = activations[component * mel.frames..(component + 1) * mel.frames]
                .iter()
                .copied()
                .sum::<f32>()
                .max(NMF_EPSILON);
            let learning_rate = if component < DRUM_COMPONENTS { 0.08 } else { 1.0 };
            for band in 0..MEL_BANDS {
                let numerator = (0..mel.frames)
                    .map(|frame| {
                        let index = band * mel.frames + frame;
                        activations[component * mel.frames + frame]
                            * mel.magnitude[index]
                            / reconstruction[index].max(NMF_EPSILON)
                    })
                    .sum::<f32>();
                let ratio = (numerator / denominator).clamp(0.25, 4.0);
                let at = band * NMF_COMPONENTS + component;
                templates[at] =
                    (templates[at] * ratio.powf(learning_rate)).max(NMF_EPSILON);
                if component < DRUM_COMPONENTS {
                    templates[at] = templates[at]
                        .clamp(priors[at] / TEMPLATE_DRIFT, priors[at] * TEMPLATE_DRIFT);
                }
            }
            normalize_template(&mut templates, &mut activations, component, mel.frames);
        }
    }
    (templates, activations)
}

fn analytic_templates(centers_hz: &[f32; MEL_BANDS]) -> Vec<f32> {
    let mut templates = vec![NMF_EPSILON; MEL_BANDS * NMF_COMPONENTS];
    for (band, &hz) in centers_hz.iter().enumerate() {
        let values = [
            0.90 * log_gaussian(hz, 65.0, 0.48) + 0.10 * log_gaussian(hz, 2_100.0, 0.48),
            0.43 * log_gaussian(hz, 230.0, 0.30) + 0.57 * log_gaussian(hz, 3_100.0, 0.68),
            log_gaussian(hz, 10_200.0, 0.30),
            log_gaussian(hz, 145.0, 0.62),
            log_gaussian(hz, 7_000.0, 0.70),
            log_gaussian(hz, 3_000.0, 0.50),
            log_gaussian(hz, 90.0, 0.90),
            log_gaussian(hz, 720.0, 0.90),
        ];
        for component in 0..NMF_COMPONENTS {
            templates[band * NMF_COMPONENTS + component] = values[component].max(NMF_EPSILON);
        }
    }
    for component in 0..NMF_COMPONENTS {
        let mut dummy = Vec::new();
        normalize_template(&mut templates, &mut dummy, component, 0);
    }
    templates
}

#[inline]
fn log_gaussian(hz: f32, center: f32, width: f32) -> f32 {
    (-0.5 * (hz.max(1.0) / center).ln().powi(2) / width.powi(2)).exp()
}

fn normalize_template(
    templates: &mut [f32],
    activations: &mut [f32],
    component: usize,
    frames: usize,
) {
    let sum = (0..MEL_BANDS)
        .map(|band| templates[band * NMF_COMPONENTS + component])
        .sum::<f32>()
        .max(NMF_EPSILON);
    for band in 0..MEL_BANDS {
        templates[band * NMF_COMPONENTS + component] /= sum;
    }
    if frames != 0 {
        for value in &mut activations[component * frames..(component + 1) * frames] {
            *value *= sum;
        }
    }
}

fn reconstruct(templates: &[f32], activations: &[f32], frames: usize, out: &mut [f32]) {
    for band in 0..MEL_BANDS {
        for frame in 0..frames {
            let mut value = NMF_EPSILON;
            for component in 0..NMF_COMPONENTS {
                value += templates[band * NMF_COMPONENTS + component]
                    * activations[component * frames + frame];
            }
            out[band * frames + frame] = value;
        }
    }
}

fn positive_difference(values: &[f32]) -> Vec<f32> {
    let mut differences = vec![0.0f32; values.len()];
    if let Some(first) = values.first() {
        differences[0] = *first;
    }
    for frame in 1..values.len() {
        differences[frame] = (values[frame] - values[frame - 1]).max(0.0);
    }
    differences
}

fn pick_peaks(
    novelty: &[f32],
    levels: Option<&[f32]>,
    sample_rate: u32,
    gap_secs: f64,
    floor_ratio: f32,
    scratch: &mut Vec<f32>,
) -> Vec<Onset> {
    if novelty.is_empty() {
        return Vec::new();
    }
    let maximum = novelty.iter().copied().fold(0.0f32, f32::max);
    if maximum <= NMF_EPSILON {
        return Vec::new();
    }
    let radius = ((ONSET_MEDIAN_SECS * sample_rate as f64) / HOP as f64).round() as usize;
    let gap = ((gap_secs * sample_rate as f64) / HOP as f64).ceil().max(1.0) as usize;
    let level_max = levels
        .map(|row| row.iter().copied().fold(0.0f32, f32::max))
        .unwrap_or(0.0);
    let mut onsets: Vec<Onset> = Vec::new();
    for frame in 0..novelty.len() {
        let start = frame.saturating_sub(radius);
        let end = (frame + radius + 1).min(novelty.len());
        scratch.clear();
        scratch.extend_from_slice(&novelty[start..end]);
        let threshold = median_in_place(scratch) + maximum * floor_ratio;
        let value = novelty[frame];
        let left = frame.checked_sub(1).map_or(0.0, |at| novelty[at]);
        let right = novelty.get(frame + 1).copied().unwrap_or(0.0);
        let level = levels.map_or(value, |row| local_peak(row, frame, 2));
        if value <= threshold
            || value < left
            || value < right
            || levels.is_some_and(|_| level < level_max * 0.012)
        {
            continue;
        }
        if let Some(previous) = onsets.last_mut().filter(|last| frame - last.frame < gap) {
            if value > previous.strength {
                *previous = Onset { frame, strength: value, level };
            }
        } else {
            onsets.push(Onset { frame, strength: value, level });
        }
    }
    onsets
}

fn nearest_onset(frame: usize, onsets: &[Onset], radius: usize) -> Option<Onset> {
    onsets
        .iter()
        .copied()
        .filter(|onset| onset.frame.abs_diff(frame) <= radius)
        .min_by(|a, b| {
            a.frame
                .abs_diff(frame)
                .cmp(&b.frame.abs_diff(frame))
                .then_with(|| b.strength.partial_cmp(&a.strength).unwrap_or(Ordering::Equal))
        })
}

fn backtrack_activation(values: &[f32], peak: usize, sample_rate: u32) -> usize {
    let maximum_backtrack = ((0.060 * sample_rate as f64) / HOP as f64).round() as usize;
    let floor = values[peak] * 0.25;
    let first = peak.saturating_sub(maximum_backtrack);
    for frame in (first..peak).rev() {
        if values[frame] <= floor {
            return frame + 1;
        }
    }
    first
}

/// Fraction of the activation peak right after `frame` that is new energy
/// rather than what was already sounding 17-58 ms before it. Comparing with
/// the preceding maximum means a bump shortly after a real attack scores
/// near zero instead of reading the pre-attack silence as its floor.
fn relative_rise(row: &[f32], frame: usize, sample_rate: u32) -> f32 {
    if row.is_empty() {
        return 0.0;
    }
    let (peak, before) = rise_levels(row, frame, sample_rate);
    (peak - before).max(0.0) / peak.max(NMF_EPSILON)
}

/// The activation peak within -6..+30 ms of `frame` and the highest level in
/// the 17-58 ms before it.
fn rise_levels(row: &[f32], frame: usize, sample_rate: u32) -> (f32, f32) {
    let frames_per_ms = sample_rate as f64 / HOP as f64 / 1000.0;
    let at_ms = |ms: f64| (ms * frames_per_ms).round() as usize;
    let last = row.len() - 1;
    let peak_start = frame.saturating_sub(1).min(last);
    let peak_end = (frame + at_ms(30.0)).min(last);
    let peak = row[peak_start..=peak_end].iter().copied().fold(0.0f32, f32::max);
    let before_start = frame.saturating_sub(at_ms(58.0));
    let before_end = frame.saturating_sub(at_ms(17.0)).min(last);
    let before = if before_end > before_start {
        row[before_start..before_end].iter().copied().fold(0.0f32, f32::max)
    } else {
        0.0
    };
    (peak, before)
}

fn local_peak(values: &[f32], frame: usize, radius: usize) -> f32 {
    let start = frame.saturating_sub(radius);
    let end = (frame + radius + 1).min(values.len());
    values[start..end].iter().copied().fold(0.0f32, f32::max)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MergeAttack {
    /// The first onset of the cluster is the attack (class activations peak
    /// late: the kick's pitch sweep, a crash's bloom).
    Earliest,
    /// The first onset carrying at least half the cluster's level is the
    /// attack (a weak spectral-flux precursor at the analysis window's edge
    /// is not).
    EarliestStrong,
}

/// Collapse onsets closer than `seconds` into one hit; the loudest level of
/// the cluster is its velocity.
fn merge_onsets(onsets: &mut Vec<Onset>, sample_rate: u32, seconds: f64, attack: MergeAttack) {
    let gap = ((seconds * sample_rate as f64) / HOP as f64).ceil() as usize;
    onsets.sort_by_key(|onset| onset.frame);
    let mut merged: Vec<Onset> = Vec::with_capacity(onsets.len());
    let mut cluster: Vec<Onset> = Vec::new();
    let flush = |cluster: &mut Vec<Onset>, merged: &mut Vec<Onset>| {
        if cluster.is_empty() {
            return;
        }
        let level = cluster.iter().map(|onset| onset.level).fold(0.0f32, f32::max);
        let strength = cluster.iter().map(|onset| onset.strength).fold(0.0f32, f32::max);
        let attack_frame = match attack {
            MergeAttack::Earliest => cluster[0].frame,
            MergeAttack::EarliestStrong => cluster
                .iter()
                .find(|onset| onset.level >= level * 0.5)
                .map_or(cluster[0].frame, |onset| onset.frame),
        };
        merged.push(Onset { frame: attack_frame, strength, level });
        cluster.clear();
    };
    for onset in onsets.drain(..) {
        if cluster.last().is_some_and(|last| onset.frame - last.frame > gap) {
            flush(&mut cluster, &mut merged);
        }
        cluster.push(onset);
    }
    flush(&mut cluster, &mut merged);
    *onsets = merged;
}

#[allow(clippy::too_many_arguments)]
fn add_spectral_anchors(
    class_onsets: &mut Vec<Onset>,
    spectral_onsets: &[Onset],
    activation: &[f32],
    rise_row: Option<&[f32]>,
    sample_rate: u32,
    snap_back: Option<&[Onset]>,
    minimum_level_ratio: f32,
    minimum_rise_ratio: f32,
) {
    let peak = activation.iter().copied().fold(0.0f32, f32::max);
    let differences = positive_difference(activation);
    let peak_rise = differences.iter().copied().fold(0.0f32, f32::max);
    let maximum_backtrack = ((0.250 * sample_rate as f64) / HOP as f64).round() as usize;
    for spectral in spectral_onsets {
        let level = local_peak(activation, spectral.frame, 3);
        let rise = local_peak(&differences, spectral.frame, 3);
        if level >= peak * minimum_level_ratio
            && rise >= peak_rise * minimum_rise_ratio
            && relative_rise(rise_row.unwrap_or(activation), spectral.frame, sample_rate)
                >= RISE_MIN
        {
            let frame = snap_back
                .and_then(|anchors| {
                    anchors
                        .iter()
                        .filter(|anchor| {
                            anchor.frame <= spectral.frame
                                && spectral.frame - anchor.frame <= maximum_backtrack
                        })
                        .max_by_key(|anchor| anchor.frame)
                        .map(|anchor| anchor.frame)
                })
                .unwrap_or(spectral.frame);
            class_onsets.push(Onset {
                frame,
                strength: rise.max(spectral.strength * NMF_EPSILON),
                level,
            });
        }
    }
    merge_onsets(class_onsets, sample_rate, CLASS_GAP_SECS, MergeAttack::Earliest);
}

fn suppress_implausible_onsets(
    onsets: &mut [Vec<Onset>; DRUM_COMPONENTS],
    mel: &MelSpectrogram,
    activations: &[f32],
    sample_rate: u32,
) {
    for component in 0..DRUM_COMPONENTS {
        let class = DrumClass::ALL[component];
        if class == DrumClass::Hat {
            continue;
        }
        onsets[component].retain(|onset| {
            let frame = onset.frame.min(mel.frames - 1);
            let shares = TransientShares::at(mel, frame, sample_rate);
            let tail_peak = mel_transient_peak_hz(mel, frame, 0.120, sample_rate, 35.0, 300.0);
            let own = onset.level;
            let spectral_match = match class {
                DrumClass::Kick => shares.low + shares.body >= 0.25 && tail_peak < 75.0,
                DrumClass::Snare => shares.body >= 0.10 && shares.wires >= 0.10 && shares.high < 0.55,
                DrumClass::Hat => unreachable!(),
                DrumClass::Tom => {
                    shares.low + shares.body >= 0.25
                        && shares.wires + shares.high < 0.18
                        && tail_peak >= 75.0
                }
                DrumClass::Crash | DrumClass::Ride => {
                    // What still rings 80-220 ms later must be a cymbal's
                    // body (2-6 kHz), not only an open hat's air; judged on
                    // the tail so a hat or kick struck at the same instant
                    // does not hide the crash.
                    let hat = component_level(activations, 2, frame, mel.frames);
                    let tail = |low, high| {
                        mel_tail_energy(mel, frame, sample_rate, low, high, 0.080, 0.220)
                    };
                    let sustain =
                        mel_sustain_ratio(mel, frame, sample_rate, 2_000.0, 6_000.0);
                    tail(2_000.0, 6_000.0) >= 0.5 * tail(7_000.0, 16_001.0)
                        && shares.wires >= 0.005
                        && own_ratio(own, hat) >= 0.08
                        && sustain >= 0.15
                }
            };
            let strongest = (0..DRUM_COMPONENTS)
                .map(|other| component_level(activations, other, frame, mel.frames))
                .fold(0.0f32, f32::max);
            let dominance = match class {
                DrumClass::Kick | DrumClass::Tom => 0.015,
                DrumClass::Snare => 0.05,
                DrumClass::Hat => unreachable!(),
                DrumClass::Crash | DrumClass::Ride => 0.001,
            };
            spectral_match && own >= strongest * dominance
        });
    }
}

/// Band shares of one onset's transient (each relative to the 20 Hz-16 kHz total).
struct TransientShares {
    low: f32,
    body: f32,
    wires: f32,
    high: f32,
    /// Absolute 7-16 kHz transient: what a hat puts on the table.
    air: f32,
}

impl TransientShares {
    fn at(mel: &MelSpectrogram, frame: usize, sample_rate: u32) -> Self {
        let band = |low: f32, high: f32| mel_transient_band_sum(mel, frame, sample_rate, low, high);
        let total = band(20.0, 16_001.0).max(NMF_EPSILON);
        let air = band(7_000.0, 16_001.0);
        Self {
            low: band(20.0, 170.0) / total,
            body: band(170.0, 520.0) / total,
            wires: band(1_000.0, 7_000.0) / total,
            high: band(6_000.0, 16_001.0) / total,
            air,
        }
    }

    /// A hat heard on its own: most of the transient above 6 kHz.
    fn is_hat_spectrum(&self) -> bool {
        self.high >= 0.30 && self.high >= self.wires * 1.2
    }
}

/// The hat grid of this loop: the median interval between spectrally certain
/// hats (heard on their own), halved when most of the midpoints between them
/// also carry a hat candidate (every other hat sits under a kick or snare).
/// A single midpoint candidate in an otherwise empty gap is not a grid; that
/// is what a ghost snare's wires or a ride's air look like.
fn prevailing_hat_spacing(certain: &[usize], candidates: &[usize], sample_rate: u32) -> Option<usize> {
    if certain.len() < 3 {
        return None;
    }
    let tolerance = ((0.035 * sample_rate as f64) / HOP as f64).ceil() as usize;
    let mut intervals: Vec<usize> = certain.windows(2).map(|pair| pair[1] - pair[0]).collect();
    intervals.sort_unstable();
    let spacing = intervals[intervals.len() / 2];
    let mut gaps = 0usize;
    let mut filled = 0usize;
    for pair in certain.windows(2) {
        if (pair[1] - pair[0]).abs_diff(spacing) > tolerance {
            continue;
        }
        gaps += 1;
        let midpoint = (pair[0] + pair[1]) / 2;
        if candidates.iter().any(|&frame| frame.abs_diff(midpoint) <= tolerance) {
            filled += 1;
        }
    }
    Some(if filled >= 2 && filled * 3 >= gaps * 2 { spacing / 2 } else { spacing })
}

/// Hats. A hat heard on its own (most of its transient above 6 kHz) stands.
/// A hat under a kick, snare or cymbal stands only on the loop's hat grid and
/// when the onset brings at least 80 % of a lone hat's air (a kick's click
/// alone brings about half) or is a low drum with more air than a click.
/// With no grid to lean on, only the latter passes. Returns the grid spacing
/// for the later co-onset rules.
fn suppress_implausible_hats(
    onsets: &mut [Vec<Onset>; DRUM_COMPONENTS],
    mel: &MelSpectrogram,
    sample_rate: u32,
) -> Option<usize> {
    let features: Vec<(Onset, TransientShares)> = onsets[2]
        .iter()
        .map(|onset| (*onset, TransientShares::at(mel, onset.frame.min(mel.frames - 1), sample_rate)))
        .collect();
    let certain: Vec<usize> = features
        .iter()
        .filter(|(_, shares)| shares.is_hat_spectrum())
        .map(|(onset, _)| onset.frame)
        .collect();
    let lone_air = percentile(
        &mut features
            .iter()
            .filter(|(_, shares)| shares.is_hat_spectrum())
            .map(|(_, shares)| shares.air)
            .collect::<Vec<_>>(),
        0.5,
    );
    // A low drum whose onset carries more air than a kick's click alone.
    let kick_with_air =
        |shares: &TransientShares| shares.low + shares.body >= 0.70 && shares.high >= 0.015;
    // At least 80 % of what a lone hat in this loop puts in the air band.
    let hat_worth_of_air = |shares: &TransientShares| !certain.is_empty() && shares.air >= lone_air * 0.8;
    let airy: Vec<usize> = features
        .iter()
        .filter(|(_, shares)| hat_worth_of_air(shares))
        .map(|(onset, _)| onset.frame)
        .collect();
    let spacing = prevailing_hat_spacing(&certain, &airy, sample_rate);
    if spacing.is_none() {
        // Too few lone hats to know the grid: only the spectrum can vouch for
        // a buried hat, and only a kick's can (a snare's own air is far more
        // than a hat's).
        onsets[2] = features
            .iter()
            .filter(|(_, shares)| shares.is_hat_spectrum() || kick_with_air(shares))
            .map(|(onset, _)| *onset)
            .collect();
        return None;
    }
    let brings_air = |shares: &TransientShares| hat_worth_of_air(shares) || kick_with_air(shares);
    // Grid membership is judged against the hats that survive, so a chain of
    // rejected candidates cannot vouch for one another.
    let mut accepted: Vec<Onset> = features.iter().map(|(onset, _)| *onset).collect();
    loop {
        let kept: Vec<Onset> = features
            .iter()
            .filter(|(onset, shares)| {
                shares.is_hat_spectrum()
                    || (brings_air(shares)
                        && regular_hat_neighbors(onset.frame, &accepted, spacing, sample_rate))
            })
            .map(|(onset, _)| *onset)
            .collect();
        let stable = kept.len() == accepted.len();
        accepted = kept;
        if stable {
            break;
        }
    }
    onsets[2] = accepted;
    spacing
}

fn suppress_snare_hat_bleed(
    onsets: &mut [Vec<Onset>; DRUM_COMPONENTS],
    activations: &[f32],
    mel: &MelSpectrogram,
    frames: usize,
    spacing: Option<usize>,
    sample_rate: u32,
) {
    let near = ((0.010 * sample_rate as f64) / HOP as f64).ceil() as usize;
    let radius = ((ONSET_MEDIAN_SECS * sample_rate as f64) / HOP as f64).round() as usize;
    let snares = onsets[1].clone();
    let hats = onsets[2].clone();
    let hat_row = &activations[2 * frames..3 * frames];
    onsets[2].retain(|hat| {
        if !snares.iter().any(|snare| snare.frame.abs_diff(hat.frame) <= near) {
            return true;
        }
        if regular_hat_neighbors(hat.frame, &hats, spacing, sample_rate) {
            return true;
        }
        let start = hat.frame.saturating_sub(radius);
        let end = (hat.frame + radius + 1).min(frames);
        let mut local = hat_row[start..end].to_vec();
        let high = TransientShares::at(mel, hat.frame.min(mel.frames - 1), sample_rate).high;
        hat.level >= 2.0 * median_in_place(&mut local) && high >= 0.30
    });
}

fn suppress_cymbals_on_snare(
    onsets: &mut [Vec<Onset>; DRUM_COMPONENTS],
    sample_rate: u32,
) {
    let near = ((0.010 * sample_rate as f64) / HOP as f64).ceil() as usize;
    let snares = onsets[1].clone();
    for component in DrumClass::CYMBALS {
        onsets[component].retain(|cymbal| {
            !snares
                .iter()
                .any(|snare| snare.frame.abs_diff(cymbal.frame) <= near)
        });
    }
}

/// A hat onset that coincides with a cymbal onset is the cymbal's own air
/// when the 7-16 kHz band is still ringing 150-250 ms later, unless the hat
/// sits on the hat grid (then a stick really hit both).
fn suppress_hats_under_cymbals(
    onsets: &mut [Vec<Onset>; DRUM_COMPONENTS],
    mel: &MelSpectrogram,
    spacing: Option<usize>,
    sample_rate: u32,
) {
    let near = ((0.015 * sample_rate as f64) / HOP as f64).ceil() as usize;
    let cymbals: Vec<Onset> = onsets[4].iter().chain(&onsets[5]).copied().collect();
    let hats = onsets[2].clone();
    onsets[2].retain(|hat| {
        if !cymbals.iter().any(|cymbal| cymbal.frame.abs_diff(hat.frame) <= near) {
            return true;
        }
        if regular_hat_neighbors(hat.frame, &hats, spacing, sample_rate) {
            return true;
        }
        let air = mel_sustain_ratio_between(
            mel,
            hat.frame,
            sample_rate,
            7_000.0,
            16_001.0,
            0.150,
            0.250,
        );
        air < 0.25
    });
}

/// Does this hat sit on the loop's hat grid: its distance to the previous and
/// the next hat is the prevailing spacing or twice it (a rest or a missed hat
/// between). A hat halfway between two grid hats is not on the grid; that is
/// where ghost snares and rides bleed into the hat band. Without a known
/// grid, equal spacing to both neighbours (up to a quarter note at 100 bpm)
/// counts.
fn regular_hat_neighbors(
    frame: usize,
    hats: &[Onset],
    spacing: Option<usize>,
    sample_rate: u32,
) -> bool {
    let tolerance = ((0.035 * sample_rate as f64) / HOP as f64).ceil() as usize;
    let maximum_step = ((0.600 * sample_rate as f64) / HOP as f64).ceil() as usize;
    let Some(index) = hats.iter().position(|hat| hat.frame == frame) else {
        return false;
    };
    let on_grid = |step: usize| match spacing {
        Some(spacing) => step.abs_diff(spacing) <= tolerance || step.abs_diff(2 * spacing) <= tolerance,
        None => step <= maximum_step,
    };
    let matches = |a: usize, b: usize| {
        on_grid(a) && on_grid(b) && (spacing.is_some() || a.abs_diff(b) <= tolerance)
    };
    if index > 0 && index + 1 < hats.len() {
        return matches(
            frame - hats[index - 1].frame,
            hats[index + 1].frame - frame,
        );
    }
    if index >= 2 {
        return matches(
            frame - hats[index - 1].frame,
            hats[index - 1].frame - hats[index - 2].frame,
        );
    }
    if index + 2 < hats.len() {
        return matches(
            hats[index + 1].frame - frame,
            hats[index + 2].frame - hats[index + 1].frame,
        );
    }
    false
}

/// Open when the hat activation still holds more than a quarter of the hit's
/// own rise 60-110 ms later. The 25th percentile over that window ignores
/// the next 16th-note hat; measuring above the pre-onset level ignores a
/// cymbal ringing underneath.
fn classify_hat(onset: &Onset, activation: &[f32], sample_rate: u32) -> DrumVoice {
    let frames_per_ms = sample_rate as f64 / HOP as f64 / 1000.0;
    let last = activation.len() - 1;
    let start = (onset.frame + (60.0 * frames_per_ms).round() as usize).min(last);
    let end = (onset.frame + (110.0 * frames_per_ms).round() as usize).min(last);
    let mut tail = activation[start..=end].to_vec();
    let tail = percentile(&mut tail, 0.25);
    let (peak, before) = rise_levels(activation, onset.frame, sample_rate);
    if tail - before > 0.25 * (peak - before) {
        DrumVoice::HiHatOpen
    } else {
        DrumVoice::HiHatClosed
    }
}

fn classify_tom(frame: usize, mel: &MelSpectrogram, sample_rate: u32) -> DrumVoice {
    match mel_transient_peak_hz(mel, frame, 0.0, sample_rate, 55.0, 500.0) {
        hz if hz >= 210.0 => DrumVoice::TomHigh,
        hz if hz >= 150.0 => DrumVoice::TomMid,
        hz if hz >= 110.0 => DrumVoice::TomLow,
        _ => DrumVoice::TomFloor,
    }
}

/// Crash or ride. A crash keeps blooming 350-450 ms after the hit and its
/// sustain is air-heavy (7-16 kHz, measured 0.55-0.6 of the 2-16 kHz tail);
/// a ride's sustain is its 2-6 kHz ping with little above 7 kHz (0.2-0.35).
fn classify_cymbal(onset: &Onset, mel: &MelSpectrogram, sample_rate: u32) -> DrumVoice {
    let bloom =
        mel_sustain_ratio_between(mel, onset.frame, sample_rate, 2_000.0, 16_001.0, 0.350, 0.450);
    let air = mel_tail_energy(mel, onset.frame, sample_rate, 7_000.0, 16_001.0, 0.080, 0.220);
    let all = mel_tail_energy(mel, onset.frame, sample_rate, 2_000.0, 16_001.0, 0.080, 0.220);
    let air_share = air / all.max(NMF_EPSILON);
    if bloom >= 0.25 || air_share >= 0.45 {
        DrumVoice::Crash
    } else {
        DrumVoice::Ride
    }
}

fn mel_transient_band_sum(
    mel: &MelSpectrogram,
    frame: usize,
    sample_rate: u32,
    low: f32,
    high: f32,
) -> f32 {
    mel.centers_hz
        .iter()
        .enumerate()
        .filter(|(_, hz)| **hz >= low && **hz < high)
        .map(|(band, _)| mel_transient(mel, band, frame, sample_rate))
        .sum()
}

/// A steady tone (bass bleed) wobbles a few percent from frame to frame; only
/// what rises more than 25 % above the pre-onset floor counts as transient.
const TRANSIENT_FLOOR_GAIN: f32 = 1.25;

fn mel_transient(mel: &MelSpectrogram, band: usize, frame: usize, sample_rate: u32) -> f32 {
    let row = &mel.magnitude[band * mel.frames..(band + 1) * mel.frames];
    let (peak, before) = rise_levels_min_floor(row, frame, sample_rate);
    (peak - TRANSIENT_FLOOR_GAIN * before).max(0.0)
}

/// Like `rise_levels`, but the floor is the minimum of the preceding window.
fn rise_levels_min_floor(row: &[f32], frame: usize, sample_rate: u32) -> (f32, f32) {
    let frames_per_ms = sample_rate as f64 / HOP as f64 / 1000.0;
    let at_ms = |ms: f64| (ms * frames_per_ms).round() as usize;
    let last = row.len() - 1;
    let frame = frame.min(last);
    let peak_start = frame.saturating_sub(1);
    let peak_end = (frame + at_ms(30.0)).min(last);
    let peak = row[peak_start..=peak_end].iter().copied().fold(0.0f32, f32::max);
    let before_start = frame.saturating_sub(at_ms(58.0));
    let before_end = frame.saturating_sub(at_ms(17.0));
    let before = if before_end > before_start {
        row[before_start..before_end].iter().copied().fold(f32::MAX, f32::min)
    } else {
        0.0
    };
    (peak, before)
}

fn mel_sustain_ratio(
    mel: &MelSpectrogram,
    frame: usize,
    sample_rate: u32,
    low: f32,
    high: f32,
) -> f32 {
    mel_sustain_ratio_between(mel, frame, sample_rate, low, high, 0.080, 0.220)
}

/// Band energy above the pre-onset background, held over the window
/// `start_secs..end_secs` after the onset, relative to the onset transient.
/// The 25th percentile over the window is what a sustained cymbal keeps up and
/// an interleaved 16th-note hat cannot fake with one spike.
fn mel_sustain_ratio_between(
    mel: &MelSpectrogram,
    frame: usize,
    sample_rate: u32,
    low: f32,
    high: f32,
    start_secs: f64,
    end_secs: f64,
) -> f32 {
    let onset = mel_transient_band_sum(mel, frame, sample_rate, low, high);
    mel_tail_energy(mel, frame, sample_rate, low, high, start_secs, end_secs)
        / onset.max(NMF_EPSILON)
}

/// 25th percentile, over `start_secs..end_secs` after the onset, of the band
/// energy above the pre-onset background, skipping frames where another
/// full-band onset is sounding (a snare 130 ms later is not this hit's tail).
/// Fewer than four free frames is no evidence of a tail.
fn mel_tail_energy(
    mel: &MelSpectrogram,
    frame: usize,
    sample_rate: u32,
    low: f32,
    high: f32,
    start_secs: f64,
    end_secs: f64,
) -> f32 {
    let frame = frame.min(mel.frames - 1);
    let frames_per_sec = sample_rate as f64 / HOP as f64;
    let start = (frame + (start_secs * frames_per_sec).round() as usize).min(mel.frames - 1);
    let end = (frame + (end_secs * frames_per_sec).round() as usize).min(mel.frames - 1);
    let bands: Vec<(usize, f32)> = mel
        .centers_hz
        .iter()
        .enumerate()
        .filter(|(_, hz)| **hz >= low && **hz < high)
        .map(|(band, _)| {
            let row = &mel.magnitude[band * mel.frames..(band + 1) * mel.frames];
            (band, rise_levels_min_floor(row, frame, sample_rate).1)
        })
        .collect();
    let mut tail: Vec<f32> = (start..=end)
        .filter(|&later| !mel.onset_mask[later])
        .map(|later| {
            bands
                .iter()
                .map(|&(band, background)| {
                    (mel.magnitude[band * mel.frames + later] - TRANSIENT_FLOOR_GAIN * background)
                        .max(0.0)
                })
                .sum()
        })
        .collect();
    if tail.len() < 4 {
        return 0.0;
    }
    percentile(&mut tail, 0.25)
}

/// The mel band with the strongest energy above the pre-onset floor, measured
/// `tail_secs` after the onset (0 = at the onset itself).
fn mel_transient_peak_hz(
    mel: &MelSpectrogram,
    frame: usize,
    tail_secs: f64,
    sample_rate: u32,
    low: f32,
    high: f32,
) -> f32 {
    let later = (frame + (tail_secs * sample_rate as f64 / HOP as f64).round() as usize)
        .min(mel.frames - 1);
    let tail_level = |band: usize| {
        let row = &mel.magnitude[band * mel.frames..(band + 1) * mel.frames];
        let (_, before) = rise_levels_min_floor(row, frame, sample_rate);
        (local_peak(row, later, 2) - TRANSIENT_FLOOR_GAIN * before).max(0.0)
    };
    mel.centers_hz
        .iter()
        .enumerate()
        .filter(|(_, hz)| **hz >= low && **hz < high)
        .max_by(|(a, _), (b, _)| tail_level(*a).partial_cmp(&tail_level(*b)).unwrap_or(Ordering::Equal))
        .map_or(0.0, |(_, hz)| *hz)
}

fn own_ratio(own: f32, other: f32) -> f32 {
    own / other.max(NMF_EPSILON)
}

fn cymbal_level(activations: &[f32], frame: usize, frames: usize) -> f32 {
    DrumClass::CYMBALS
        .iter()
        .map(|&component| component_level(activations, component, frame, frames))
        .fold(0.0f32, f32::max)
}

fn component_level(activations: &[f32], component: usize, frame: usize, frames: usize) -> f32 {
    local_peak(
        &activations[component * frames..(component + 1) * frames],
        frame,
        8,
    )
}

fn percentile(values: &mut [f32], quantile: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let index = ((values.len() - 1) as f32 * quantile).round() as usize;
    values[index]
}

/// Track one bass/melody line with a YIN-style difference function.
pub fn transcribe_monophonic(
    mono: &[f32],
    sample_rate: u32,
    clock: &LoopClock,
) -> Vec<PitchedNote> {
    if mono.is_empty() || sample_rate == 0 || !clock.bpm.is_finite() || clock.bpm <= 0.0 {
        return Vec::new();
    }
    let frame_count = mono.len().div_ceil(HOP).max(1);
    let mut energies = vec![0.0f32; frame_count];
    let mut pitches = vec![None; frame_count];
    let mut frame = vec![0.0f32; WINDOW];
    let decimation = (sample_rate / 8_000).max(1) as usize;
    let effective_rate = sample_rate as f64 / decimation as f64;
    let mut downsampled = Vec::with_capacity(WINDOW.div_ceil(decimation));
    let mut difference = Vec::new();
    let mut cmnd = Vec::new();

    for index in 0..frame_count {
        let center = index * HOP;
        copy_centered(mono, center, &mut frame);
        let mean = frame.iter().copied().sum::<f32>() / WINDOW as f32;
        let mut square = 0.0f64;
        for value in &mut frame {
            *value -= mean;
            square += f64::from(*value) * f64::from(*value);
        }
        energies[index] = (square / WINDOW as f64).sqrt() as f32;
        downsampled.clear();
        for samples in frame.chunks(decimation) {
            downsampled.push(samples.iter().copied().sum::<f32>() / samples.len() as f32);
        }
        pitches[index] = yin_pitch(
            &downsampled,
            effective_rate,
            &mut difference,
            &mut cmnd,
        );
    }

    let peak_energy = energies.iter().copied().fold(0.0f32, f32::max);
    if peak_energy <= 1e-7 {
        return Vec::new();
    }
    let silence = peak_energy * 10.0f32.powf(-30.0 / 20.0);
    for (pitch, energy) in pitches.iter_mut().zip(&energies) {
        if *energy < silence {
            *pitch = None;
        }
    }
    let pitches = median_smooth(&pitches);
    segment_notes(&pitches, &energies, peak_energy, mono.len(), sample_rate, clock)
}

fn magnitude_at(spectrum: &[f32], frames: usize, bin: usize, frame: usize) -> f32 {
    let at = (bin * frames + frame) * 2;
    spectrum[at].hypot(spectrum[at + 1])
}

fn median_in_place(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let middle = values.len() / 2;
    let (_, value, _) = values.select_nth_unstable_by(middle, |a, b| {
        a.partial_cmp(b).unwrap_or(Ordering::Equal)
    });
    *value
}

fn copy_centered(source: &[f32], center: usize, target: &mut [f32]) {
    target.fill(0.0);
    let left = center as isize - target.len() as isize / 2;
    for (offset, value) in target.iter_mut().enumerate() {
        let source_index = left + offset as isize;
        if source_index >= 0 {
            if let Some(source) = source.get(source_index as usize) {
                *value = *source;
            }
        }
    }
}

fn yin_pitch(
    samples: &[f32],
    sample_rate: f64,
    difference: &mut Vec<f64>,
    cmnd: &mut Vec<f64>,
) -> Option<f64> {
    let min_lag = (sample_rate / YIN_MAX_HZ).floor().max(2.0) as usize;
    let max_lag = (sample_rate / YIN_MIN_HZ).ceil() as usize;
    if samples.len() <= max_lag + 4 || min_lag >= max_lag {
        return None;
    }
    let compared = samples.len() - max_lag;
    difference.clear();
    difference.resize(max_lag + 1, 0.0);
    cmnd.clear();
    cmnd.resize(max_lag + 1, 1.0);
    for lag in 1..=max_lag {
        let mut sum = 0.0f64;
        for index in 0..compared {
            let delta = f64::from(samples[index] - samples[index + lag]);
            sum += delta * delta;
        }
        difference[lag] = sum;
    }
    let mut cumulative = 0.0;
    for lag in 1..=max_lag {
        cumulative += difference[lag];
        cmnd[lag] = if cumulative > 1e-15 {
            difference[lag] * lag as f64 / cumulative
        } else {
            1.0
        };
    }
    let mut lag = min_lag;
    while lag <= max_lag {
        if cmnd[lag] < YIN_APERIODICITY {
            while lag < max_lag && cmnd[lag + 1] < cmnd[lag] {
                lag += 1;
            }
            let refined = parabolic_minimum(cmnd, lag);
            let pitch = sample_rate / refined;
            return (YIN_MIN_HZ..=YIN_MAX_HZ).contains(&pitch).then_some(pitch);
        }
        lag += 1;
    }
    None
}

fn parabolic_minimum(values: &[f64], index: usize) -> f64 {
    if index == 0 || index + 1 >= values.len() {
        return index as f64;
    }
    let left = values[index - 1];
    let center = values[index];
    let right = values[index + 1];
    let denominator = left - 2.0 * center + right;
    if denominator.abs() < 1e-12 {
        index as f64
    } else {
        index as f64 + (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
    }
}

fn median_smooth(pitches: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut smoothed = vec![None; pitches.len()];
    let mut scratch = Vec::with_capacity(5);
    for (index, pitch) in pitches.iter().enumerate() {
        if pitch.is_none() {
            continue;
        }
        scratch.clear();
        let start = index.saturating_sub(2);
        let end = (index + 3).min(pitches.len());
        scratch.extend(pitches[start..end].iter().flatten().copied());
        scratch.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        smoothed[index] = scratch.get(scratch.len() / 2).copied();
    }
    smoothed
}

fn segment_notes(
    pitches: &[Option<f64>],
    energies: &[f32],
    peak_energy: f32,
    sample_count: usize,
    sample_rate: u32,
    clock: &LoopClock,
) -> Vec<PitchedNote> {
    let mut notes = Vec::new();
    let mut start = None;
    for index in 0..=pitches.len() {
        let pitch = pitches.get(index).copied().flatten();
        let boundary = match (start, pitch) {
            (Some(segment_start), Some(current)) if index > segment_start => {
                let energy_rise = energies[index]
                    > energies[index.saturating_sub(1)].max(1e-9) * 10.0f32.powf(6.0 / 20.0);
                let prior = recent_median_pitch(pitches, segment_start, index).unwrap_or(current);
                let jump = semitone_distance(prior, current) > 1.0
                    && pitch_is_held(pitches, index, current);
                energy_rise || jump
            }
            (Some(_), None) => true,
            _ => false,
        };
        if boundary {
            if let Some(segment_start) = start.take() {
                push_note(
                    &mut notes,
                    pitches,
                    energies,
                    segment_start,
                    index,
                    peak_energy,
                    sample_count,
                    sample_rate,
                    clock,
                );
            }
        }
        if pitch.is_some() && start.is_none() {
            start = Some(index);
        }
    }
    notes
}

fn pitch_is_held(pitches: &[Option<f64>], start: usize, pitch: f64) -> bool {
    (start..start.saturating_add(3)).all(|index| {
        pitches
            .get(index)
            .copied()
            .flatten()
            .is_some_and(|value| semitone_distance(value, pitch) < 0.75)
    })
}

fn median_pitch(pitches: &[Option<f64>]) -> Option<f64> {
    let mut values: Vec<f64> = pitches.iter().flatten().copied().collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    values.get(values.len() / 2).copied()
}

fn recent_median_pitch(pitches: &[Option<f64>], start: usize, end: usize) -> Option<f64> {
    let mut values = [0.0f64; 5];
    let mut count = 0;
    for pitch in pitches[start.max(end.saturating_sub(values.len()))..end]
        .iter()
        .flatten()
    {
        values[count] = *pitch;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    values[..count].sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    Some(values[count / 2])
}

#[allow(clippy::too_many_arguments)]
fn push_note(
    notes: &mut Vec<PitchedNote>,
    pitches: &[Option<f64>],
    energies: &[f32],
    start: usize,
    end: usize,
    peak_energy: f32,
    sample_count: usize,
    sample_rate: u32,
    clock: &LoopClock,
) {
    let Some(pitch) = median_pitch(&pitches[start..end]) else { return };
    let start_sample = (start * HOP).min(sample_count);
    let end_sample = (end * HOP).min(sample_count).max((start_sample + HOP).min(sample_count));
    if end_sample <= start_sample {
        return;
    }
    let midi = (69.0 + 12.0 * (pitch / 440.0).log2()).round().clamp(0.0, 127.0) as u8;
    let segment_peak = energies[start..end].iter().copied().fold(0.0f32, f32::max);
    let beats_per_second = clock.bpm / 60.0;
    let total_beats = f64::from(clock.bars) * f64::from(clock.beats_per_bar);
    let onset_beats = start_sample as f64 / sample_rate as f64 * beats_per_second;
    if onset_beats >= total_beats {
        return;
    }
    notes.push(PitchedNote {
        onset_beats,
        duration_beats: ((end_sample - start_sample) as f64 / sample_rate as f64
            * beats_per_second)
            .min(total_beats - onset_beats),
        midi,
        velocity: (segment_peak / peak_energy).clamp(0.0, 1.0),
    });
}

fn semitone_distance(a: f64, b: f64) -> f64 {
    (12.0 * (a / b).log2()).abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_drumkit::{DrumKit, DrumVoice as KitVoice, SampleBank};
    use std::f32::consts::TAU;
    use std::sync::{Arc, OnceLock};

    const RATE: u32 = 44_100;

    fn clock() -> LoopClock {
        LoopClock { bpm: 120.0, bars: 1, beats_per_bar: 4 }
    }

    fn local_sample_bank() -> Option<Arc<SampleBank>> {
        static BANK: OnceLock<Result<Arc<SampleBank>, String>> = OnceLock::new();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/score-corpus/drums/OH");
        if !dir.is_dir() {
            eprintln!("skipping drum transcription kit test: {} is absent", dir.display());
            return None;
        }
        match BANK.get_or_init(|| SampleBank::load(&dir).map(Arc::new)) {
            Ok(bank) => Some(bank.clone()),
            Err(error) => panic!("load local Salamander corpus: {error}"),
        }
    }

    fn render_kit(events: &[(f64, KitVoice, f32)], bars: u32) -> Option<Vec<f32>> {
        let sample_count = bars as usize * 4 * RATE as usize / 2;
        let mut out = vec![0.0f32; sample_count];
        let mut kit = DrumKit::new(RATE as f32);
        kit.set_bank(local_sample_bank()?);
        let mut event_index = 0;
        for (sample_index, sample) in out.iter_mut().enumerate() {
            while let Some(&(beat, voice, velocity)) = events.get(event_index) {
                let event_sample = (beat * RATE as f64 * 0.5).round() as usize;
                if event_sample > sample_index {
                    break;
                }
                kit.trigger(voice, velocity);
                event_index += 1;
            }
            let mut frame = [[0.0f32; 2]];
            kit.process(&mut frame);
            *sample = (frame[0][0] + frame[0][1]) * 0.5;
        }
        Some(out)
    }

    fn has_hit(hits: &[DrumHit], voice: DrumVoice, beat: f64) -> bool {
        hits.iter().any(|hit| {
            hit.voice == voice && (hit.time_beats - beat).abs() <= 1.0 / 32.0
        })
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum HitClass {
        Kick,
        Snare,
        Hat,
        Tom,
        Cymbal,
    }

    impl HitClass {
        const ALL: [Self; 5] = [Self::Kick, Self::Snare, Self::Hat, Self::Tom, Self::Cymbal];

        fn index(self) -> usize {
            match self {
                Self::Kick => 0,
                Self::Snare => 1,
                Self::Hat => 2,
                Self::Tom => 3,
                Self::Cymbal => 4,
            }
        }
    }

    #[derive(Clone, Copy, Default)]
    struct Counts {
        true_positive: usize,
        false_positive: usize,
        false_negative: usize,
    }

    impl Counts {
        fn f_measure(self) -> f32 {
            let denominator = 2 * self.true_positive + self.false_positive + self.false_negative;
            if denominator == 0 {
                1.0
            } else {
                2.0 * self.true_positive as f32 / denominator as f32
            }
        }

        fn add(&mut self, other: Self) {
            self.true_positive += other.true_positive;
            self.false_positive += other.false_positive;
            self.false_negative += other.false_negative;
        }
    }

    fn kit_class(voice: KitVoice) -> HitClass {
        match voice {
            KitVoice::Kick => HitClass::Kick,
            KitVoice::Snare | KitVoice::SideStick | KitVoice::Clap => HitClass::Snare,
            KitVoice::HiHatClosed | KitVoice::HiHatOpen | KitVoice::HiHatPedal => HitClass::Hat,
            KitVoice::TomHigh | KitVoice::TomMid | KitVoice::TomLow | KitVoice::TomFloor => {
                HitClass::Tom
            }
            KitVoice::Ride | KitVoice::RideBell | KitVoice::Crash => HitClass::Cymbal,
            // The kit's clap has no score voice of its own; it sits where
            // a snare sits in the harness.
            KitVoice::Clap => HitClass::Snare,
        }
    }

    fn score_class(voice: DrumVoice) -> HitClass {
        match voice {
            DrumVoice::Kick => HitClass::Kick,
            DrumVoice::Snare | DrumVoice::SideStick => HitClass::Snare,
            DrumVoice::HiHatClosed | DrumVoice::HiHatOpen | DrumVoice::HiHatPedal => HitClass::Hat,
            DrumVoice::TomHigh | DrumVoice::TomMid | DrumVoice::TomLow | DrumVoice::TomFloor => {
                HitClass::Tom
            }
            DrumVoice::Ride | DrumVoice::RideBell | DrumVoice::Crash => HitClass::Cymbal,
        }
    }

    fn onset_counts(
        truth: &[(f64, KitVoice, f32)],
        detected: &[DrumHit],
        class: HitClass,
    ) -> Counts {
        let truth: Vec<f64> = truth
            .iter()
            .filter(|event| kit_class(event.1) == class)
            .map(|event| event.0)
            .collect();
        let detected: Vec<f64> = detected
            .iter()
            .filter(|hit| score_class(hit.voice) == class)
            .map(|hit| hit.time_beats)
            .collect();
        let mut used = vec![false; detected.len()];
        let mut true_positive = 0;
        for expected in &truth {
            let nearest = detected
                .iter()
                .enumerate()
                .filter(|(index, at)| !used[*index] && (*at - expected).abs() <= 0.05)
                .min_by(|(_, a), (_, b)| {
                    (*a - expected)
                        .abs()
                        .partial_cmp(&(*b - expected).abs())
                        .unwrap_or(Ordering::Equal)
                })
                .map(|(index, _)| index);
            if let Some(index) = nearest {
                used[index] = true;
                true_positive += 1;
            }
        }
        Counts {
            true_positive,
            false_positive: detected.len() - true_positive,
            false_negative: truth.len() - true_positive,
        }
    }

    fn synthetic_patterns() -> Vec<Vec<(f64, KitVoice, f32)>> {
        let mut patterns = Vec::new();

        let mut rock = Vec::new();
        for step in 1..16 {
            rock.push((step as f64 * 0.5, KitVoice::HiHatClosed, 0.48 + 0.16 * (step % 2) as f32));
        }
        for beat in [0.5, 2.5, 4.5, 6.5] {
            rock.push((beat, KitVoice::Kick, 0.78));
        }
        for beat in [1.5, 3.5, 5.5, 7.5] {
            rock.push((beat, KitVoice::Snare, 0.84));
        }
        rock.push((0.5, KitVoice::Crash, 0.88));
        rock.push((2.25, KitVoice::Ride, 0.62));
        rock.push((6.25, KitVoice::Ride, 0.68));
        patterns.push(rock);

        let mut disco = Vec::new();
        for step in 1..16 {
            let beat = 0.25 + step as f64 * 0.5;
            disco.push((
                beat,
                if step == 7 || step == 15 {
                    KitVoice::HiHatOpen
                } else {
                    KitVoice::HiHatClosed
                },
                if step % 2 == 0 { 0.50 } else { 0.72 },
            ));
        }
        for beat in [0.75, 1.75, 2.75, 3.75, 4.75, 5.75, 6.75, 7.75] {
            disco.push((beat, KitVoice::Kick, 0.82));
        }
        for beat in [1.75, 3.75, 5.75, 7.75] {
            disco.push((beat, KitVoice::Snare, 0.79));
        }
        patterns.push(disco);

        let mut hip_hop = Vec::new();
        for beat in [0.5, 1.25, 2.75, 4.5, 5.0, 6.75] {
            hip_hop.push((beat, KitVoice::Kick, 0.52 + 0.05 * beat as f32));
        }
        for beat in [1.5, 3.5, 5.5, 7.5] {
            hip_hop.push((beat, KitVoice::Snare, 0.72));
        }
        for step in 2..32 {
            hip_hop.push((step as f64 * 0.25, KitVoice::HiHatClosed, 0.32 + 0.08 * (step % 4) as f32));
        }
        patterns.push(hip_hop);

        let mut breakbeat = Vec::new();
        for beat in [0.5, 1.25, 2.5, 3.25, 4.5, 5.75, 6.5] {
            breakbeat.push((beat, KitVoice::Kick, 0.74));
        }
        for (beat, velocity) in [
            (1.5, 0.86),
            (2.25, 0.27),
            (3.5, 0.82),
            (4.25, 0.24),
            (5.5, 0.88),
            (6.25, 0.30),
            (7.5, 0.84),
        ] {
            breakbeat.push((beat, KitVoice::Snare, velocity));
        }
        for beat in [0.75, 1.75, 2.75, 3.75, 4.75, 5.75, 6.75, 7.75] {
            breakbeat.push((beat, KitVoice::HiHatClosed, 0.56));
        }
        patterns.push(breakbeat);

        let mut tom_fill = vec![
            (0.5, KitVoice::Kick, 0.82),
            (1.5, KitVoice::Snare, 0.78),
            (2.5, KitVoice::Kick, 0.74),
            (3.5, KitVoice::Snare, 0.84),
            (4.0, KitVoice::TomHigh, 0.55),
            (4.5, KitVoice::TomHigh, 0.70),
            (5.0, KitVoice::TomMid, 0.62),
            (5.5, KitVoice::TomMid, 0.78),
            (6.0, KitVoice::TomLow, 0.68),
            (6.5, KitVoice::TomLow, 0.82),
            (7.0, KitVoice::TomFloor, 0.76),
            (7.5, KitVoice::TomFloor, 0.92),
            (7.5, KitVoice::Crash, 0.88),
        ];
        for step in 1..8 {
            tom_fill.push((step as f64 * 0.5, KitVoice::HiHatClosed, 0.52));
        }
        patterns.push(tom_fill);

        let mut hats = Vec::new();
        for step in 2..32 {
            hats.push((
                step as f64 * 0.25,
                if step == 8 || step == 20 || step == 28 {
                    KitVoice::HiHatOpen
                } else {
                    KitVoice::HiHatClosed
                },
                0.38 + 0.12 * (step % 4) as f32,
            ));
        }
        patterns.push(hats);

        for pattern in &mut patterns {
            pattern.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
        }
        patterns
    }

    fn add_bleed_and_smear(clean: &[f32]) -> Vec<f32> {
        let peak = clean.iter().copied().map(f32::abs).fold(0.0f32, f32::max);
        let bass_level = peak * 10.0f32.powf(-18.0 / 20.0);
        let delay = (RATE as f32 * 0.030).round() as usize;
        let mut degraded = clean.to_vec();
        for index in 0..degraded.len() {
            let fade = (index as f32 / (RATE as f32 * 0.020)).min(1.0);
            degraded[index] +=
                (TAU * 55.0 * index as f32 / RATE as f32).sin() * bass_level * fade;
            if index >= delay {
                degraded[index] += clean[index - delay] * 0.28;
            }
        }
        degraded
    }

    #[test]
    fn synthetic_drum_hits_land_on_their_beats() {
        let Some(samples) = render_kit(
            &[
                (0.5, KitVoice::Kick, 0.85),
                (1.5, KitVoice::Snare, 0.8),
                (2.5, KitVoice::HiHatClosed, 0.7),
                (3.25, KitVoice::Kick, 0.6),
                (3.5, KitVoice::Snare, 0.9),
            ],
            1,
        ) else { return };
        let hits = transcribe_drums(&samples, RATE, &clock());
        for beat in [0.5, 1.5, 2.5, 3.25, 3.5] {
            assert!(
                hits.iter().any(|hit| (hit.time_beats - beat).abs() <= 1.0 / 32.0),
                "missing sample-kit onset at beat {beat}: {hits:?}"
            );
        }
    }

    #[test]
    fn drum_transcribe_single_kit_hits_have_the_right_class() {
        let mut failures = Vec::new();
        for kit_voice in [
            KitVoice::Kick,
            KitVoice::Snare,
            KitVoice::HiHatClosed,
            KitVoice::HiHatOpen,
            KitVoice::TomHigh,
            KitVoice::TomMid,
            KitVoice::TomLow,
            KitVoice::TomFloor,
            KitVoice::Ride,
            KitVoice::Crash,
        ] {
            let Some(samples) = render_kit(&[(1.0, kit_voice, 0.8)], 1) else { return };
            let hits = transcribe_drums(&samples, RATE, &clock());
            if !hits.iter().any(|hit| (hit.time_beats - 1.0).abs() <= 1.0 / 32.0) {
                failures.push(format!("missing onset for {kit_voice:?}: {hits:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn drum_transcribe_keeps_kick_hat_co_onset_and_rejects_snare_hat_bleed() {
        let Some(co_onset) = render_kit(
            &[(1.0, KitVoice::Kick, 0.9), (1.0, KitVoice::HiHatClosed, 0.7)],
            1,
        ) else { return };
        let hits = transcribe_drums(&co_onset, RATE, &clock());
        assert!(has_hit(&hits, DrumVoice::Kick, 1.0), "missing kick: {hits:?}");
        assert!(
            hits.iter().filter(|hit| (hit.time_beats - 1.0).abs() <= 1.0 / 32.0).count() >= 2,
            "co-onset collapsed to one event: {hits:?}"
        );

        let Some(snare) = render_kit(&[(1.0, KitVoice::Snare, 0.9)], 1) else { return };
        let hits = transcribe_drums(&snare, RATE, &clock());
        assert!(has_hit(&hits, DrumVoice::Snare, 1.0), "missing snare: {hits:?}");
        assert!(
            !hits.iter().any(|hit| matches!(
                hit.voice,
                DrumVoice::HiHatClosed | DrumVoice::HiHatOpen | DrumVoice::HiHatPedal
            )),
            "snare produced a false hat: {hits:?}"
        );
    }

    #[test]
    fn drum_transcribe_is_deterministic() {
        let Some(samples) = render_kit(
            &[
                (0.5, KitVoice::Kick, 0.75),
                (1.0, KitVoice::Snare, 0.55),
                (1.5, KitVoice::HiHatOpen, 0.9),
            ],
            1,
        ) else { return };
        let first = transcribe_drums(&samples, RATE, &clock());
        let second = transcribe_drums(&samples, RATE, &clock());
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.time_beats.to_bits(), b.time_beats.to_bits());
            assert_eq!(a.voice, b.voice);
            assert_eq!(a.velocity.to_bits(), b.velocity.to_bits());
        }
    }

    #[test]
    fn drum_transcribe_synthetic_acceptance() {
        let mut clean = [Counts::default(); 5];
        let mut degraded = [Counts::default(); 5];
        let mut snare_without_hat = 0usize;
        let mut false_hats_on_snare = 0usize;
        let mut truth_events = 0usize;
        let mut clean_timed = 0usize;
        let mut degraded_timed = 0usize;
        let clock = LoopClock {
            bpm: 120.0,
            bars: 2,
            beats_per_bar: 4,
        };
        for (pattern_index, truth) in synthetic_patterns().into_iter().enumerate() {
            let Some(samples) = render_kit(&truth, 2) else { return };
            let clean_hits = transcribe_drums(&samples, RATE, &clock);
            let degraded_hits = transcribe_drums(&add_bleed_and_smear(&samples), RATE, &clock);
            eprintln!(
                "pattern {pattern_index} truth={} clean={:?} degraded={:?}",
                truth.len(),
                clean_hits
                    .iter()
                    .map(|hit| (hit.time_beats, score_class(hit.voice)))
                    .collect::<Vec<_>>(),
                degraded_hits
                    .iter()
                    .map(|hit| (hit.time_beats, score_class(hit.voice)))
                    .collect::<Vec<_>>()
            );
            for class in HitClass::ALL {
                clean[class.index()].add(onset_counts(&truth, &clean_hits, class));
                degraded[class.index()].add(onset_counts(&truth, &degraded_hits, class));
            }
            for &(beat, voice, _) in &truth {
                truth_events += 1;
                clean_timed += usize::from(
                    clean_hits.iter().any(|hit| (hit.time_beats - beat).abs() <= 0.05),
                );
                degraded_timed += usize::from(
                    degraded_hits.iter().any(|hit| (hit.time_beats - beat).abs() <= 0.05),
                );
                if kit_class(voice) != HitClass::Snare
                    || truth.iter().any(|event| {
                        kit_class(event.1) == HitClass::Hat && (event.0 - beat).abs() <= 0.05
                    })
                {
                    continue;
                }
                snare_without_hat += 1;
                if clean_hits.iter().any(|hit| {
                    score_class(hit.voice) == HitClass::Hat
                        && (hit.time_beats - beat).abs() <= 0.05
                }) {
                    false_hats_on_snare += 1;
                }
            }
        }
        let clean_f: [f32; 5] = std::array::from_fn(|index| clean[index].f_measure());
        let degraded_f: [f32; 5] = std::array::from_fn(|index| degraded[index].f_measure());
        let clean_timing_recall = clean_timed as f32 / truth_events as f32;
        let degraded_timing_recall = degraded_timed as f32 / truth_events as f32;
        eprintln!(
            "drum synthetic F clean={clean_f:?} bleed+smear={degraded_f:?}; onset recall={clean_timing_recall:.3}/{degraded_timing_recall:.3}; false hats={false_hats_on_snare}/{snare_without_hat}"
        );
        assert!(clean_timing_recall >= 0.70, "clean onset recall {clean_timing_recall:.3}");
        assert!(
            degraded_timing_recall >= 0.50,
            "bleed+smear onset recall {degraded_timing_recall:.3}"
        );
        assert!(
            false_hats_on_snare * 20 <= snare_without_hat,
            "false hats on snare: {false_hats_on_snare}/{snare_without_hat}"
        );
    }

    #[test]
    fn synthetic_bass_line_has_four_midi_notes() {
        let midi = [28u8, 33, 38, 43];
        let note_samples = RATE as usize / 2;
        let gap = RATE as usize / 100;
        let mut samples = vec![0.0; note_samples * midi.len()];
        for (note, midi) in midi.into_iter().enumerate() {
            let hz = 440.0 * 2.0f32.powf((midi as f32 - 69.0) / 12.0);
            let start = note * note_samples;
            let end = (start + note_samples - gap).min(samples.len());
            for (offset, sample) in samples[start..end].iter_mut().enumerate() {
                let edge = (offset.min(end - start - 1 - offset) as f32 / 128.0).min(1.0);
                *sample = (TAU * hz * offset as f32 / RATE as f32).sin() * 0.7 * edge;
            }
        }
        let notes = transcribe_monophonic(&samples, RATE, &clock());
        assert_eq!(notes.iter().map(|note| note.midi).collect::<Vec<_>>(), midi);
    }

    #[test]
    fn silence_is_empty() {
        assert!(transcribe_drums(&vec![0.0; WINDOW * 2], RATE, &clock()).is_empty());
        assert!(transcribe_monophonic(&vec![0.0; WINDOW * 2], RATE, &clock()).is_empty());
    }

    #[test]
    fn one_frame_inputs_do_not_panic() {
        assert!(transcribe_drums(&[0.0], RATE, &clock()).is_empty());
        assert!(transcribe_monophonic(&[0.0], RATE, &clock()).is_empty());
    }

    fn read_wav_pcm16_mono(path: &std::path::Path) -> Option<(Vec<f32>, u32)> {
        let bytes = std::fs::read(path).ok()?;
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return None;
        }
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let mut channels = 0usize;
        let mut rate = 0u32;
        let mut bits = 0u16;
        let mut cursor = 12;
        while cursor + 8 <= bytes.len() {
            let id = &bytes[cursor..cursor + 4];
            let size = u32_at(cursor + 4) as usize;
            let body = cursor + 8;
            if id == b"fmt " && body + 16 <= bytes.len() {
                channels = u16_at(body + 2) as usize;
                rate = u32_at(body + 4);
                bits = u16_at(body + 14);
            } else if id == b"data" {
                if bits != 16 || channels == 0 || rate == 0 {
                    return None;
                }
                let end = (body + size).min(bytes.len());
                let frame_bytes = 2 * channels;
                let frames = (end - body) / frame_bytes;
                let mut mono = Vec::with_capacity(frames);
                for frame in 0..frames {
                    let at = body + frame * frame_bytes;
                    let sum: f32 = (0..channels)
                        .map(|channel| i16::from_le_bytes([bytes[at + 2 * channel], bytes[at + 2 * channel + 1]]) as f32 / 32768.0)
                        .sum();
                    mono.push(sum / channels as f32);
                }
                return Some((mono, rate));
            }
            cursor = body + size + (size & 1);
        }
        None
    }

    fn write_wav_mono16(path: &std::path::Path, samples: &[f32], rate: u32) -> std::io::Result<()> {
        let data_len = samples.len() * 2;
        let mut out = Vec::with_capacity(44 + data_len);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data_len as u32).to_le_bytes());
        for sample in samples {
            out.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
        }
        std::fs::write(path, out)
    }

    /// Render a transcription through the kit at the clock's tempo.
    fn render_hits(hits: &[DrumHit], clock: &LoopClock, rate: u32, length: usize) -> Option<Vec<f32>> {
        let mut events: Vec<(f64, KitVoice, f32)> = hits
            .iter()
            .filter_map(|hit| {
                KitVoice::try_from(hit.voice.gm_note())
                    .ok()
                    .map(|voice| (hit.time_beats * 60.0 / clock.bpm, voice, hit.velocity))
            })
            .collect();
        events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
        let mut out = vec![0.0f32; length];
        let mut kit = DrumKit::new(rate as f32);
        kit.set_bank(local_sample_bank()?);
        let mut next = 0;
        for (index, sample) in out.iter_mut().enumerate() {
            while let Some(&(secs, voice, velocity)) = events.get(next) {
                if (secs * rate as f64).round() as usize > index {
                    break;
                }
                kit.trigger(voice, velocity);
                next += 1;
            }
            let mut frame = [[0.0f32; 2]];
            kit.process(&mut frame);
            *sample = (frame[0][0] + frame[0][1]) * 0.5;
        }
        Some(out)
    }

    /// Second-order Butterworth low-pass (RBJ cookbook), run twice for a 4th-order slope.
    fn low_pass(samples: &[f32], rate: u32, cutoff_hz: f32) -> Vec<f32> {
        let w0 = TAU * cutoff_hz / rate as f32;
        let alpha = w0.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let cos = w0.cos();
        let a0 = 1.0 + alpha;
        let b0 = (1.0 - cos) / 2.0 / a0;
        let b1 = (1.0 - cos) / a0;
        let b2 = b0;
        let a1 = -2.0 * cos / a0;
        let a2 = (1.0 - alpha) / a0;
        let mut out = samples.to_vec();
        for _ in 0..2 {
            let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for value in &mut out {
                let x0 = *value;
                let y0 = b0 * x0 + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
                x2 = x1;
                x1 = x0;
                y2 = y1;
                y1 = y0;
                *value = y0;
            }
        }
        out
    }

    /// 10 ms RMS envelopes of the <200 Hz, 200 Hz-3 kHz and >5 kHz bands.
    fn band_envelopes(samples: &[f32], rate: u32) -> [Vec<f32>; 3] {
        let low = low_pass(samples, rate, 200.0);
        let below_3k = low_pass(samples, rate, 3_000.0);
        let below_5k = low_pass(samples, rate, 5_000.0);
        let mid: Vec<f32> = below_3k.iter().zip(&low).map(|(a, b)| a - b).collect();
        let high: Vec<f32> = samples.iter().zip(&below_5k).map(|(a, b)| a - b).collect();
        let block = (rate as usize) / 100;
        let envelope = |band: &[f32]| -> Vec<f32> {
            band.chunks(block)
                .map(|chunk| (chunk.iter().map(|v| v * v).sum::<f32>() / chunk.len() as f32).sqrt())
                .collect()
        };
        [envelope(&low), envelope(&mid), envelope(&high)]
    }

    fn pearson(a: &[f32], b: &[f32]) -> f64 {
        let n = a.len().min(b.len());
        if n < 2 {
            return 0.0;
        }
        let mean = |values: &[f32]| values[..n].iter().map(|v| f64::from(*v)).sum::<f64>() / n as f64;
        let (mean_a, mean_b) = (mean(a), mean(b));
        let (mut cov, mut var_a, mut var_b) = (0.0f64, 0.0f64, 0.0f64);
        for index in 0..n {
            let da = f64::from(a[index]) - mean_a;
            let db = f64::from(b[index]) - mean_b;
            cov += da * db;
            var_a += da * da;
            var_b += db * db;
        }
        cov / (var_a * var_b).sqrt().max(1e-12)
    }

    fn sounds_like(original: &[Vec<f32>; 3], rendered: &[f32], rate: u32) -> [f64; 3] {
        let envelopes = band_envelopes(rendered, rate);
        std::array::from_fn(|band| pearson(&original[band], &envelopes[band]))
    }


    /// Real separated drums stem (gitignored): transcribe, render through the kit and
    /// report how much the render's 10 ms band envelopes follow the original's
    /// (Pearson per band: <200 Hz, 200 Hz-3 kHz, >5 kHz). Writes
    /// `target/drum_ab/{original,after}.wav` for listening. The band grid is only
    /// a beats<->seconds mapping here, so a nominal 120 bpm clock covers the file.
    #[test]
    #[ignore]
    fn drum_ab_real_stem_sounds_like() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let stem = root.join("local/stems_demo/seg_drums.wav");
        let Some((mono, rate)) = read_wav_pcm16_mono(&stem) else {
            eprintln!("no real stem at {}; skipping", stem.display());
            return;
        };
        let seconds = mono.len() as f64 / rate as f64;
        let clock = LoopClock { bpm: 120.0, bars: (seconds / 2.0).ceil() as u32, beats_per_bar: 4 };
        let original = band_envelopes(&mono, rate);
        let out_dir = root.join("target/drum_ab");
        std::fs::create_dir_all(&out_dir).unwrap();
        let started = std::time::Instant::now();
        let hits = transcribe_drums(&mono, rate, &clock);
        let elapsed = started.elapsed();
        let Some(rendered) = render_hits(&hits, &clock, rate, mono.len()) else { return };
        let bands = sounds_like(&original, &rendered, rate);
        let paths = [("original", &mono), ("after", &rendered)];
        for (label, samples) in paths {
            let path = out_dir.join(format!("{label}.wav"));
            write_wav_mono16(&path, samples, rate).unwrap();
            eprintln!("wrote {}", path.canonicalize().unwrap().display());
        }
        eprintln!(
            "sounds-like: low={:.3} mid={:.3} high={:.3} mean={:.3} ({} hits)",
            bands[0],
            bands[1],
            bands[2],
            (bands[0] + bands[1] + bands[2]) / 3.0,
            hits.len()
        );
        eprintln!("transcribe_drums on {seconds:.1} s of stem: {elapsed:?}");
    }

    /// Four bars at 120 bpm through the kit, timed (run in release: < 200 ms on one core).
    #[test]
    #[ignore]
    fn drum_transcribe_four_bars_timing() {
        let mut pattern = Vec::new();
        for bar in 0..4 {
            for event in &synthetic_patterns()[bar % 2] {
                pattern.push((event.0 + bar as f64 * 4.0, event.1, event.2));
            }
        }
        pattern.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
        let Some(samples) = render_kit(&pattern, 4) else { return };
        let clock = LoopClock { bpm: 120.0, bars: 4, beats_per_bar: 4 };
        let started = std::time::Instant::now();
        let hits = transcribe_drums(&samples, RATE, &clock);
        let elapsed = started.elapsed();
        eprintln!("transcribe_drums 4 bars @120: {elapsed:?} ({} hits)", hits.len());
        assert!(!hits.is_empty());
    }

}
