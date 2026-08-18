//! Dtype-agnostic safetensors header (accepts F8_E4M3 as a raw string).
//! Shared by FLUX.2 and H3 NVFP4 so those crates do not cycle.

use crate::error::{DiffusionError, Result};
use crate::json::Json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// --- dtype-agnostic safetensors header reader -----------------------------------

/// One tensor entry from a raw safetensors header. `dtype` is the verbatim
/// header string ("BF16", "F32", "F8_E4M3", ...) — unlike the mlx reader
/// this makes no dtype support claim, so fp8-quantized files (which
/// `MlxSafetensorsHeader` rejects) can still be inspected and indexed for
/// the dequant-at-load path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flux2TensorInfo {
    pub dtype: String,
    pub shape: Vec<u64>,
    /// Byte span within the data section (start, end).
    pub data_offsets: (u64, u64),
}

/// Raw safetensors header of a (possibly fp8-quantized) single-file bundle.
pub struct Flux2SafetensorsHeader {
    pub path: PathBuf,
    pub header_len: u64,
    pub tensors: HashMap<String, Flux2TensorInfo>,
}

impl Flux2SafetensorsHeader {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        use std::io::Read;
        let path = path.as_ref().to_path_buf();
        let mut file = std::fs::File::open(&path)
            .map_err(|err| DiffusionError::io(&path, err.to_string()))?;
        let mut len_bytes = [0_u8; 8];
        file.read_exact(&mut len_bytes)
            .map_err(|err| DiffusionError::io(&path, err.to_string()))?;
        let header_len = u64::from_le_bytes(len_bytes);
        if header_len > 256 * 1024 * 1024 {
            return Err(DiffusionError::model(format!(
                "implausible safetensors header length {header_len} in {}",
                path.display()
            )));
        }
        let mut header_bytes = vec![0_u8; header_len as usize];
        file.read_exact(&mut header_bytes)
            .map_err(|err| DiffusionError::io(&path, err.to_string()))?;
        let text = std::str::from_utf8(&header_bytes)
            .map_err(|err| DiffusionError::model(format!("header not utf-8: {err}")))?;
        let root = Json::parse(text).map_err(|msg| DiffusionError::json(&path, msg))?;
        let entries = root
            .as_obj()
            .ok_or_else(|| DiffusionError::model("safetensors header is not an object"))?;

        let mut tensors = HashMap::with_capacity(entries.len());
        for (name, value) in entries {
            if name == "__metadata__" {
                continue;
            }
            let dtype = value
                .get("dtype")
                .and_then(Json::as_str)
                .ok_or_else(|| DiffusionError::model(format!("{name} has no dtype")))?
                .to_owned();
            let shape = value
                .get("shape")
                .and_then(Json::as_arr)
                .ok_or_else(|| DiffusionError::model(format!("{name} has no shape")))?
                .iter()
                .map(|dim| {
                    dim.as_f64()
                        .filter(|n| *n >= 0.0 && n.fract() == 0.0)
                        .map(|n| n as u64)
                        .ok_or_else(|| {
                            DiffusionError::model(format!("{name} has a non-integer dim"))
                        })
                })
                .collect::<Result<Vec<u64>>>()?;
            let offsets = value
                .get("data_offsets")
                .and_then(Json::as_arr)
                .filter(|arr| arr.len() == 2)
                .ok_or_else(|| DiffusionError::model(format!("{name} has no data_offsets")))?;
            let start = offsets[0].as_f64().unwrap_or(-1.0);
            let end = offsets[1].as_f64().unwrap_or(-1.0);
            if start < 0.0 || end < start {
                return Err(DiffusionError::model(format!(
                    "{name} has malformed data_offsets"
                )));
            }
            tensors.insert(
                name.clone(),
                Flux2TensorInfo {
                    dtype,
                    shape,
                    data_offsets: (start as u64, end as u64),
                },
            );
        }
        Ok(Self {
            path,
            header_len,
            tensors,
        })
    }

    pub fn tensor(&self, name: &str) -> Option<&Flux2TensorInfo> {
        self.tensors.get(name)
    }

    pub fn has_tensor(&self, name: &str) -> bool {
        self.tensors.contains_key(name)
    }

    /// Byte offset of the tensor payload from the start of the file.
    pub fn file_offset(&self, info: &Flux2TensorInfo) -> u64 {
        8 + self.header_len + info.data_offsets.0
    }

    pub fn read_bytes(&self, name: &str) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let info = self.tensor(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "flux2 tensor '{name}' missing in {}",
                self.path.display()
            ))
        })?;
        let len = (info.data_offsets.1 - info.data_offsets.0) as usize;
        let mut file = std::fs::File::open(&self.path)
            .map_err(|err| DiffusionError::io(&self.path, err.to_string()))?;
        file.seek(SeekFrom::Start(self.file_offset(info)))
            .map_err(|err| DiffusionError::io(&self.path, err.to_string()))?;
        let mut bytes = vec![0_u8; len];
        file.read_exact(&mut bytes)
            .map_err(|err| DiffusionError::io(&self.path, err.to_string()))?;
        Ok(bytes)
    }

    /// Decode a tensor to f32. FP8-E4M3 weights are dequantized with an
    /// optional sibling `.weight_scale` (Comfy static-act-quant); without a
    /// scale the raw e4m3 mapping is used. Documented: Klein-4B official
    /// transformer is BF16; this path exists for the fp8mixed / Qwen3-FP8
    /// artifacts. Dequant-at-load is host-side and caches f16/f32, not an
    /// fp8 GEMM.
    pub fn read_f32(&self, name: &str) -> Result<Vec<f32>> {
        let info = self.tensor(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "flux2 tensor '{name}' missing in {}",
                self.path.display()
            ))
        })?;
        let bytes = self.read_bytes(name)?;
        match info.dtype.as_str() {
            "F32" => Ok(bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()),
            "BF16" => Ok(bytes
                .chunks_exact(2)
                .map(|chunk| {
                    let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                    f32::from_bits((word as u32) << 16)
                })
                .collect()),
            "F16" => Ok(bytes
                .chunks_exact(2)
                .map(|chunk| {
                    let word = u16::from_le_bytes([chunk[0], chunk[1]]);
                    makepad_ggml::f16_to_f32(word)
                })
                .collect()),
            "F8_E4M3" => {
                let scale = self
                    .tensor(&format!("{name}_scale"))
                    .or_else(|| self.tensor(&format!("{name}.weight_scale")))
                    .map(|_| self.read_f32(&format!("{name}_scale")).ok())
                    .flatten()
                    .or_else(|| self.read_f32(&format!("{name}.weight_scale")).ok())
                    .unwrap_or_else(|| vec![1.0]);
                let scale = scale.first().copied().unwrap_or(1.0);
                Ok(bytes
                    .iter()
                    .map(|&byte| makepad_ggml::f8_e4m3_to_f32(byte) * scale)
                    .collect())
            }
            other => Err(DiffusionError::model(format!(
                "flux2 tensor '{name}' has unsupported dtype {other}"
            ))),
        }
    }

    /// Decode a rank-2 weight to IEEE f16 bytes (row-major). FP8 is
    /// dequantized through f32 then rounded to f16.
    pub fn read_f16_bytes(&self, name: &str) -> Result<Vec<u8>> {
        let values = self.read_f32(name)?;
        let mut out = Vec::with_capacity(values.len() * 2);
        for value in values {
            out.extend_from_slice(&makepad_ggml::f32_to_f16(value).to_le_bytes());
        }
        Ok(out)
    }

    /// Decode a rank-2 weight to BF16 bytes (round-trip via f32).
    pub fn read_bf16_bytes(&self, name: &str) -> Result<Vec<u8>> {
        let info = self.tensor(name).ok_or_else(|| {
            DiffusionError::model(format!(
                "flux2 tensor '{name}' missing in {}",
                self.path.display()
            ))
        })?;
        if info.dtype == "BF16" {
            return self.read_bytes(name);
        }
        let values = self.read_f32(name)?;
        let mut out = Vec::with_capacity(values.len() * 2);
        for value in values {
            let bits = value.to_bits();
            let rounding = (bits >> 16) & 1;
            let word = ((bits + 0x7fff + rounding) >> 16) as u16;
            out.extend_from_slice(&word.to_le_bytes());
        }
        Ok(out)
    }
}

