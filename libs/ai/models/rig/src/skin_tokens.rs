//! Native SkinTokens / TokenRig contracts and checkpoint inventory.
//!
//! SkinTokens is not a text model even though its autoregressive core is a
//! Qwen3-0.6B decoder.  A point-cloud encoder and a conditional skin VAE
//! produce the continuous prefix, a rig-specific grammar constrains the
//! decoder, and the VAE turns four generated FSQ symbols per bone back into
//! dense skin weights.  Keeping that contract explicit prevents a tempting
//! but incorrect implementation which loads the checkpoint through the
//! ordinary chat-LLM path.
//!
//! The released training checkpoint is a Lightning pickle.  Production
//! inference consumes the equivalent flat safetensors conversion (same 672
//! named tensors); Python/Torch is only used by the parity oracle.

use crate::{DiffusionError, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::path::Path;

/// Declarative source artifact consumed by the shared ai-content lifecycle.
/// The runtime itself never downloads or guesses paths; the service maps this
/// requirement to its resumable/checksummed cache and invokes the native
/// converter when `converted_cache_as` is absent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkinTokensArtifactRequirement {
    pub repo: &'static str,
    pub revision: &'static str,
    pub path: &'static str,
    pub cache_as: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub converted_cache_as: &'static str,
    pub converted_size: u64,
    pub converted_sha256: &'static str,
}

pub const SKIN_TOKENS_SOURCE_REVISION: &str =
    "79736cad0fd84de384d5eede659b4ebd24effe33";
pub const SKIN_TOKENS_SOURCE_PATH: &str =
    "experiments/articulation_xl_quantization_256_token_4/grpo_1400.ckpt";

/// Complete model artifact inventory.  The embedded TokenRig checkpoint
/// already contains Michelangelo, Qwen and SkinVAE/FSQ weights, so no second
/// neural artifact is required.
pub const SKIN_TOKENS_ARTIFACTS: &[SkinTokensArtifactRequirement] =
    &[SkinTokensArtifactRequirement {
        repo: "VAST-AI/SkinTokens",
        revision: SKIN_TOKENS_SOURCE_REVISION,
        path: SKIN_TOKENS_SOURCE_PATH,
        cache_as: "rig/skintokens/upstream/grpo_1400.ckpt",
        size: 1_131_603_979,
        sha256: "f4e4706a11cfb520cdde65156a0358545e4fbf8f36237aca01ea5e79d5cb5692",
        converted_cache_as: "rig/skintokens/tokenrig.bf16.safetensors",
        // The deterministic native writer deliberately matches the standard
        // safetensors conversion published as an independent cross-check by
        // mlx-community/SkinTokens-bf16.
        converted_size: 1_190_606_876,
        converted_sha256:
            "d251c50d182c9ca17b88261cde1f88cdae4b844587389ddadd75756bbc8aa988",
    }];

pub const SKIN_TOKENS_SAMPLE_COUNT: usize = 54_000;
/// The released inference sampler is surface-only.  Although `SamplerMix`
/// stores `num_vertex_samples=16384`, upstream does not pass it to the primary
/// unrigged-mesh `sample_vertex_groups` call.  It is used only by the optional
/// dense skin/teacher-forcing branch.
pub const SKIN_TOKENS_INFERENCE_VERTEX_SAMPLE_COUNT: usize = 0;
pub const SKIN_TOKENS_TEACHER_VERTEX_SAMPLE_COUNT: usize = 16_384;

/// Effective continuous-prefix length.  The released simplified encoder has
/// `num_latents=256` in config but `no_query=True/query_method=False`; that
/// branch ignores `num_latents` and FPS-samples `token_num=512` queries.
pub const SKIN_TOKENS_MESH_LATENTS: usize = 512;
pub const SKIN_TOKENS_MESH_CONFIG_NUM_LATENTS: usize = 256;
pub const SKIN_TOKENS_MESH_WIDTH: usize = 512;
pub const SKIN_TOKENS_MESH_HEADS: usize = 8;
pub const SKIN_TOKENS_MESH_LAYERS: usize = 8;
pub const SKIN_TOKENS_MESH_FOURIER_FREQS: usize = 8;

pub const SKIN_TOKENS_QWEN_WIDTH: usize = 896;
pub const SKIN_TOKENS_QWEN_LAYERS: usize = 28;
// TokenRig overrides Qwen3-0.6B's hidden width but deliberately retains its
// 16/8 attention heads and 128-wide head geometry.  Consequently q_proj is
// 2048x896 and k/v are 1024x896; `width / heads` is NOT the head dimension.
pub const SKIN_TOKENS_QWEN_HEADS: usize = 16;
pub const SKIN_TOKENS_QWEN_KV_HEADS: usize = 8;
pub const SKIN_TOKENS_QWEN_HEAD_DIM: usize = 128;
pub const SKIN_TOKENS_QWEN_FFN: usize = 3_072;
pub const SKIN_TOKENS_VOCAB: usize = 33_036;
pub const SKIN_TOKENS_QWEN_CONTEXT: usize = 3_192;

pub const SKIN_TOKENS_VAE_COND_TOKENS: usize = 384;
pub const SKIN_TOKENS_VAE_SAMPLE_TOKENS: usize = 32;
pub const SKIN_TOKENS_VAE_LATENT: usize = 512;
pub const SKIN_TOKENS_VAE_WIDTH: usize = 768;
pub const SKIN_TOKENS_VAE_HEADS: usize = 12;
pub const SKIN_TOKENS_VAE_ENCODER_LAYERS: usize = 2;
pub const SKIN_TOKENS_VAE_DECODER_LAYERS: usize = 10;
pub const SKIN_TOKENS_FSQ_LEVELS: [usize; 5] = [8, 8, 8, 8, 8];
pub const SKIN_TOKENS_FSQ_VOCAB: usize = 32_768;
pub const SKIN_TOKENS_PER_BONE: usize = 4;

pub const SKIN_TOKENS_CHECKPOINT_TENSORS: usize = 672;
pub const SKIN_TOKENS_CHECKPOINT_PARAMS: u64 = 595_262_598;
pub const SKIN_TOKENS_QWEN_TENSORS: usize = 311;
pub const SKIN_TOKENS_QWEN_PARAMS: u64 = 444_610_432;
pub const SKIN_TOKENS_VAE_TENSORS: usize = 252;
pub const SKIN_TOKENS_VAE_PARAMS: u64 = 121_803_782;
pub const SKIN_TOKENS_MESH_TENSORS: usize = 106;
pub const SKIN_TOKENS_MESH_PARAMS: u64 = 28_387_840;
pub const SKIN_TOKENS_PROJECTION_TENSORS: usize = 3;
pub const SKIN_TOKENS_PROJECTION_PARAMS: u64 = 460_544;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SkinTokensComponentInventory {
    pub tensors: usize,
    pub parameters: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SkinTokensCheckpointInventory {
    pub all: SkinTokensComponentInventory,
    pub qwen: SkinTokensComponentInventory,
    pub vae: SkinTokensComponentInventory,
    pub mesh_encoder: SkinTokensComponentInventory,
    pub output_projection: SkinTokensComponentInventory,
    pub unknown: SkinTokensComponentInventory,
}

/// Header-only view of the converted TokenRig checkpoint.  Tensor bytes stay
/// on disk and individual stages upload them on demand, which is important on
/// the 24-GB reference box where TRELLIS may already be resident.
pub struct SkinTokensWeights {
    header: MlxSafetensorsHeader,
    inventory: SkinTokensCheckpointInventory,
}

impl SkinTokensWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let header = MlxSafetensorsHeader::load(path).map_err(DiffusionError::from)?;
        let inventory = checkpoint_inventory(&header);
        validate_checkpoint_inventory(path, inventory)?;
        validate_architecture_tensors(&header)?;
        Ok(Self { header, inventory })
    }

    pub fn header(&self) -> &MlxSafetensorsHeader {
        &self.header
    }

    pub fn inventory(&self) -> SkinTokensCheckpointInventory {
        self.inventory
    }

    pub fn tensor_dtype_shape(&self, name: &str) -> Result<(MlxDType, Vec<u64>)> {
        let tensor = self.header.tensor(name).ok_or_else(|| {
            DiffusionError::model(format!("SkinTokens tensor '{name}' is missing"))
        })?;
        Ok((tensor.dtype, tensor.shape.clone()))
    }

    pub fn tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        self.header
            .read_tensor_bytes(name)
            .map_err(DiffusionError::from)
    }

    /// Decode one row of a rank-2 tensor without reading the complete matrix.
    /// Autoregressive TokenRig generation uses this for one embedding row per
    /// step while the 33k x 896 table remains disk-backed.
    pub fn tensor_row_f32(&self, name: &str, row: u64) -> Result<Vec<f32>> {
        let tensor = self.header.tensor(name).ok_or_else(|| {
            DiffusionError::model(format!("SkinTokens tensor '{name}' is missing"))
        })?;
        let dtype = tensor.dtype;
        let bytes = self
            .header
            .read_rank2_row_bytes(name, row)
            .map_err(DiffusionError::from)?;
        match dtype {
            MlxDType::F32 => Ok(bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()),
            MlxDType::BF16 => Ok(bytes
                .chunks_exact(2)
                .map(|chunk| {
                    let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                    f32::from_bits((word as u32) << 16)
                })
                .collect()),
            other => Err(DiffusionError::model(format!(
                "SkinTokens tensor '{name}' has unsupported row dtype {other:?}",
            ))),
        }
    }

    /// Decode one checkpoint tensor to f32 while preserving its exact BF16
    /// values. Small biases and normalization vectors live on the host; large
    /// matrices stay as raw BF16 and stream through the CUDA weight cache.
    pub fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        let (dtype, _) = self.tensor_dtype_shape(name)?;
        let bytes = self.tensor_bytes(name)?;
        match dtype {
            MlxDType::F32 => Ok(bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()),
            MlxDType::BF16 => Ok(bytes
                .chunks_exact(2)
                .map(|chunk| {
                    let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                    f32::from_bits((word as u32) << 16)
                })
                .collect()),
            other => Err(DiffusionError::model(format!(
                "SkinTokens tensor '{name}' has unsupported runtime dtype {other:?}",
            ))),
        }
    }
}

fn add(entry: &mut SkinTokensComponentInventory, elements: u64, bytes: u64) {
    entry.tensors += 1;
    entry.parameters += elements;
    entry.bytes += bytes;
}

pub fn checkpoint_inventory(header: &MlxSafetensorsHeader) -> SkinTokensCheckpointInventory {
    let mut out = SkinTokensCheckpointInventory::default();
    for (name, tensor) in &header.tensors {
        let elements = tensor.element_count();
        let bytes = tensor.data_len_bytes();
        add(&mut out.all, elements, bytes);
        let bucket = if name.starts_with("transformer.") {
            &mut out.qwen
        } else if name.starts_with("vae.") {
            &mut out.vae
        } else if name.starts_with("mesh_encoder.") {
            &mut out.mesh_encoder
        } else if name.starts_with("output_proj.") {
            &mut out.output_projection
        } else {
            &mut out.unknown
        };
        add(bucket, elements, bytes);
    }
    out
}

fn validate_checkpoint_inventory(
    path: &Path,
    found: SkinTokensCheckpointInventory,
) -> Result<()> {
    let checks = [
        ("checkpoint", found.all, SKIN_TOKENS_CHECKPOINT_TENSORS, SKIN_TOKENS_CHECKPOINT_PARAMS),
        ("Qwen", found.qwen, SKIN_TOKENS_QWEN_TENSORS, SKIN_TOKENS_QWEN_PARAMS),
        ("skin VAE", found.vae, SKIN_TOKENS_VAE_TENSORS, SKIN_TOKENS_VAE_PARAMS),
        ("mesh encoder", found.mesh_encoder, SKIN_TOKENS_MESH_TENSORS, SKIN_TOKENS_MESH_PARAMS),
        (
            "output projection",
            found.output_projection,
            SKIN_TOKENS_PROJECTION_TENSORS,
            SKIN_TOKENS_PROJECTION_PARAMS,
        ),
    ];
    for (label, actual, tensors, parameters) in checks {
        if actual.tensors != tensors || actual.parameters != parameters {
            return Err(DiffusionError::model(format!(
                "{}: SkinTokens {label} inventory is {}/{} tensors/parameters, expected {tensors}/{parameters}",
                path.display(),
                actual.tensors,
                actual.parameters,
            )));
        }
    }
    if found.unknown.tensors != 0 {
        return Err(DiffusionError::model(format!(
            "{}: SkinTokens checkpoint has {} unclassified tensors",
            path.display(),
            found.unknown.tensors,
        )));
    }
    Ok(())
}

fn expect_shape(header: &MlxSafetensorsHeader, name: &str, shape: &[u64]) -> Result<()> {
    let found = header.tensor(name).ok_or_else(|| {
        DiffusionError::model(format!("SkinTokens checkpoint is missing '{name}'"))
    })?;
    if found.shape != shape {
        return Err(DiffusionError::model(format!(
            "SkinTokens tensor '{name}' has shape {:?}, expected {shape:?}",
            found.shape,
        )));
    }
    Ok(())
}

fn validate_architecture_tensors(header: &MlxSafetensorsHeader) -> Result<()> {
    // A compact set of sentinels catches wrong checkpoints and axis-swapped
    // conversions before a multi-gigabyte CUDA upload starts.
    expect_shape(
        header,
        "transformer.model.embed_tokens.weight",
        &[SKIN_TOKENS_VOCAB as u64, SKIN_TOKENS_QWEN_WIDTH as u64],
    )?;
    expect_shape(
        header,
        "transformer.model.layers.0.self_attn.q_proj.weight",
        &[
            (SKIN_TOKENS_QWEN_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM) as u64,
            SKIN_TOKENS_QWEN_WIDTH as u64,
        ],
    )?;
    expect_shape(
        header,
        "transformer.model.layers.27.mlp.down_proj.weight",
        &[SKIN_TOKENS_QWEN_WIDTH as u64, SKIN_TOKENS_QWEN_FFN as u64],
    )?;
    expect_shape(
        header,
        "mesh_encoder.encoder.cross_attn.attn.c_q.weight",
        &[SKIN_TOKENS_MESH_WIDTH as u64, SKIN_TOKENS_MESH_WIDTH as u64],
    )?;
    expect_shape(
        header,
        "output_proj.0.weight",
        &[SKIN_TOKENS_QWEN_WIDTH as u64, SKIN_TOKENS_MESH_WIDTH as u64],
    )?;
    expect_shape(
        header,
        "vae.model.encoder.learned_queries",
        &[SKIN_TOKENS_VAE_SAMPLE_TOKENS as u64, SKIN_TOKENS_VAE_WIDTH as u64],
    )?;
    Ok(())
}

/// Convert a five-dimensional FSQ digit vector to the vocabulary index used
/// by the official `[8,8,8,8,8]` quantizer (last dimension is most
/// significant).  This host helper is also used by the decoder parity tests.
pub fn fsq_digits_to_index(digits: [usize; 5]) -> Result<usize> {
    let mut index = 0usize;
    let mut basis = 1usize;
    for (digit, level) in digits.into_iter().zip(SKIN_TOKENS_FSQ_LEVELS) {
        if digit >= level {
            return Err(DiffusionError::workflow(format!(
                "FSQ digit {digit} is outside level {level}",
            )));
        }
        index += digit * basis;
        basis *= level;
    }
    Ok(index)
}

pub fn fsq_index_to_digits(mut index: usize) -> Result<[usize; 5]> {
    if index >= SKIN_TOKENS_FSQ_VOCAB {
        return Err(DiffusionError::workflow(format!(
            "FSQ index {index} is outside {SKIN_TOKENS_FSQ_VOCAB}",
        )));
    }
    let mut digits = [0usize; 5];
    for (digit, level) in digits.iter_mut().zip(SKIN_TOKENS_FSQ_LEVELS) {
        *digit = index % level;
        index /= level;
    }
    Ok(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsq_mixed_radix_roundtrip() {
        for index in 0..SKIN_TOKENS_FSQ_VOCAB {
            let digits = fsq_index_to_digits(index).unwrap();
            assert_eq!(fsq_digits_to_index(digits).unwrap(), index);
        }
        assert!(fsq_index_to_digits(SKIN_TOKENS_FSQ_VOCAB).is_err());
        assert!(fsq_digits_to_index([8, 0, 0, 0, 0]).is_err());
    }
}
