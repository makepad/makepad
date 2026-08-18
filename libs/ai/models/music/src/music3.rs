//! MiniMax-Music3 native port foundation.
//!
//! Official graph (diffusers `MiniMaxMusic3ModularPipeline`, weights revision
//! `bd348f9c`, ModularPipeline commit `dafe3733`):
//!
//! 1. **Text** — assemble caption+lyrics with checkpoint special tokens
//!    (`<|im_start|><|caption_start|>…<|caption_end|><|lyrics_start|>…`),
//!    tokenize with Qwen2 BPE (`tokenizer/`), then build the CFG pair by
//!    replacing tokens `[1:-2]` with `_AUDIO_CFG_TOKEN_ID` (151654).
//! 2. **Global LM** — Qwen3ForCausalLM 8B fine-tune (`language_model/`):
//!    hidden 4096, 36 layers, 32 q / 8 kv heads, head_dim 128, SiLU MLP
//!    12288, vocab 200000, max_pos 10240, rope_theta 1e6, rms_eps 1e-6,
//!    q/k RMSNorm. The official semantic block calls the *inner*
//!    `language_model.model(...)`, not the outer causal-LM wrapper.
//!    25 frames/s; one semantic codebook token per frame (16384-entry
//!    codebook at offset 151675). AR CFG scale 1.5, top-k 50.
//! 3. **RVQ depth decoder** — 0.6B local LM (`rvq_depth_decoder/`): 4
//!    layers, hidden 4096, 16 heads, FF 6144, 7 residual codebooks × 1024
//!    entries. Hidden states of (global last + 7 residual steps) fuse to
//!    8×4096 per frame.
//! 4. **Condition encoder** — softmax-mix the 8 layer hiddens, Conv1d
//!    4096→2048 k=3, nearest-resample to Flow-VAE time
//!    (`frames * 44100/24000 * 960/512` ≈ 3.445 latents/frame).
//! 5. **Flow DiT** — 2.4B `MiniMaxMusic3Transformer1DModel`: 36 layers,
//!    32 heads × 64, dim 2048, FF 8192, rotary_dim 32, Fourier 256,
//!    in_channels 128. Concat `[latent, zeros, cond]` → residual 1×1 conv
//!    → Linear → prepend time token → blocks → proj_out. 30 Euler steps,
//!    CFG 1.7, uncond cond is zeros. Chunks: 200 frames / 100-frame hop,
//!    overlap 172 latents.
//! 6. **Vocoder** — DAC-style Flow-VAE decoder, latent 128 (two folded
//!    64-ch streams), upsample 8×8×4×2 = 512 samples/latent, 44.1 kHz
//!    stereo tanh.
//!
//! Token / schedule constants are the checkpoint contract (whitespace in
//! the assembled prompt changes the song). Oracle dumps live on 169 at
//! `C:\ai\music3_oracle\<fixture>\` and are compared by `music3-validate`.

use crate::backend::{
    gpu_add, gpu_conv2d_planar_cached, gpu_device_available, gpu_download, gpu_upload,
};
use makepad_ai_h3::h3_tokenizer::H3Tokenizer;
use crate::{DiffusionError, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::path::{Path, PathBuf};

pub const MUSIC3_SAMPLE_RATE: usize = 44_100;
pub const MUSIC3_AUDIO_CHANNELS: usize = 2;
pub const MUSIC3_FRAME_RATE: f64 = 25.0;
pub const MUSIC3_LATENT_HOP: usize = 512;
pub const MUSIC3_MIN_SECONDS: f64 = 5.0;
pub const MUSIC3_MAX_SECONDS: f64 = 300.0;
pub const MUSIC3_MAX_AUDIO_FRAMES: usize = 9_000;
pub const MUSIC3_MAX_PROMPT_TOKENS: usize = 5_000;

pub const MUSIC3_FLOW_STEPS: usize = 30;
pub const MUSIC3_FLOW_CFG: f32 = 1.7;
pub const MUSIC3_AR_CFG: f32 = 1.5;
pub const MUSIC3_AR_TOP_K: usize = 50;
pub const MUSIC3_CHUNK_FRAMES: usize = 200;
pub const MUSIC3_CHUNK_HOP: usize = 100;
pub const MUSIC3_OVERLAP_LATENT: usize = 172;
/// Official vocoder stitch: drop 86 leading latents after the first window
/// and 258 trailing latents before the last (`344 - 86`).
pub const MUSIC3_CROP_LEFT_LATENT: usize = 86;
pub const MUSIC3_CROP_RIGHT_LATENT: usize = 344 - 86;

pub const MUSIC3_LM_HIDDEN: usize = 4096;
pub const MUSIC3_LM_LAYERS: usize = 36;
pub const MUSIC3_LM_HEADS: usize = 32;
pub const MUSIC3_LM_KV_HEADS: usize = 8;
pub const MUSIC3_LM_HEAD_DIM: usize = 128;
pub const MUSIC3_LM_FF: usize = 12_288;
pub const MUSIC3_LM_VOCAB: usize = 200_000;
pub const MUSIC3_LM_MAX_POS: usize = 10_240;
pub const MUSIC3_LM_ROPE_THETA: f32 = 1_000_000.0;
pub const MUSIC3_LM_RMS_EPS: f32 = 1e-6;

pub const MUSIC3_SEMANTIC_VOCAB: usize = 16_384;
pub const MUSIC3_AUDIO_VOCAB: usize = 1024;
pub const MUSIC3_NUM_CODEBOOKS: usize = 8;
pub const MUSIC3_AUDIO_CODE_OFFSET: u32 = 151_675;
pub const MUSIC3_AUDIO_END_TOKEN_ID: u32 = 151_670;
pub const MUSIC3_AUDIO_CFG_TOKEN_ID: u32 = 151_654;
pub const MUSIC3_IM_START_ID: u32 = 151_644;
pub const MUSIC3_IM_END_ID: u32 = 151_645;
pub const MUSIC3_AUDIO_START_ID: u32 = 151_669;
pub const MUSIC3_CAPTION_START_ID: u32 = 151_671;
pub const MUSIC3_CAPTION_END_ID: u32 = 151_672;
pub const MUSIC3_LYRICS_START_ID: u32 = 151_673;
pub const MUSIC3_LYRICS_END_ID: u32 = 151_674;

pub const MUSIC3_RVQ_HIDDEN: usize = 4096;
pub const MUSIC3_RVQ_FF: usize = 6144;
pub const MUSIC3_RVQ_HEADS: usize = 16;
pub const MUSIC3_RVQ_LAYERS: usize = 4;
pub const MUSIC3_RVQ_MAX_POS: usize = 16;

pub const MUSIC3_COND_HIDDEN: usize = 4096;
pub const MUSIC3_COND_OUT: usize = 2048;
pub const MUSIC3_COND_LAYERS: usize = 8;
pub const MUSIC3_COND_IN_SR: usize = 24_000;
pub const MUSIC3_COND_IN_HOP: usize = 960;
pub const MUSIC3_COND_OUT_SR: usize = 44_100;
pub const MUSIC3_COND_OUT_HOP: usize = 512;

pub const MUSIC3_DIT_HEADS: usize = 32;
pub const MUSIC3_DIT_HEAD_DIM: usize = 64;
pub const MUSIC3_DIT_DIM: usize = MUSIC3_DIT_HEADS * MUSIC3_DIT_HEAD_DIM;
pub const MUSIC3_DIT_FF: usize = 8192;
pub const MUSIC3_DIT_LAYERS: usize = 36;
pub const MUSIC3_DIT_FOURIER: usize = 256;
pub const MUSIC3_DIT_IN_CHANNELS: usize = 128;
pub const MUSIC3_DIT_ROPE: usize = 32;
pub const MUSIC3_DIT_COND: usize = 2048;
pub const MUSIC3_DIT_CONCAT: usize = 2 * MUSIC3_DIT_IN_CHANNELS + MUSIC3_DIT_COND;

pub const MUSIC3_VAE_LATENT: usize = 128;
pub const MUSIC3_VAE_DEC_IN: usize = 1024;
pub const MUSIC3_VAE_DEC_HIDDEN: usize = 1536;
pub const MUSIC3_VAE_UPSAMPLE: [usize; 4] = [8, 8, 4, 2];

const IM_START: &str = "<|im_start|>";
const IM_END: &str = "<|im_end|>";
const CAPTION_START: &str = "<|caption_start|>";
const CAPTION_END: &str = "<|caption_end|>";
const LYRICS_START: &str = "<|lyrics_start|>";
const LYRICS_END: &str = "<|lyrics_end|>";
const AUDIO_START: &str = "<|audio_start|>";

/// Official oracle-dump fixture (`music3_oracle_dump.py` DEFAULT_*).
pub const MUSIC3_PINE_PROMPT: &str = "Genre: acoustic pop. BPM: 96. Key: C major. Warm and intimate, \
building gently into the chorus. Vocals: soft female lead, close and \
breathy, light stacked harmonies in the chorus. Arrangement: \
fingerpicked guitar and soft piano; brushed drums and upright bass \
enter in the chorus.";
pub const MUSIC3_PINE_LYRICS: &str = "[verse]\nMorning light filtering through the pine\nEvery quiet street is yours and mine\n[chorus]\nSoftly the world begins to breathe";

/// Cache-relative model root used by the registry (`minimax-music3`).
pub fn music3_cache_subdir() -> PathBuf {
    PathBuf::from("music").join("MiniMax-Music3")
}

pub fn music3_max_frames(seconds: f64) -> usize {
    let frames = (seconds * MUSIC3_FRAME_RATE) as usize;
    frames.min(MUSIC3_MAX_AUDIO_FRAMES).max(1)
}

/// Latent frames for `num_semantic_frames` after nearest resample.
/// `frames * 44100/24000 * 960/512` = `frames * 3.4453125`.
pub fn music3_latent_len(num_frames: usize) -> usize {
    let numer = num_frames
        * MUSIC3_COND_OUT_SR
        * MUSIC3_COND_IN_HOP;
    let denom = MUSIC3_COND_IN_SR * MUSIC3_COND_OUT_HOP;
    (numer / denom).max(1)
}

/// Official PrepareChunks starts: `[0]` if `frames <= 200`, else
/// `range(0, frames-100, 100)`.
pub fn music3_chunk_starts(frames: usize) -> Vec<usize> {
    if frames <= MUSIC3_CHUNK_FRAMES {
        vec![0]
    } else {
        (0..frames.saturating_sub(MUSIC3_CHUNK_HOP))
            .step_by(MUSIC3_CHUNK_HOP)
            .collect()
    }
}

/// Official Music3 scheduler (`scheduler_config.json`):
/// `set_timesteps(sigmas=linspace(1, 1/steps, steps))` with
/// `invert_sigmas=true`, `shift=1`, `num_train_timesteps=1`.
/// Yields transformer time 0 → 1−1/steps and a terminal sigma of 1.
pub fn music3_flow_sigmas(steps: usize) -> Vec<f32> {
    let n = steps.max(1);
    let start = 1.0f32;
    let end = 1.0 / n as f32;
    let mut sigmas = Vec::with_capacity(n + 1);
    for i in 0..n {
        let u = if n == 1 {
            0.0
        } else {
            i as f32 / (n - 1) as f32
        };
        let raw = start + (end - start) * u;
        sigmas.push(1.0 - raw);
    }
    sigmas.push(1.0);
    sigmas
}

/// Rewrite `<|tag rest|>` → `tag is rest`, strip markdown, collapse blanks.
/// Mirrors `MiniMaxMusic3TextEncoderStep._clean_caption`.
pub fn clean_caption(caption: &str) -> String {
    let mut rewritten = String::with_capacity(caption.len());
    let mut i = 0;
    while i < caption.len() {
        if caption[i..].starts_with("<|") {
            if let Some(end) = caption[i + 2..].find("|>") {
                let inner = caption[i + 2..i + 2 + end].trim();
                let mut parts = inner.splitn(2, char::is_whitespace);
                let tag = parts.next().unwrap_or("");
                if let Some(rest) = parts.next().map(str::trim).filter(|s| !s.is_empty()) {
                    rewritten.push_str(tag);
                    rewritten.push_str(" is ");
                    rewritten.push_str(rest);
                } else {
                    rewritten.push_str(inner);
                }
                i += 2 + end + 2;
                continue;
            }
        }
        let ch = caption[i..].chars().next().unwrap();
        rewritten.push(ch);
        i += ch.len_utf8();
    }

    let mut lines_out = Vec::new();
    for line in rewritten.lines() {
        let mut line = line.to_string();
        line = strip_leading_heading(&line);
        line = strip_leading_bullet(&line);
        loop {
            let updated = strip_bold(&line);
            if updated == line {
                break;
            }
            line = updated;
        }
        line = strip_italics(&line);
        lines_out.push(line.trim_end().to_string());
    }
    let mut text = lines_out.join("\n");
    let mut cleaned = String::new();
    for line in text.lines() {
        if is_hr(line) {
            continue;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }
    if cleaned.ends_with('\n') {
        cleaned.pop();
    }
    text = cleaned.replace("• ", "").replace("    ", "");
    collapse_blank_lines(&text)
}

/// Keep only leading `[tags]` on a tag line; lowercase tag names; prefix `[start]`.
/// Mirrors `MiniMaxMusic3TextEncoderStep._normalize_lyrics`.
pub fn normalize_lyrics(lyrics: &str) -> String {
    let mut output = Vec::new();
    for line in lyrics.split('\n') {
        if let Some(tags) = leading_structure_tags(line) {
            output.push(tags);
        } else {
            output.push(line.to_string());
        }
    }
    let mut text = output.join("\n");
    text = text.replace("] ", "]\n");
    text = text.replace(" [", "\n[");
    text = text.replace(" ^ ", "\n");
    text = lowercase_bracket_tags(&text);
    format!("[start]\n{text}")
}

/// Checkpoint-contract prompt string (any whitespace change changes the song).
pub fn assemble_prompt(caption: &str, lyrics: &str) -> String {
    format!(
        "{IM_START}{CAPTION_START}{}{CAPTION_END}{LYRICS_START}{}{LYRICS_END}{IM_END}{AUDIO_START}",
        clean_caption(caption),
        normalize_lyrics(lyrics)
    )
}

/// Conditional token ids plus the official CFG counterpart.
pub fn tokenize_cfg_pair(tokenizer: &H3Tokenizer, caption: &str, lyrics: &str) -> Result<Vec<[u32; 2]>> {
    let text = assemble_prompt(caption, lyrics);
    let cond = tokenizer.encode(&text);
    if cond.len() > MUSIC3_MAX_PROMPT_TOKENS {
        return Err(DiffusionError::model(format!(
            "music3 prompt has {} tokens; max is {}",
            cond.len(),
            MUSIC3_MAX_PROMPT_TOKENS
        )));
    }
    if cond.len() < 3 {
        return Err(DiffusionError::model("music3 assembled prompt too short"));
    }
    let mut pairs = Vec::with_capacity(cond.len());
    let last = cond.len() - 1;
    // Fail closed: if specials BPE-split, CFG masks the wrong slots and the
    // model free-runs into music-like noise that is not a song.
    if cond[0] != MUSIC3_IM_START_ID
        || cond[last - 1] != MUSIC3_IM_END_ID
        || cond[last] != MUSIC3_AUDIO_START_ID
    {
        return Err(DiffusionError::model(format!(
            "music3 specials mismatch: first={} last2=[{}, {}] (want im_start={} im_end={} audio_start={}; T={})",
            cond[0],
            cond[last - 1],
            cond[last],
            MUSIC3_IM_START_ID,
            MUSIC3_IM_END_ID,
            MUSIC3_AUDIO_START_ID,
            cond.len()
        )));
    }
    for (i, &id) in cond.iter().enumerate() {
        let uncond = if i == 0 || i == last || i == last - 1 {
            id
        } else {
            MUSIC3_AUDIO_CFG_TOKEN_ID
        };
        pairs.push([id, uncond]);
    }
    Ok(pairs)
}

pub fn load_tokenizer(tokenizer_dir: &Path) -> Result<H3Tokenizer> {
    H3Tokenizer::load(tokenizer_dir)
}

// ---------------------------------------------------------------------------
// Weight header inventory.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Music3TensorInfo {
    pub name: String,
    pub dtype: String,
    pub shape: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Music3WeightInventory {
    pub files: Vec<PathBuf>,
    pub tensors: Vec<Music3TensorInfo>,
}

impl Music3WeightInventory {
    pub fn get(&self, name: &str) -> Option<&Music3TensorInfo> {
        self.tensors.iter().find(|t| t.name == name)
    }
}

/// Parse safetensors headers for every component under a MiniMax-Music3 dir.
pub fn inventory_weights(model_dir: &Path) -> Result<Music3WeightInventory> {
    let mut files = Vec::new();
    let mut tensors = Vec::new();
    let shards = [
        "condition_encoder/diffusion_pytorch_model.safetensors",
        "language_model/model-00001-of-00004.safetensors",
        "language_model/model-00002-of-00004.safetensors",
        "language_model/model-00003-of-00004.safetensors",
        "language_model/model-00004-of-00004.safetensors",
        "rvq_depth_decoder/diffusion_pytorch_model.safetensors",
        "transformer/diffusion_pytorch_model-00001-of-00002.safetensors",
        "transformer/diffusion_pytorch_model-00002-of-00002.safetensors",
        "vocoder/diffusion_pytorch_model.safetensors",
    ];
    for rel in shards {
        let path = model_dir.join(rel);
        if !path.is_file() {
            return Err(DiffusionError::model(format!(
                "music3 missing shard {}",
                path.display()
            )));
        }
        let header = MlxSafetensorsHeader::load(&path).map_err(|err| {
            DiffusionError::model(format!("music3 header {}: {err:?}", path.display()))
        })?;
        files.push(path);
        for (name, entry) in &header.tensors {
            tensors.push(Music3TensorInfo {
                name: name.clone(),
                dtype: format!("{:?}", entry.dtype),
                shape: entry.shape.iter().map(|&v| v as usize).collect(),
            });
        }
    }
    tensors.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Music3WeightInventory { files, tensors })
}

/// Hard shape canaries from the official configs (revision bd348f9c).
pub fn expected_shape_canaries() -> &'static [(&'static str, &'static [usize])] {
    &[
        ("model.embed_tokens.weight", &[MUSIC3_LM_VOCAB, MUSIC3_LM_HIDDEN]),
        ("lm_head.weight", &[MUSIC3_LM_VOCAB, MUSIC3_LM_HIDDEN]),
        ("model.norm.weight", &[MUSIC3_LM_HIDDEN]),
        (
            "model.layers.0.self_attn.q_proj.weight",
            &[MUSIC3_LM_HEADS * MUSIC3_LM_HEAD_DIM, MUSIC3_LM_HIDDEN],
        ),
        (
            "model.layers.0.self_attn.k_proj.weight",
            &[MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM, MUSIC3_LM_HIDDEN],
        ),
        (
            "model.layers.0.self_attn.v_proj.weight",
            &[MUSIC3_LM_KV_HEADS * MUSIC3_LM_HEAD_DIM, MUSIC3_LM_HIDDEN],
        ),
        (
            "model.layers.0.mlp.gate_proj.weight",
            &[MUSIC3_LM_FF, MUSIC3_LM_HIDDEN],
        ),
        (
            "audio_embeddings.weight",
            &[MUSIC3_AUDIO_VOCAB * (MUSIC3_NUM_CODEBOOKS - 1), MUSIC3_RVQ_HIDDEN],
        ),
        ("projection.weight", &[MUSIC3_RVQ_HIDDEN, MUSIC3_RVQ_HIDDEN]),
        ("pos_embedding.weight", &[MUSIC3_RVQ_MAX_POS, MUSIC3_RVQ_HIDDEN]),
        (
            "audio_heads.0.weight",
            &[MUSIC3_AUDIO_VOCAB, MUSIC3_RVQ_HIDDEN],
        ),
        ("layer_weight_logits", &[MUSIC3_COND_LAYERS]),
        ("proj.weight", &[MUSIC3_COND_OUT, MUSIC3_COND_HIDDEN, 3]),
        (
            "proj_in.weight",
            &[MUSIC3_DIT_DIM, MUSIC3_DIT_CONCAT],
        ),
        (
            "proj_out.weight",
            &[MUSIC3_DIT_IN_CHANNELS, MUSIC3_DIT_DIM],
        ),
        ("time_proj.weight", &[MUSIC3_DIT_FOURIER / 2, 1]),
        (
            "transformer_blocks.0.attn.to_q.weight",
            &[MUSIC3_DIT_DIM, MUSIC3_DIT_DIM],
        ),
        (
            "transformer_blocks.0.ff_in.weight",
            &[MUSIC3_DIT_FF * 2, MUSIC3_DIT_DIM],
        ),
    ]
}

/// Official condition encoder: mix 8 per-frame hiddens, Conv1d 4096→2048, nearest
/// resample onto the Flow-VAE timeline. CPU f32; dump compare allows bf16 noise.
pub struct Music3ConditionEncoder {
    pub layer_weight_logits: Vec<f32>,
    pub layer_scale: f32,
    pub proj_weight: Vec<f32>, // [2048, 4096, 3]
    pub proj_bias: Vec<f32>,   // [2048]
}

impl Music3ConditionEncoder {
    /// Build from already-decoded tensors (safetensors or GGUF).
    pub fn from_parts(
        layer_weight_logits: Vec<f32>,
        layer_scale: f32,
        proj_weight: Vec<f32>,
        proj_bias: Vec<f32>,
    ) -> Result<Self> {
        if layer_weight_logits.len() != MUSIC3_COND_LAYERS {
            return Err(DiffusionError::model(format!(
                "layer_weight_logits len {}",
                layer_weight_logits.len()
            )));
        }
        if proj_weight.len() != MUSIC3_COND_OUT * MUSIC3_COND_HIDDEN * 3 {
            return Err(DiffusionError::model(format!(
                "proj.weight len {}, expected {}",
                proj_weight.len(),
                MUSIC3_COND_OUT * MUSIC3_COND_HIDDEN * 3
            )));
        }
        if proj_bias.len() != MUSIC3_COND_OUT {
            return Err(DiffusionError::model(format!(
                "proj.bias len {}, expected {MUSIC3_COND_OUT}",
                proj_bias.len()
            )));
        }
        Ok(Self {
            layer_weight_logits,
            layer_scale,
            proj_weight,
            proj_bias,
        })
    }

    pub fn load(model_dir: &Path) -> Result<Self> {
        let path = model_dir.join("condition_encoder/diffusion_pytorch_model.safetensors");
        let header = MlxSafetensorsHeader::load(&path).map_err(|err| {
            DiffusionError::model(format!("music3 cond encoder {}: {err:?}", path.display()))
        })?;
        let logits = read_f32(&header, "layer_weight_logits")?;
        let scale = read_f32(&header, "layer_scale")?;
        let proj_weight =
            read_f32_shaped(&header, "proj.weight", &[MUSIC3_COND_OUT, MUSIC3_COND_HIDDEN, 3])?;
        let proj_bias = read_f32_shaped(&header, "proj.bias", &[MUSIC3_COND_OUT])?;
        Self::from_parts(
            logits,
            scale.first().copied().unwrap_or(1.0),
            proj_weight,
            proj_bias,
        )
    }

    /// `hidden` is `[frames, layers * hidden]` (batch squeezed). Returns `[latents, out_dim]`.
    pub fn forward(&self, hidden: &[f32], frames: usize) -> Result<Vec<f32>> {
        self.forward_with_progress(hidden, frames, &mut |_, _, _| {})
    }

    pub fn forward_with_progress(
        &self,
        hidden: &[f32],
        frames: usize,
        progress: &mut dyn FnMut(&str, usize, usize),
    ) -> Result<Vec<f32>> {
        let expected = frames * MUSIC3_COND_LAYERS * MUSIC3_COND_HIDDEN;
        if hidden.len() != expected {
            return Err(DiffusionError::model(format!(
                "cond encoder input {} values, expected {expected} for {frames} frames",
                hidden.len()
            )));
        }
        let mut weights = [0f32; MUSIC3_COND_LAYERS];
        let mut max = self.layer_weight_logits[0];
        for &v in &self.layer_weight_logits {
            if v > max {
                max = v;
            }
        }
        let mut sum = 0f32;
        for (i, &v) in self.layer_weight_logits.iter().enumerate() {
            weights[i] = (v - max).exp();
            sum += weights[i];
        }
        for w in &mut weights {
            *w /= sum;
        }

        // Mix layers: dump/official layout after reshape is (layers, hidden, frames)
        // from transpose(1,2) of (B, F, L*H) then view (B, L, H, F).
        let mut mixed = vec![0f32; MUSIC3_COND_HIDDEN * frames];
        let mix_tick = (frames / 8).max(1);
        for frame in 0..frames {
            for layer in 0..MUSIC3_COND_LAYERS {
                let w = weights[layer];
                let src = frame * (MUSIC3_COND_LAYERS * MUSIC3_COND_HIDDEN)
                    + layer * MUSIC3_COND_HIDDEN;
                for h in 0..MUSIC3_COND_HIDDEN {
                    mixed[h * frames + frame] += hidden[src + h] * w;
                }
            }
            if frame == 0 || (frame + 1) % mix_tick == 0 || frame + 1 == frames {
                progress("mix", frame + 1, frames);
            }
        }
        for v in &mut mixed {
            *v *= self.layer_scale;
        }

        // Official condition_encoder is Conv1d 4096→2048 k=3 pad=1.
        // The fast-wrong snapshot used a 3x1 planar Conv2d on this 1d
        // weight. Do the same math as the CPU loop: three 1x1 GEMMs on
        // time-shifted copies (tap k at t+k-1).
        progress("conv", 0, 1);
        let use_gpu = gpu_device_available()
            && std::env::var("MAKEPAD_MUSIC3_CPU_COND").ok().as_deref() != Some("1");
        let conv = if use_gpu {
            cond_conv1d_gpu_taps(&mixed, frames, &self.proj_weight, &self.proj_bias)?
        } else {
            let mut conv = vec![0f32; MUSIC3_COND_OUT * frames];
            let conv_tick = (MUSIC3_COND_OUT / 16).max(1);
            for o in 0..MUSIC3_COND_OUT {
                let w_o = &self.proj_weight
                    [o * MUSIC3_COND_HIDDEN * 3..(o + 1) * MUSIC3_COND_HIDDEN * 3];
                for t in 0..frames {
                    let mut acc = self.proj_bias[o];
                    for k in 0..3 {
                        let src_t = t as isize + k as isize - 1;
                        if src_t < 0 || src_t >= frames as isize {
                            continue;
                        }
                        let st = src_t as usize;
                        for i in 0..MUSIC3_COND_HIDDEN {
                            acc += w_o[i * 3 + k] * mixed[i * frames + st];
                        }
                    }
                    conv[o * frames + t] = acc;
                }
                if o == 0 || (o + 1) % conv_tick == 0 || o + 1 == MUSIC3_COND_OUT {
                    progress("conv", o + 1, MUSIC3_COND_OUT);
                }
            }
            conv
        };
        progress("conv", 1, 1);

        let latents = music3_latent_len(frames);
        let mut out = vec![0f32; latents * MUSIC3_COND_OUT];
        for t in 0..latents {
            let src = ((t as u64 * frames as u64) / latents as u64) as usize;
            let src = src.min(frames - 1);
            for o in 0..MUSIC3_COND_OUT {
                out[t * MUSIC3_COND_OUT + o] = conv[o * frames + src];
            }
        }
        Ok(out)
    }
}

/// Conv1d k=3 pad=1 as three 1x1 GEMMs on time-shifted copies.
/// Weight is official `[out, in, 3]`. Same arithmetic as the CPU loop.
fn cond_conv1d_gpu_taps(
    mixed: &[f32],
    frames: usize,
    weight: &[f32],
    bias: &[f32],
) -> Result<Vec<f32>> {
    let cin = MUSIC3_COND_HIDDEN;
    let cout = MUSIC3_COND_OUT;
    if mixed.len() != cin * frames
        || weight.len() != cout * cin * 3
        || bias.len() != cout
        || frames == 0
    {
        return Err(DiffusionError::model("music3 cond conv1d tap shapes"));
    }
    let zeros = vec![0f32; cout];
    let mut acc: Option<crate::backend::GpuTensor> = None;
    for k in 0..3 {
        let mut shifted = vec![0f32; cin * frames];
        for t in 0..frames {
            let src = t as isize + k as isize - 1;
            if src >= 0 && src < frames as isize {
                let st = src as usize;
                for i in 0..cin {
                    shifted[i * frames + t] = mixed[i * frames + st];
                }
            }
        }
        let mut tap = vec![0f32; cout * cin];
        for o in 0..cout {
            let w_o = &weight[o * cin * 3..(o + 1) * cin * 3];
            for i in 0..cin {
                tap[o * cin + i] = w_o[i * 3 + k];
            }
        }
        let x = gpu_upload(&shifted, cin, frames).map_err(DiffusionError::model)?;
        let y = gpu_conv2d_planar_cached(
            &x,
            frames,
            1,
            "music3-cond",
            &format!("proj_tap{k}"),
            &tap,
            if k == 0 { bias } else { &zeros },
            cout,
            1,
            1,
            0,
            0,
        )
        .map_err(DiffusionError::model)?;
        acc = Some(match acc.take() {
            None => y,
            Some(prev) => gpu_add(&prev, &y).map_err(DiffusionError::model)?,
        });
    }
    let y = acc.ok_or_else(|| DiffusionError::model("music3 cond conv1d empty"))?;
    gpu_download(&y).map_err(DiffusionError::model)
}

fn read_f32(header: &MlxSafetensorsHeader, name: &str) -> Result<Vec<f32>> {
    let entry = header
        .tensors
        .get(name)
        .ok_or_else(|| DiffusionError::model(format!("music3 tensor missing: {name}")))?;
    let bytes = header
        .read_tensor_bytes(name)
        .map_err(|err| DiffusionError::model(format!("music3 read {name}: {err:?}")))?;
    match entry.dtype {
        MlxDType::F32 => Ok(bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()),
        MlxDType::BF16 => Ok(bytes
            .chunks_exact(2)
            .map(|c| f32::from_bits(u32::from(u16::from_le_bytes([c[0], c[1]])) << 16))
            .collect()),
        other => Err(DiffusionError::model(format!(
            "music3 tensor {name}: unsupported dtype {other:?}"
        ))),
    }
}

fn read_f32_shaped(header: &MlxSafetensorsHeader, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
    let entry = header
        .tensors
        .get(name)
        .ok_or_else(|| DiffusionError::model(format!("music3 tensor missing: {name}")))?;
    let actual: Vec<usize> = entry.shape.iter().map(|&v| v as usize).collect();
    if actual != shape {
        return Err(DiffusionError::model(format!(
            "music3 tensor {name}: shape {actual:?}, expected {shape:?}"
        )));
    }
    read_f32(header, name)
}

pub fn validate_shape_canaries(inv: &Music3WeightInventory) -> Result<Vec<String>> {
    let mut ok = Vec::new();
    for &(name, shape) in expected_shape_canaries() {
        let Some(info) = inv.get(name) else {
            return Err(DiffusionError::model(format!(
                "music3 tensor missing: {name}"
            )));
        };
        if info.shape != shape {
            return Err(DiffusionError::model(format!(
                "music3 tensor {name}: shape {:?}, expected {shape:?}",
                info.shape
            )));
        }
        ok.push(format!("{name} {:?} {}", info.shape, info.dtype));
    }
    let lm_layers = inv
        .tensors
        .iter()
        .filter(|t| t.name.starts_with("model.layers.") && t.name.ends_with(".input_layernorm.weight"))
        .count();
    if lm_layers != MUSIC3_LM_LAYERS {
        return Err(DiffusionError::model(format!(
            "music3 LM layers {lm_layers}, expected {MUSIC3_LM_LAYERS}"
        )));
    }
    let dit_blocks = inv
        .tensors
        .iter()
        .filter(|t| {
            t.name.starts_with("transformer_blocks.") && t.name.ends_with(".attn.to_q.weight")
        })
        .count();
    if dit_blocks != MUSIC3_DIT_LAYERS {
        return Err(DiffusionError::model(format!(
            "music3 DiT blocks {dit_blocks}, expected {MUSIC3_DIT_LAYERS}"
        )));
    }
    Ok(ok)
}

// ---------------------------------------------------------------------------
// Caption / lyrics helpers (string-level, no regex crate).
// ---------------------------------------------------------------------------

fn strip_leading_heading(line: &str) -> String {
    let trimmed = line.trim_start_matches(|c: char| c == ' ' || c == '\t');
    let spaces = line.len() - trimmed.len();
    if spaces > 3 {
        return line.to_string();
    }
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &trimmed[hashes..];
        if rest.starts_with(|c: char| c.is_whitespace()) {
            return rest.trim_start().to_string();
        }
    }
    line.to_string()
}

fn strip_leading_bullet(line: &str) -> String {
    let trimmed = line.trim_start();
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(c) if c == '*' || c == '+' || c == '-' => {
            if matches!(chars.next(), Some(w) if w.is_whitespace()) {
                return trimmed[c.len_utf8()..].trim_start().to_string();
            }
        }
        _ => {}
    }
    line.to_string()
}

fn strip_bold(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    let mut changed = false;
    while i < line.len() {
        if line[i..].starts_with("**") {
            if let Some(rel) = line[i + 2..].find("**") {
                let inner = &line[i + 2..i + 2 + rel];
                if !inner.is_empty() && !inner.contains('*') {
                    out.push_str(inner);
                    i += 2 + rel + 2;
                    changed = true;
                    continue;
                }
            }
        }
        let ch = line[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    if changed {
        out
    } else {
        line.to_string()
    }
}

fn strip_italics(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        let prev_star = i > 0 && line[..i].ends_with('*');
        if line[i..].starts_with('*') && !prev_star {
            if let Some(rel) = line[i + 1..].find('*') {
                let inner = &line[i + 1..i + 1 + rel];
                let after = i + 1 + rel + 1;
                let next_star = line[after..].starts_with('*');
                if !inner.is_empty() && !inner.contains('*') && !inner.contains('\n') && !next_star
                {
                    out.push_str(inner);
                    i = after;
                    continue;
                }
            }
        }
        let ch = line[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_hr(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| c == '-' || c == '*' || c == '_')
}

fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.split('\n') {
        if line.is_empty() {
            if !blank {
                if !out.is_empty() {
                    out.push('\n');
                }
            }
            blank = true;
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
            blank = false;
        }
    }
    out
}

fn leading_structure_tags(line: &str) -> Option<String> {
    let rest = line.trim_start_matches(|c: char| c == ' ' || c == '\t');
    if !rest.starts_with('[') {
        return None;
    }
    let mut i = 0;
    let mut last = 0;
    while rest[i..].starts_with('[') {
        if let Some(end) = rest[i + 1..].find(']') {
            last = i + 1 + end + 1;
            i = last;
            while rest[i..].starts_with(' ') || rest[i..].starts_with('\t') {
                i += 1;
            }
        } else {
            return None;
        }
    }
    if last == 0 {
        return None;
    }
    Some(rest[..last].trim().to_string())
}

fn lowercase_bracket_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with('[') {
            if let Some(end) = text[i + 1..].find(']') {
                out.push('[');
                out.push_str(&text[i + 1..i + 1 + end].to_ascii_lowercase());
                out.push(']');
                i += 1 + end + 1;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyrics_normalize_drops_inline_tag_text_and_prefixes_start() {
        let out = normalize_lyrics("[Verse]\nMorning light\n[Chorus] dropped\nSoftly");
        assert_eq!(out, "[start]\n[verse]\nMorning light\n[chorus]\nSoftly");
    }

    #[test]
    fn caption_strips_markdown_and_special_tags() {
        let out = clean_caption("## Genre: pop\n<|mood warm|>\n**bold** and *soft*");
        assert!(out.contains("Genre: pop"));
        assert!(out.contains("mood is warm"));
        assert!(out.contains("bold and soft"));
        assert!(!out.contains("**"));
    }

    #[test]
    fn assemble_prompt_wraps_official_specials() {
        let text = assemble_prompt("warm acoustic pop", "[verse]\nhello");
        assert!(text.starts_with("<|im_start|><|caption_start|>"));
        assert!(text.contains("<|lyrics_start|>[start]\n[verse]\nhello<|lyrics_end|>"));
        assert!(text.ends_with("<|im_end|><|audio_start|>"));
    }

    #[test]
    fn pine_fixture_matches_oracle_dump_python() {
        // Same DEFAULT_PROMPT / DEFAULT_LYRICS as music3_oracle_dump.py.
        let text = assemble_prompt(MUSIC3_PINE_PROMPT, MUSIC3_PINE_LYRICS);
        assert_eq!(
            text,
            format!(
                "<|im_start|><|caption_start|>{}<|caption_end|><|lyrics_start|>{}<|lyrics_end|><|im_end|><|audio_start|>",
                clean_caption(MUSIC3_PINE_PROMPT),
                normalize_lyrics(MUSIC3_PINE_LYRICS)
            )
        );
        assert!(normalize_lyrics(MUSIC3_PINE_LYRICS).starts_with("[start]\n[verse]\n"));
        assert_eq!(music3_max_frames(5.0), 125);
    }

    #[test]
    fn latent_len_matches_official_resample() {
        // 125 frames * 44100/24000 * 960/512 = 430.664… → 430
        assert_eq!(music3_latent_len(125), 430);
        assert_eq!(music3_max_frames(5.0), 125);
    }

    #[test]
    fn chunk_starts_and_flow_sigmas_match_python() {
        assert_eq!(music3_chunk_starts(125), vec![0]);
        assert_eq!(music3_chunk_starts(200), vec![0]);
        assert_eq!(music3_chunk_starts(201), vec![0, 100]);
        let s = music3_flow_sigmas(4);
        assert_eq!(s.len(), 5);
        assert!((s[0] - 0.0).abs() < 1e-6);
        assert!((s[1] - 0.25).abs() < 1e-6);
        assert!((s[4] - 1.0).abs() < 1e-6);
        assert!((s[1] - s[0] - 0.25).abs() < 1e-6);
    }
}
