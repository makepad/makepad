//! Plain sharded-safetensors-directory reader: every `*.safetensors` in a
//! dir (headers only, no index.json needed) or one single `.safetensors`
//! file, streamed per-tensor (never a whole-model host copy). Tensor names
//! are used as-is (canonical == file-local spelling) — family crates that
//! need name canonicalization (e.g. H3's video-VAE repack remap) or
//! quantized backends (GGUF/NVFP4) wrap this type as one variant of their
//! own weight-source enum instead of using it directly.
//!
//! This is the primary-split fallback for lane T6a (/aiarch.md §1): H3's
//! `H3ShardedWeights` (libs/diffusion/src/h3.rs) also does sharded
//! safetensors reading, but its struct is entangled with GGUF/NVFP4 variant
//! payloads that are H3-private, so it was left as-is rather than rebuilt
//! on top of this type. `Music3Shards` (libs/diffusion/src/music3_weights.rs)
//! only entangles a GGUF variant the same way, and its plain-safetensors
//! variant is a near-verbatim duplicate of this loader, so it was
//! re-pointed to wrap `ShardedSafetensors` instead of carrying its own copy.

use crate::error::{DiffusionError, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct ShardedSafetensors {
    pub dir: PathBuf,
    shards: Vec<MlxSafetensorsHeader>,
    map: HashMap<String, (usize, String)>,
}

impl ShardedSafetensors {
    /// Open a safetensors weight source: either every `*.safetensors` in a
    /// dir (headers only, no index.json needed) or one single `.safetensors`
    /// file. `label` is used only in error messages (e.g. "h3 weights",
    /// "music3 weights") so callers keep their existing diagnostics.
    pub fn load(dir: impl AsRef<Path>, label: &str) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let mut files: Vec<PathBuf> = if dir.is_file() {
            vec![dir.clone()]
        } else {
            std::fs::read_dir(&dir)
                .map_err(|err| {
                    DiffusionError::model(format!("{label} {}: {err}", dir.display()))
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
                "{label} {} holds no safetensors",
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

    fn shard_for(&self, name: &str, label: &str) -> Result<(&MlxSafetensorsHeader, &str)> {
        let (index, file_name) = self.map.get(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "{label} tensor '{name}' not found in {}",
                self.dir.display()
            ))
        })?;
        Ok((&self.shards[*index], file_name.as_str()))
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    /// Raw tensor payload as stored (safetensors BF16/F32 stream).
    pub fn tensor_bytes(&self, name: &str, label: &str) -> Result<Vec<u8>> {
        let (shard, file_name) = self.shard_for(name, label)?;
        shard
            .read_tensor_bytes(file_name)
            .map_err(|err| DiffusionError::model(format!("{label} tensor '{name}': {err}")))
    }

    /// Contiguous rank-2 rows as stored (BF16/F32 bytes).
    pub fn tensor_row_range_bytes(
        &self,
        name: &str,
        row0: u64,
        nrows: u64,
        label: &str,
    ) -> Result<Vec<u8>> {
        let (shard, file_name) = self.shard_for(name, label)?;
        let mut out = Vec::new();
        for row in row0..row0.saturating_add(nrows) {
            let bytes = shard.read_rank2_row_bytes(file_name, row).map_err(|err| {
                DiffusionError::model(format!(
                    "{label} tensor '{name}' rows {row0}+{nrows}: {err}"
                ))
            })?;
            if out.is_empty() {
                out.reserve((nrows as usize).saturating_mul(bytes.len()));
            }
            out.extend_from_slice(&bytes);
        }
        Ok(out)
    }

    pub fn tensor_row_f32(&self, name: &str, row: u64, label: &str) -> Result<Vec<f32>> {
        let (shard, file_name) = self.shard_for(name, label)?;
        let entry = shard
            .tensor(file_name)
            .ok_or_else(|| DiffusionError::model(format!("{label} tensor '{name}' missing entry")))?;
        let bytes = shard.read_rank2_row_bytes(file_name, row).map_err(|err| {
            DiffusionError::model(format!("{label} tensor '{name}' row {row}: {err}"))
        })?;
        bytes_to_f32(&bytes, entry.dtype, name, label)
    }

    pub fn tensor_f32(&self, name: &str, label: &str) -> Result<Vec<f32>> {
        let (shard, file_name) = self.shard_for(name, label)?;
        let entry = shard
            .tensor(file_name)
            .ok_or_else(|| DiffusionError::model(format!("{label} tensor '{name}' missing entry")))?;
        let bytes = self.tensor_bytes(name, label)?;
        bytes_to_f32(&bytes, entry.dtype, name, label)
    }
}

fn bytes_to_f32(bytes: &[u8], dtype: MlxDType, name: &str, label: &str) -> Result<Vec<f32>> {
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
            "{label} tensor '{name}': unsupported dtype {other:?}"
        ))),
    }
}
