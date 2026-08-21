//! Safetensors header/reader. Moved verbatim from libs/mlx/src/core/{model,tensors,util}.rs
//! (the mlx crate's only weight-I/O slice that has real consumers — see /aiarch.md §1).
//! Original type names are kept as the canonical exports so consumer edits are pure
//! import-path swaps (`use makepad_mlx::X` -> `use makepad_ai_loader::X`).
//! The mlx copy stays in place until lane T3 deletes libs/mlx.

use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type Result<T> = std::result::Result<T, MlxRtError>;

#[derive(Debug)]
pub enum MlxRtError {
    Io { path: PathBuf, message: String },
    Json { path: PathBuf, message: String },
    MissingFile { path: PathBuf },
    InvalidModelDir { path: PathBuf, message: String },
    InvalidSafetensors { path: PathBuf, message: String },
}

impl std::fmt::Display for MlxRtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(f, "I/O error at {}: {}", path.display(), message)
            }
            Self::Json { path, message } => {
                write!(f, "JSON decode error at {}: {}", path.display(), message)
            }
            Self::MissingFile { path } => write!(f, "missing required file {}", path.display()),
            Self::InvalidModelDir { path, message } => {
                write!(f, "invalid model dir {}: {}", path.display(), message)
            }
            Self::InvalidSafetensors { path, message } => {
                write!(
                    f,
                    "invalid safetensors file {}: {}",
                    path.display(),
                    message
                )
            }
        }
    }
}

impl std::error::Error for MlxRtError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MlxDType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F16,
    BF16,
    F32,
    F64,
    /// Signed FP8 E4M3FN (safetensors "F8_E4M3"): 1 sign + 4 exponent (bias 7)
    /// + 3 mantissa bits, no infinities, 0x7f/0xff are NaN. Raw storage dtype
    /// only — consumers must opt in explicitly (no implicit f32 widening).
    F8E4M3,
}

impl MlxDType {
    pub fn from_safetensors_str(value: &str) -> Result<Self> {
        match value {
            "BOOL" => Ok(Self::Bool),
            "U8" => Ok(Self::U8),
            "U16" => Ok(Self::U16),
            "U32" => Ok(Self::U32),
            "U64" => Ok(Self::U64),
            "I8" => Ok(Self::I8),
            "I16" => Ok(Self::I16),
            "I32" => Ok(Self::I32),
            "I64" => Ok(Self::I64),
            "F16" => Ok(Self::F16),
            "BF16" => Ok(Self::BF16),
            "F32" => Ok(Self::F32),
            "F64" => Ok(Self::F64),
            "F8_E4M3" => Ok(Self::F8E4M3),
            other => Err(MlxRtError::InvalidSafetensors {
                path: PathBuf::new(),
                message: format!("unsupported dtype {}", other),
            }),
        }
    }

    pub fn byte_width(self) -> u64 {
        match self {
            Self::Bool | Self::U8 | Self::I8 | Self::F8E4M3 => 1,
            Self::U16 | Self::I16 | Self::F16 | Self::BF16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::U64 | Self::I64 | Self::F64 => 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MlxTensorEntry {
    pub dtype: MlxDType,
    pub shape: Vec<u64>,
    pub data_offsets: [u64; 2],
}

impl MlxTensorEntry {
    pub fn element_count(&self) -> u64 {
        self.shape.iter().copied().product::<u64>()
    }

    pub fn data_len_bytes(&self) -> u64 {
        self.data_offsets[1] - self.data_offsets[0]
    }

    pub fn expected_len_bytes(&self) -> u64 {
        self.element_count() * self.dtype.byte_width()
    }

    pub fn file_offsets(&self, payload_base_offset: u64) -> [u64; 2] {
        [
            payload_base_offset + self.data_offsets[0],
            payload_base_offset + self.data_offsets[1],
        ]
    }
}

#[derive(Clone, Debug)]
pub struct MlxSafetensorsHeader {
    pub path: PathBuf,
    pub file_len: u64,
    pub header_len: u64,
    pub metadata: HashMap<String, String>,
    pub tensors: HashMap<String, MlxTensorEntry>,
    file: Arc<Mutex<fs::File>>,
}

impl MlxSafetensorsHeader {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = fs::File::open(&path).map_err(|err| MlxRtError::Io {
            path: path.clone(),
            message: err.to_string(),
        })?;
        let file_len = file
            .metadata()
            .map_err(|err| MlxRtError::Io {
                path: path.clone(),
                message: err.to_string(),
            })?
            .len();

        let mut header_len_bytes = [0u8; 8];
        file.read_exact(&mut header_len_bytes)
            .map_err(|err| MlxRtError::Io {
                path: path.clone(),
                message: err.to_string(),
            })?;
        let header_len = u64::from_le_bytes(header_len_bytes);
        let payload_base_offset =
            8u64.checked_add(header_len)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: path.clone(),
                    message: "header length overflow".to_string(),
                })?;
        if payload_base_offset > file_len {
            return Err(MlxRtError::InvalidSafetensors {
                path: path.clone(),
                message: format!(
                    "header extends past EOF: payload base {} > file len {}",
                    payload_base_offset, file_len
                ),
            });
        }

        let mut header_bytes = vec![0u8; header_len as usize];
        file.read_exact(&mut header_bytes)
            .map_err(|err| MlxRtError::Io {
                path: path.clone(),
                message: err.to_string(),
            })?;
        let header_text =
            String::from_utf8(header_bytes).map_err(|err| MlxRtError::InvalidSafetensors {
                path: path.clone(),
                message: err.to_string(),
            })?;
        let header_map =
            HashMap::<String, JsonValue>::deserialize_json(&header_text).map_err(|err| {
                MlxRtError::Json {
                    path: path.clone(),
                    message: format!("{:?}", err),
                }
            })?;

        let mut metadata = HashMap::new();
        let mut tensors = HashMap::new();

        for (name, value) in header_map {
            if name == "__metadata__" {
                metadata = json_string_map(&path, "__metadata__", &value)?;
                continue;
            }
            let object = json_object(&path, &name, &value)?;
            let dtype = json_dtype(&path, &name, object.get("dtype"))?;
            let shape = json_u64_array(&path, &name, object.get("shape"))?;
            let data_offsets = json_two_u64s(&path, &name, object.get("data_offsets"))?;
            let entry = MlxTensorEntry {
                dtype,
                shape,
                data_offsets,
            };
            let file_offsets = entry.file_offsets(payload_base_offset);
            if file_offsets[1] > file_len {
                return Err(MlxRtError::InvalidSafetensors {
                    path: path.clone(),
                    message: format!(
                        "tensor {} ends past EOF: {} > {}",
                        name, file_offsets[1], file_len
                    ),
                });
            }
            if entry.data_len_bytes() != entry.expected_len_bytes() {
                return Err(MlxRtError::InvalidSafetensors {
                    path: path.clone(),
                    message: format!(
                        "tensor {} length mismatch: stored {} expected {}",
                        name,
                        entry.data_len_bytes(),
                        entry.expected_len_bytes()
                    ),
                });
            }
            tensors.insert(name, entry);
        }

        Ok(Self {
            path,
            file_len,
            header_len,
            metadata,
            tensors,
            file: Arc::new(Mutex::new(file)),
        })
    }

    pub fn payload_base_offset(&self) -> u64 {
        8 + self.header_len
    }

    pub fn tensor(&self, name: &str) -> Option<&MlxTensorEntry> {
        self.tensors.get(name)
    }

    fn read_file_range(&self, start: u64, len: usize) -> Result<Vec<u8>> {
        let mut file = self.file.lock().map_err(|_| MlxRtError::Io {
            path: self.path.clone(),
            message: "safetensors file mutex poisoned".to_string(),
        })?;
        file.seek(SeekFrom::Start(start))
            .map_err(|err| MlxRtError::Io {
                path: self.path.clone(),
                message: err.to_string(),
            })?;
        let mut bytes = vec![0u8; len];
        file.read_exact(&mut bytes).map_err(|err| MlxRtError::Io {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        Ok(bytes)
    }

    pub fn read_tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        let file_offsets = entry.file_offsets(self.payload_base_offset());
        self.read_file_range(file_offsets[0], entry.data_len_bytes() as usize)
    }

    /// Component view of a combined checkpoint: keeps only the tensors whose
    /// name starts with `prefix`, with the prefix stripped, sharing this
    /// header's file handle and payload offsets. Loaders written against a
    /// separate component file (e.g. a standalone t5xxl/clip/vae safetensors)
    /// work unchanged on the view, and per-tensor range reads mean only the
    /// component's byte ranges are ever read. Errors when nothing matches or
    /// when stripping collides with an existing name (malformed header).
    pub fn scoped_to_prefix(&self, prefix: &str) -> Result<Self> {
        let mut tensors = HashMap::new();
        for (name, entry) in &self.tensors {
            let Some(stripped) = name.strip_prefix(prefix) else {
                continue;
            };
            if stripped.is_empty() {
                continue;
            }
            if tensors.insert(stripped.to_string(), entry.clone()).is_some() {
                return Err(MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!(
                        "prefix scope {:?} produces duplicate tensor name {:?}",
                        prefix, stripped
                    ),
                });
            }
        }
        if tensors.is_empty() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("no tensors under prefix {:?}", prefix),
            });
        }
        Ok(Self {
            path: self.path.clone(),
            file_len: self.file_len,
            header_len: self.header_len,
            metadata: self.metadata.clone(),
            tensors,
            file: Arc::clone(&self.file),
        })
    }

    /// Bytes of a contiguous RUN of rows along a tensor's leading axis.
    ///
    /// For loaders that must carve several canonical tensors out of one
    /// FUSED checkpoint tensor (a packed `to_qkv`, say). A "row" is one
    /// index of `shape[0]`, so this works for any rank >= 1 — rank 1 gives
    /// single elements. Only the requested byte range is read.
    pub fn read_row_run_bytes(&self, name: &str, first_row: u64, rows: u64) -> Result<Vec<u8>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        let Some(&leading) = entry.shape.first() else {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} is rank 0, it has no rows", name),
            });
        };
        if first_row.saturating_add(rows) > leading {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensor {} rows {}..{} out of range (leading axis {})",
                    name,
                    first_row,
                    first_row + rows,
                    leading
                ),
            });
        }
        let row_bytes = entry.shape[1..].iter().product::<u64>() * entry.dtype.byte_width();
        let file_offsets = entry.file_offsets(self.payload_base_offset());
        let start = file_offsets[0] + first_row * row_bytes;
        let len = rows * row_bytes;
        if start + len > file_offsets[1] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} row run extends past tensor payload", name),
            });
        }
        self.read_file_range(start, len as usize)
    }

    pub fn read_rank2_row_bytes(&self, name: &str, row: u64) -> Result<Vec<u8>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.shape.len() != 2 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected rank 2, got {:?}", name, entry.shape),
            });
        }
        if row >= entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} row {} out of range", name, row),
            });
        }
        let row_bytes = entry.shape[1] * entry.dtype.byte_width();
        let file_offsets = entry.file_offsets(self.payload_base_offset());
        let start = file_offsets[0] + row * row_bytes;
        let end = start + row_bytes;
        if end > file_offsets[1] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} row {} extends past tensor payload", name, row),
            });
        }
        self.read_file_range(start, row_bytes as usize)
    }

    pub fn read_rank2_row_u32_words(&self, name: &str, row: u64) -> Result<Vec<u32>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected U32, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank2_row_bytes(name, row)?;
        let mut out = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.chunks_exact(4) {
            out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(out)
    }

    pub fn read_rank2_row_bf16_words(&self, name: &str, row: u64) -> Result<Vec<u16>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected BF16, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank2_row_bytes(name, row)?;
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(out)
    }

    fn read_rank3_plane_bytes(&self, name: &str, plane: u64) -> Result<Vec<u8>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.shape.len() != 3 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected rank 3, got {:?}", name, entry.shape),
            });
        }
        if plane >= entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane {} out of range", name, plane),
            });
        }
        let plane_elems = entry.shape[1].checked_mul(entry.shape[2]).ok_or_else(|| {
            MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane element count overflow", name),
            }
        })?;
        let plane_bytes = plane_elems
            .checked_mul(entry.dtype.byte_width())
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane byte count overflow", name),
            })?;
        let file_offsets = entry.file_offsets(self.payload_base_offset());
        let start = file_offsets[0]
            .checked_add(plane.checked_mul(plane_bytes).ok_or_else(|| {
                MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} plane offset overflow", name),
                }
            })?)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane start overflow", name),
            })?;
        let end = start
            .checked_add(plane_bytes)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane end overflow", name),
            })?;
        if end > file_offsets[1] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensor {} plane {} extends past tensor payload",
                    name, plane
                ),
            });
        }
        self.read_file_range(start, plane_bytes as usize)
    }

    pub fn read_rank3_plane_u32_words(&self, name: &str, plane: u64) -> Result<Vec<u32>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected U32, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank3_plane_bytes(name, plane)?;
        let mut out = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.chunks_exact(4) {
            out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(out)
    }

    pub fn read_rank3_plane_bf16_words(&self, name: &str, plane: u64) -> Result<Vec<u16>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected BF16, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank3_plane_bytes(name, plane)?;
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(out)
    }

    pub fn read_u32_tensor_words(&self, name: &str) -> Result<Vec<u32>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected U32, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_tensor_bytes(name)?;
        let mut out = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.chunks_exact(4) {
            out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(out)
    }

    pub fn read_bf16_tensor_words(&self, name: &str) -> Result<Vec<u16>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected BF16, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_tensor_bytes(name)?;
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(out)
    }
}

fn json_object<'a>(
    path: &Path,
    context: &str,
    value: &'a JsonValue,
) -> Result<&'a HashMap<String, JsonValue>> {
    match value {
        JsonValue::Object(object) => Ok(object),
        other => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected object, got {:?}", context, other),
        }),
    }
}

fn json_string(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<String> {
    match value {
        Some(JsonValue::String(text)) => Ok(text.clone()),
        Some(other) => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected string, got {:?}", context, other),
        }),
        None => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} missing string field", context),
        }),
    }
}

fn json_u64(path: &Path, context: &str, value: &JsonValue) -> Result<u64> {
    match value {
        JsonValue::U64(number) => Ok(*number),
        JsonValue::U128(number) => {
            u64::try_from(*number).map_err(|_| MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} value {} does not fit in u64", context, number),
            })
        }
        JsonValue::I64(number) => {
            u64::try_from(*number).map_err(|_| MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} value {} is negative", context, number),
            })
        }
        JsonValue::I128(number) => {
            u64::try_from(*number).map_err(|_| MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} value {} is negative or too large", context, number),
            })
        }
        other => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected integer, got {:?}", context, other),
        }),
    }
}

fn json_u64_array(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<Vec<u64>> {
    let array = match value {
        Some(JsonValue::Array(array)) => array,
        Some(other) => {
            return Err(MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} expected integer array, got {:?}", context, other),
            });
        }
        None => {
            return Err(MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} missing integer array", context),
            });
        }
    };
    let mut out = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        out.push(json_u64(path, &format!("{}[{}]", context, index), item)?);
    }
    Ok(out)
}

fn json_two_u64s(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<[u64; 2]> {
    let values = json_u64_array(path, context, value)?;
    if values.len() != 2 {
        return Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected two integers, got {}", context, values.len()),
        });
    }
    Ok([values[0], values[1]])
}

fn json_string_map(
    path: &Path,
    context: &str,
    value: &JsonValue,
) -> Result<HashMap<String, String>> {
    let object = json_object(path, context, value)?;
    let mut out = HashMap::with_capacity(object.len());
    for (key, value) in object {
        out.insert(
            key.clone(),
            json_string(path, &format!("{}.{}", context, key), Some(value))?,
        );
    }
    Ok(out)
}

fn json_dtype(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<MlxDType> {
    let dtype_str = json_string(path, &format!("{}.dtype", context), value)?;
    MlxDType::from_safetensors_str(&dtype_str).map_err(|_| MlxRtError::InvalidSafetensors {
        path: path.to_path_buf(),
        message: format!("{} unsupported dtype {}", context, dtype_str),
    })
}
