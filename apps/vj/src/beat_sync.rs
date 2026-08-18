//! Dependency-free beat-grid tracking for the VJ input worker.
//!
//! The analyzer deliberately owns fixed-size rings and performs no allocation in
//! [`BeatSyncAnalyzer::push_mono`].  It is intended to live on a worker thread:
//! the platform audio callback can copy/down-mix into a bounded queue, while the
//! worker feeds arbitrary-sized chunks here and publishes the returned `Copy`
//! snapshot.

use std::f32::consts::PI;

const HISTORY_HOPS: usize = 4096;
const MAX_ONSETS: usize = 256;
const ANALYSIS_SECONDS: f64 = 12.0;
// A locked deck also keeps a short, recent-only view. The long view is what
// makes mastered/compressed material stable, while the short view prevents a
// previous song from voting for its BPM for most of the 12-second history.
const CHANGE_ANALYSIS_SECONDS: f64 = 3.0;
const MIN_BPM: f64 = 70.0;
const MAX_BPM: f64 = 180.0;
const FILTER_BANDS: usize = 4;
const CROSSOVERS: usize = FILTER_BANDS - 1;

/// How trustworthy and current the tracked grid is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BeatLockState {
    #[default]
    Unlocked,
    Acquiring,
    Locked,
    /// A previously good grid is being extrapolated through a short dropout.
    Holdover,
    /// A previously good grid has gone stale and must not drive hard sync.
    Lost,
}

/// A coherent, trivially-copyable view of the beat grid at one sample instant.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BeatSnapshot {
    pub sample_rate: f64,
    pub sample_position: u64,
    pub bpm: f64,
    pub beat_period_samples: f64,
    /// Most recent predicted whole-beat boundary at or before `sample_position`.
    pub phase_sample: f64,
    pub beat_index: i64,
    pub confidence: f32,
    pub state: BeatLockState,
    pub last_onset_sample: Option<u64>,
}

impl BeatSnapshot {
    pub fn has_grid(&self) -> bool {
        self.beat_period_samples.is_finite() && self.beat_period_samples > 1.0
    }

    pub fn is_locked(&self) -> bool {
        matches!(self.state, BeatLockState::Locked | BeatLockState::Holdover)
    }

    pub fn seconds_per_beat(&self) -> Option<f64> {
        self.has_grid()
            .then_some(self.beat_period_samples / self.sample_rate)
    }

    /// Fractional beat phase in `[0, 1)` for an absolute sample position.
    pub fn phase_at(&self, sample: u64) -> Option<f64> {
        if !self.has_grid() {
            return None;
        }
        Some((sample as f64 - self.phase_sample).rem_euclid(self.beat_period_samples)
            / self.beat_period_samples)
    }

    /// First strict future boundary of a beat subdivision.
    pub fn next_boundary(&self, after_sample: u64, subdivision: u32) -> Option<u64> {
        next_boundary_sample(
            after_sample,
            self.phase_sample,
            self.beat_period_samples,
            subdivision,
        )
    }
}

/// Return the first subdivision boundary strictly after `after_sample`.
///
/// Computing from the phase reference instead of incrementing a cached deadline
/// means late UI frames automatically skip every missed boundary.
pub fn next_boundary_sample(
    after_sample: u64,
    phase_sample: f64,
    beat_period_samples: f64,
    subdivision: u32,
) -> Option<u64> {
    if !phase_sample.is_finite()
        || !beat_period_samples.is_finite()
        || beat_period_samples <= 1.0
        || subdivision == 0
    {
        return None;
    }
    let step = beat_period_samples / subdivision as f64;
    if step < 1.0 {
        return None;
    }
    let n = ((after_sample as f64 - phase_sample) / step).floor() + 1.0;
    let boundary = phase_sample + n * step;
    if boundary.is_finite() && boundary >= 0.0 {
        Some(boundary.ceil() as u64)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BeatFit {
    pub beats: u32,
    /// Source playback multiplier. `1.02` plays two percent faster.
    pub playback_rate: f64,
    pub target_duration_seconds: f64,
    pub relative_rate_error: f64,
    pub confidence: f32,
    pub within_rate_limit: bool,
}

/// Fit a source loop to a musically useful power-of-two beat count.
pub fn fit_loop_to_grid(
    source_duration_seconds: f64,
    bpm: f64,
    max_rate_deviation: f64,
) -> Option<BeatFit> {
    const MUSICAL_LENGTHS: &[u32] = &[1, 2, 4, 8, 16, 32];
    fit_loop_to_beats(
        source_duration_seconds,
        bpm,
        MUSICAL_LENGTHS,
        max_rate_deviation,
    )
}

/// Fit a source loop to one of `allowed_beats` and report whether the needed
/// rate is inside the caller's A/V-safe range.
pub fn fit_loop_to_beats(
    source_duration_seconds: f64,
    bpm: f64,
    allowed_beats: &[u32],
    max_rate_deviation: f64,
) -> Option<BeatFit> {
    if !source_duration_seconds.is_finite()
        || source_duration_seconds <= 0.0
        || !bpm.is_finite()
        || !(MIN_BPM..=MAX_BPM).contains(&bpm)
        || !max_rate_deviation.is_finite()
        || max_rate_deviation < 0.0
    {
        return None;
    }

    let mut best: Option<BeatFit> = None;
    for &beats in allowed_beats {
        if beats == 0 {
            continue;
        }
        let target = beats as f64 * 60.0 / bpm;
        let rate = source_duration_seconds / target;
        let error = (rate - 1.0).abs();
        let within = error <= max_rate_deviation;
        // Below the safe limit confidence stays high. Beyond it, return a useful
        // degraded proposal rather than silently claiming that sync is safe.
        let confidence = if max_rate_deviation > 0.0 && within {
            (1.0 - 0.25 * error / max_rate_deviation) as f32
        } else if error <= f64::EPSILON {
            1.0
        } else {
            (0.5 * (-4.0 * (error - max_rate_deviation).max(0.0)).exp()) as f32
        };
        let fit = BeatFit {
            beats,
            playback_rate: rate,
            target_duration_seconds: target,
            relative_rate_error: error,
            confidence,
            within_rate_limit: within,
        };
        if best
            .as_ref()
            .map(|old| {
                error < old.relative_rate_error - 1e-9
                    || ((error - old.relative_rate_error).abs() <= 1e-9 && beats < old.beats)
            })
            .unwrap_or(true)
        {
            best = Some(fit);
        }
    }
    best
}

#[derive(Clone, Copy, Debug, Default)]
struct Onset {
    sample: u64,
    strength: f32,
}

#[derive(Clone, Copy, Debug)]
struct TempoEstimate {
    period: f64,
    phase_origin: f64,
    phase_quality: f32,
    support: usize,
    aligned_support: usize,
    best_score: f32,
    confidence: f32,
    recent: bool,
}

/// Streaming mono transient/tempo/phase tracker.
pub struct BeatSyncAnalyzer {
    sample_rate: f64,
    hop_size: usize,
    sample_clock: u64,
    hop_samples: usize,
    hop_square_sum: f64,
    hop_band_square_sum: [f64; FILTER_BANDS],
    previous_input: f32,
    dc_output: f32,
    dc_coefficient: f32,
    lowpass_state: [f32; CROSSOVERS],
    lowpass_alpha: [f32; CROSSOVERS],
    band_previous_log_energy: [f32; FILTER_BANDS],
    band_flux_mean: [f32; FILTER_BANDS],
    band_flux_variance: [f32; FILTER_BANDS],
    band_novelty_tail: [f32; FILTER_BANDS],
    peak_left: f32,
    peak_middle: f32,
    peak_middle_sample: u64,
    last_onset_sample: Option<u64>,
    onset_history: [f32; HISTORY_HOPS],
    band_onset_history: [[f32; HISTORY_HOPS]; FILTER_BANDS],
    history_write: usize,
    history_count: usize,
    onsets: [Onset; MAX_ONSETS],
    onset_write: usize,
    onset_count: usize,
    hops_since_analysis: usize,
    grid_origin: f64,
    period_samples: f64,
    confidence: f32,
    candidate_period: f64,
    candidate_confidence: f32,
    candidate_streak: u8,
    change_period: f64,
    change_phase_origin: f64,
    change_confidence: f32,
    change_streak: u8,
    change_misses: u8,
    change_published: bool,
    change_previous_period: f64,
    change_previous_origin: f64,
    change_previous_confidence: f32,
    change_previous_state: BeatLockState,
    state: BeatLockState,
    ever_locked: bool,
}

impl BeatSyncAnalyzer {
    pub fn new(sample_rate: f64) -> Self {
        assert!(sample_rate.is_finite() && sample_rate >= 8_000.0);
        let dc_coefficient = (-2.0 * PI * 25.0 / sample_rate as f32).exp();
        let mut lowpass_alpha = [0.0; CROSSOVERS];
        for (alpha, cutoff) in lowpass_alpha.iter_mut().zip([170.0f32, 700.0, 2_800.0]) {
            let cutoff = cutoff.min(sample_rate as f32 * 0.4);
            *alpha = 1.0 - (-2.0 * PI * cutoff / sample_rate as f32).exp();
        }
        Self {
            sample_rate,
            hop_size: (sample_rate / 100.0).round().max(32.0) as usize,
            sample_clock: 0,
            hop_samples: 0,
            hop_square_sum: 0.0,
            hop_band_square_sum: [0.0; FILTER_BANDS],
            previous_input: 0.0,
            dc_output: 0.0,
            dc_coefficient,
            lowpass_state: [0.0; CROSSOVERS],
            lowpass_alpha,
            band_previous_log_energy: [0.0; FILTER_BANDS],
            band_flux_mean: [0.0; FILTER_BANDS],
            band_flux_variance: [1e-6; FILTER_BANDS],
            band_novelty_tail: [0.0; FILTER_BANDS],
            peak_left: 0.0,
            peak_middle: 0.0,
            peak_middle_sample: 0,
            last_onset_sample: None,
            onset_history: [0.0; HISTORY_HOPS],
            band_onset_history: [[0.0; HISTORY_HOPS]; FILTER_BANDS],
            history_write: 0,
            history_count: 0,
            onsets: [Onset::default(); MAX_ONSETS],
            onset_write: 0,
            onset_count: 0,
            hops_since_analysis: 0,
            grid_origin: 0.0,
            period_samples: 0.0,
            confidence: 0.0,
            candidate_period: 0.0,
            candidate_confidence: 0.0,
            candidate_streak: 0,
            change_period: 0.0,
            change_phase_origin: 0.0,
            change_confidence: 0.0,
            change_streak: 0,
            change_misses: 0,
            change_published: false,
            change_previous_period: 0.0,
            change_previous_origin: 0.0,
            change_previous_confidence: 0.0,
            change_previous_state: BeatLockState::Unlocked,
            state: BeatLockState::Unlocked,
            ever_locked: false,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.sample_rate);
    }

    /// Feed arbitrary callback-sized mono PCM. Non-finite samples are ignored.
    /// This method performs no heap allocation.
    pub fn push_mono(&mut self, samples: &[f32]) -> BeatSnapshot {
        for &sample in samples {
            let sample = if sample.is_finite() {
                sample.clamp(-8.0, 8.0)
            } else {
                0.0
            };
            // A DC blocker followed by three one-pole crossovers is cheap
            // enough to run sample-by-sample, and is much more revealing than
            // broadband RMS on mastered material. Kick, body, presence and
            // high-frequency transients each get an independent novelty lane.
            let dc = sample - self.previous_input + self.dc_coefficient * self.dc_output;
            self.previous_input = sample;
            self.dc_output = dc;
            for index in 0..CROSSOVERS {
                self.lowpass_state[index] +=
                    self.lowpass_alpha[index] * (dc - self.lowpass_state[index]);
            }
            let bands = [
                self.lowpass_state[0],
                self.lowpass_state[1] - self.lowpass_state[0],
                self.lowpass_state[2] - self.lowpass_state[1],
                dc - self.lowpass_state[2],
            ];
            self.hop_square_sum += (sample as f64) * (sample as f64);
            for (sum, band) in self.hop_band_square_sum.iter_mut().zip(bands) {
                *sum += (band as f64) * (band as f64);
            }
            self.hop_samples += 1;
            self.sample_clock = self.sample_clock.saturating_add(1);
            if self.hop_samples == self.hop_size {
                self.finish_hop();
            }
        }
        self.snapshot()
    }

    pub fn snapshot(&self) -> BeatSnapshot {
        let (phase_sample, beat_index) = if self.period_samples > 1.0 {
            let index = ((self.sample_clock as f64 - self.grid_origin) / self.period_samples)
                .floor() as i64;
            (
                self.grid_origin + index as f64 * self.period_samples,
                index,
            )
        } else {
            (0.0, 0)
        };
        BeatSnapshot {
            sample_rate: self.sample_rate,
            sample_position: self.sample_clock,
            bpm: if self.period_samples > 1.0 {
                60.0 * self.sample_rate / self.period_samples
            } else {
                0.0
            },
            beat_period_samples: self.period_samples,
            phase_sample,
            beat_index,
            confidence: self.confidence.clamp(0.0, 1.0),
            state: self.state,
            last_onset_sample: self.last_onset_sample,
        }
    }

    fn finish_hop(&mut self) {
        let first_hop = self.history_count == 0;
        let _broadband_rms = (self.hop_square_sum / self.hop_samples as f64).sqrt() as f32;
        self.hop_square_sum = 0.0;
        let hop_samples = self.hop_samples;
        self.hop_samples = 0;

        let mut band_novelty = [0.0f32; FILTER_BANDS];
        let weights = [1.30f32, 1.05, 0.90, 0.65];
        let mut novelty_square_sum = 0.0;
        let mut weight_sum = 0.0;
        for band in 0..FILTER_BANDS {
            let rms = (self.hop_band_square_sum[band] / hop_samples as f64).sqrt() as f32;
            self.hop_band_square_sum[band] = 0.0;

            // Log compression makes a 2 dB transient useful whether system
            // audio arrives at -35 dBFS or has been limited close to full
            // scale. Each lane adapts to its own stationary flux distribution.
            let log_energy = (1.0 + 96.0 * rms).ln();
            let flux = if first_hop {
                0.0
            } else {
                (log_energy - self.band_previous_log_energy[band]).max(0.0)
            };
            self.band_previous_log_energy[band] = log_energy;

            let deviation = self.band_flux_variance[band].max(1e-8).sqrt();
            let threshold = self.band_flux_mean[band] + 0.55 * deviation + 0.0015;
            let novelty = ((flux - threshold).max(0.0) / (deviation + 0.006)).min(6.0);

            // Let a transient occupy two or three 10 ms cells. This makes the
            // lag correlation tolerant of non-integral beat periods without
            // delaying the local-maximum timestamp used for phase.
            self.band_novelty_tail[band] = novelty.max(self.band_novelty_tail[band] * 0.42);
            band_novelty[band] = self.band_novelty_tail[band];
            novelty_square_sum += weights[band] * band_novelty[band] * band_novelty[band];
            weight_sum += weights[band];

            // Do not let a single large kick raise the floor enough to hide
            // the following snare. The clipped value still follows changing
            // programme material within roughly half a second.
            let robust_flux = flux.min(self.band_flux_mean[band] + 2.5 * deviation + 0.018);
            let delta = robust_flux - self.band_flux_mean[band];
            self.band_flux_mean[band] += 0.020 * delta;
            self.band_flux_variance[band] =
                (0.980 * self.band_flux_variance[band] + 0.020 * delta * delta).max(1e-8);
        }
        let onset_value = (novelty_square_sum / weight_sum).sqrt().min(6.0);

        self.onset_history[self.history_write] = onset_value;
        for band in 0..FILTER_BANDS {
            self.band_onset_history[band][self.history_write] = band_novelty[band];
        }
        self.history_write = (self.history_write + 1) % HISTORY_HOPS;
        self.history_count = (self.history_count + 1).min(HISTORY_HOPS);

        if self.peak_middle > self.peak_left
            && self.peak_middle >= onset_value
            && self.peak_middle > 0.18
        {
            let refractory = (self.sample_rate * 0.09) as u64;
            if self
                .last_onset_sample
                .map(|last| self.peak_middle_sample.saturating_sub(last) >= refractory)
                .unwrap_or(true)
            {
                self.record_onset(self.peak_middle_sample, self.peak_middle);
            }
        }
        self.peak_left = self.peak_middle;
        self.peak_middle = onset_value;
        self.peak_middle_sample = self.sample_clock.saturating_sub((self.hop_size / 2) as u64);

        self.hops_since_analysis += 1;
        let analysis_interval = (self.sample_rate / self.hop_size as f64 * 0.25)
            .round()
            .max(1.0) as usize;
        if self.hops_since_analysis >= analysis_interval {
            self.hops_since_analysis = 0;
            self.analyze_grid();
        }
        self.update_dropout_state();
    }

    fn record_onset(&mut self, sample: u64, strength: f32) {
        self.onsets[self.onset_write] = Onset { sample, strength };
        self.onset_write = (self.onset_write + 1) % MAX_ONSETS;
        self.onset_count = (self.onset_count + 1).min(MAX_ONSETS);
        self.last_onset_sample = Some(sample);
        if matches!(self.state, BeatLockState::Unlocked | BeatLockState::Lost) {
            self.state = BeatLockState::Acquiring;
        }
    }

    fn history(&self, logical_index: usize) -> f32 {
        let oldest = (self.history_write + HISTORY_HOPS - self.history_count) % HISTORY_HOPS;
        self.onset_history[(oldest + logical_index) % HISTORY_HOPS]
    }

    fn band_history(&self, band: usize, logical_index: usize) -> f32 {
        let oldest = (self.history_write + HISTORY_HOPS - self.history_count) % HISTORY_HOPS;
        self.band_onset_history[band][(oldest + logical_index) % HISTORY_HOPS]
    }

    fn onset(&self, logical_index: usize) -> Onset {
        let oldest = (self.onset_write + MAX_ONSETS - self.onset_count) % MAX_ONSETS;
        self.onsets[(oldest + logical_index) % MAX_ONSETS]
    }

    fn correlation_at(&self, band: Option<usize>, lag: usize, used: usize) -> f32 {
        if lag >= used || used - lag < 16 {
            return 0.0;
        }
        let offset = self.history_count - used;
        let count = used - lag;
        let mut sx = 0.0f64;
        let mut sy = 0.0f64;
        let mut sxx = 0.0f64;
        let mut syy = 0.0f64;
        let mut sxy = 0.0f64;
        for i in lag..used {
            let x = band
                .map(|band| self.band_history(band, offset + i))
                .unwrap_or_else(|| self.history(offset + i)) as f64;
            let y = band
                .map(|band| self.band_history(band, offset + i - lag))
                .unwrap_or_else(|| self.history(offset + i - lag)) as f64;
            sx += x;
            sy += y;
            sxx += x * x;
            syy += y * y;
            sxy += x * y;
        }
        let n = count as f64;
        let covariance = sxy - sx * sy / n;
        let vx = (sxx - sx * sx / n).max(0.0);
        let vy = (syy - sy * sy / n).max(0.0);
        if vx <= 1e-12 || vy <= 1e-12 {
            0.0
        } else {
            (covariance / (vx * vy).sqrt()).clamp(-1.0, 1.0) as f32
        }
    }

    fn direct_tempo_score(&self, lag: usize, used: usize) -> f32 {
        let composite = self.correlation_at(None, lag, used).max(0.0);
        let mut strongest = 0.0f32;
        let mut second = 0.0f32;
        for band in 0..FILTER_BANDS {
            let score = self.correlation_at(Some(band), lag, used).max(0.0);
            if score > strongest {
                second = strongest;
                strongest = score;
            } else if score > second {
                second = score;
            }
        }

        // Separate band correlations resolve the classic kick/snare ambiguity:
        // their combined envelope repeats every half beat, while each timbral
        // lane repeats at the intended beat period.
        (0.24 * composite + 0.55 * strongest + 0.21 * second).clamp(0.0, 1.0)
    }

    fn tempo_score_at(&self, lag: usize, used: usize) -> f32 {
        let direct = self.direct_tempo_score(lag, used);
        let second = self.direct_tempo_score(lag * 2, used);
        let third = self.direct_tempo_score(lag * 3, used);
        (0.82 * direct + 0.12 * second + 0.06 * third).clamp(0.0, 1.0)
    }

    fn refine_period(&self, initial_period: f64, oldest_allowed: u64) -> f64 {
        let mut period = initial_period;
        // Pairwise IOIs retain useful whole-beat intervals even when adjacent
        // detected events are eighth-note hats or alternating kick/snare hits.
        for _ in 0..2 {
            let mut sum = 0.0;
            let mut weight_sum = 0.0;
            for later_index in 0..self.onset_count {
                let later = self.onset(later_index);
                if later.sample < oldest_allowed {
                    continue;
                }
                for earlier_index in 0..later_index {
                    let earlier = self.onset(earlier_index);
                    if earlier.sample < oldest_allowed {
                        continue;
                    }
                    let difference = later.sample.saturating_sub(earlier.sample) as f64;
                    let multiple = (difference / period).round();
                    if !(1.0..=16.0).contains(&multiple) {
                        continue;
                    }
                    let estimate = difference / multiple;
                    if (estimate / period - 1.0).abs() <= 0.105 {
                        let strength = later.strength.min(earlier.strength).clamp(0.05, 4.0);
                        let weight = strength as f64 / multiple.sqrt();
                        sum += estimate * weight;
                        weight_sum += weight;
                    }
                }
            }
            if weight_sum > 0.0 {
                period = sum / weight_sum;
            }
        }
        period
    }

    fn estimate_tempo(&self, used: usize) -> Option<TempoEstimate> {
        if used > self.history_count || used < 16 || self.onset_count < 4 {
            return None;
        }
        let hop_rate = self.sample_rate / self.hop_size as f64;
        let min_lag = (60.0 * hop_rate / MAX_BPM).floor().max(2.0) as usize;
        let max_lag = (60.0 * hop_rate / MIN_BPM).ceil() as usize;
        let mut best_lag = 0usize;
        let mut best_score = 0.0f32;
        let mut runner_up = 0.0f32;
        for lag in min_lag..=max_lag.min(used / 2) {
            let score = self.tempo_score_at(lag, used);
            if score > best_score {
                if best_lag.abs_diff(lag) > 3 {
                    runner_up = best_score.max(runner_up);
                }
                best_score = score;
                best_lag = lag;
            } else if best_lag.abs_diff(lag) > 3 {
                runner_up = runner_up.max(score);
            }
        }
        if best_lag == 0 {
            return None;
        }

        // Autocorrelation cannot intrinsically distinguish a period from an
        // equally regular two-period bar pattern. Only when an octave mate is
        // genuinely competitive, use a broad musical-rate prior centred at
        // 120 BPM as the tie-breaker (90 beats 180; 150 beats 75).
        for octave_lag in [best_lag / 2, best_lag.saturating_mul(2)] {
            if octave_lag < min_lag || octave_lag > max_lag.min(used / 2) {
                continue;
            }
            let octave_score = self.tempo_score_at(octave_lag, used);
            if octave_score < 0.72 * best_score {
                continue;
            }
            let best_bpm = 60.0 * hop_rate / best_lag as f64;
            let octave_bpm = 60.0 * hop_rate / octave_lag as f64;
            let best_prior = (-0.5 * ((best_bpm / 120.0).ln() / 0.38).powi(2)).exp();
            let octave_prior = (-0.5 * ((octave_bpm / 120.0).ln() / 0.38).powi(2)).exp();
            if octave_prior > best_prior {
                runner_up = runner_up.max(best_score);
                best_lag = octave_lag;
                best_score = octave_score;
            }
        }

        let oldest_allowed = self
            .sample_clock
            .saturating_sub((used * self.hop_size) as u64);
        let period = self.refine_period(best_lag as f64 * self.hop_size as f64, oldest_allowed);
        let bpm = 60.0 * self.sample_rate / period;
        if !(MIN_BPM..=MAX_BPM).contains(&bpm) {
            return None;
        }

        let (phase_origin, phase_quality, support, aligned_support) =
            self.estimate_phase(period, oldest_allowed);
        let separation = ((best_score - runner_up).max(0.0) / best_score.max(1e-5)).min(1.0);
        let support_quality = (aligned_support as f32 / 6.0).min(1.0);
        let estimate_confidence = ((0.70 * best_score + 0.30 * phase_quality)
            * (0.80 + 0.20 * support_quality)
            * (0.92 + 0.08 * separation))
            .clamp(0.0, 1.0);

        let recent = self
            .last_onset_sample
            .map(|last| self.sample_clock.saturating_sub(last) as f64 <= 2.25 * period)
            .unwrap_or(false);
        Some(TempoEstimate {
            period,
            phase_origin,
            phase_quality,
            support,
            aligned_support,
            best_score,
            confidence: estimate_confidence,
            recent,
        })
    }

    fn analyze_grid(&mut self) {
        let hop_rate = self.sample_rate / self.hop_size as f64;
        let used = self
            .history_count
            .min((ANALYSIS_SECONDS * hop_rate).round() as usize);
        if used < (3.0 * hop_rate) as usize || self.onset_count < 4 {
            return;
        }

        let long_estimate = self.estimate_tempo(used);
        if self.ever_locked {
            let recent_hops = (CHANGE_ANALYSIS_SECONDS * hop_rate).round() as usize;
            if self.history_count >= recent_hops {
                let recent_estimate = self.estimate_tempo(recent_hops);
                if self.track_tempo_change(recent_estimate, recent_hops) {
                    return;
                }
            }
        }
        let Some(estimate) = long_estimate else {
            return;
        };
        self.apply_tempo_estimate(estimate);
    }

    fn track_tempo_change(
        &mut self,
        estimate: Option<TempoEstimate>,
        recent_hops: usize,
    ) -> bool {
        let credible = estimate.is_some_and(|estimate| {
            estimate.best_score >= 0.24
                && estimate.confidence >= 0.30
                && estimate.phase_quality >= 0.18
                && estimate.aligned_support >= 3
                && estimate.support >= 4
                && estimate.recent
        });

        if !credible {
            if self.change_streak > 0 {
                self.change_misses = self.change_misses.saturating_add(1);
                let miss_limit = if self.change_published { 8 } else { 2 };
                if self.change_misses >= miss_limit {
                    self.cancel_change_candidate();
                }
            }
            return self.change_streak > 0;
        }
        let estimate = estimate.unwrap();

        if self.change_streak == 0 {
            if self.period_samples <= 1.0
                || (estimate.period / self.period_samples - 1.0).abs() < 0.14
            {
                return false;
            }

            // The recent window must actively prefer the alternative over the
            // established grid. This is the key guard against treating a quiet
            // breakdown or a newly prominent off-beat layer as a song change.
            let locked_lag = (self.period_samples / self.hop_size as f64)
                .round()
                .max(1.0) as usize;
            let locked_score = self.tempo_score_at(locked_lag, recent_hops);
            let beats_locked_grid = estimate.best_score >= locked_score + 0.035
                || estimate.best_score >= locked_score * 1.12;
            if !beats_locked_grid {
                return false;
            }

            let ratio = estimate.period / self.period_samples;
            let octave_jump = (1.82..=2.18).contains(&ratio)
                || ((1.0 / 2.18)..=(1.0 / 1.82)).contains(&ratio);
            if octave_jump
                && estimate.confidence < (self.confidence * 1.25).max(0.74)
            {
                return false;
            }

            self.change_period = estimate.period;
            self.change_phase_origin = estimate.phase_origin;
            self.change_confidence = estimate.confidence;
            self.change_streak = 1;
            self.change_misses = 0;
            self.change_previous_period = self.period_samples;
            self.change_previous_origin = self.grid_origin;
            self.change_previous_confidence = self.confidence;
            self.change_previous_state = self.state;
        } else if (estimate.period / self.change_period - 1.0).abs() <= 0.065 {
            self.change_period = self.change_period * 0.62 + estimate.period * 0.38;
            self.change_phase_origin = estimate.phase_origin;
            self.change_confidence =
                self.change_confidence * 0.55 + estimate.confidence * 0.45;
            self.change_streak = self.change_streak.saturating_add(1);
            self.change_misses = 0;
        } else {
            self.change_misses = self.change_misses.saturating_add(1);
            let miss_limit = if self.change_published { 8 } else { 2 };
            if self.change_misses >= miss_limit {
                self.cancel_change_candidate();
            }
            return self.change_streak > 0;
        }

        if self.change_streak >= 2 && self.change_confidence >= 0.32 {
            self.period_samples = self.change_period;
            self.grid_origin = self.change_phase_origin;
            self.confidence = self.change_confidence;
            self.state = BeatLockState::Acquiring;
            self.change_published = true;
        }

        if self.change_streak >= 7
            && self.change_confidence >= 0.36
            && estimate.best_score >= 0.24
            && estimate.phase_quality >= 0.18
        {
            self.period_samples = self.change_period;
            self.grid_origin = self.change_phase_origin;
            self.confidence = self.change_confidence;
            self.state = BeatLockState::Locked;
            self.candidate_period = self.change_period;
            self.candidate_confidence = self.change_confidence;
            self.candidate_streak = 5;
            self.trim_tempo_history(CHANGE_ANALYSIS_SECONDS);
            self.clear_change_candidate();
        }
        true
    }

    fn clear_change_candidate(&mut self) {
        self.change_period = 0.0;
        self.change_phase_origin = 0.0;
        self.change_confidence = 0.0;
        self.change_streak = 0;
        self.change_misses = 0;
        self.change_published = false;
        self.change_previous_period = 0.0;
        self.change_previous_origin = 0.0;
        self.change_previous_confidence = 0.0;
        self.change_previous_state = BeatLockState::Unlocked;
    }

    fn cancel_change_candidate(&mut self) {
        if self.change_published && self.change_previous_period > 1.0 {
            self.period_samples = self.change_previous_period;
            self.grid_origin = self.change_previous_origin;
            self.confidence = self.change_previous_confidence;
            self.state = self.change_previous_state;
        }
        self.clear_change_candidate();
    }

    fn trim_tempo_history(&mut self, seconds: f64) {
        let keep_hops = (seconds * self.sample_rate / self.hop_size as f64).round() as usize;
        self.history_count = self.history_count.min(keep_hops);
        let oldest_allowed = self
            .sample_clock
            .saturating_sub((seconds * self.sample_rate) as u64);
        let mut keep_onsets = 0usize;
        for index in 0..self.onset_count {
            if self.onset(index).sample >= oldest_allowed {
                keep_onsets += 1;
            }
        }
        self.onset_count = keep_onsets;
    }

    fn apply_tempo_estimate(&mut self, estimate: TempoEstimate) {
        let TempoEstimate {
            period,
            phase_origin,
            phase_quality,
            support,
            aligned_support,
            best_score,
            confidence: estimate_confidence,
            recent,
        } = estimate;
        // Tempo stability and phase quality are related but distinct. A dense
        // eighth-note texture can have a very stable quarter-note recurrence
        // while producing many off-grid local peaks; do not reset the tempo
        // streak merely because those peaks dilute the phase histogram.
        let credible = best_score >= 0.20 && estimate_confidence >= 0.27 && recent;
        if !credible {
            if !self.ever_locked {
                self.candidate_streak = self.candidate_streak.saturating_sub(1);
                self.candidate_confidence = estimate_confidence;
                if support > 0 {
                    self.state = BeatLockState::Acquiring;
                }
            }
            return;
        }

        if self.candidate_period > 1.0 && (period / self.candidate_period - 1.0).abs() <= 0.065 {
            self.candidate_period = self.candidate_period * 0.65 + period * 0.35;
            self.candidate_streak = self.candidate_streak.saturating_add(1);
        } else {
            self.candidate_period = period;
            self.candidate_streak = 1;
        }
        self.candidate_confidence = if self.candidate_streak > 1 {
            self.candidate_confidence * 0.55 + estimate_confidence * 0.45
        } else {
            estimate_confidence
        };

        // Publish a deliberately provisional grid after two consistent
        // analyses. The UI can show a useful BPM around 3--5 seconds while
        // transport-changing callers continue to gate on `is_locked()`.
        if self.candidate_streak >= 2
            && phase_quality >= 0.18
            && aligned_support >= 2
            && !self.ever_locked
        {
            self.period_samples = self.candidate_period;
            self.grid_origin = phase_origin;
            self.confidence = self.candidate_confidence;
            self.state = BeatLockState::Acquiring;
        }

        let lock_ready = self.candidate_streak >= 5
            && best_score >= 0.22
            && phase_quality >= 0.18
            && aligned_support >= 2
            && self.candidate_confidence >= 0.36;
        if lock_ready {
            let tracked_period = self.candidate_period;
            if self.ever_locked && self.period_samples > 1.0 {
                let ratio = tracked_period / self.period_samples;
                let octave_jump = (1.82..=2.18).contains(&ratio)
                    || ((1.0 / 2.18)..=(1.0 / 1.82)).contains(&ratio);
                // Half/double-time peaks are both mathematically legitimate.
                // Once phase is established, require substantially stronger
                // evidence before changing octave instead of letting a newly
                // prominent hat or half-time snare flip the displayed BPM.
                if octave_jump
                    && self.candidate_confidence < (self.confidence * 1.25).max(0.72)
                {
                    self.state = BeatLockState::Locked;
                    return;
                }
            }
            if self.period_samples > 1.0
                && (tracked_period / self.period_samples - 1.0).abs() < 0.12
                && self.ever_locked
            {
                self.period_samples = self.period_samples * 0.82 + tracked_period * 0.18;
                let now = self.sample_clock as f64;
                let predicted = self.grid_origin
                    + ((now - self.grid_origin) / self.period_samples).round()
                        * self.period_samples;
                let measured = phase_origin
                    + ((now - phase_origin) / self.period_samples).round()
                        * self.period_samples;
                let correction = (measured - predicted)
                    .clamp(-0.18 * self.period_samples, 0.18 * self.period_samples);
                self.grid_origin += correction * 0.28;
            } else {
                self.period_samples = tracked_period;
                self.grid_origin = phase_origin;
            }
            self.confidence = if self.ever_locked {
                self.confidence * 0.72 + self.candidate_confidence * 0.28
            } else {
                self.candidate_confidence
            };
            self.state = BeatLockState::Locked;
            self.ever_locked = true;
        }
    }

    fn estimate_phase(
        &self,
        period: f64,
        oldest_allowed: u64,
    ) -> (f64, f32, usize, usize) {
        let mut best_origin = 0.0;
        let mut best_score = 0.0f64;
        let mut best_aligned = 0usize;
        let mut total_strength = 0.0f64;
        let mut support = 0usize;
        for i in 0..self.onset_count {
            let onset = self.onset(i);
            if onset.sample >= oldest_allowed {
                total_strength += onset.strength.max(1e-5) as f64;
                support += 1;
            }
        }
        if support == 0 || total_strength <= 0.0 {
            return (0.0, 0.0, 0, 0);
        }
        let sigma = period * 0.075;
        for candidate_index in 0..self.onset_count {
            let candidate = self.onset(candidate_index);
            if candidate.sample < oldest_allowed {
                continue;
            }
            let origin = candidate.sample as f64;
            let mut score = 0.0;
            let mut aligned = 0usize;
            for i in 0..self.onset_count {
                let onset = self.onset(i);
                if onset.sample < oldest_allowed {
                    continue;
                }
                let remainder = (onset.sample as f64 - origin).rem_euclid(period);
                let distance = remainder.min(period - remainder);
                let kernel = (-0.5 * (distance / sigma).powi(2)).exp();
                score += kernel * onset.strength.max(1e-5) as f64;
                if distance <= 0.12 * period {
                    aligned += 1;
                }
            }
            if score > best_score {
                best_score = score;
                best_origin = origin;
                best_aligned = aligned;
            }
        }
        (
            best_origin,
            (best_score / total_strength).clamp(0.0, 1.0) as f32,
            support,
            best_aligned,
        )
    }

    fn update_dropout_state(&mut self) {
        if !self.ever_locked || self.period_samples <= 1.0 {
            return;
        }
        let age_beats = self
            .last_onset_sample
            .map(|last| self.sample_clock.saturating_sub(last) as f64 / self.period_samples)
            .unwrap_or(f64::INFINITY);
        if age_beats > 8.0 {
            self.state = BeatLockState::Lost;
            self.confidence = 0.0;
        } else if age_beats > 2.5 {
            self.state = BeatLockState::Holdover;
            let hold = ((8.0 - age_beats) / 5.5).clamp(0.0, 1.0) as f32;
            self.confidence = self.confidence.min(hold);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_track(sample_rate: usize, bpm: f64, seconds: f64) -> Vec<f32> {
        let len = (sample_rate as f64 * seconds) as usize;
        let mut samples = vec![0.0; len];
        let period = 60.0 * sample_rate as f64 / bpm;
        let mut beat = (0.23 * sample_rate as f64) as usize;
        while beat < len {
            for i in 0..(sample_rate / 500).max(4) {
                if beat + i < len {
                    samples[beat + i] = (1.0 - i as f32 / (sample_rate / 500).max(4) as f32)
                        .max(0.0);
                }
            }
            beat = (beat as f64 + period).round() as usize;
        }
        samples
    }

    fn compressed_music_mix(sample_rate: usize, bpm: f64, seconds: f64) -> Vec<f32> {
        let len = (sample_rate as f64 * seconds) as usize;
        let period = 60.0 * sample_rate as f64 / bpm;
        let half_period = period * 0.5;
        let start = 0.19 * sample_rate as f64;
        let mut seed = 0x93c4_6a7du32;
        let mut previous_noise = 0.0f32;
        let mut samples = Vec::with_capacity(len);
        for index in 0..len {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
            let high_noise = noise - previous_noise;
            previous_noise = noise;

            let time = index as f64 / sample_rate as f64;
            let mut sample = 0.10 * (2.0 * std::f64::consts::PI * 196.0 * time).sin() as f32
                + 0.07 * (2.0 * std::f64::consts::PI * 293.67 * time).sin() as f32
                + 0.018 * noise;
            if index as f64 >= start {
                let position = index as f64 - start;
                let beat_index = (position / period).floor() as usize;
                let beat_seconds = (position - beat_index as f64 * period) / sample_rate as f64;
                let half_index = (position / half_period).floor() as usize;
                let half_seconds =
                    (position - half_index as f64 * half_period) / sample_rate as f64;

                let kick_accent = [1.0f32, 0.72, 0.86, 0.68][beat_index & 3];
                if beat_seconds < 0.20 {
                    let envelope = (-22.0 * beat_seconds).exp() as f32;
                    let sweep_phase = 2.0
                        * std::f64::consts::PI
                        * (78.0 * beat_seconds - 65.0 * beat_seconds * beat_seconds);
                    sample += 0.42 * kick_accent * envelope * sweep_phase.sin() as f32;
                }

                // Backbeat snare and eighth-note hats deliberately make the
                // broadband envelope busier than the intended quarter-note
                // grid. Their spectra remain independently periodic.
                if beat_index & 1 == 1 && beat_seconds < 0.11 {
                    let envelope = (-30.0 * beat_seconds).exp() as f32;
                    sample += 0.24 * envelope * high_noise
                        + 0.08
                            * envelope
                            * (2.0 * std::f64::consts::PI * 185.0 * time).sin() as f32;
                }
                if half_seconds < 0.032 {
                    let envelope = (-95.0 * half_seconds).exp() as f32;
                    let accent = if half_index & 1 == 0 { 0.10 } else { 0.075 };
                    sample += accent * envelope * high_noise;
                }

                let bass_envelope = 0.42 + 0.58 * (-7.0 * beat_seconds).exp() as f32;
                let bass_frequency = [55.0, 55.0, 65.41, 49.0][(beat_index / 4) & 3];
                sample += 0.16
                    * bass_envelope
                    * (2.0 * std::f64::consts::PI * bass_frequency * time).sin() as f32;
            }

            // A hard soft clip leaves little broadband level movement, like a
            // loud master or a system-capture path with gain normalization.
            samples.push((2.9 * sample).tanh() * 0.42);
        }
        samples
    }

    fn first_grid_and_lock(
        samples: &[f32],
        sample_rate: usize,
    ) -> (Option<f64>, Option<f64>, BeatSnapshot) {
        let mut analyzer = BeatSyncAnalyzer::new(sample_rate as f64);
        let mut first_grid = None;
        let mut first_lock = None;
        for chunk in samples.chunks(317) {
            let snapshot = analyzer.push_mono(chunk);
            let seconds = snapshot.sample_position as f64 / sample_rate as f64;
            if snapshot.has_grid() && first_grid.is_none() {
                first_grid = Some(seconds);
            }
            if snapshot.is_locked() && first_lock.is_none() {
                first_lock = Some(seconds);
            }
        }
        (first_grid, first_lock, analyzer.snapshot())
    }

    fn analyze_chunks(samples: &[f32], sample_rate: usize, chunks: &[usize]) -> BeatSnapshot {
        let mut analyzer = BeatSyncAnalyzer::new(sample_rate as f64);
        let mut offset = 0;
        let mut chunk_index = 0;
        while offset < samples.len() {
            let size = chunks[chunk_index % chunks.len()];
            let end = (offset + size).min(samples.len());
            analyzer.push_mono(&samples[offset..end]);
            offset = end;
            chunk_index += 1;
        }
        analyzer.snapshot()
    }

    fn tempo_change_timing(
        old_bpm: f64,
        new_bpm: f64,
    ) -> (Option<f64>, Option<f64>, BeatSnapshot) {
        const SAMPLE_RATE: usize = 48_000;
        let mut analyzer = BeatSyncAnalyzer::new(SAMPLE_RATE as f64);
        let old_song = compressed_music_mix(SAMPLE_RATE, old_bpm, 10.0);
        for chunk in old_song.chunks(317) {
            analyzer.push_mono(chunk);
        }
        let old_snapshot = analyzer.snapshot();
        assert!(old_snapshot.is_locked(), "old_bpm={old_bpm} {old_snapshot:?}");
        assert!(
            (old_snapshot.bpm - old_bpm).abs() < 1.8,
            "old_bpm={old_bpm} {old_snapshot:?}"
        );

        let new_song = compressed_music_mix(SAMPLE_RATE, new_bpm, 6.0);
        let mut consumed = 0usize;
        let mut first_provisional = None;
        let mut first_lock = None;
        for chunk in new_song.chunks(317) {
            let snapshot = analyzer.push_mono(chunk);
            consumed += chunk.len();
            let seconds = consumed as f64 / SAMPLE_RATE as f64;
            let new_tempo = (snapshot.bpm - new_bpm).abs() < 2.0;
            if new_tempo
                && snapshot.state == BeatLockState::Acquiring
                && first_provisional.is_none()
            {
                first_provisional = Some(seconds);
            }
            if new_tempo && snapshot.is_locked() && first_lock.is_none() {
                first_lock = Some(seconds);
            }
        }
        (first_provisional, first_lock, analyzer.snapshot())
    }

    #[test]
    fn locks_synthetic_tempos_at_both_sample_rates() {
        for &rate in &[44_100usize, 48_000] {
            for &bpm in &[90.0, 120.0, 150.0] {
                let samples = click_track(rate, bpm, 14.0);
                let snapshot = analyze_chunks(&samples, rate, &[127, 511, 64, 997]);
                assert!(
                    snapshot.is_locked(),
                    "rate={rate} bpm={bpm} snapshot={snapshot:?}"
                );
                assert!(
                    (snapshot.bpm - bpm).abs() < 1.6,
                    "rate={rate} bpm={bpm} snapshot={snapshot:?}"
                );
                assert!(snapshot.confidence > 0.50, "{snapshot:?}");
            }
        }
    }

    #[test]
    fn publishes_early_bpm_and_locks_compressed_music_by_eight_seconds() {
        for &bpm in &[90.0, 120.0, 150.0] {
            let samples = compressed_music_mix(48_000, bpm, 8.25);
            let (first_grid, first_lock, snapshot) = first_grid_and_lock(&samples, 48_000);
            assert!(
                first_grid.is_some_and(|seconds| seconds <= 5.0),
                "bpm={bpm} first_grid={first_grid:?} first_lock={first_lock:?} snapshot={snapshot:?}"
            );
            assert!(
                first_lock.is_some_and(|seconds| seconds <= 8.0),
                "bpm={bpm} first_grid={first_grid:?} first_lock={first_lock:?} snapshot={snapshot:?}"
            );
            assert!(
                (snapshot.bpm - bpm).abs() < 1.8,
                "bpm={bpm} first_grid={first_grid:?} first_lock={first_lock:?} snapshot={snapshot:?}"
            );
        }
    }

    #[test]
    fn reacquires_abrupt_compressed_music_tempo_changes_quickly() {
        for &(old_bpm, new_bpm) in &[(90.0, 140.0), (120.0, 85.0)] {
            let (first_provisional, first_lock, snapshot) =
                tempo_change_timing(old_bpm, new_bpm);
            assert!(
                first_provisional.is_some_and(|seconds| seconds <= 3.25),
                "{old_bpm}->{new_bpm} provisional={first_provisional:?} lock={first_lock:?} snapshot={snapshot:?}"
            );
            assert!(
                first_lock.is_some_and(|seconds| seconds <= 5.25),
                "{old_bpm}->{new_bpm} provisional={first_provisional:?} lock={first_lock:?} snapshot={snapshot:?}"
            );
            assert!(
                (snapshot.bpm - new_bpm).abs() < 2.0,
                "{old_bpm}->{new_bpm} snapshot={snapshot:?}"
            );
        }
    }

    #[test]
    fn ordinary_breakdown_preserves_the_established_grid() {
        const SAMPLE_RATE: usize = 48_000;
        let mut analyzer = BeatSyncAnalyzer::new(SAMPLE_RATE as f64);
        analyzer.push_mono(&compressed_music_mix(SAMPLE_RATE, 120.0, 10.0));
        let before = analyzer.snapshot();
        assert!(before.is_locked(), "{before:?}");

        let mut breakdown = Vec::with_capacity(SAMPLE_RATE * 2);
        for sample in 0..SAMPLE_RATE * 2 {
            let seconds = sample as f64 / SAMPLE_RATE as f64;
            breakdown.push(
                (0.16 * (2.0 * std::f64::consts::PI * 220.0 * seconds).sin()) as f32,
            );
        }
        for chunk in breakdown.chunks(317) {
            let snapshot = analyzer.push_mono(chunk);
            assert!(snapshot.has_grid(), "{snapshot:?}");
            assert!((snapshot.bpm - 120.0).abs() < 1.8, "{snapshot:?}");
            assert!(
                matches!(snapshot.state, BeatLockState::Locked | BeatLockState::Holdover),
                "{snapshot:?}"
            );
        }

        analyzer.push_mono(&compressed_music_mix(SAMPLE_RATE, 120.0, 3.0));
        let after = analyzer.snapshot();
        assert!(after.is_locked(), "{after:?}");
        assert!((after.bpm - 120.0).abs() < 1.8, "{after:?}");
    }

    #[test]
    fn arbitrary_chunking_keeps_the_same_grid() {
        let samples = click_track(48_000, 120.0, 13.0);
        let regular = analyze_chunks(&samples, 48_000, &[480]);
        let odd = analyze_chunks(&samples, 48_000, &[1, 37, 1024, 79, 333]);
        assert!((regular.bpm - odd.bpm).abs() < 0.01, "{regular:?} {odd:?}");
        assert!(
            (regular.phase_sample - odd.phase_sample).abs() <= 1.0,
            "{regular:?} {odd:?}"
        );
    }

    #[test]
    fn silence_and_stationary_noise_do_not_lock() {
        let silence = vec![0.0; 48_000 * 12];
        let snapshot = analyze_chunks(&silence, 48_000, &[256]);
        assert!(!snapshot.is_locked(), "{snapshot:?}");

        let mut seed = 0x1234_5678u32;
        let mut noise = vec![0.0; 48_000 * 12];
        for sample in &mut noise {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *sample = (((seed >> 8) as f32 / 16_777_216.0) - 0.5) * 0.04;
        }
        let snapshot = analyze_chunks(&noise, 48_000, &[197, 503]);
        assert!(!snapshot.is_locked(), "{snapshot:?}");
    }

    #[test]
    fn a_locked_grid_holds_over_then_expires() {
        let mut analyzer = BeatSyncAnalyzer::new(48_000.0);
        let clicks = click_track(48_000, 120.0, 12.0);
        analyzer.push_mono(&clicks);
        assert_eq!(analyzer.snapshot().state, BeatLockState::Locked);

        analyzer.push_mono(&vec![0.0; 48_000 * 2]);
        let holdover = analyzer.snapshot();
        assert_eq!(holdover.state, BeatLockState::Holdover, "{holdover:?}");
        assert!(holdover.has_grid());

        analyzer.push_mono(&vec![0.0; 48_000 * 3]);
        let lost = analyzer.snapshot();
        assert_eq!(lost.state, BeatLockState::Lost, "{lost:?}");
        assert!(!lost.is_locked());
    }

    #[test]
    fn missed_boundary_is_advanced_not_replayed() {
        let boundary = next_boundary_sample(12_345, 100.0, 1_000.0, 1).unwrap();
        assert_eq!(boundary, 13_100);
        let eighth = next_boundary_sample(12_345, 100.0, 1_000.0, 2).unwrap();
        assert_eq!(eighth, 12_600);
    }

    #[test]
    fn two_seconds_at_120_is_four_beats() {
        let fit = fit_loop_to_grid(2.0, 120.0, 0.08).unwrap();
        assert_eq!(fit.beats, 4);
        assert!((fit.playback_rate - 1.0).abs() < 1e-9);
        assert!(fit.within_rate_limit);
    }

    #[test]
    fn unsafe_rate_is_reported_as_degraded() {
        let fit = fit_loop_to_grid(3.3, 120.0, 0.08).unwrap();
        assert_eq!(fit.beats, 8);
        assert!(!fit.within_rate_limit, "{fit:?}");
        assert!(fit.confidence < 0.5, "{fit:?}");
    }
}
