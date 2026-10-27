//! The one onset front end both modes read.
//!
//! A DC blocker followed by three one-pole crossovers splits the signal into
//! four lanes — kick, body, presence and air — at a 10 ms hop. Each lane's
//! RMS is log-compressed and differenced (half-wave rectified), so a 2 dB
//! transient counts whether the audio arrives at -35 dBFS or limited near
//! full scale. On top of the raw flux each lane keeps an adaptive floor
//! (a running mean and deviation) and publishes a thresholded NOVELTY.
//!
//! The streaming tracker reads the novelty: it has to decide on the fly and
//! the adaptive floor is what keeps a quiet intro and a loud drop comparable
//! inside a twelve-second window. The whole-file pass reads the raw flux and
//! normalises it over the whole record instead, which a file can afford.
//! Either way it is the same filters, the same hop and the same arithmetic,
//! sample for sample.
//!
//! Allocation-free: [`OnsetFrontEnd::push`] may run on a realtime thread.

use std::f32::consts::PI;

/// Four lanes: kick, body, presence, air.
pub const BANDS: usize = 4;
const CROSSOVERS: usize = BANDS - 1;
/// Crossover frequencies between the lanes, Hz.
const CROSSOVER_HZ: [f32; CROSSOVERS] = [170.0, 700.0, 2_800.0];
/// How much each lane's novelty counts in the composite onset value.
pub const BAND_WEIGHTS: [f32; BANDS] = [1.30, 1.05, 0.90, 0.65];
/// Analysis hops per second (a 10 ms hop).
pub const HOPS_PER_SEC: f64 = 100.0;

/// One finished hop.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OnsetHop {
    /// Samples consumed so far, this hop included.
    pub end_sample: u64,
    /// RMS per lane over the hop.
    pub band_rms: [f32; BANDS],
    /// Rectified log-energy rise per lane since the previous hop.
    pub flux: [f32; BANDS],
    /// Flux over each lane's adaptive floor, with a short tail so a
    /// transient occupies two or three hops.
    pub novelty: [f32; BANDS],
    /// The weighted composite of the lane novelties, 0..=6.
    pub onset: f32,
}

/// Sample-by-sample band split and per-hop onset measurement.
#[derive(Clone, Debug)]
pub struct OnsetFrontEnd {
    hop_size: usize,
    sample_clock: u64,
    hop_samples: usize,
    hop_band_square_sum: [f64; BANDS],
    previous_input: f32,
    dc_output: f32,
    dc_coefficient: f32,
    lowpass_state: [f32; CROSSOVERS],
    lowpass_alpha: [f32; CROSSOVERS],
    band_previous_log_energy: [f32; BANDS],
    band_flux_mean: [f32; BANDS],
    band_flux_variance: [f32; BANDS],
    band_novelty_tail: [f32; BANDS],
    first_hop: bool,
}

impl OnsetFrontEnd {
    pub fn new(sample_rate: f64) -> Self {
        let rate = sample_rate.max(1.0) as f32;
        let dc_coefficient = (-2.0 * PI * 25.0 / rate).exp();
        let mut lowpass_alpha = [0.0; CROSSOVERS];
        for (alpha, cutoff) in lowpass_alpha.iter_mut().zip(CROSSOVER_HZ) {
            let cutoff = cutoff.min(rate * 0.4);
            *alpha = 1.0 - (-2.0 * PI * cutoff / rate).exp();
        }
        Self {
            hop_size: (sample_rate / HOPS_PER_SEC).round().max(32.0) as usize,
            sample_clock: 0,
            hop_samples: 0,
            hop_band_square_sum: [0.0; BANDS],
            previous_input: 0.0,
            dc_output: 0.0,
            dc_coefficient,
            lowpass_state: [0.0; CROSSOVERS],
            lowpass_alpha,
            band_previous_log_energy: [0.0; BANDS],
            band_flux_mean: [0.0; BANDS],
            band_flux_variance: [1e-6; BANDS],
            band_novelty_tail: [0.0; BANDS],
            first_hop: true,
        }
    }

    /// Samples per hop.
    pub fn hop_size(&self) -> usize {
        self.hop_size
    }

    /// Samples consumed so far.
    pub fn sample_clock(&self) -> u64 {
        self.sample_clock
    }

    /// Feed one sample; returns the hop it completes, if it completes one.
    /// Non-finite samples count as silence.
    #[inline]
    pub fn push(&mut self, sample: f32) -> Option<OnsetHop> {
        let sample = if sample.is_finite() { sample.clamp(-8.0, 8.0) } else { 0.0 };
        let dc = sample - self.previous_input + self.dc_coefficient * self.dc_output;
        self.previous_input = sample;
        self.dc_output = dc;
        for index in 0..CROSSOVERS {
            self.lowpass_state[index] += self.lowpass_alpha[index] * (dc - self.lowpass_state[index]);
        }
        let bands = [
            self.lowpass_state[0],
            self.lowpass_state[1] - self.lowpass_state[0],
            self.lowpass_state[2] - self.lowpass_state[1],
            dc - self.lowpass_state[2],
        ];
        for (sum, band) in self.hop_band_square_sum.iter_mut().zip(bands) {
            *sum += (band as f64) * (band as f64);
        }
        self.hop_samples += 1;
        self.sample_clock = self.sample_clock.saturating_add(1);
        if self.hop_samples == self.hop_size {
            Some(self.finish_hop())
        } else {
            None
        }
    }

    fn finish_hop(&mut self) -> OnsetHop {
        let hop_samples = self.hop_samples.max(1);
        self.hop_samples = 0;
        let mut out = OnsetHop { end_sample: self.sample_clock, ..OnsetHop::default() };
        let mut novelty_square_sum = 0.0;
        let mut weight_sum = 0.0;
        for band in 0..BANDS {
            let rms = (self.hop_band_square_sum[band] / hop_samples as f64).sqrt() as f32;
            self.hop_band_square_sum[band] = 0.0;
            let log_energy = (1.0 + 96.0 * rms).ln();
            let flux = if self.first_hop {
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
            let weight = BAND_WEIGHTS[band];
            novelty_square_sum += weight * self.band_novelty_tail[band] * self.band_novelty_tail[band];
            weight_sum += weight;

            // Do not let a single large kick raise the floor enough to hide
            // the following snare. The clipped value still follows changing
            // programme material within roughly half a second.
            let robust_flux = flux.min(self.band_flux_mean[band] + 2.5 * deviation + 0.018);
            let delta = robust_flux - self.band_flux_mean[band];
            self.band_flux_mean[band] += 0.020 * delta;
            self.band_flux_variance[band] =
                (0.980 * self.band_flux_variance[band] + 0.020 * delta * delta).max(1e-8);

            out.band_rms[band] = rms;
            out.flux[band] = flux;
            out.novelty[band] = self.band_novelty_tail[band];
        }
        self.first_hop = false;
        out.onset = (novelty_square_sum / weight_sum).sqrt().min(6.0);
        out
    }
}

/// The whole-file envelopes the offline pass reads: every hop of a record
/// through [`OnsetFrontEnd`].
#[derive(Clone, Debug, Default)]
pub struct Envelopes {
    /// Per-hop weighted lane flux, raw: what the octave rule compares.
    pub flux: Vec<f32>,
    /// Per-hop tempo input: the weighted lane flux with its local mean
    /// removed, so a dense texture does not swamp the correlation.
    pub onset: Vec<f32>,
    /// Kick-lane flux per hop (the downbeat fallback reads it).
    pub low_onset: Vec<f32>,
    /// Broadband loudness per hop, natural-log RMS (the phrase detector).
    pub loudness: Vec<f32>,
    /// The streaming composite per hop, for callers that want to show it.
    pub novelty: Vec<f32>,
    pub hop: usize,
    pub sample_rate: f64,
}

/// Collects [`OnsetHop`]s into [`Envelopes`].
#[derive(Clone, Debug, Default)]
pub struct EnvelopeBuilder {
    flux: Vec<f32>,
    low_onset: Vec<f32>,
    loudness: Vec<f32>,
    novelty: Vec<f32>,
}

impl EnvelopeBuilder {
    pub fn with_capacity(hops: usize) -> Self {
        EnvelopeBuilder {
            flux: Vec::with_capacity(hops),
            low_onset: Vec::with_capacity(hops),
            loudness: Vec::with_capacity(hops),
            novelty: Vec::with_capacity(hops),
        }
    }

    pub fn push(&mut self, hop: &OnsetHop) {
        let mut sum = 0.0f32;
        for band in 0..BANDS {
            sum += BAND_WEIGHTS[band] * hop.flux[band];
        }
        self.flux.push(sum);
        self.low_onset.push(hop.flux[0]);
        let energy: f32 = hop.band_rms.iter().map(|r| r * r).sum();
        self.loudness.push((energy.sqrt() + 1e-6).ln());
        self.novelty.push(hop.onset);
    }

    pub fn finish(self, hop: usize, sample_rate: f64) -> Envelopes {
        // Subtract a half-second moving mean so a dense texture does not
        // swamp the correlation with its own DC.
        let window = (0.5 * HOPS_PER_SEC) as usize;
        let smoothed = moving_mean(&self.flux, window.max(1));
        let onset = self.flux.iter().zip(&smoothed).map(|(v, m)| (v - m).max(0.0)).collect();
        Envelopes {
            onset,
            flux: self.flux,
            low_onset: self.low_onset,
            loudness: self.loudness,
            novelty: self.novelty,
            hop,
            sample_rate,
        }
    }
}

impl Envelopes {
    /// Run the front end over a whole mono record.
    pub fn from_mono(samples: &[f32], sample_rate: f64) -> Envelopes {
        let mut front = OnsetFrontEnd::new(sample_rate);
        let mut builder = EnvelopeBuilder::with_capacity(samples.len() / front.hop_size() + 1);
        for &sample in samples {
            if let Some(hop) = front.push(sample) {
                builder.push(&hop);
            }
        }
        builder.finish(front.hop_size(), sample_rate)
    }

    /// Hops per second.
    pub fn hop_rate(&self) -> f64 {
        self.sample_rate / self.hop.max(1) as f64
    }

    pub fn hop_secs(&self) -> f64 {
        self.hop as f64 / self.sample_rate.max(1.0)
    }
}

pub(crate) fn moving_mean(values: &[f32], window: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; values.len()];
    if values.is_empty() {
        return out;
    }
    let half = (window / 2).max(1);
    let mut sum = 0.0f64;
    let mut start = 0usize;
    let mut end = 0usize;
    for index in 0..values.len() {
        let want_start = index.saturating_sub(half);
        let want_end = (index + half + 1).min(values.len());
        while end < want_end {
            sum += values[end] as f64;
            end += 1;
        }
        while start < want_start {
            sum -= values[start] as f64;
            start += 1;
        }
        out[index] = (sum / (end - start).max(1) as f64) as f32;
    }
    out
}
