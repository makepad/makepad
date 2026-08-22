//! FLUX.2 shared foundation: pinned architecture facts, configs, the
//! empirical-mu flow schedule, and weight-layout readers for the new bundle.
//!
//! Fact sources (gathered 2026-08-13, see local/agent_state/flux2-port.md):
//! - github.com/black-forest-labs/flux2 `src/flux2/{model,sampling,
//!   autoencoder,text_encoder}.py` (canonical params, verbatim mu formula,
//!   VAE params, TE layer taps),
//! - diffusers `Flux2Transformer2DModel`/`Flux2Pipeline`,
//! - HF `black-forest-labs/FLUX.2-dev` file listing; the text encoder shards
//!   are BYTE-IDENTICAL (LFS sha) to ungated
//!   `mistralai/Mistral-Small-3.2-24B-Instruct-2506`, whose config.json is
//!   downloaded under local/models/flux2/text_encoder/.
//!
//! Everything here is host-side scaffolding: nothing depends on oracle dumps.
//! The compute graphs (flux2_transformer / flux2_text / flux2_vae) follow
//! once the .169 reference dumps exist to validate against.

use crate::error::{DiffusionError, Result};
use makepad_ai_h3::h3::H3ShardedWeights;
use makepad_ai_common::json::Json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The system message the reference pipeline puts in front of every t2i
/// prompt (diffusers 0.39.0 `pipelines/flux2/system_messages.py::SYSTEM_MESSAGE`,
/// copied there from the BFL flux2 repo). The `\n` mid-sentence is real: the
/// reference triple-quoted string wraps between "object" and "attribution".
pub const FLUX2_SYSTEM_MESSAGE: &str = "You are an AI that reasons about image descriptions. You give structured responses focusing on object relationships, object\nattribution and actions without speculation.";

/// Mistral-3 hidden-state layers tapped for conditioning; the three states
/// are stacked feature-wise into `3 * hidden = 15360` channels per token.
pub const FLUX2_HIDDEN_STATE_TAPS: [usize; 3] = [10, 20, 30];

/// Qwen3-4B hidden-state taps used by FLUX.2-klein (BFL `OUTPUT_LAYERS_QWEN3`).
/// `hidden_states[k]` is the raw output of decoder layer `k-1` (index 0 is
/// the embedding), stacked feature-wise into `3 * 2560 = 7680` channels.
pub const FLUX2_KLEIN_HIDDEN_STATE_TAPS: [usize; 3] = [9, 18, 27];

/// Qwen3 pad id (`<|endoftext|>`) used by the Klein TE window.
pub const FLUX2_KLEIN_PAD_TOKEN: u32 = 151643;

// --- transformer -----------------------------------------------------------

/// FLUX.2 rectified-flow DiT parameters (BFL `Flux2Params`).
///
/// vs FLUX.1: modulation is GLOBAL (three shared SiLU+Linear modules computed
/// once per step from the time/guidance embedding — no per-block adaLN
/// weights), all linears are bias-free, FFNs are SwiGLU at ratio 3.0 (vs GELU
/// 4.0), RoPE has four axes (T,H,W,L) at theta 2000, there is no CLIP pooled
/// `vector_in`, and patchify (2x2 over 32 VAE channels -> 128) happens in the
/// pipeline so the model's `patch_size` is 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flux2TransformerConfig {
    /// Packed latent channels per token (32 VAE channels x 2x2 patch).
    pub in_channels: u32,
    /// Conditioning width from the text encoder (3 x 5120).
    pub context_in_dim: u32,
    pub hidden_size: u32,
    pub num_heads: u32,
    pub head_dim: u32,
    /// Double-stream (img/txt) block count.
    pub depth: u32,
    /// Single-stream (fused parallel attn+mlp) block count.
    pub depth_single_blocks: u32,
    /// SwiGLU inner width (mlp_ratio 3.0 x hidden).
    pub mlp_hidden_size: u32,
    /// RoPE per-axis channel split over head_dim: (T, H, W, L).
    pub axes_dim: [u32; 4],
    pub theta: u32,
    pub guidance_embed: bool,
}

impl Flux2TransformerConfig {
    pub const fn flux2_dev() -> Self {
        Self {
            in_channels: 128,
            context_in_dim: 15360,
            hidden_size: 6144,
            num_heads: 48,
            head_dim: 128,
            depth: 8,
            depth_single_blocks: 48,
            mlp_hidden_size: 18432,
            axes_dim: [32, 32, 32, 32],
            theta: 2000,
            guidance_embed: true,
        }
    }

    /// FLUX.2-klein-4B (pinned from the ungated repo's transformer/config.json,
    /// fetched 2026-08-13): a strict parameter shrink of dev sharing head_dim,
    /// rope axes/theta and the 128-channel packed latent space; distilled
    /// WITHOUT a guidance input. Its text encoder is stock Qwen3-4B (hidden
    /// 2560) tapped at layers [9, 18, 27] -> context 7680.
    pub const fn flux2_klein_4b() -> Self {
        Self {
            in_channels: 128,
            context_in_dim: 7680,
            hidden_size: 3072,
            num_heads: 24,
            head_dim: 128,
            depth: 5,
            depth_single_blocks: 20,
            mlp_hidden_size: 9216,
            axes_dim: [32, 32, 32, 32],
            theta: 2000,
            guidance_embed: false,
        }
    }

    /// Unpacked VAE latent channels (before the pipeline's 2x2 patchify).
    pub const fn latent_channels(&self) -> u32 {
        self.in_channels / 4
    }

    /// Single-block fused `linear1` output width: qkv + both SwiGLU inputs.
    pub const fn single_linear1_out(&self) -> u32 {
        3 * self.hidden_size + 2 * self.mlp_hidden_size
    }

    /// Single-block fused `linear2` input width: attn out + gated mlp.
    pub const fn single_linear2_in(&self) -> u32 {
        self.hidden_size + self.mlp_hidden_size
    }

    /// Image tokens for a pixel resolution (16x total compression: VAE 8x
    /// spatial, then 2x2 patchify).
    pub const fn image_seq_len(&self, width_px: u32, height_px: u32) -> u32 {
        (width_px / 16) * (height_px / 16)
    }
}

/// State-dict name style of a FLUX.2 transformer file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flux2TransformerNameStyle {
    /// BFL canonical (`double_blocks.N.img_attn.qkv.weight`, ...): the
    /// single-file releases and the Comfy-Org repack.
    Canonical,
    /// diffusers `Flux2Transformer2DModel` module names.
    Diffusers,
    Unknown,
}

/// Detect the name style from a tensor-name listing (safetensors header).
/// NOTE: the `double_stream_modulation_*`/`single_stream_modulation` module
/// names appear in BOTH styles (the canonical fp8mixed file carries them as
/// `*.lin.weight`, pinned from its real header), so only the block prefixes
/// discriminate.
pub fn detect_flux2_transformer_name_style<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Flux2TransformerNameStyle {
    let mut saw_canonical = false;
    let mut saw_diffusers = false;
    for name in names {
        if name.starts_with("double_blocks.") || name.starts_with("single_blocks.") {
            saw_canonical = true;
        }
        if name.starts_with("transformer_blocks.")
            || name.starts_with("single_transformer_blocks.")
        {
            saw_diffusers = true;
        }
    }
    match (saw_canonical, saw_diffusers) {
        (true, false) => Flux2TransformerNameStyle::Canonical,
        (false, true) => Flux2TransformerNameStyle::Diffusers,
        _ => Flux2TransformerNameStyle::Unknown,
    }
}

/// Canonical (BFL) transformer tensor names, pinned VERBATIM from the real
/// Comfy-Org fp8mixed header (555 tensors, read 2026-08-13):
/// - per double block N in 0..8: `double_blocks.N.{img,txt}_attn.qkv.weight`
///   [18432,6144] bf16, `..._attn.proj.weight` [6144,6144] bf16,
///   `..._attn.norm.{query,key}_norm.scale` [128] bf16,
///   `..._mlp.{0,2}.weight` ([36864,6144]/[6144,18432], fp8_e4m3 in the
///   Comfy file with scalar f32 `.weight_scale` + `.input_scale` siblings)
/// - per single block N in 0..48: `single_blocks.N.linear{1,2}.weight`
///   ([55296,6144]/[6144,24576], fp8 + scales), `single_blocks.N.norm.
///   {query,key}_norm.scale` [128] bf16
/// - globals (all bf16, NO biases anywhere):
///   `img_in.weight` [6144,128], `txt_in.weight` [6144,15360],
///   `time_in.{in,out}_layer.weight` [6144,256]/[6144,6144],
///   `guidance_in.{in,out}_layer.weight` [6144,256]/[6144,6144],
///   `double_stream_modulation_img.lin.weight` [36864,6144] (6 x hidden),
///   `double_stream_modulation_txt.lin.weight` [36864,6144],
///   `single_stream_modulation.lin.weight`     [18432,6144] (3 x hidden),
///   `final_layer.adaLN_modulation.1.weight` [12288,6144],
///   `final_layer.linear.weight` [128,6144]
pub const FLUX2_CANONICAL_GLOBAL_TENSORS: [&str; 11] = [
    "img_in.weight",
    "txt_in.weight",
    "time_in.in_layer.weight",
    "time_in.out_layer.weight",
    "guidance_in.in_layer.weight",
    "guidance_in.out_layer.weight",
    "double_stream_modulation_img.lin.weight",
    "double_stream_modulation_txt.lin.weight",
    "single_stream_modulation.lin.weight",
    "final_layer.adaLN_modulation.1.weight",
    "final_layer.linear.weight",
];

// --- text encoder ------------------------------------------------------------

/// Mistral-Small-3.2-24B language model (the FLUX.2 conditioning tower).
/// Pinned from the downloaded `text_encoder/config.json` `text_config`.
/// Plain dense decoder: GQA 32q/8kv at head_dim 128, SwiGLU 32768, RMSNorm,
/// rotate-half RoPE at theta 1e9, NO qk-norm, NO biases (both confirmed by
/// the shard index tensor listing), global attention (`sliding_window: null`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mistral3TextConfig {
    pub hidden_size: u32,
    pub num_layers: u32,
    pub num_attention_heads: u32,
    pub num_key_value_heads: u32,
    pub head_dim: u32,
    pub intermediate_size: u32,
    pub rms_norm_eps: f32,
    pub rope_theta: f64,
    pub vocab_size: u32,
}

impl Mistral3TextConfig {
    pub const fn flux2_dev() -> Self {
        Self {
            hidden_size: 5120,
            num_layers: 40,
            num_attention_heads: 32,
            num_key_value_heads: 8,
            head_dim: 128,
            intermediate_size: 32768,
            rms_norm_eps: 1e-5,
            rope_theta: 1e9,
            vocab_size: 131072,
        }
    }

    /// Layers that must actually run for t2i conditioning: hidden-state tap
    /// `k` is the output of decoder layer `k-1` (HF `hidden_states[k]`
    /// counts the embedding as index 0), so layers `0..taps.max()` suffice —
    /// layers 30..40, the final norm and lm_head are dead weight for t2i.
    pub fn layers_required_for_taps(&self) -> u32 {
        *FLUX2_HIDDEN_STATE_TAPS.iter().max().unwrap() as u32
    }

    /// Conditioning channels handed to the DiT (stacked taps).
    pub const fn conditioning_dim(&self) -> u32 {
        self.hidden_size * FLUX2_HIDDEN_STATE_TAPS.len() as u32
    }
}

/// Stock Qwen3-4B language model (the FLUX.2-klein-4B conditioning tower).
/// Pinned from `black-forest-labs/FLUX.2-klein-4B` `text_encoder/config.json`.
/// Dense decoder: GQA 32q/8kv at head_dim 128, SwiGLU 9728, RMSNorm, qk-norm,
/// rotate-half RoPE at theta 1e6, no biases, global attention.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Qwen3TextConfig {
    pub hidden_size: u32,
    pub num_layers: u32,
    pub num_attention_heads: u32,
    pub num_key_value_heads: u32,
    pub head_dim: u32,
    pub intermediate_size: u32,
    pub rms_norm_eps: f32,
    pub rope_theta: f64,
    pub vocab_size: u32,
}

impl Qwen3TextConfig {
    pub const fn flux2_klein_4b() -> Self {
        Self {
            hidden_size: 2560,
            num_layers: 36,
            num_attention_heads: 32,
            num_key_value_heads: 8,
            head_dim: 128,
            intermediate_size: 9728,
            rms_norm_eps: 1e-6,
            rope_theta: 1_000_000.0,
            vocab_size: 151936,
        }
    }

    /// Layers that must run for Klein conditioning: hidden-state tap `k`
    /// is the output of decoder layer `k-1`.
    pub fn layers_required_for_taps(&self) -> u32 {
        *FLUX2_KLEIN_HIDDEN_STATE_TAPS.iter().max().unwrap() as u32
    }

    pub const fn conditioning_dim(&self) -> u32 {
        self.hidden_size * FLUX2_KLEIN_HIDDEN_STATE_TAPS.len() as u32
    }
}

// --- VAE ---------------------------------------------------------------------

/// FLUX.2 autoencoder (BFL `AutoEncoderParams` + the latent BatchNorm).
/// Same conv skeleton family as FLUX.1's AE (ch 128, mult [1,2,4,4], 2 res
/// blocks, mid attention, GroupNorm+swish) with z_channels 32 (vs 16),
/// DETERMINISTIC encode (mean only), and — replacing FLUX.1's
/// scaling/shift factors — a BatchNorm2d (affine=false, running stats, eps
/// 1e-4) applied over the 2x2-PACKED 128-channel latents:
/// `z_norm = (z - running_mean) / sqrt(running_var + eps)`, decode inverts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flux2VaeConfig {
    pub ch: u32,
    pub ch_mult: [u32; 4],
    pub num_res_blocks: u32,
    pub z_channels: u32,
    /// Pipeline-side patchify factor; the BatchNorm operates on
    /// `z_channels * patch^2` packed channels.
    pub patch: u32,
    pub batch_norm_eps: f32,
}

impl Flux2VaeConfig {
    pub const fn flux2_dev() -> Self {
        Self {
            ch: 128,
            ch_mult: [1, 2, 4, 4],
            num_res_blocks: 2,
            z_channels: 32,
            patch: 2,
            batch_norm_eps: 1e-4,
        }
    }

    pub const fn packed_channels(&self) -> u32 {
        self.z_channels * self.patch * self.patch
    }

    /// Klein shares the Flux.2 autoencoder (32ch + 2x2 pack + BN).
    pub const fn flux2_klein_4b() -> Self {
        Self::flux2_dev()
    }
}

/// Time-axis stride between successive reference images (BFL `encode_image_refs`).
pub const FLUX2_REF_TIME_SCALE: i32 = 10;

/// One packed image token's 4-axis RoPE id `(T, H, W, L)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flux2PosId {
    pub t: f32,
    pub h: f32,
    pub w: f32,
    pub l: f32,
}

impl Flux2PosId {
    pub fn as_array(self) -> [f32; 4] {
        [self.t, self.h, self.w, self.l]
    }
}

/// Packed latent plane: `channels = 128` after the VAE's 2x2 pack.
#[derive(Clone, Debug)]
pub struct Flux2PackedLatents {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    /// Channel-major planar `[C, H*W]`.
    pub data: Vec<f32>,
}

impl Flux2PackedLatents {
    pub fn token_count(&self) -> usize {
        self.width * self.height
    }

    /// Flatten to token-major `[H*W, C]` (DiT / `prc_img` layout).
    pub fn to_tokens(&self) -> Vec<f32> {
        let tokens = self.token_count();
        let mut out = vec![0.0f32; tokens * self.channels];
        for token in 0..tokens {
            for channel in 0..self.channels {
                out[token * self.channels + channel] = self.data[channel * tokens + token];
            }
        }
        out
    }

    pub fn from_tokens(tokens: &[f32], width: usize, height: usize, channels: usize) -> Result<Self> {
        let count = width * height;
        if tokens.len() != count * channels {
            return Err(DiffusionError::workflow(format!(
                "flux2 packed tokens expected {} values, got {}",
                count * channels,
                tokens.len()
            )));
        }
        let mut data = vec![0.0f32; channels * count];
        for token in 0..count {
            for channel in 0..channels {
                data[channel * count + token] = tokens[token * channels + channel];
            }
        }
        Ok(Self {
            width,
            height,
            channels,
            data,
        })
    }
}

/// Pack a mean-only 32-channel latent plane into the 128-channel 2x2 layout
/// (`rearrange "... c (i pi) (j pj) -> ... (c pi pj) i j"` with `pi=pj=2`).
pub fn flux2_pack_latents(
    mean: &[f32],
    width: usize,
    height: usize,
    z_channels: usize,
) -> Result<Flux2PackedLatents> {
    if width % 2 != 0 || height % 2 != 0 {
        return Err(DiffusionError::workflow(format!(
            "flux2 pack requires even latent size, got {width}x{height}"
        )));
    }
    let src_plane = width * height;
    if mean.len() != z_channels * src_plane {
        return Err(DiffusionError::workflow(format!(
            "flux2 pack expected {} values, got {}",
            z_channels * src_plane,
            mean.len()
        )));
    }
    let packed_w = width / 2;
    let packed_h = height / 2;
    let packed_ch = z_channels * 4;
    let packed_plane = packed_w * packed_h;
    let mut data = vec![0.0f32; packed_ch * packed_plane];
    for c in 0..z_channels {
        for pi in 0..2 {
            for pj in 0..2 {
                let dest_c = c * 4 + pi * 2 + pj;
                for i in 0..packed_h {
                    for j in 0..packed_w {
                        let src = c * src_plane + (i * 2 + pi) * width + (j * 2 + pj);
                        let dest = dest_c * packed_plane + i * packed_w + j;
                        data[dest] = mean[src];
                    }
                }
            }
        }
    }
    Ok(Flux2PackedLatents {
        width: packed_w,
        height: packed_h,
        channels: packed_ch,
        data,
    })
}

/// Inverse of [`flux2_pack_latents`].
pub fn flux2_unpack_latents(packed: &Flux2PackedLatents, z_channels: usize) -> Result<Vec<f32>> {
    if packed.channels != z_channels * 4 {
        return Err(DiffusionError::workflow(format!(
            "flux2 unpack expected {} packed channels, got {}",
            z_channels * 4,
            packed.channels
        )));
    }
    let width = packed.width * 2;
    let height = packed.height * 2;
    let src_plane = packed.token_count();
    let dest_plane = width * height;
    let mut mean = vec![0.0f32; z_channels * dest_plane];
    for c in 0..z_channels {
        for pi in 0..2 {
            for pj in 0..2 {
                let src_c = c * 4 + pi * 2 + pj;
                for i in 0..packed.height {
                    for j in 0..packed.width {
                        let src = src_c * src_plane + i * packed.width + j;
                        let dest = c * dest_plane + (i * 2 + pi) * width + (j * 2 + pj);
                        mean[dest] = packed.data[src];
                    }
                }
            }
        }
    }
    Ok(mean)
}

/// Image-token ids for one packed plane at time offset `t`.
pub fn flux2_image_ids(width: usize, height: usize, t: i32) -> Vec<Flux2PosId> {
    let mut ids = Vec::with_capacity(width * height);
    for h in 0..height {
        for w in 0..width {
            ids.push(Flux2PosId {
                t: t as f32,
                h: h as f32,
                w: w as f32,
                l: 0.0,
            });
        }
    }
    ids
}

/// Text-token ids: L-axis `arange(seq)`, other axes zero.
pub fn flux2_text_ids(seq: usize) -> Vec<Flux2PosId> {
    (0..seq)
        .map(|l| Flux2PosId {
            t: 0.0,
            h: 0.0,
            w: 0.0,
            l: l as f32,
        })
        .collect()
}

/// Concatenate generated-image tokens with reference tokens on the sequence
/// axis. Each subsequent reference uses T = 10, 20, … (BFL `encode_image_refs`).
pub fn flux2_concat_ref_tokens(
    generated: &[f32],
    generated_ids: &[Flux2PosId],
    refs: &[(Vec<f32>, Vec<Flux2PosId>)],
) -> (Vec<f32>, Vec<Flux2PosId>) {
    let mut tokens = generated.to_vec();
    let mut ids = generated_ids.to_vec();
    for (ref_tokens, ref_ids) in refs {
        tokens.extend_from_slice(ref_tokens);
        ids.extend_from_slice(ref_ids);
    }
    (tokens, ids)
}

/// BFL `encode_image_refs` time offsets: `10 + 10 * index`.
pub fn flux2_ref_time_offset(index: usize) -> i32 {
    FLUX2_REF_TIME_SCALE * (1 + index as i32)
}

// --- schedule ------------------------------------------------------------------

/// Verbatim port of BFL `compute_empirical_mu` (src/flux2/sampling.py):
/// piecewise in `image_seq_len`, interpolated in `num_steps` below 4300.
pub fn flux2_compute_empirical_mu(image_seq_len: usize, num_steps: usize) -> f64 {
    let (a1, b1) = (8.73809524e-05_f64, 1.89833333_f64);
    let (a2, b2) = (0.00016927_f64, 0.45666666_f64);
    let seq = image_seq_len as f64;

    if image_seq_len > 4300 {
        return a2 * seq + b2;
    }

    let m_200 = a2 * seq + b2;
    let m_10 = a1 * seq + b1;

    let a = (m_200 - m_10) / 190.0;
    let b = m_200 - 200.0 * a;
    a * num_steps as f64 + b
}

/// Sigma schedule with the exact arithmetic of the frozen reference
/// (`Flux2Pipeline.__call__` + `FlowMatchEulerDiscreteScheduler.set_timesteps`,
/// diffusers 0.39.0): `np.linspace(1.0, 1/num_steps, num_steps)` in f64,
/// `.astype(np.float32)`, then the exponential dynamic shift computed IN F32 —
/// NEP-50 weak scalars demote `math.exp(mu)` to f32 before the array ops —
/// and a terminal 0.0 appended. Returns `num_steps + 1` sigmas, first 1.0,
/// last 0.0; the euler update is `x += (sigma_next - sigma) * pred` with the
/// sample upcast to f32 and the result rounded back to bf16.
///
/// The f32-vs-f64 shift ordering only moves the last couple of ulps, but the
/// validator gates these values against the oracle `sigmas` dump exactly, so
/// the port reproduces the reference arithmetic operation for operation.
pub fn flux2_schedule(num_steps: usize, image_seq_len: usize) -> Result<Vec<f32>> {
    if num_steps == 0 {
        return Err(DiffusionError::workflow(
            "flux2 schedule requires at least one inference step",
        ));
    }
    let mu = flux2_compute_empirical_mu(image_seq_len, num_steps);
    let exp_mu = mu.exp() as f32;
    let start = 1.0_f64;
    let stop = 1.0_f64 / num_steps as f64;
    let mut sigmas = Vec::with_capacity(num_steps + 1);
    for index in 0..num_steps {
        // np.linspace: start + i*step in f64, endpoint overwritten with stop.
        let value = if num_steps == 1 {
            start
        } else if index + 1 == num_steps {
            stop
        } else {
            start + index as f64 * ((stop - start) / (num_steps - 1) as f64)
        };
        let sigma = value as f32;
        sigmas.push(exp_mu / (exp_mu + (1.0f32 / sigma - 1.0f32)));
    }
    sigmas.push(0.0);
    Ok(sigmas)
}

// --- text encoder weight index ---------------------------------------------------

/// Reader for the sharded `text_encoder/` bundle via its
/// `model.safetensors.index.json` — resolves which shard holds each tensor
/// without opening any weight file, so it works mid-download and lets the
/// loader stream only the t2i-relevant subset.
pub struct Flux2TextEncoderIndex {
    pub dir: PathBuf,
    weight_map: HashMap<String, String>,
}

/// What the index says about the bundle, verified against
/// [`Mistral3TextConfig`].
#[derive(Clone, Debug)]
pub struct Mistral3IndexReport {
    pub tensor_count: usize,
    pub language_model_layers: u32,
    pub has_vision_tower: bool,
    pub has_lm_head: bool,
    /// Shard files needed for t2i conditioning (embeddings + layers
    /// `0..layers_required_for_taps()`), sorted.
    pub t2i_shard_files: Vec<String>,
    /// All shard files, sorted.
    pub all_shard_files: Vec<String>,
}

const MISTRAL3_LAYER_TENSORS: [&str; 9] = [
    "input_layernorm.weight",
    "post_attention_layernorm.weight",
    "self_attn.q_proj.weight",
    "self_attn.k_proj.weight",
    "self_attn.v_proj.weight",
    "self_attn.o_proj.weight",
    "mlp.gate_proj.weight",
    "mlp.up_proj.weight",
    "mlp.down_proj.weight",
];

pub const MISTRAL3_EMBED_TOKENS: &str = "language_model.model.embed_tokens.weight";

pub fn mistral3_layer_prefix(layer: u32) -> String {
    format!("language_model.model.layers.{layer}.")
}

impl Flux2TextEncoderIndex {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let path = dir.join("model.safetensors.index.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|err| DiffusionError::io(&path, err.to_string()))?;
        let root = Json::parse(&text).map_err(|msg| DiffusionError::json(&path, msg))?;
        let map = root
            .get("weight_map")
            .and_then(Json::as_obj)
            .ok_or_else(|| DiffusionError::model("index.json has no weight_map object"))?;
        let mut weight_map = HashMap::with_capacity(map.len());
        for (name, shard) in map {
            let shard = shard.as_str().ok_or_else(|| {
                DiffusionError::model(format!("weight_map[{:?}] is not a string", name))
            })?;
            weight_map.insert(name.clone(), shard.to_owned());
        }
        Ok(Self { dir, weight_map })
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.weight_map.contains_key(name)
    }

    pub fn shard_for(&self, name: &str) -> Option<&str> {
        self.weight_map.get(name).map(String::as_str)
    }

    pub fn tensor_count(&self) -> usize {
        self.weight_map.len()
    }

    /// Verify the language-model side matches the pinned config and derive
    /// the t2i shard subset. Errors on missing tensors or on tensors the
    /// pinned architecture says must not exist (biases, qk-norms).
    pub fn verify_mistral3(&self, config: &Mistral3TextConfig) -> Result<Mistral3IndexReport> {
        if !self.has_tensor(MISTRAL3_EMBED_TOKENS) {
            return Err(DiffusionError::model(format!(
                "text encoder index is missing {}",
                MISTRAL3_EMBED_TOKENS
            )));
        }
        for layer in 0..config.num_layers {
            let prefix = mistral3_layer_prefix(layer);
            for suffix in MISTRAL3_LAYER_TENSORS {
                let name = format!("{prefix}{suffix}");
                if !self.has_tensor(&name) {
                    return Err(DiffusionError::model(format!(
                        "text encoder index is missing {name}"
                    )));
                }
            }
        }
        // The pinned architecture is bias-free with no qk-norm; a file that
        // has them is NOT the model these constants describe.
        for name in self.weight_map.keys() {
            if name.starts_with("language_model.")
                && (name.ends_with(".bias")
                    || name.contains(".q_norm.")
                    || name.contains(".k_norm."))
            {
                return Err(DiffusionError::model(format!(
                    "unexpected tensor for Mistral-3: {name}"
                )));
            }
        }
        // No layer beyond the configured count.
        let beyond = mistral3_layer_prefix(config.num_layers);
        if self.weight_map.keys().any(|name| name.starts_with(&beyond)) {
            return Err(DiffusionError::model(format!(
                "text encoder index has layers beyond the configured {}",
                config.num_layers
            )));
        }

        let mut t2i_names: Vec<String> = vec![MISTRAL3_EMBED_TOKENS.to_owned()];
        for layer in 0..config.layers_required_for_taps() {
            let prefix = mistral3_layer_prefix(layer);
            for suffix in MISTRAL3_LAYER_TENSORS {
                t2i_names.push(format!("{prefix}{suffix}"));
            }
        }
        let mut t2i_shard_files: Vec<String> = t2i_names
            .iter()
            .filter_map(|name| self.weight_map.get(name).cloned())
            .collect();
        t2i_shard_files.sort();
        t2i_shard_files.dedup();

        let mut all_shard_files: Vec<String> = self.weight_map.values().cloned().collect();
        all_shard_files.sort();
        all_shard_files.dedup();

        Ok(Mistral3IndexReport {
            tensor_count: self.weight_map.len(),
            language_model_layers: config.num_layers,
            has_vision_tower: self
                .weight_map
                .keys()
                .any(|name| name.starts_with("vision_tower.")),
            has_lm_head: self.has_tensor("language_model.lm_head.weight"),
            t2i_shard_files,
            all_shard_files,
        })
    }

    /// True when every shard file referenced by the index exists on disk
    /// (does NOT validate sizes — a mid-download shard still counts; open
    /// with [`Flux2TextEncoderWeights::load`] for the real header check).
    pub fn shard_files_present(&self) -> bool {
        let mut files: Vec<&String> = self.weight_map.values().collect();
        files.sort();
        files.dedup();
        files.iter().all(|file| self.dir.join(file).is_file())
    }
}

pub use makepad_ai_common::raw_st::{Flux2SafetensorsHeader, Flux2TensorInfo};

/// Safetensors-style uppercase dtype tag for a ggml tensor type (what the
/// rest of the flux2 loader matches on: `"BF16"`, `"F32"`, `"Q4_K"`, …).
pub fn flux2_gguf_dtype_name(ggml_type: u32) -> &'static str {
    use makepad_ai_loader::quant::*;
    match ggml_type {
        GGML_TYPE_F32 => "F32",
        GGML_TYPE_F16 => "F16",
        GGML_TYPE_BF16 => "BF16",
        GGML_TYPE_Q4_0 => "Q4_0",
        GGML_TYPE_Q4_1 => "Q4_1",
        GGML_TYPE_Q5_0 => "Q5_0",
        GGML_TYPE_Q5_1 => "Q5_1",
        GGML_TYPE_Q8_0 => "Q8_0",
        GGML_TYPE_Q2_K => "Q2_K",
        GGML_TYPE_Q3_K => "Q3_K",
        GGML_TYPE_Q4_K => "Q4_K",
        GGML_TYPE_Q5_K => "Q5_K",
        GGML_TYPE_Q6_K => "Q6_K",
        GGML_TYPE_Q8_K => "Q8_K",
        _ => "UNKNOWN",
    }
}

/// A GGUF DiT file (city96 / ComfyUI-GGUF `FLUX.2-dev-gguf`) behind the
/// same tensor-name API as the safetensors headers: names are the canonical
/// BFL `double_blocks.*` / `single_blocks.*` / `img_in.*` set, GGUF dims are
/// ggml order (`[k, n]`) and are reported reversed as a `[n, k]` shape,
/// `dtype` is the ggml type name (`"Q4_K"`, `"Q5_K"`, `"BF16"`, `"F32"`, …).
/// Quantized tensors are served raw (`read_bytes`); the f32/bf16/f16 readers
/// only accept scalar types.
pub struct Flux2GgufSource {
    pub file: makepad_ai_llm::GgufFile,
    tensors: HashMap<String, Flux2TensorInfo>,
}

impl Flux2GgufSource {
    pub fn open(path: &Path) -> Result<Self> {
        let file = makepad_ai_llm::GgufFile::open(path)
            .map_err(|err| DiffusionError::io(path, err.to_string()))?;
        let mut tensors = HashMap::with_capacity(file.tensors.len());
        for tensor in &file.tensors {
            let shape: Vec<u64> = tensor.dimensions.iter().rev().copied().collect();
            tensors.insert(
                tensor.name.clone(),
                Flux2TensorInfo {
                    dtype: flux2_gguf_dtype_name(tensor.tensor_type.ggml_type()).to_string(),
                    shape,
                    data_offsets: (tensor.offset, tensor.offset + tensor.size_bytes),
                },
            );
        }
        Ok(Self { file, tensors })
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.tensors.contains_key(name)
    }

    pub fn tensor(&self, name: &str) -> Result<&Flux2TensorInfo> {
        self.tensors
            .get(name)
            .ok_or_else(|| DiffusionError::model(format!("flux2 gguf tensor '{name}' not found")))
    }

    /// ggml type id of a tensor (for the quantized linear path).
    pub fn ggml_type(&self, name: &str) -> Result<u32> {
        Ok(self
            .file
            .get_tensor(name)
            .ok_or_else(|| DiffusionError::model(format!("flux2 gguf tensor '{name}' not found")))?
            .tensor_type
            .ggml_type())
    }

    pub fn read_bytes(&self, name: &str) -> Result<Vec<u8>> {
        self.file
            .read_tensor_bytes(name)
            .map_err(|err| DiffusionError::io(&self.file.path, err.to_string()))
    }

    pub fn read_f32(&self, name: &str) -> Result<Vec<f32>> {
        let info = self.tensor(name)?;
        let bytes = self.read_bytes(name)?;
        match info.dtype.as_str() {
            "F32" => Ok(bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()),
            "F16" => Ok(bytes
                .chunks_exact(2)
                .map(|chunk| makepad_ai_common::f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
                .collect()),
            "BF16" => Ok(bytes
                .chunks_exact(2)
                .map(|chunk| f32::from_bits((u16::from_le_bytes([chunk[0], chunk[1]]) as u32) << 16))
                .collect()),
            other => Err(DiffusionError::model(format!(
                "flux2 gguf tensor '{name}' is {other}: not readable as f32 (quantized linears go through the resident quant path)"
            ))),
        }
    }

    pub fn read_bf16_bytes(&self, name: &str) -> Result<Vec<u8>> {
        if self.tensor(name)?.dtype == "BF16" {
            return self.read_bytes(name);
        }
        let values = self.read_f32(name)?;
        let mut out = Vec::with_capacity(values.len() * 2);
        for value in values {
            // RN-even to the bf16 grid (same rounding as the safetensors twin).
            let bits = value.to_bits();
            let rounding = 0x7fffu32 + ((bits >> 16) & 1);
            out.extend_from_slice(&(((bits.wrapping_add(rounding)) >> 16) as u16).to_le_bytes());
        }
        Ok(out)
    }

    pub fn read_f16_bytes(&self, name: &str) -> Result<Vec<u8>> {
        if self.tensor(name)?.dtype == "F16" {
            return self.read_bytes(name);
        }
        let values = self.read_f32(name)?;
        let mut out = Vec::with_capacity(values.len() * 2);
        for value in values {
            out.extend_from_slice(&makepad_ai_common::f32_to_f16(value).to_le_bytes());
        }
        Ok(out)
    }
}

/// Single-file (or sharded-dir) Klein/FLUX.2 weight source. Tensor lookup
/// prefers the first file that contains the name. FP8 weights dequant to
/// f16/f32 at read (see [`Flux2SafetensorsHeader::read_f32`]). A `.gguf`
/// file opens as a [`Flux2GgufSource`] instead (quantized DiT tiers).
pub struct Flux2WeightFile {
    pub files: Vec<Flux2SafetensorsHeader>,
    pub gguf: Option<Flux2GgufSource>,
}

impl Flux2WeightFile {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.is_file() {
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
            {
                return Ok(Self {
                    files: Vec::new(),
                    gguf: Some(Flux2GgufSource::open(path)?),
                });
            }
            return Ok(Self {
                files: vec![Flux2SafetensorsHeader::load(path)?],
                gguf: None,
            });
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|err| DiffusionError::io(path, err.to_string()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|p| p.extension().map(|ext| ext == "safetensors").unwrap_or(false))
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(DiffusionError::model(format!(
                "flux2 weights dir {} holds no safetensors",
                path.display()
            )));
        }
        let headers = files
            .iter()
            .map(Flux2SafetensorsHeader::load)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            files: headers,
            gguf: None,
        })
    }

    pub fn header_for(&self, name: &str) -> Result<&Flux2SafetensorsHeader> {
        self.files
            .iter()
            .find(|file| file.has_tensor(name))
            .ok_or_else(|| DiffusionError::model(format!("flux2 tensor '{name}' not found")))
    }

    pub fn is_gguf(&self) -> bool {
        self.gguf.is_some()
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        if let Some(gguf) = &self.gguf {
            return gguf.has_tensor(name);
        }
        self.files.iter().any(|file| file.has_tensor(name))
    }

    pub fn tensor(&self, name: &str) -> Result<&Flux2TensorInfo> {
        if let Some(gguf) = &self.gguf {
            return gguf.tensor(name);
        }
        Ok(self.header_for(name)?.tensor(name).unwrap())
    }

    /// ggml type id of a GGUF tensor; `None` for safetensors sources.
    pub fn gguf_ggml_type(&self, name: &str) -> Option<u32> {
        self.gguf.as_ref().and_then(|gguf| gguf.ggml_type(name).ok())
    }

    pub fn read_f32(&self, name: &str) -> Result<Vec<f32>> {
        if let Some(gguf) = &self.gguf {
            return gguf.read_f32(name);
        }
        self.header_for(name)?.read_f32(name)
    }

    pub fn read_f16_bytes(&self, name: &str) -> Result<Vec<u8>> {
        if let Some(gguf) = &self.gguf {
            return gguf.read_f16_bytes(name);
        }
        self.header_for(name)?.read_f16_bytes(name)
    }

    pub fn read_bf16_bytes(&self, name: &str) -> Result<Vec<u8>> {
        if let Some(gguf) = &self.gguf {
            return gguf.read_bf16_bytes(name);
        }
        self.header_for(name)?.read_bf16_bytes(name)
    }

    pub fn read_bytes(&self, name: &str) -> Result<Vec<u8>> {
        if let Some(gguf) = &self.gguf {
            return gguf.read_bytes(name);
        }
        self.header_for(name)?.read_bytes(name)
    }

    /// Detect Klein vs dev from `txt_in` / `context_embedder` width.
    pub fn detect_transformer_config(&self) -> Result<Flux2TransformerConfig> {
        for name in [
            "txt_in.weight",
            "context_embedder.weight",
            "transformer.txt_in.weight",
            "transformer.context_embedder.weight",
        ] {
            if !self.has_tensor(name) {
                continue;
            }
            let shape = &self.tensor(name)?.shape;
            if shape.last() == Some(&7680) {
                return Ok(Flux2TransformerConfig::flux2_klein_4b());
            }
            if shape.last() == Some(&15360) {
                return Ok(Flux2TransformerConfig::flux2_dev());
            }
        }
        Err(DiffusionError::model(
            "flux2 transformer config could not be inferred from txt_in/context_embedder",
        ))
    }
}

/// The opened text-encoder bundle (headers of every shard), reusing the
/// generic sharded safetensors reader.
pub struct Flux2TextEncoderWeights {
    pub shards: H3ShardedWeights,
}

impl Flux2TextEncoderWeights {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            shards: H3ShardedWeights::load(dir)?,
        })
    }

    /// Cross-check opened shard headers against the pinned config shapes:
    /// embed [vocab, hidden], q [heads*hd, hidden], k/v [kv*hd, hidden].
    pub fn verify_shapes(&self, config: &Mistral3TextConfig) -> Result<()> {
        let checks: [(String, [u64; 2]); 5] = [
            (
                MISTRAL3_EMBED_TOKENS.to_owned(),
                [config.vocab_size as u64, config.hidden_size as u64],
            ),
            (
                format!("{}self_attn.q_proj.weight", mistral3_layer_prefix(0)),
                [
                    (config.num_attention_heads * config.head_dim) as u64,
                    config.hidden_size as u64,
                ],
            ),
            (
                format!("{}self_attn.k_proj.weight", mistral3_layer_prefix(0)),
                [
                    (config.num_key_value_heads * config.head_dim) as u64,
                    config.hidden_size as u64,
                ],
            ),
            (
                format!("{}mlp.gate_proj.weight", mistral3_layer_prefix(0)),
                [config.intermediate_size as u64, config.hidden_size as u64],
            ),
            (
                format!("{}mlp.down_proj.weight", mistral3_layer_prefix(0)),
                [config.hidden_size as u64, config.intermediate_size as u64],
            ),
        ];
        for (name, expected) in checks {
            let (_, shape) = self.shards.tensor_dtype_shape(&name)?;
            if shape != expected {
                return Err(DiffusionError::model(format!(
                    "{name} has shape {:?}, expected {:?}",
                    shape, expected
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_config_invariants() {
        let config = Flux2TransformerConfig::flux2_dev();
        assert_eq!(config.hidden_size, config.num_heads * config.head_dim);
        assert_eq!(config.mlp_hidden_size, config.hidden_size * 3);
        assert_eq!(
            config.axes_dim.iter().sum::<u32>(),
            config.head_dim,
            "rope axes must cover head_dim"
        );
        assert_eq!(config.latent_channels(), 32);
        assert_eq!(config.single_linear1_out(), 55296);
        assert_eq!(config.single_linear2_in(), 24576);
        // 1024px -> 64x64 tokens; joint sequence adds the 512-token text.
        assert_eq!(config.image_seq_len(1024, 1024), 4096);

        let te = Mistral3TextConfig::flux2_dev();
        assert_eq!(te.conditioning_dim(), config.context_in_dim);
        assert_eq!(te.layers_required_for_taps(), 30);

        let klein = Flux2TransformerConfig::flux2_klein_4b();
        assert_eq!(klein.hidden_size, klein.num_heads * klein.head_dim);
        assert_eq!(klein.mlp_hidden_size, klein.hidden_size * 3);
        assert_eq!(klein.axes_dim.iter().sum::<u32>(), klein.head_dim);
        assert_eq!(klein.context_in_dim, 3 * 2560, "3 stacked Qwen3-4B taps");
        assert!(!klein.guidance_embed);
        assert_eq!(klein.single_linear1_out(), 27648);
        assert_eq!(klein.single_linear2_in(), 12288);

        let qwen = Qwen3TextConfig::flux2_klein_4b();
        assert_eq!(qwen.conditioning_dim(), klein.context_in_dim);
        assert_eq!(qwen.layers_required_for_taps(), 27);

        let vae = Flux2VaeConfig::flux2_dev();
        assert_eq!(vae.packed_channels(), config.in_channels);
    }

    /// Pinned against the verbatim BFL formula evaluated in float64
    /// (python, 2026-08-13): the reference constants, not our re-derivation.
    #[test]
    fn empirical_mu_matches_pins() {
        let cases = [
            (4096, 50, 2.02335116),
            (4096, 28, 2.15144316),
            (1024, 4, 2.03068971),
            (1024, 28, 1.85917658),
            (4608, 50, 1.23666282),
            (9216, 50, 2.01665898),
        ];
        for (seq, steps, expected) in cases {
            let mu = flux2_compute_empirical_mu(seq, steps);
            assert!(
                (mu - expected).abs() < 1e-7,
                "mu({seq},{steps}) = {mu}, expected {expected}"
            );
        }
    }

    #[test]
    fn schedule_matches_pins() {
        let sigmas = flux2_schedule(50, 4096).expect("schedule");
        assert_eq!(sigmas.len(), 51);
        assert_eq!(sigmas[0], 1.0);
        assert_eq!(sigmas[50], 0.0);
        for (index, expected) in [(1, 0.99730906_f32), (25, 0.88322708), (49, 0.13371896)] {
            assert!(
                (sigmas[index] - expected).abs() < 2e-6,
                "sigma[{index}] = {}, expected {expected}",
                sigmas[index]
            );
        }

        let sigmas = flux2_schedule(4, 1024).expect("schedule");
        assert_eq!(sigmas.len(), 5);
        for (index, expected) in [(1, 0.95808537_f32), (2, 0.88398183), (3, 0.71749656)] {
            assert!(
                (sigmas[index] - expected).abs() < 2e-6,
                "sigma[{index}] = {}, expected {expected}",
                sigmas[index]
            );
        }

        // Monotone decreasing throughout.
        for window in sigmas.windows(2) {
            assert!(window[0] > window[1]);
        }
    }

    #[test]
    fn name_style_detection() {
        assert_eq!(
            detect_flux2_transformer_name_style(
                ["double_blocks.0.img_attn.qkv.weight", "img_in.weight"]
            ),
            Flux2TransformerNameStyle::Canonical
        );
        // stream_modulation module names occur in BOTH styles (pinned from
        // the canonical fp8mixed header) — they must not flip the verdict.
        assert_eq!(
            detect_flux2_transformer_name_style([
                "double_blocks.0.img_attn.qkv.weight",
                "double_stream_modulation_img.lin.weight",
            ]),
            Flux2TransformerNameStyle::Canonical
        );
        assert_eq!(
            detect_flux2_transformer_name_style([
                "transformer_blocks.0.attn.to_q.weight",
                "double_stream_modulation_img.linear.weight",
            ]),
            Flux2TransformerNameStyle::Diffusers
        );
        assert_eq!(
            detect_flux2_transformer_name_style(["encoder.down.0.block.0.conv1.weight"]),
            Flux2TransformerNameStyle::Unknown
        );
    }

    /// Layout verification against the real Comfy-Org fp8mixed DiT header
    /// (skips when the 35.5GB download is absent). Pins: canonical naming,
    /// the global-modulation tensor names/shapes, fp8 scope (mlp + single
    /// linears only) and every config-derived dimension.
    #[test]
    fn real_fp8_transformer_header_when_downloaded() {
        let path = match std::env::var("FLUX2_DIT_FILE") {
            Ok(path) => PathBuf::from(path),
            Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../local/models/flux2/flux2_dev_fp8mixed.safetensors"),
        };
        if !path.is_file() {
            eprintln!("flux2 dit header: fp8mixed file missing, skipping");
            return;
        }
        let header = Flux2SafetensorsHeader::load(&path).expect("load header");
        let names: Vec<&str> = header.tensors.keys().map(String::as_str).collect();
        assert_eq!(
            detect_flux2_transformer_name_style(names.iter().copied()),
            Flux2TransformerNameStyle::Canonical
        );
        assert_eq!(header.tensors.len(), 555);

        let config = Flux2TransformerConfig::flux2_dev();
        let hidden = config.hidden_size as u64;
        let shape_of = |name: &str| -> Vec<u64> {
            header
                .tensor(name)
                .unwrap_or_else(|| panic!("missing tensor {name}"))
                .shape
                .clone()
        };
        for name in FLUX2_CANONICAL_GLOBAL_TENSORS {
            assert!(header.tensor(name).is_some(), "missing global {name}");
        }
        assert_eq!(shape_of("img_in.weight"), [hidden, config.in_channels as u64]);
        assert_eq!(
            shape_of("txt_in.weight"),
            [hidden, config.context_in_dim as u64]
        );
        assert_eq!(
            shape_of("double_stream_modulation_img.lin.weight"),
            [6 * hidden, hidden]
        );
        assert_eq!(
            shape_of("single_stream_modulation.lin.weight"),
            [3 * hidden, hidden]
        );
        assert_eq!(
            shape_of("final_layer.linear.weight"),
            [config.in_channels as u64, hidden]
        );
        assert_eq!(
            shape_of("double_blocks.0.img_attn.qkv.weight"),
            [3 * hidden, hidden]
        );
        assert_eq!(
            shape_of("double_blocks.7.txt_mlp.0.weight"),
            [2 * config.mlp_hidden_size as u64, hidden]
        );
        assert_eq!(
            shape_of("single_blocks.47.linear1.weight"),
            [config.single_linear1_out() as u64, hidden]
        );
        assert_eq!(
            shape_of("single_blocks.47.linear2.weight"),
            [hidden, config.single_linear2_in() as u64]
        );
        // Block counts: no block at the configured depth, one right below.
        assert!(header.tensor("double_blocks.7.img_attn.qkv.weight").is_some());
        assert!(header.tensor("double_blocks.8.img_attn.qkv.weight").is_none());
        assert!(header.tensor("single_blocks.47.linear1.weight").is_some());
        assert!(header.tensor("single_blocks.48.linear1.weight").is_none());
        // No biases anywhere.
        assert!(!names.iter().any(|name| name.ends_with(".bias")));
    }

    fn local_text_encoder_dir() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var("FLUX2_TE_DIR") {
            return Some(PathBuf::from(dir));
        }
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/models/flux2/text_encoder");
        dir.join("model.safetensors.index.json")
            .is_file()
            .then_some(dir)
    }

    /// Index-level verification of the real downloaded bundle (skips when
    /// the download is absent so the suite stays green elsewhere).
    #[test]
    fn real_text_encoder_index_when_downloaded() {
        let Some(dir) = local_text_encoder_dir() else {
            eprintln!("flux2 te index: local text_encoder dir missing, skipping");
            return;
        };
        let index = Flux2TextEncoderIndex::load(&dir).expect("load index");
        let config = Mistral3TextConfig::flux2_dev();
        let report = index.verify_mistral3(&config).expect("verify mistral3");
        assert_eq!(report.tensor_count, 585);
        assert!(report.has_vision_tower, "Mistral3 VLM bundles Pixtral");
        assert!(report.has_lm_head);
        assert_eq!(report.all_shard_files.len(), 10);
        assert!(
            report.t2i_shard_files.len() < report.all_shard_files.len(),
            "t2i subset should drop at least the tail shards \
             (layers 30..40 + lm_head are dead weight)"
        );
    }

    /// Full shard-header verification — only meaningful once every shard file
    /// finished downloading; skips (never fails) on partial downloads because
    /// a truncated safetensors header cannot be told apart from a missing one
    /// without sizes in the index.
    #[test]
    fn real_text_encoder_shapes_when_fully_downloaded() {
        let Some(dir) = local_text_encoder_dir() else {
            eprintln!("flux2 te shapes: local text_encoder dir missing, skipping");
            return;
        };
        let index = Flux2TextEncoderIndex::load(&dir).expect("load index");
        if !index.shard_files_present() {
            eprintln!("flux2 te shapes: shards incomplete, skipping");
            return;
        }
        let weights = match Flux2TextEncoderWeights::load(&dir) {
            Ok(weights) => weights,
            Err(err) => {
                eprintln!("flux2 te shapes: shard headers unreadable ({err:?}), skipping");
                return;
            }
        };
        weights
            .verify_shapes(&Mistral3TextConfig::flux2_dev())
            .expect("shape check");
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let width = 4;
        let height = 4;
        let z = 2;
        let mut mean = Vec::new();
        for c in 0..z {
            for y in 0..height {
                for x in 0..width {
                    mean.push((c * 100 + y * 10 + x) as f32);
                }
            }
        }
        let packed = flux2_pack_latents(&mean, width, height, z).expect("pack");
        assert_eq!(packed.width, 2);
        assert_eq!(packed.height, 2);
        assert_eq!(packed.channels, 8);
        let restored = flux2_unpack_latents(&packed, z).expect("unpack");
        assert_eq!(restored, mean);
        let tokens = packed.to_tokens();
        let again = Flux2PackedLatents::from_tokens(&tokens, 2, 2, 8).expect("from tokens");
        assert_eq!(again.data, packed.data);
    }

    #[test]
    fn ref_time_offsets_match_bfl() {
        assert_eq!(flux2_ref_time_offset(0), 10);
        assert_eq!(flux2_ref_time_offset(1), 20);
        let ids = flux2_image_ids(2, 1, 10);
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0].t, 10.0);
        assert_eq!(ids[1].w, 1.0);
        let txt = flux2_text_ids(3);
        assert_eq!(txt[2].l, 2.0);
        assert_eq!(txt[2].t, 0.0);
    }
}
