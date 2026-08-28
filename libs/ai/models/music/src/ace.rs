//! ACE-Step 1.5 shared foundation: official XL-turbo geometry, the flow-matching
//! schedule, SFT prompt packing, and the CPU helpers used by the text encoder,
//! condition encoder, DiT and Oobleck VAE.
//!
//! Formulas are pinned from the HuggingFace `AceStepPipeline` /
//! `AceStepTransformer1DModel` / `AceStepConditionEncoder` / `AutoencoderOobleck`
//! sources (diffusers v0.39) plus the official XL-turbo configs:
//! `ACE-Step/acestep-v15-xl-turbo-diffusers`.

use crate::error::{DiffusionError, Result};
use makepad_ai_h3::h3::H3ShardedWeights;
use makepad_ai_sfx::moss::{moss_rope_apply_half, moss_rope_tables_half};
use makepad_ai_sfx::sa3::{par_rows, rms_norm_rows, silu};
use std::path::Path;

pub const ACE_SAMPLE_RATE: usize = 48_000;
pub const ACE_AUDIO_CHANNELS: usize = 2;
/// VAE hop: product of `[2, 4, 4, 6, 10]`.
pub const ACE_HOP: usize = 1_920;
pub const ACE_LATENTS_PER_SECOND: f64 = ACE_SAMPLE_RATE as f64 / ACE_HOP as f64; // 25
pub const ACE_LATENT_DIM: usize = 64;
pub const ACE_IN_CHANNELS: usize = 192; // src(64) + chunk_mask(64) + noisy(64)
pub const ACE_CONTEXT_DIM: usize = 128; // src + chunk_mask

pub const ACE_MIN_SECONDS: f64 = 10.0;
pub const ACE_MAX_SECONDS: f64 = 600.0;
pub const ACE_DEFAULT_SECONDS: f64 = 60.0;
/// The pinned checkpoint is XL **Turbo**, whose model card specifies EIGHT
/// steps and NO classifier-free guidance — the guidance is distilled into
/// the weights. This tree defaulted to a 50-step, CFG-7 schedule borrowed
/// from the non-turbo model: six times the work for a result the checkpoint
/// was never trained to produce.
pub const ACE_DEFAULT_STEPS: usize = 8;
pub const ACE_DEFAULT_SHIFT: f32 = 3.0;
/// 1.0 IS "no CFG": the APG/uncond branch is gated on `guidance > 1.0`, so
/// this both matches the card and stops running a second denoise pass whose
/// only effect on a turbo checkpoint is to distort it.
pub const ACE_DEFAULT_CFG: f32 = 1.0;
pub const ACE_BASE_CFG: f32 = 1.0;
pub const ACE_APG_MOMENTUM: f32 = -0.75;
pub const ACE_APG_ETA: f32 = 0.0;
pub const ACE_APG_NORM_THRESHOLD: f32 = 2.5;
pub const ACE_TIMBRE_SECONDS: f64 = 30.0;

// XL turbo DiT.
pub const ACE_DIT_DIM: usize = 2_560;
pub const ACE_DIT_LAYERS: usize = 32;
pub const ACE_DIT_HEADS: usize = 32;
pub const ACE_DIT_KV_HEADS: usize = 8;
pub const ACE_DIT_HEAD_DIM: usize = 128;
pub const ACE_DIT_FFN: usize = 9_728;
pub const ACE_DIT_ENCODER_DIM: usize = 2_048;
pub const ACE_PATCH_SIZE: usize = 2;
pub const ACE_SLIDING_WINDOW: usize = 128;
pub const ACE_ROPE_THETA: f64 = 1_000_000.0;
pub const ACE_RMS_EPS: f32 = 1e-6;
pub const ACE_TIME_FREQ_DIM: usize = 256;
pub const ACE_TIME_SCALE: f32 = 1_000.0;

// Condition encoder (lyric 8L + timbre 4L @ 2048).
pub const ACE_COND_DIM: usize = 2_048;
pub const ACE_COND_FFN: usize = 6_144;
pub const ACE_COND_HEADS: usize = 16;
pub const ACE_COND_KV_HEADS: usize = 8;
pub const ACE_LYRIC_LAYERS: usize = 8;
pub const ACE_TIMBRE_LAYERS: usize = 4;
pub const ACE_TEXT_HIDDEN: usize = 1_024;

// Qwen3-0.6B text encoder.
pub const ACE_TE_HIDDEN: usize = 1_024;
pub const ACE_TE_LAYERS: usize = 28;
pub const ACE_TE_Q_HEADS: usize = 16;
pub const ACE_TE_KV_HEADS: usize = 8;
pub const ACE_TE_HEAD_DIM: usize = 128;
pub const ACE_TE_FFN: usize = 3_072;
pub const ACE_TE_ROPE_THETA: f64 = 1_000_000.0;
pub const ACE_MAX_TEXT_TOKENS: usize = 256;
pub const ACE_MAX_LYRIC_TOKENS: usize = 2_048;
pub const ACE_TE_PAD_ID: u32 = 151_643;

// Oobleck VAE decoder.
pub const ACE_VAE_CHANNELS: usize = 128;
pub const ACE_VAE_STRIDES: [usize; 5] = [10, 6, 4, 4, 2];
pub const ACE_VAE_MULTS: [usize; 6] = [1, 1, 2, 4, 8, 16];

pub const ACE_INSTRUCTION: &str = "Fill the audio semantic mask based on the given conditions:";
const SFT_GEN_PROMPT: &str = "# Instruction\n{}\n\n# Caption\n{}\n\n# Metas\n{}<|endoftext|>\n";

/// Linear schedule in `[1, 0]` with `steps+1` points, drop the terminal 0,
/// then apply the flow-matching shift `s*t / (1+(s-1)*t)`. Turbo 8-step
/// tables for `shift ∈ {1,2,3}` are recovered exactly by this formula.
pub fn ace_sigmas(steps: usize, shift: f32) -> Vec<f32> {
    let mut sigmas = Vec::with_capacity(steps + 1);
    for i in 0..steps {
        let t = 1.0 - i as f32 / steps as f32;
        let shifted = if (shift - 1.0).abs() < 1e-6 {
            t
        } else {
            shift * t / (1.0 + (shift - 1.0) * t)
        };
        sigmas.push(shifted);
    }
    sigmas.push(0.0);
    sigmas
}

pub fn ace_latent_len(seconds: f64) -> usize {
    (seconds * ACE_LATENTS_PER_SECOND).ceil() as usize
}

pub fn ace_clamp_seconds(seconds: f64) -> f64 {
    seconds.clamp(ACE_MIN_SECONDS, ACE_MAX_SECONDS)
}

/// Metadata block from `_build_metadata_string`. Duration is integer seconds.
pub fn ace_metadata_string(
    bpm: Option<u32>,
    keyscale: Option<&str>,
    timesignature: Option<&str>,
    audio_duration: f64,
) -> String {
    let bpm_str = bpm
        .filter(|v| *v > 0)
        .map(|v| v.to_string())
        .unwrap_or_else(|| "N/A".to_string());
    let ts_str = timesignature
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("N/A");
    let ks_str = keyscale
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("N/A");
    let dur = if audio_duration > 0.0 {
        format!("{} seconds", audio_duration as i64)
    } else {
        "30 seconds".to_string()
    };
    format!("- bpm: {bpm_str}\n- timesignature: {ts_str}\n- keyscale: {ks_str}\n- duration: {dur}\n")
}

/// Official SFT prompt + lyric packing. The newlines after each section label
/// are load-bearing.
pub fn ace_format_prompt(
    prompt: &str,
    lyrics: &str,
    vocal_language: &str,
    audio_duration: f64,
    instruction: Option<&str>,
    bpm: Option<u32>,
    keyscale: Option<&str>,
    timesignature: Option<&str>,
) -> (String, String) {
    let mut instruction = instruction.unwrap_or(ACE_INSTRUCTION).to_string();
    if !instruction.ends_with(':') {
        instruction.push(':');
    }
    let metas = ace_metadata_string(bpm, keyscale, timesignature, audio_duration);
    let formatted_text = SFT_GEN_PROMPT
        .replacen("{}", &instruction, 1)
        .replacen("{}", prompt, 1)
        .replacen("{}", &metas, 1);
    let formatted_lyrics = format!(
        "# Languages\n{}\n\n# Lyric\n{}<|endoftext|>",
        vocal_language, lyrics
    );
    (formatted_text, formatted_lyrics)
}

/// Pack two masked sequences: concatenate, stable-sort so mask=1 tokens come
/// first, return a fresh contiguous mask. Batch size is 1 in this port.
pub fn ace_pack_sequences(
    hidden1: &[f32],
    mask1: &[bool],
    hidden2: &[f32],
    mask2: &[bool],
    dim: usize,
) -> Result<(Vec<f32>, Vec<bool>)> {
    let n1 = mask1.len();
    let n2 = mask2.len();
    if hidden1.len() != n1 * dim || hidden2.len() != n2 * dim {
        return Err(DiffusionError::model(format!(
            "ace pack: hidden {}/{} vs mask {}/{} dim {dim}",
            hidden1.len(),
            hidden2.len(),
            n1,
            n2
        )));
    }
    let total = n1 + n2;
    let mut items: Vec<(bool, usize, bool)> = Vec::with_capacity(total);
    for i in 0..n1 {
        items.push((mask1[i], i, true));
    }
    for i in 0..n2 {
        items.push((mask2[i], i, false));
    }
    items.sort_by(|a, b| b.0.cmp(&a.0)); // stable: Rust sort_by is stable
    let mut packed = vec![0f32; total * dim];
    let mut mask = vec![false; total];
    for (out, &(keep, src, from_first)) in items.iter().enumerate() {
        mask[out] = keep;
        let src_row = if from_first {
            &hidden1[src * dim..(src + 1) * dim]
        } else {
            &hidden2[src * dim..(src + 1) * dim]
        };
        packed[out * dim..(out + 1) * dim].copy_from_slice(src_row);
    }
    Ok((packed, mask))
}

/// Round-to-nearest-even BF16 store, expanded back to f32 bits.
pub fn ace_bf16_round_f32(v: f32) -> f32 {
    let bits = v.to_bits();
    let rounding_bias = 0x7FFFu32 + ((bits >> 16) & 1);
    f32::from_bits((bits + rounding_bias) & 0xFFFF_0000)
}

/// Diffusers `Timesteps(256, flip_sin_to_cos=True, downscale_freq_shift=0)`
/// applied to `t * 1000`. Official: `t` is already bf16, `t*1000` is a bf16
/// store, then the sinusoid itself is f32 (`timesteps.float()`).
pub fn ace_time_sinusoid(t: f32) -> Vec<f32> {
    let t = ace_bf16_round_f32(ace_bf16_round_f32(t) * ACE_TIME_SCALE);
    let half = ACE_TIME_FREQ_DIM / 2;
    let mut out = vec![0f32; ACE_TIME_FREQ_DIM];
    for i in 0..half {
        let freq = (-10_000f32.ln() * (i as f32) / half as f32).exp();
        let arg = t * freq;
        out[i] = arg.cos();
        out[half + i] = arg.sin();
    }
    out
}

pub fn ace_device_enabled() -> bool {
    if std::env::var("ACE_DEVICE")
        .map(|v| v == "0")
        .unwrap_or(false)
    {
        return false;
    }
    makepad_ai_common::backend::cuda::gpu_device_available()
}

pub fn ace_open_shards(dir: impl AsRef<Path>) -> Result<H3ShardedWeights> {
    H3ShardedWeights::load(dir.as_ref()).map_err(|e| {
        DiffusionError::model(format!(
            "ace weights {}: {e:?}",
            dir.as_ref().display()
        ))
    })
}

/// First matching tensor name; used because the official converter keeps
/// either original (`q_proj`) or remapped (`to_q`) spellings.
pub fn ace_tensor_any(weights: &H3ShardedWeights, names: &[&str]) -> Result<Vec<f32>> {
    for name in names {
        if weights.has_tensor(name) {
            return weights.tensor_f32(name);
        }
    }
    Err(DiffusionError::model(format!(
        "ace tensor missing (tried {})",
        names.join(", ")
    )))
}

pub fn ace_tensor_shaped(
    weights: &H3ShardedWeights,
    names: &[&str],
    expected: usize,
) -> Result<Vec<f32>> {
    let value = ace_tensor_any(weights, names)?;
    if value.len() != expected {
        return Err(DiffusionError::model(format!(
            "ace tensor {} : {} values, expected {expected}",
            names[0],
            value.len()
        )));
    }
    Ok(value)
}

pub fn ace_rope_tables(positions: usize) -> (Vec<f32>, Vec<f32>) {
    moss_rope_tables_half(positions, ACE_DIT_HEAD_DIM, ACE_ROPE_THETA)
}

/// Official ACE-Step RoPE tables: f32 freqs (`get_1d_rotary_pos_embed`) then
/// `.to(bf16)` stored values. Half-width tables for `gpu_rope_half`.
pub fn ace_rope_tables_bf16(positions: usize) -> (Vec<f32>, Vec<f32>) {
    let dim = ACE_DIT_HEAD_DIM;
    let half = dim / 2;
    let theta = ACE_ROPE_THETA as f32;
    let mut cos = vec![0f32; positions * half];
    let mut sin = vec![0f32; positions * half];
    for pos in 0..positions {
        for j in 0..half {
            let exp = (2 * j) as f32 / dim as f32;
            let freq = 1.0 / theta.powf(exp);
            let arg = (pos as f32) * freq;
            cos[pos * half + j] = ace_bf16_round_f32(arg.cos());
            sin[pos * half + j] = ace_bf16_round_f32(arg.sin());
        }
    }
    (cos, sin)
}

pub fn ace_apply_rope_half(
    row: &mut [f32],
    pos: usize,
    heads: usize,
    cos: &[f32],
    sin: &[f32],
) {
    moss_rope_apply_half(row, pos, heads, ACE_DIT_HEAD_DIM, cos, sin);
}

pub fn ace_rms_head(head: &mut [f32], gamma: &[f32]) {
    let mut sum = 0f32;
    for v in head.iter() {
        sum += v * v;
    }
    let inv = 1.0 / (sum / head.len() as f32 + ACE_RMS_EPS).sqrt();
    for (v, g) in head.iter_mut().zip(gamma.iter()) {
        *v = *v * inv * *g;
    }
}

/// Bidirectional GQA attention. `window = Some(w)` keeps `|i-j| <= w`.
/// `mask_k[j] = false` drops key j (padding). Causal when `causal` is set.
pub fn ace_gqa_attention(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    q_heads: usize,
    kv_heads: usize,
    head_dim: usize,
    seq_q: usize,
    seq_k: usize,
    window: Option<usize>,
    causal: bool,
    mask_k: Option<&[bool]>,
) -> Vec<f32> {
    let group = q_heads / kv_heads.max(1);
    let scale = 1.0 / (head_dim as f32).sqrt();
    let mut out = vec![0f32; seq_q * q_heads * head_dim];
    par_rows(&mut out, q_heads * head_dim, &|row, out_row| {
        let mut scores = vec![0f32; seq_k];
        for head in 0..q_heads {
            let kv_head = head / group;
            let q_vec = &q[row * q_heads * head_dim + head * head_dim..][..head_dim];
            let mut max_s = f32::NEG_INFINITY;
            let mut any = false;
            for key_row in 0..seq_k {
                let mut keep = true;
                if let Some(mask) = mask_k {
                    keep &= mask.get(key_row).copied().unwrap_or(false);
                }
                if causal && key_row > row {
                    keep = false;
                }
                if let Some(w) = window {
                    let diff = (row as isize - key_row as isize).unsigned_abs();
                    if diff > w {
                        keep = false;
                    }
                }
                if !keep {
                    scores[key_row] = f32::NEG_INFINITY;
                    continue;
                }
                let k_vec = &k[key_row * kv_heads * head_dim + kv_head * head_dim..][..head_dim];
                let mut dot = 0f32;
                for i in 0..head_dim {
                    dot += q_vec[i] * k_vec[i];
                }
                scores[key_row] = dot * scale;
                max_s = max_s.max(scores[key_row]);
                any = true;
            }
            let out_vec = &mut out_row[head * head_dim..(head + 1) * head_dim];
            out_vec.fill(0.0);
            if !any {
                continue;
            }
            let mut denom = 0f32;
            for score in scores.iter_mut() {
                if score.is_finite() {
                    *score = (*score - max_s).exp();
                    denom += *score;
                } else {
                    *score = 0.0;
                }
            }
            let inv = 1.0 / denom.max(1e-30);
            for (key_row, &score) in scores.iter().enumerate() {
                if score == 0.0 {
                    continue;
                }
                let w = score * inv;
                let v_vec = &v[key_row * kv_heads * head_dim + kv_head * head_dim..][..head_dim];
                for i in 0..head_dim {
                    out_vec[i] += w * v_vec[i];
                }
            }
        }
    });
    out
}

pub fn ace_swiglu(gate: &[f32], up: &[f32]) -> Vec<f32> {
    debug_assert_eq!(gate.len(), up.len());
    let mut out = vec![0f32; gate.len()];
    for (o, (&g, &u)) in out.iter_mut().zip(gate.iter().zip(up.iter())) {
        *o = silu(g) * u;
    }
    out
}

pub fn ace_adaln(x: &[f32], gamma: &[f32], shift: &[f32], scale: &[f32], dim: usize) -> Vec<f32> {
    let mut y = x.to_vec();
    rms_norm_rows(&mut y, gamma, dim, ACE_RMS_EPS);
    for (row, chunk) in y.chunks_mut(dim).enumerate() {
        let _ = row;
        for i in 0..dim {
            chunk[i] = chunk[i] * (1.0 + scale[i]) + shift[i];
        }
    }
    y
}

pub fn ace_gated_residual(x: &mut [f32], delta: &[f32], gate: &[f32], dim: usize) {
    for (row, (xv, dv)) in x.chunks_mut(dim).zip(delta.chunks(dim)).enumerate() {
        let _ = row;
        for i in 0..dim {
            xv[i] += dv[i] * gate[i];
        }
    }
}

pub fn ace_add_inplace(x: &mut [f32], delta: &[f32]) {
    for (a, b) in x.iter_mut().zip(delta.iter()) {
        *a += *b;
    }
}

/// Official APG `MomentumBuffer` (`running = momentum * running + update`).
pub struct AceApgMomentum {
    momentum: f32,
    running: Option<Vec<f64>>,
}

impl AceApgMomentum {
    pub fn new(momentum: f32) -> Self {
        Self {
            momentum,
            running: None,
        }
    }
}

/// Adaptive Projected Guidance matching diffusers `normalized_guidance`
/// with `eta=0`, `norm_threshold=2.5`, `use_original_formulation=True`,
/// `norm_dim=(1,)`. `pred` is `[T, C]` token-major (batch=1). `guidance_scale`
/// is the value already passed to `normalized_guidance` (pipeline uses
/// `cfg - 1`).
pub fn ace_apg(
    pred_cond: &[f32],
    pred_uncond: &[f32],
    frames: usize,
    channels: usize,
    guidance_scale: f32,
    momentum: &mut AceApgMomentum,
    eta: f32,
    norm_threshold: f32,
) -> Vec<f32> {
    debug_assert_eq!(pred_cond.len(), frames * channels);
    debug_assert_eq!(pred_uncond.len(), frames * channels);
    let n = pred_cond.len();
    let mut diff = vec![0f64; n];
    for i in 0..n {
        diff[i] = pred_cond[i] as f64 - pred_uncond[i] as f64;
    }
    if let Some(run) = momentum.running.as_mut() {
        for i in 0..n {
            run[i] = momentum.momentum as f64 * run[i] + diff[i];
            diff[i] = run[i];
        }
    } else {
        momentum.running = Some(diff.clone());
    }

    // L2 over dim=1 (time) with keepdim → one scale per channel.
    if norm_threshold > 0.0 {
        let thresh = norm_threshold as f64;
        for c in 0..channels {
            let mut sumsq = 0f64;
            for t in 0..frames {
                let v = diff[t * channels + c];
                sumsq += v * v;
            }
            let norm = sumsq.sqrt().max(1e-30);
            let scale = (thresh / norm).min(1.0);
            if scale < 1.0 {
                for t in 0..frames {
                    diff[t * channels + c] *= scale;
                }
            }
        }
    }

    // Project: v1 = normalize(pred_cond, dim=1); parallel = (diff·v1) v1
    let mut out = vec![0f32; n];
    let gs = guidance_scale as f64;
    for c in 0..channels {
        let mut cnorm = 0f64;
        for t in 0..frames {
            let v = pred_cond[t * channels + c] as f64;
            cnorm += v * v;
        }
        let inv = 1.0 / cnorm.sqrt().max(1e-30);
        let mut dot = 0f64;
        for t in 0..frames {
            let v1 = pred_cond[t * channels + c] as f64 * inv;
            dot += diff[t * channels + c] * v1;
        }
        for t in 0..frames {
            let v1 = pred_cond[t * channels + c] as f64 * inv;
            let parallel = dot * v1;
            let orthogonal = diff[t * channels + c] - parallel;
            let update = orthogonal + eta as f64 * parallel;
            out[t * channels + c] = (pred_cond[t * channels + c] as f64 + gs * update) as f32;
        }
    }
    out
}

/// Seeded gaussian (splitmix64 + Box-Muller). Deterministic per seed, not
/// torch-compatible — dump replay uses [`AceDumpNoise`].
pub struct AceSeededNoise {
    state: u64,
    spare: Option<f32>,
}

impl AceSeededNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
            spare: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 + 0.5) / 16_777_216.0
    }

    pub fn next_gaussian(&mut self) -> f32 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let u1 = self.next_f32();
        let u2 = self.next_f32();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f32::consts::PI * u2;
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }

    pub fn draw(&mut self, len: usize) -> Vec<f32> {
        (0..len).map(|_| self.next_gaussian()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turbo_shift3_8step_matches_formula() {
        let s = ace_sigmas(8, 3.0);
        assert_eq!(s.len(), 9);
        assert!((s[0] - 1.0).abs() < 1e-6);
        assert!((s[8] - 0.0).abs() < 1e-6);
        // linspace(1,0,9)[1] = 0.875 -> 3*0.875/(1+2*0.875) = 2.625/2.75
        assert!((s[1] - 2.625 / 2.75).abs() < 1e-6);
    }

    #[test]
    fn prompt_format_is_load_bearing() {
        let (text, lyrics) = ace_format_prompt(
            "a piano ballad",
            "[verse]\nhello",
            "en",
            30.0,
            None,
            Some(120),
            Some("C major"),
            Some("4"),
        );
        assert!(text.starts_with("# Instruction\nFill the audio semantic mask based on the given conditions:\n\n# Caption\na piano ballad\n\n# Metas\n"));
        assert!(text.contains("- bpm: 120\n"));
        assert!(text.contains("- duration: 30 seconds\n"));
        assert!(text.contains("<|endoftext|>\n"));
        assert_eq!(
            lyrics,
            "# Languages\nen\n\n# Lyric\n[verse]\nhello<|endoftext|>"
        );
    }

    #[test]
    fn pack_puts_valid_tokens_first() {
        let h1 = vec![1.0, 2.0, 3.0, 4.0];
        let m1 = vec![true, false];
        let h2 = vec![5.0, 6.0];
        let m2 = vec![true];
        let (p, m) = ace_pack_sequences(&h1, &m1, &h2, &m2, 2).unwrap();
        assert_eq!(p, vec![1.0, 2.0, 5.0, 6.0, 3.0, 4.0]);
        assert_eq!(m, vec![true, true, false]);
    }

    #[test]
    fn latent_len_is_25hz() {
        assert_eq!(ace_latent_len(1.0), 25);
        assert_eq!(ace_latent_len(30.0), 750);
        assert_eq!(ace_latent_len(10.04), 251);
    }
}
