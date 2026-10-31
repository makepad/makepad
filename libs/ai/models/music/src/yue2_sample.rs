//! YuE2 AR sampling: `yue2.sampling.distribution` + torch CUDA multinomial.
//!
//! Order (reference): allowed-vocab mask -> end token banned below
//! `min_tokens` -> window repetition penalty (count-based, `penalty**freq`,
//! negative logits multiplied, positive divided) -> temperature -> top-k
//! (keep >= k-th value) -> top-p (sorted cumsum minus own prob > p removed,
//! first kept) -> softmax -> multinomial. Scores are f32 (the release keeps
//! BF16 + the first three only for the historical `cot=off` path).
//!
//! The multinomial draw replays torch's CUDA generator (Philox Gumbel-max,
//! subsequence = full-vocab index) via [`TorchPhilox`], verified against
//! torch for the Music3 port.

use crate::music3_ar::TorchPhilox;
use crate::yue2::{Yue2Phase, Yue2Sampling};
use crate::backend::{
    gpu_alloc_f32, gpu_download_u32_scalar, gpu_sample_history_push, gpu_sample_top_k_top_p,
    gpu_upload_u32, gpu_upload_u32_into, GpuSamplingParams, GpuTensor,
};
use crate::{DiffusionError, Result};

pub struct Yue2Sampler {
    rng: TorchPhilox,
}

/// Persistent CUDA sampler state. The host reference below deliberately
/// stays independent. A live decode transfers only `read_token()` (4 B);
/// history counts, min-length state and Philox counters remain on device.
pub struct Yue2DeviceSampler {
    allowed: GpuTensor,
    counts: GpuTensor,
    history: GpuTensor,
    state: GpuTensor,
    token: GpuTensor,
    workspace: GpuTensor,
    params: GpuSamplingParams,
}

impl Yue2DeviceSampler {
    pub fn new(
        seed: u64,
        phase: Yue2Phase,
        sampling: &Yue2Sampling,
        legacy_off: bool,
        guidance: f32,
    ) -> Result<Self> {
        let (lo, n) = phase.head_rows();
        let window = u32::try_from(sampling.penalty_window)
            .map_err(|_| DiffusionError::model("yue2 penalty window exceeds u32"))?;
        let min_tokens = u32::try_from(sampling.min_tokens)
            .map_err(|_| DiffusionError::model("yue2 min tokens exceeds u32"))?;
        let mask: Vec<u32> = (lo..lo + n).map(|id| phase.allowed(id) as u32).collect();
        Ok(Self {
            allowed: gpu_upload_u32(&mask).map_err(DiffusionError::model)?,
            counts: gpu_upload_u32(&vec![0; n as usize]).map_err(DiffusionError::model)?,
            history: gpu_upload_u32(&vec![u32::MAX; sampling.penalty_window.max(1)])
                .map_err(DiffusionError::model)?,
            state: gpu_upload_u32(&[0, 0, 0, 0]).map_err(DiffusionError::model)?,
            token: gpu_upload_u32(&[lo]).map_err(DiffusionError::model)?,
            workspace: gpu_alloc_f32(5, n as usize).map_err(DiffusionError::model)?,
            params: GpuSamplingParams {
                vocab_start: lo,
                end_token: phase.end_token(),
                min_tokens,
                top_k: sampling.top_k.min(n as usize).max(1) as u32,
                penalty_window: window,
                keep_first: if legacy_off { 3 } else { 1 },
                round_logits_bf16: legacy_off as u32,
                temperature: sampling.temperature,
                top_p: sampling.top_p,
                repetition_penalty: sampling.repetition_penalty,
                guidance,
                seed,
            },
        })
    }

    /// Enqueue a draw; no synchronization, allocation or host work on scores.
    pub fn sample(&self, logits: &GpuTensor, unconditional: Option<&GpuTensor>) -> Result<()> {
        gpu_sample_top_k_top_p(
            logits, unconditional, &self.allowed, &self.counts, &self.state,
            &self.token, &self.workspace, self.params,
        ).map_err(DiffusionError::model)
    }

    pub fn read_token(&self) -> Result<u32> {
        gpu_download_u32_scalar(&self.token).map_err(DiffusionError::model)
    }

    pub fn token(&self) -> &GpuTensor {
        &self.token
    }

    /// Call once before continuing after a draw. EOS is never accepted by
    /// the generation loop. Oracle checks may force a token before this.
    pub fn accept_previous(&self) -> Result<()> {
        // Parameters are fixed for the session. Counts are never consumed
        // without a penalty; otherwise the device ring updates O(1) entries.
        if self.params.penalty_window == 0 || self.params.repetition_penalty == 1.0 {
            return Ok(());
        }
        gpu_sample_history_push(
            &self.counts, &self.history, &self.state, &self.token,
            self.params.vocab_start, self.params.penalty_window as usize,
        ).map_err(DiffusionError::model)
    }

    /// Teacher-forcing input only; ordinary generation never uploads an id.
    pub fn force_token(&self, token: u32) -> Result<()> {
        gpu_upload_u32_into(&self.token, &[token]).map_err(DiffusionError::model)
    }
}

fn bf16_round(v: f32) -> f32 {
    if !v.is_finite() {
        return v;
    }
    let bits = v.to_bits();
    let rounded = bits.wrapping_add(0x7fff + ((bits >> 16) & 1)) & 0xffff_0000;
    f32::from_bits(rounded)
}

impl Yue2Sampler {
    pub fn new(seed: u64) -> Self {
        Self { rng: TorchPhilox::new(seed) }
    }

    /// Pick the next full-vocab id. `logits` covers rows `[lo, lo+len)` of
    /// the vocabulary (the phase's head window); `history` holds the
    /// phase's previously sampled ids (full-vocab, end token excluded).
    pub fn sample(
        &mut self,
        logits: &[f32],
        lo: u32,
        phase: Yue2Phase,
        sampling: &Yue2Sampling,
        history: &[u32],
        step: usize,
        legacy_off: bool,
    ) -> u32 {
        let end = phase.end_token();
        let mut scores: Vec<f32> = logits
            .iter()
            .enumerate()
            .map(|(j, &v)| {
                let id = lo + j as u32;
                if !phase.allowed(id) {
                    f32::NEG_INFINITY
                } else if legacy_off {
                    bf16_round(v)
                } else {
                    v
                }
            })
            .collect();
        let end_index = (end - lo) as usize;
        if step < sampling.min_tokens {
            scores[end_index] = f32::NEG_INFINITY;
        }
        // Window repetition penalty over the last `penalty_window` ids.
        if sampling.repetition_penalty != 1.0 && !history.is_empty() {
            let window = &history[history.len().saturating_sub(sampling.penalty_window)..];
            let mut counts: Vec<(usize, u32)> = Vec::new();
            for &id in window {
                if id < lo || (id - lo) as usize >= scores.len() {
                    continue;
                }
                let j = (id - lo) as usize;
                match counts.iter_mut().find(|(k, _)| *k == j) {
                    Some((_, c)) => *c += 1,
                    None => counts.push((j, 1)),
                }
            }
            for (j, count) in counts {
                let alpha = sampling.repetition_penalty.powi(count as i32);
                let v = scores[j];
                scores[j] = if v < 0.0 { v * alpha } else { v / alpha };
            }
        }
        if sampling.temperature == 0.0 {
            // torch argmax: first maximal index.
            let mut best = 0usize;
            for (j, &v) in scores.iter().enumerate() {
                if v > scores[best] {
                    best = j;
                }
            }
            self.rng.next_counter();
            return lo + best as u32;
        }
        if sampling.temperature != 1.0 {
            for v in &mut scores {
                *v /= sampling.temperature;
            }
        }
        // top-k: keep values >= the k-th largest.
        let k = sampling.top_k.min(scores.len()).max(1);
        let mut sorted: Vec<f32> = scores.clone();
        sorted.select_nth_unstable_by(k - 1, |a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let threshold = sorted[k - 1];
        let mut cand: Vec<(usize, f32)> = scores
            .iter()
            .enumerate()
            .filter(|(_, &v)| v >= threshold && v.is_finite())
            .map(|(j, &v)| (j, v))
            .collect();
        // top-p over the survivors, descending.
        if sampling.top_p < 1.0 && !cand.is_empty() {
            cand.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
            let max = cand[0].1;
            let weights: Vec<f32> = cand.iter().map(|(_, v)| (v - max).exp()).collect();
            let total: f32 = weights.iter().sum();
            let keep_first = if legacy_off { 3 } else { 1 };
            let mut cumsum = 0.0f32;
            let mut kept = Vec::with_capacity(cand.len());
            for (i, (&(j, v), w)) in cand.iter().zip(weights.iter()).enumerate() {
                let p = w / total;
                cumsum += p;
                if i < keep_first || cumsum - p <= sampling.top_p {
                    kept.push((j, v));
                }
            }
            cand = kept;
        }
        // softmax + Gumbel-max multinomial (torch CUDA, n_sample = 1).
        let max = cand.iter().map(|c| c.1).fold(f32::NEG_INFINITY, f32::max);
        let weights: Vec<f32> = cand.iter().map(|(_, v)| (v - max).exp()).collect();
        let total: f32 = weights.iter().sum();
        let mut best = (f32::NEG_INFINITY, cand.first().map_or(0, |c| c.0));
        for (&(j, _), w) in cand.iter().zip(weights.iter()) {
            let p = w / total;
            if !(p > 0.0) {
                continue;
            }
            let mut u = self.rng.uniform_at((lo as usize + j) as u64);
            u = u.clamp(1.0e-12, 1.0 - 1.0e-7);
            let score = p.ln() - (-u.ln()).ln();
            if score > best.0 {
                best = (score, j);
            }
        }
        self.rng.next_counter();
        lo + best.1 as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yue2::{ABC_END, MUSIC_END};

    #[test]
    fn greedy_respects_mask_and_min_tokens() {
        let (lo, n) = Yue2Phase::Semantic.head_rows();
        let mut logits = vec![0.0f32; n as usize];
        logits[0] = 10.0; // MUSIC_END
        logits[5] = 5.0;
        let mut s = Yue2Sampler::new(1);
        let mut g = Yue2Sampling::GREEDY;
        g.min_tokens = 2;
        assert_eq!(s.sample(&logits, lo, Yue2Phase::Semantic, &g, &[], 0, false), lo + 5);
        assert_eq!(s.sample(&logits, lo, Yue2Phase::Semantic, &g, &[], 2, false), MUSIC_END);
        let (alo, an) = Yue2Phase::Abc.head_rows();
        let mut abc = vec![0.0f32; an as usize];
        abc[(ABC_END - 1) as usize] = 9.0; // a control id between EOD and ABC_END: masked
        abc[7] = 1.0;
        assert_eq!(s.sample(&abc, alo, Yue2Phase::Abc, &Yue2Sampling::GREEDY, &[], 0, false), 7);
    }

    #[test]
    fn penalty_divides_positive_multiplies_negative() {
        let (lo, n) = Yue2Phase::Semantic.head_rows();
        let mut logits = vec![-50.0f32; n as usize];
        logits[3] = 2.0;
        logits[4] = 1.9;
        let mut s = Yue2Sampler::new(1);
        let mut g = Yue2Sampling::GREEDY;
        g.repetition_penalty = 1.2;
        g.penalty_window = 50;
        // id lo+3 seen once: 2.0/1.2 < 1.9 -> picks lo+4.
        assert_eq!(s.sample(&logits, lo, Yue2Phase::Semantic, &g, &[lo + 3], 0, false), lo + 4);
    }
}
