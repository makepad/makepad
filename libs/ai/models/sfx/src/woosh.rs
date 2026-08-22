//! Sony Woosh text-to-SFX shared foundation (DFlow-first port).
//!
//! Woosh (Sony AI, CC-BY-NC weights / MIT code) is the third SFX voice of the
//! audio domain beside SA3 and MOSS — picked for speed (4-NFE distilled
//! student, ~0.06s warm e2e on a 5090 in the torch reference) and for take
//! variety. Fixed 5.0s mono 48 kHz output; there is NO duration conditioning.
//!
//! Architecture (pinned from checkpoints/Woosh-DFlow/config.yaml + the repo
//! read + oracle dumps in local/woosh_ref/dumps):
//! - text conditioner: RoBERTa-large (24L d1024), hidden_states[-2] (=output
//!   of layer 23-of-24 -> we run 23 layers), tokenizer = GPT-2 byte BPE with
//!   RoBERTa post-processing (BOS 0 / EOS 2 / PAD 1, max_length 77);
//! - DiT "mmmssflux": 12 layers = 6 dual-stream MMM + 6 single-stream, d 1024,
//!   8 heads x 128 (112 rope + 16 nope, DeepSeek-style partial rope with YaRN
//!   scaling: table len 501*2=1002 > original 501), AdaLN modulation from a
//!   (t, r, cfg) fixed-Fourier embedding (CFG is EMBEDDED -> single forward
//!   per step), 1 memory token prepended to the 501 audio tokens, text stream
//!   77 tokens; v-prediction with the DFlow sign flip folded into the final
//!   linear at load;
//! - sampler: 4-step Euler with renoise [0, 0.5, 0.5, 0.3] (fresh gaussian at
//!   steps 1..3), t = linspace(1, 0, 5);
//! - AE decoder: VOCOS ConvNeXt backbone (d 2048, 8 blocks, exact-erf GELU)
//!   with an ISTFTCircleHead (softplus magnitude, circle-normalized phase,
//!   n_fft 960 / hop 480 / centered iSTFT) -> 501 frames x 480 hop = 240,000
//!   samples = 5.0 s mono 48 kHz. Deterministic (no decoder noise).

use std::path::Path;

use crate::error::Result;
use crate::sa3::Sa3Tensors;

pub const WOOSH_SAMPLE_RATE: usize = 48_000;
pub const WOOSH_SECONDS: f64 = 5.0;
pub const WOOSH_LATENT_DIM: usize = 128;
pub const WOOSH_LATENT_FRAMES: usize = 501;

pub const WOOSH_DIM: usize = 1024;
pub const WOOSH_INTER_DIM: usize = 4096;
pub const WOOSH_LAYERS: usize = 12;
pub const WOOSH_MM_LAYERS: usize = 6;
pub const WOOSH_HEADS: usize = 8;
pub const WOOSH_HEAD_DIM: usize = 128;
pub const WOOSH_ROPE_DIM: usize = 112;
pub const WOOSH_LN_EPS: f32 = 1e-6;
pub const WOOSH_TIMESTEP_FEATURES: usize = 256;

/// Text stream length (max_description_length; no description memory tokens).
pub const WOOSH_DESC_TOKENS: usize = 77;
/// Audio stream length: 1 memory token + 501 latent tokens.
pub const WOOSH_AUDIO_TOKENS: usize = 1 + WOOSH_LATENT_FRAMES;
/// Joint attention sequence: audio stream + text stream.
pub const WOOSH_JOINT_TOKENS: usize = WOOSH_AUDIO_TOKENS + WOOSH_DESC_TOKENS;

// YaRN rope geometry (config: rope_len_multiplier 2, original_seq_len 501,
// theta 1e4, factor 40, beta_fast 32, beta_slow 1).
pub const WOOSH_ROPE_TABLE_LEN: usize = WOOSH_LATENT_FRAMES * 2; // 1002
pub const WOOSH_ROPE_FREQS: usize = WOOSH_HEAD_DIM / 2; // 64 freq columns
pub const WOOSH_ROPE_THETA: f32 = 10_000.0;
pub const WOOSH_ROPE_FACTOR: f32 = 40.0;
pub const WOOSH_ROPE_BETA_FAST: f64 = 32.0;
pub const WOOSH_ROPE_BETA_SLOW: f64 = 1.0;

// TE (RoBERTa-large) geometry.
pub const WOOSH_TE_HIDDEN: usize = 1024;
pub const WOOSH_TE_LAYERS_TOTAL: usize = 24;
/// hidden_states[-2] = output of the 23rd layer -> run 23 of the 24.
pub const WOOSH_TE_LAYERS_RUN: usize = 23;
pub const WOOSH_TE_HEADS: usize = 16;
pub const WOOSH_TE_HEAD_DIM: usize = 64;
pub const WOOSH_TE_FFN: usize = 4096;
pub const WOOSH_TE_LN_EPS: f32 = 1e-5;
pub const WOOSH_TE_PAD_ID: u32 = 1;
pub const WOOSH_TE_BOS_ID: u32 = 0;
pub const WOOSH_TE_EOS_ID: u32 = 2;

// AE decoder geometry.
pub const WOOSH_AE_DIM: usize = 2048;
pub const WOOSH_AE_INTER: usize = 3072;
pub const WOOSH_AE_LAYERS: usize = 8;
pub const WOOSH_AE_NFFT: usize = 960;
pub const WOOSH_AE_HOP: usize = 480;
pub const WOOSH_AE_BINS: usize = WOOSH_AE_NFFT / 2 + 1; // 481
pub const WOOSH_AE_HEAD_OUT: usize = 3 * WOOSH_AE_BINS; // 1443
pub const WOOSH_AE_SAMPLES: usize = (WOOSH_LATENT_FRAMES - 1) * WOOSH_AE_HOP; // 240000

// Sampler defaults (test_Woosh-DFlow.py).
pub const WOOSH_DEFAULT_STEPS: usize = 4;
pub const WOOSH_DEFAULT_CFG: f32 = 4.5;
pub const WOOSH_DEFAULT_RENOISE: [f32; 4] = [0.0, 0.5, 0.5, 0.3];

/// Opens one single-file f32 safetensors checkpoint (all three Woosh zips
/// extract to `checkpoints/<Name>/weights.safetensors`).
pub fn woosh_open(path: impl AsRef<Path>) -> Result<Sa3Tensors> {
    Sa3Tensors::load(path)
}

// ---------------------------------------------------------------------------
// Rope tables (precompute_freqs_cis port, f32 op order mirroring torch).
// ---------------------------------------------------------------------------

/// The full audio-stream rope table: 1 memory-token row (= row 501 of the
/// YaRN table, the reference's "downsampled" prepend) followed by the 1002
/// YaRN rows. Layout: rows x [cos, sin] x 64 freq pairs, flattened
/// `(row * 64 + freq) * 2 + {0: cos, 1: sin}` — matching the dumped
/// `freqs_cis_audio_real` (1003, 64, 2) exactly.
pub fn woosh_freqs_cis_audio() -> Vec<f32> {
    let dim = WOOSH_HEAD_DIM; // max(qk_rope 112, qkv_head 128) = 128
    let half = WOOSH_ROPE_FREQS; // 64
    let seqlen = WOOSH_ROPE_TABLE_LEN; // 1002
    let original = WOOSH_LATENT_FRAMES; // 501
    let base = WOOSH_ROPE_THETA;

    // freqs = 1 / base^(2i/dim), f32 like torch.
    let mut freqs = vec![0f32; half];
    for (i, f) in freqs.iter_mut().enumerate() {
        *f = 1.0 / base.powf((2 * i) as f32 / dim as f32);
    }

    // YaRN correction (seqlen 1002 > original 501): blend freqs/factor with
    // freqs over the beta_fast..beta_slow correction ramp.
    if seqlen > original {
        let correction_dim = |num_rotations: f64| -> f64 {
            dim as f64 * (original as f64 / (num_rotations * 2.0 * std::f64::consts::PI)).ln()
                / (2.0 * (base as f64).ln())
        };
        let low = correction_dim(WOOSH_ROPE_BETA_FAST).floor().max(0.0);
        let high = correction_dim(WOOSH_ROPE_BETA_SLOW).ceil().min((dim - 1) as f64);
        let (low, high) = if low == high { (low, high + 0.001) } else { (low, high) };
        for (i, f) in freqs.iter_mut().enumerate() {
            // linear_ramp_factor over dim/2 entries, clamped to [0,1];
            // smooth = 1 - ramp.
            let ramp = ((i as f64 - low) / (high - low)).clamp(0.0, 1.0) as f32;
            let smooth = 1.0 - ramp;
            *f = *f / WOOSH_ROPE_FACTOR * (1.0 - smooth) + *f * smooth;
        }
    }

    let row = |pos: usize, out: &mut Vec<f32>| {
        for &f in &freqs {
            let angle = pos as f32 * f;
            out.push(angle.cos());
            out.push(angle.sin());
        }
    };
    let mut out = Vec::with_capacity((seqlen + 1) * half * 2);
    // memory-token row: freqs_cis[downsampling_factor/2 :: downsampling_factor]
    // with downsampling_factor = 1002 (n_memory_tokens_rope 1) -> row 501.
    row(seqlen / 2, &mut out);
    for pos in 0..seqlen {
        row(pos, &mut out);
    }
    out
}

// ---------------------------------------------------------------------------
// Timestep features.
// ---------------------------------------------------------------------------

/// FixedFourierFeaturesTime(1, 256, max_period 1e4, time_factor 1):
/// args = t * exp(-ln(1e4) * i/128), out = [cos(args), sin(args)] (256).
/// The freq table is also a checkpoint buffer; passing it in keeps parity
/// bit-tight with the reference (identical stored values).
pub fn woosh_fixed_fourier(t: f32, freqs: &[f32]) -> Vec<f32> {
    let half = freqs.len();
    let mut out = vec![0f32; half * 2];
    for (i, &f) in freqs.iter().enumerate() {
        let arg = t * f;
        out[i] = arg.cos();
        out[half + i] = arg.sin();
    }
    out
}

/// FourierFeaturesTime (the Flow-era learned-random features; only used to
/// validate the m_plus tap): f = 2*pi*t*W^T, out = [cos(f), sin(f)].
pub fn woosh_learned_fourier(t: f32, weight: &[f32]) -> Vec<f32> {
    let half = weight.len();
    let mut out = vec![0f32; half * 2];
    for (i, &w) in weight.iter().enumerate() {
        let arg = 2.0 * std::f32::consts::PI * t * w;
        out[i] = arg.cos();
        out[half + i] = arg.sin();
    }
    out
}

// ---------------------------------------------------------------------------
// Scalar math shared by the TE (erf-GELU) and AE (erf-GELU + softplus).
// ---------------------------------------------------------------------------

/// Exact-erf GELU: 0.5 * x * (1 + erf(x / sqrt(2))) — torch nn.GELU()
/// default and RoBERTa's "gelu"; computed through the f64 [`erf`] below.
pub fn gelu_erf(x: f32) -> f32 {
    (0.5 * x as f64 * (1.0 + erf(x as f64 / std::f64::consts::SQRT_2))) as f32
}

/// torch softplus (beta 1, threshold 20): x > 20 -> x, else ln(1 + e^x).
pub fn softplus(x: f32) -> f32 {
    if x > 20.0 {
        x
    } else {
        x.exp().ln_1p()
    }
}

/// Double-precision error function, |err| < 1e-13 over the whole line
/// (Maclaurin series below 2, Lentz continued fraction for erfc above).
/// Accurate far past f32 round-off; unit-tested against reference values.
pub fn erf(x: f64) -> f64 {
    let ax = x.abs();
    if ax < 2.0 {
        // erf(x) = 2/sqrt(pi) * sum_{n>=0} (-1)^n x^(2n+1) / (n! (2n+1))
        let x2 = x * x;
        let mut term = x;
        let mut sum = x;
        let mut n = 1.0f64;
        loop {
            term *= -x2 / n;
            let contrib = term / (2.0 * n + 1.0);
            sum += contrib;
            if contrib.abs() < 1e-18 * sum.abs().max(1e-30) || n > 80.0 {
                break;
            }
            n += 1.0;
        }
        sum * 2.0 / std::f64::consts::PI.sqrt()
    } else if ax >= 6.5 {
        // erfc < 4e-20: saturated at f64 granularity relevant here.
        x.signum()
    } else {
        // erfc(a) = exp(-a^2)/sqrt(pi) * K where K is the continued fraction
        // 1/(a + (1/2)/(a + (2/2)/(a + (3/2)/(a + ...)))) (Lentz's method).
        let a = ax;
        let mut f = a;
        let mut c = a;
        let mut d = 0.0f64;
        let mut k = 0.5f64;
        for _ in 0..200 {
            d = a + k * d;
            if d.abs() < 1e-300 {
                d = 1e-300;
            }
            c = a + k / c;
            if c.abs() < 1e-300 {
                c = 1e-300;
            }
            d = 1.0 / d;
            let delta = c * d;
            f *= delta;
            if (delta - 1.0).abs() < 1e-17 {
                break;
            }
            k += 0.5;
        }
        // f approximates a + K_tail; erfc = exp(-a^2)/(sqrt(pi) * f).
        let erfc = (-a * a).exp() / (std::f64::consts::PI.sqrt() * f);
        x.signum() * (1.0 - erfc)
    }
}

// ---------------------------------------------------------------------------
// DFlow Euler + renoise schedule.
// ---------------------------------------------------------------------------

/// One renoise adjustment for step i: given (t, r, renoise) returns
/// (t_hat, scale, std) with x' = scale * x + std * fresh_noise and the model
/// then called at (t_hat, r). renoise == 0 (or t_hat <= t) keeps x and t.
pub fn woosh_renoise(t: f32, r: f32, renoise: f32) -> Option<(f32, f32, f32)> {
    if renoise <= 0.0 {
        return None;
    }
    let gamma = renoise * (t - r);
    let t_hat = (t + gamma).min(1.0);
    if t_hat <= t {
        return None;
    }
    let scale = (1.0 - t_hat) / (1.0 - t + 1e-12);
    let std = (t_hat * t_hat - (t * scale) * (t * scale)).sqrt();
    Some((t_hat, scale, std))
}

/// t schedule: torch.linspace(1, 0, steps+1).
pub fn woosh_t_schedule(steps: usize) -> Vec<f32> {
    (0..=steps)
        .map(|i| 1.0 - i as f32 / steps as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erf_matches_reference_values() {
        // Reference values from python math.erf.
        let cases: [(f64, f64); 10] = [
            (0.0, 0.0),
            (0.1, 0.1124629160182849),
            (0.5, 0.5204998778130465),
            (1.0, 0.8427007929497149),
            (1.5, 0.9661051464753107),
            (2.0, 0.9953222650189527),
            (2.5, 0.999593047982555),
            (3.0, 0.9999779095030014),
            (4.0, 0.9999999845827421),
            (5.0, 0.9999999999984626),
        ];
        for (x, expected) in cases {
            assert!((erf(x) - expected).abs() < 1e-13, "erf({x}) = {}", erf(x));
            assert!((erf(-x) + expected).abs() < 1e-13, "erf(-{x})");
        }
    }

    #[test]
    fn softplus_matches_torch_threshold() {
        assert!((softplus(0.0) - 0.6931472).abs() < 1e-6);
        assert!((softplus(-5.0) - 0.006715348).abs() < 1e-8);
        assert_eq!(softplus(25.0), 25.0);
        assert!((softplus(19.0) - 19.000000005602796_f32).abs() < 1e-6);
    }

    #[test]
    fn t_schedule_and_renoise_match_reference() {
        let t = woosh_t_schedule(4);
        assert_eq!(t, vec![1.0, 0.75, 0.5, 0.25, 0.0]);
        // Values checked against the captured dump t_hat per step
        // (step1 t=0.875, step2 t=0.625, step3 t=0.325).
        assert!(woosh_renoise(1.0, 0.75, 0.0).is_none());
        let (t1, s1, n1) = woosh_renoise(0.75, 0.5, 0.5).unwrap();
        assert!((t1 - 0.875).abs() < 1e-7 && (s1 - 0.5).abs() < 1e-6);
        assert!((n1 - 0.625f32.sqrt()).abs() < 1e-6);
        let (t2, s2, n2) = woosh_renoise(0.5, 0.25, 0.5).unwrap();
        assert!((t2 - 0.625).abs() < 1e-7 && (s2 - 0.75).abs() < 1e-6);
        assert!((n2 - 0.5).abs() < 1e-6);
        let (t3, s3, n3) = woosh_renoise(0.25, 0.0, 0.3).unwrap();
        assert!((t3 - 0.325).abs() < 1e-7 && (s3 - 0.9).abs() < 1e-6);
        assert!((n3 - 0.055f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn audio_freqs_table_geometry() {
        let table = woosh_freqs_cis_audio();
        assert_eq!(table.len(), (WOOSH_ROPE_TABLE_LEN + 1) * WOOSH_ROPE_FREQS * 2);
        // memtok row equals YaRN row 501 (stored at offset (1 + 501) rows).
        let row = |i: usize| &table[i * WOOSH_ROPE_FREQS * 2..(i + 1) * WOOSH_ROPE_FREQS * 2];
        assert_eq!(row(0), row(502));
        // YaRN row 0 is all (cos 1, sin 0).
        for pair in row(1).chunks(2) {
            assert_eq!(pair, &[1.0, 0.0]);
        }
    }
}
