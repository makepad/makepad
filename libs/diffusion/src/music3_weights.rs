//! Sharded MiniMax-Music3 safetensors loader (headers + per-tensor reads).
//! Same streaming contract as H3: never hold a whole-model host copy.

use crate::{DiffusionError, Result};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MUSIC3_LM_NAMESPACE: &str = "music3lm";
pub const MUSIC3_RVQ_NAMESPACE: &str = "music3rvq";
pub const MUSIC3_DIT_NAMESPACE: &str = "music3dit";
pub const MUSIC3_VAE_NAMESPACE: &str = "music3vae";

pub struct Music3Shards {
    pub dir: PathBuf,
    shards: Vec<MlxSafetensorsHeader>,
    map: HashMap<String, (usize, String)>,
}

impl Music3Shards {
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
        Ok(Self { dir, shards, map })
    }

    fn shard_for(&self, name: &str) -> Result<(&MlxSafetensorsHeader, &str)> {
        let (index, file_name) = self.map.get(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "music3 tensor '{name}' not found in {}",
                self.dir.display()
            ))
        })?;
        Ok((&self.shards[*index], file_name.as_str()))
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    pub fn tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        let (shard, file_name) = self.shard_for(name)?;
        shard
            .read_tensor_bytes(file_name)
            .map_err(|err| DiffusionError::model(format!("music3 tensor '{name}': {err}")))
    }

    /// Contiguous rank-2 rows as stored (BF16/F32 bytes). Used to cache a
    /// sliced `lm_head` covering only audio_end + the semantic codebook.
    pub fn tensor_row_range_bytes(&self, name: &str, row0: u64, nrows: u64) -> Result<Vec<u8>> {
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
        let (shard, file_name) = self.shard_for(name)?;
        let entry = shard.tensor(file_name).ok_or_else(|| {
            DiffusionError::model(format!("music3 tensor '{name}' missing entry"))
        })?;
        let bytes = self.tensor_bytes(name)?;
        bytes_to_f32(&bytes, entry.dtype, name)
    }

    pub fn linear_ggml_type(&self) -> u32 {
        makepad_ggml::quant::GGML_TYPE_BF16
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
