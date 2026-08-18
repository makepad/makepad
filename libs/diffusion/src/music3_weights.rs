//! Sharded MiniMax-Music3 safetensors loader (headers + per-tensor reads).
//! Same streaming contract as H3: never hold a whole-model host copy.
//!
//! A shard set can also be backed by one official audio.cpp GGUF file
//! ([`Music3Shards::from_gguf`]): same tensor names (`audiocpp` packs are
//! `tensor_name_format = native`), same API, per-tensor ggml types so Q4_0
//! linears stay packed all the way into the device weight cache.

use crate::music3_quant::Music3GgufFile;
use crate::{DiffusionError, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MUSIC3_LM_NAMESPACE: &str = "music3lm";
pub const MUSIC3_RVQ_NAMESPACE: &str = "music3rvq";
pub const MUSIC3_DIT_NAMESPACE: &str = "music3dit";
pub const MUSIC3_VAE_NAMESPACE: &str = "music3vae";

enum Music3WeightSource {
    Safetensors {
        shards: Vec<MlxSafetensorsHeader>,
        map: HashMap<String, (usize, String)>,
    },
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
        let dir = dir.as_ref().to_path_buf();
        let mut files: Vec<PathBuf> = if dir.is_file() {
            vec![dir.clone()]
        } else {
            std::fs::read_dir(&dir)
                .map_err(|err| {
                    DiffusionError::model(format!("music3 weights {}: {err}", dir.display()))
                })?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|path| {
                    path.extension()
                        .map(|ext| ext == "safetensors")
                        .unwrap_or(false)
                })
                .collect()
        };
        files.sort();
        if files.is_empty() {
            return Err(DiffusionError::model(format!(
                "music3 weights {} holds no safetensors",
                dir.display()
            )));
        }
        let mut shards = Vec::with_capacity(files.len());
        let mut map = HashMap::new();
        for (index, path) in files.iter().enumerate() {
            let header = MlxSafetensorsHeader::load(path)
                .map_err(|err| DiffusionError::model(format!("{}: {err}", path.display())))?;
            for name in header.tensors.keys() {
                map.insert(name.clone(), (index, name.clone()));
            }
            shards.push(header);
        }
        let dir = if dir.is_file() {
            dir.parent().map(|p| p.to_path_buf()).unwrap_or(dir)
        } else {
            dir
        };
        Ok(Self {
            dir,
            source: Music3WeightSource::Safetensors { shards, map },
        })
    }

    fn shard_for(&self, name: &str) -> Result<(&MlxSafetensorsHeader, &str)> {
        let Music3WeightSource::Safetensors { shards, map } = &self.source else {
            return Err(DiffusionError::model(format!(
                "music3 tensor '{name}': gguf source has no safetensors shard"
            )));
        };
        let (index, file_name) = map.get(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "music3 tensor '{name}' not found in {}",
                self.dir.display()
            ))
        })?;
        Ok((&shards[*index], file_name.as_str()))
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        match &self.source {
            Music3WeightSource::Safetensors { map, .. } => map.contains_key(name),
            Music3WeightSource::Gguf(file) => file.has_tensor(name),
        }
    }

    /// Raw tensor payload as stored: safetensors BF16/F32 stream, or the
    /// GGUF per-tensor payload (F32/BF16 stream or a packed quant block
    /// stream ready for `gpu_weight_cache_ensure_quant`).
    pub fn tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        if let Music3WeightSource::Gguf(file) = &self.source {
            return file.read_bytes_uncached(name);
        }
        let (shard, file_name) = self.shard_for(name)?;
        shard
            .read_tensor_bytes(file_name)
            .map_err(|err| DiffusionError::model(format!("music3 tensor '{name}': {err}")))
    }

    /// Contiguous rank-2 rows as stored (BF16/F32 bytes, or whole packed
    /// Q4_0 rows — every ggml row is a self-contained block run). Used to
    /// cache a sliced `lm_head` covering only audio_end + the semantic
    /// codebook.
    pub fn tensor_row_range_bytes(&self, name: &str, row0: u64, nrows: u64) -> Result<Vec<u8>> {
        if let Music3WeightSource::Gguf(file) = &self.source {
            let row_bytes = file.row_bytes(name)?;
            let mut out = Vec::with_capacity((nrows as usize).saturating_mul(row_bytes));
            for row in row0..row0.saturating_add(nrows) {
                out.extend_from_slice(&file.read_row_bytes(name, row)?);
            }
            return Ok(out);
        }
        let (shard, file_name) = self.shard_for(name)?;
        let mut out = Vec::new();
        for row in row0..row0.saturating_add(nrows) {
            let bytes = shard.read_rank2_row_bytes(file_name, row).map_err(|err| {
                DiffusionError::model(format!("music3 tensor '{name}' rows {row0}+{nrows}: {err}"))
            })?;
            if out.is_empty() {
                out.reserve((nrows as usize).saturating_mul(bytes.len()));
            }
            out.extend_from_slice(&bytes);
        }
        Ok(out)
    }

    pub fn tensor_row_f32(&self, name: &str, row: u64) -> Result<Vec<f32>> {
        if let Music3WeightSource::Gguf(file) = &self.source {
            let row = u32::try_from(row).map_err(|_| {
                DiffusionError::model(format!("music3 tensor '{name}' row {row} exceeds u32"))
            })?;
            return file.gather_rows(name, &[row]);
        }
        let (shard, file_name) = self.shard_for(name)?;
        let entry = shard.tensor(file_name).ok_or_else(|| {
            DiffusionError::model(format!("music3 tensor '{name}' missing entry"))
        })?;
        let bytes = shard.read_rank2_row_bytes(file_name, row).map_err(|err| {
            DiffusionError::model(format!("music3 tensor '{name}' row {row}: {err}"))
        })?;
        bytes_to_f32(&bytes, entry.dtype, name)
    }

    pub fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        if let Music3WeightSource::Gguf(file) = &self.source {
            return file.read_f32_any(name);
        }
        let (shard, file_name) = self.shard_for(name)?;
        let entry = shard.tensor(file_name).ok_or_else(|| {
            DiffusionError::model(format!("music3 tensor '{name}' missing entry"))
        })?;
        let bytes = self.tensor_bytes(name)?;
        bytes_to_f32(&bytes, entry.dtype, name)
    }

    /// Storage ggml type of one linear weight. Safetensors shards are BF16
    /// end to end; GGUF members answer per tensor (Q4_0 / BF16 / F32 / F16).
    pub fn linear_ggml_type(&self, name: &str) -> u32 {
        match &self.source {
            Music3WeightSource::Safetensors { .. } => makepad_ggml::quant::GGML_TYPE_BF16,
            Music3WeightSource::Gguf(file) => file
                .file
                .get_tensor(name)
                .map(|info| info.tensor_type.ggml_type())
                .unwrap_or(makepad_ggml::quant::GGML_TYPE_BF16),
        }
    }
}

fn bytes_to_f32(bytes: &[u8], dtype: MlxDType, name: &str) -> Result<Vec<f32>> {
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
            "music3 tensor '{name}': unsupported dtype {other:?}"
        ))),
    }
}
