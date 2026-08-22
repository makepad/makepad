//! Sharded MiniMax-Music3 safetensors loader (headers + per-tensor reads).
//! Same streaming contract as H3: never hold a whole-model host copy. The
//! plain-safetensors path wraps `makepad_ai_common::sharded::ShardedSafetensors`
//! (lane T6a, /aiarch.md §1) — that reader was extracted verbatim from this
//! file, which was a near-duplicate of H3's own sharded reader.
//!
//! A shard set can also be backed by one official audio.cpp GGUF file
//! ([`Music3Shards::from_gguf`]): same tensor names (`audiocpp` packs are
//! `tensor_name_format = native`), same API, per-tensor ggml types so Q4_0
//! linears stay packed all the way into the device weight cache.

use crate::music3_quant::Music3GgufFile;
use crate::{DiffusionError, Result};
use makepad_ai_common::sharded::ShardedSafetensors;
use std::path::{Path, PathBuf};

pub const MUSIC3_LM_NAMESPACE: &str = "music3lm";
pub const MUSIC3_RVQ_NAMESPACE: &str = "music3rvq";
pub const MUSIC3_DIT_NAMESPACE: &str = "music3dit";
pub const MUSIC3_VAE_NAMESPACE: &str = "music3vae";

const LABEL: &str = "music3";

enum Music3WeightSource {
    Safetensors(ShardedSafetensors),
    Gguf(Music3GgufFile),
}

pub struct Music3Shards {
    pub dir: PathBuf,
    source: Music3WeightSource,
}

impl Music3Shards {
    /// Wrap one opened GGUF pack member. `dir` is the pack directory.
    pub fn from_gguf(file: Music3GgufFile) -> Self {
        let dir = file
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| file.path.clone());
        Self {
            dir,
            source: Music3WeightSource::Gguf(file),
        }
    }

    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let shards = ShardedSafetensors::load(dir, LABEL)?;
        Ok(Self {
            dir: shards.dir.clone(),
            source: Music3WeightSource::Safetensors(shards),
        })
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        match &self.source {
            Music3WeightSource::Safetensors(shards) => shards.has_tensor(name),
            Music3WeightSource::Gguf(file) => file.has_tensor(name),
        }
    }

    /// Raw tensor payload as stored: safetensors BF16/F32 stream, or the
    /// GGUF per-tensor payload (F32/BF16 stream or a packed quant block
    /// stream ready for `gpu_weight_cache_ensure_quant`).
    pub fn tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        match &self.source {
            Music3WeightSource::Gguf(file) => file.read_bytes_uncached(name),
            Music3WeightSource::Safetensors(shards) => shards.tensor_bytes(name, LABEL),
        }
    }

    /// Contiguous rank-2 rows as stored (BF16/F32 bytes, or whole packed
    /// Q4_0 rows — every ggml row is a self-contained block run). Used to
    /// cache a sliced `lm_head` covering only audio_end + the semantic
    /// codebook.
    pub fn tensor_row_range_bytes(&self, name: &str, row0: u64, nrows: u64) -> Result<Vec<u8>> {
        match &self.source {
            Music3WeightSource::Gguf(file) => {
                let row_bytes = file.row_bytes(name)?;
                let mut out = Vec::with_capacity((nrows as usize).saturating_mul(row_bytes));
                for row in row0..row0.saturating_add(nrows) {
                    out.extend_from_slice(&file.read_row_bytes(name, row)?);
                }
                Ok(out)
            }
            Music3WeightSource::Safetensors(shards) => {
                shards.tensor_row_range_bytes(name, row0, nrows, LABEL)
            }
        }
    }

    pub fn tensor_row_f32(&self, name: &str, row: u64) -> Result<Vec<f32>> {
        match &self.source {
            Music3WeightSource::Gguf(file) => {
                let row = u32::try_from(row).map_err(|_| {
                    DiffusionError::model(format!("music3 tensor '{name}' row {row} exceeds u32"))
                })?;
                file.gather_rows(name, &[row])
            }
            Music3WeightSource::Safetensors(shards) => shards.tensor_row_f32(name, row, LABEL),
        }
    }

    pub fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        match &self.source {
            Music3WeightSource::Gguf(file) => file.read_f32_any(name),
            Music3WeightSource::Safetensors(shards) => shards.tensor_f32(name, LABEL),
        }
    }

    /// Storage ggml type of one linear weight. Safetensors shards are BF16
    /// end to end; GGUF members answer per tensor (Q4_0 / BF16 / F32 / F16).
    pub fn linear_ggml_type(&self, name: &str) -> u32 {
        match &self.source {
            Music3WeightSource::Safetensors { .. } => makepad_ai_common::quant::GGML_TYPE_BF16,
            Music3WeightSource::Gguf(file) => file
                .file
                .get_tensor(name)
                .map(|info| info.tensor_type.ggml_type())
                .unwrap_or(makepad_ai_common::quant::GGML_TYPE_BF16),
        }
    }
}
