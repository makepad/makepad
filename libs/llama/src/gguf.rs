use crate::error::{LlamaError, Result};
use makepad_ggml::{block_elements, block_size, TensorType};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const GGUF_MAGIC: &[u8; 4] = b"GGUF";
const GGUF_VERSION: u32 = 3;
const GGUF_DEFAULT_ALIGNMENT: u64 = 32;
const GGUF_MAX_STRING_LENGTH: u64 = 1024 * 1024 * 1024;
const GGUF_MAX_ARRAY_ELEMENTS: u64 = 1024 * 1024 * 1024;
const GGUF_KEY_GENERAL_ALIGNMENT: &str = "general.alignment";

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GgufType {
    Uint8 = 0,
    Int8 = 1,
    Uint16 = 2,
    Int16 = 3,
    Uint32 = 4,
    Int32 = 5,
    Float32 = 6,
    Bool = 7,
    String = 8,
    Array = 9,
    Uint64 = 10,
    Int64 = 11,
    Float64 = 12,
}

impl GgufType {
    pub fn from_i32(value: i32) -> Option<Self> {
        Some(match value {
            0 => Self::Uint8,
            1 => Self::Int8,
            2 => Self::Uint16,
            3 => Self::Int16,
            4 => Self::Uint32,
            5 => Self::Int32,
            6 => Self::Float32,
            7 => Self::Bool,
            8 => Self::String,
            9 => Self::Array,
            10 => Self::Uint64,
            11 => Self::Int64,
            12 => Self::Float64,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Uint8 => "u8",
            Self::Int8 => "i8",
            Self::Uint16 => "u16",
            Self::Int16 => "i16",
            Self::Uint32 => "u32",
            Self::Int32 => "i32",
            Self::Float32 => "f32",
            Self::Bool => "bool",
            Self::String => "str",
            Self::Array => "arr",
            Self::Uint64 => "u64",
            Self::Int64 => "i64",
            Self::Float64 => "f64",
        }
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct GgufString(Vec<u8>);

impl GgufString {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }

    pub fn try_utf8(&self) -> Result<&str> {
        std::str::from_utf8(&self.0)
            .map_err(|err| LlamaError::format(format!("invalid utf-8 gguf string: {}", err)))
    }
}

impl Debug for GgufString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.to_string_lossy())
    }
}

#[derive(Clone, Debug)]
pub enum GgufArray {
    Uint8(Vec<u8>),
    Int8(Vec<i8>),
    Uint16(Vec<u16>),
    Int16(Vec<i16>),
    Uint32(Vec<u32>),
    Int32(Vec<i32>),
    Float32(Vec<f32>),
    Bool(Vec<bool>),
    String(Vec<GgufString>),
    Uint64(Vec<u64>),
    Int64(Vec<i64>),
    Float64(Vec<f64>),
}

impl GgufArray {
    pub fn len(&self) -> usize {
        match self {
            Self::Uint8(v) => v.len(),
            Self::Int8(v) => v.len(),
            Self::Uint16(v) => v.len(),
            Self::Int16(v) => v.len(),
            Self::Uint32(v) => v.len(),
            Self::Int32(v) => v.len(),
            Self::Float32(v) => v.len(),
            Self::Bool(v) => v.len(),
            Self::String(v) => v.len(),
            Self::Uint64(v) => v.len(),
            Self::Int64(v) => v.len(),
            Self::Float64(v) => v.len(),
        }
    }

    pub fn element_type(&self) -> GgufType {
        match self {
            Self::Uint8(_) => GgufType::Uint8,
            Self::Int8(_) => GgufType::Int8,
            Self::Uint16(_) => GgufType::Uint16,
            Self::Int16(_) => GgufType::Int16,
            Self::Uint32(_) => GgufType::Uint32,
            Self::Int32(_) => GgufType::Int32,
            Self::Float32(_) => GgufType::Float32,
            Self::Bool(_) => GgufType::Bool,
            Self::String(_) => GgufType::String,
            Self::Uint64(_) => GgufType::Uint64,
            Self::Int64(_) => GgufType::Int64,
            Self::Float64(_) => GgufType::Float64,
        }
    }

    pub fn as_u32_slice(&self) -> Option<&[u32]> {
        match self {
            Self::Uint32(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum GgufValue {
    Uint8(u8),
    Int8(i8),
    Uint16(u16),
    Int16(i16),
    Uint32(u32),
    Int32(i32),
    Float32(f32),
    Bool(bool),
    String(GgufString),
    Array(GgufArray),
    Uint64(u64),
    Int64(i64),
    Float64(f64),
}

impl GgufValue {
    pub fn value_type(&self) -> GgufType {
        match self {
            Self::Uint8(_) => GgufType::Uint8,
            Self::Int8(_) => GgufType::Int8,
            Self::Uint16(_) => GgufType::Uint16,
            Self::Int16(_) => GgufType::Int16,
            Self::Uint32(_) => GgufType::Uint32,
            Self::Int32(_) => GgufType::Int32,
            Self::Float32(_) => GgufType::Float32,
            Self::Bool(_) => GgufType::Bool,
            Self::String(_) => GgufType::String,
            Self::Array(_) => GgufType::Array,
            Self::Uint64(_) => GgufType::Uint64,
            Self::Int64(_) => GgufType::Int64,
            Self::Float64(_) => GgufType::Float64,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Self::Uint32(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Uint64(v) => Some(*v),
            Self::Uint32(v) => Some((*v).into()),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::Float32(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&GgufString> {
        match self {
            Self::String(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&GgufArray> {
        match self {
            Self::Array(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GgufKeyValue {
    pub key: String,
    pub value: GgufValue,
}

#[derive(Clone, Debug)]
pub struct GgufTensorInfo {
    pub name: String,
    pub dimensions: Vec<u64>,
    pub tensor_type: TensorType,
    pub offset: u64,
    pub size_bytes: u64,
}

impl GgufTensorInfo {
    pub fn absolute_offset(&self, data_offset: u64) -> Result<u64> {
        data_offset.checked_add(self.offset).ok_or_else(|| {
            LlamaError::format(format!(
                "overflow computing absolute tensor offset for '{}'",
                self.name
            ))
        })
    }
}

#[derive(Clone, Debug)]
pub struct GgufFile {
    pub path: PathBuf,
    pub file_size: u64,
    pub version: u32,
    pub alignment: u64,
    pub data_offset: u64,
    pub kv: Vec<GgufKeyValue>,
    pub tensors: Vec<GgufTensorInfo>,
    kv_index: HashMap<String, usize>,
    tensor_index: HashMap<String, usize>,
}

impl GgufFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let file_size = file.metadata()?.len();
        let mut reader = BufReader::new(file);

        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic != GGUF_MAGIC {
            return Err(LlamaError::format(format!(
                "invalid magic {:?}, expected {:?}",
                magic, GGUF_MAGIC
            )));
        }

        let version = read_u32(&mut reader)?;
        if version == 0 || version > GGUF_VERSION {
            return Err(LlamaError::unsupported(format!(
                "unsupported gguf version {}",
                version
            )));
        }

        let n_tensors = read_i64(&mut reader)?;
        let n_kv = read_i64(&mut reader)?;
        if n_tensors < 0 || n_kv < 0 {
            return Err(LlamaError::format(format!(
                "negative counts in header: n_tensors={}, n_kv={}",
                n_tensors, n_kv
            )));
        }

        let mut kv = Vec::with_capacity(n_kv as usize);
        let mut kv_index = HashMap::with_capacity(n_kv as usize);

        for _ in 0..n_kv as usize {
            let key = read_key_string(&mut reader)?;
            let value_type_raw = read_i32(&mut reader)?;
            let value_type = GgufType::from_i32(value_type_raw).ok_or_else(|| {
                LlamaError::format(format!("invalid gguf value type {}", value_type_raw))
            })?;

            let value = read_value(&mut reader, value_type)?;
            kv_index.insert(key.clone(), kv.len());
            kv.push(GgufKeyValue { key, value });
        }

        let alignment = kv
            .iter()
            .find(|entry| entry.key == GGUF_KEY_GENERAL_ALIGNMENT)
            .and_then(|entry| entry.value.as_u32())
            .map(u64::from)
            .unwrap_or(GGUF_DEFAULT_ALIGNMENT);

        if alignment == 0 {
            return Err(LlamaError::format("invalid zero gguf alignment"));
        }

        let mut tensors = Vec::with_capacity(n_tensors as usize);
        let mut tensor_index = HashMap::with_capacity(n_tensors as usize);

        for _ in 0..n_tensors as usize {
            let name = read_key_string(&mut reader)?;
            let n_dims = read_u32(&mut reader)?;
            if n_dims == 0 || n_dims > 4 {
                return Err(LlamaError::format(format!(
                    "invalid tensor rank {} for '{}'",
                    n_dims, name
                )));
            }

            let mut dimensions = Vec::with_capacity(n_dims as usize);
            for _ in 0..n_dims as usize {
                let dim = read_u64_from_i64(&mut reader)?;
                dimensions.push(dim);
            }

            let ggml_type = read_i32(&mut reader)?;
            let tensor_type = TensorType::from_ggml_type(ggml_type as u32).ok_or_else(|| {
                LlamaError::unsupported(format!(
                    "unsupported ggml tensor type {} for '{}'",
                    ggml_type, name
                ))
            })?;
            let offset = read_u64(&mut reader)?;
            let size_bytes = ggml_tensor_size_bytes(tensor_type, &dimensions)?;

            tensor_index.insert(name.clone(), tensors.len());
            tensors.push(GgufTensorInfo {
                name,
                dimensions,
                tensor_type,
                offset,
                size_bytes,
            });
        }

        let data_offset = align_up(reader.stream_position()?, alignment)?;

        for tensor in &tensors {
            let start = tensor.absolute_offset(data_offset)?;
            let end = start.checked_add(tensor.size_bytes).ok_or_else(|| {
                LlamaError::format(format!(
                    "overflow computing tensor end for '{}'",
                    tensor.name
                ))
            })?;
            if end > file_size {
                return Err(LlamaError::format(format!(
                    "tensor '{}' extends beyond file bounds: end={} file_size={}",
                    tensor.name, end, file_size
                )));
            }
        }

        Ok(Self {
            path,
            file_size,
            version,
            alignment,
            data_offset,
            kv,
            tensors,
            kv_index,
            tensor_index,
        })
    }

    pub fn get_value(&self, key: &str) -> Option<&GgufValue> {
        self.kv_index
            .get(key)
            .and_then(|idx| self.kv.get(*idx))
            .map(|entry| &entry.value)
    }

    pub fn require_value(&self, key: &str) -> Result<&GgufValue> {
        self.get_value(key)
            .ok_or_else(|| LlamaError::format(format!("missing required gguf key '{}'", key)))
    }

    pub fn get_tensor(&self, name: &str) -> Option<&GgufTensorInfo> {
        self.tensor_index
            .get(name)
            .and_then(|idx| self.tensors.get(*idx))
    }

    pub fn require_tensor(&self, name: &str) -> Result<&GgufTensorInfo> {
        self.get_tensor(name)
            .ok_or_else(|| LlamaError::format(format!("missing tensor '{}'", name)))
    }

    pub fn read_tensor_prefix(&self, name: &str, len: usize) -> Result<Vec<u8>> {
        let tensor = self.require_tensor(name)?;
        let mut file = File::open(&self.path)?;
        let start = tensor.absolute_offset(self.data_offset)?;
        let to_read = len.min(tensor.size_bytes as usize);
        file.seek(SeekFrom::Start(start))?;
        let mut out = vec![0u8; to_read];
        file.read_exact(&mut out)?;
        Ok(out)
    }

    pub fn tensor_summary(&self, name: &str) -> Result<String> {
        let tensor = self.require_tensor(name)?;
        Ok(format!(
            "{} type={} dims={:?} size_bytes={} offset={}",
            tensor.name,
            tensor.tensor_type.name(),
            tensor.dimensions,
            tensor.size_bytes,
            tensor.offset
        ))
    }
}

fn align_up(value: u64, alignment: u64) -> Result<u64> {
    let rem = value % alignment;
    if rem == 0 {
        return Ok(value);
    }
    value
        .checked_add(alignment - rem)
        .ok_or_else(|| LlamaError::format("overflow while aligning gguf data offset"))
}

fn ggml_tensor_size_bytes(tensor_type: TensorType, dimensions: &[u64]) -> Result<u64> {
    if dimensions.is_empty() {
        return Err(LlamaError::format("tensor must have at least one dimension"));
    }

    let block_elems = block_elements(tensor_type.ggml_type()) as u64;
    let block_bytes = block_size(tensor_type.ggml_type()) as u64;
    let row_blocks = dimensions[0]
        .checked_add(block_elems - 1)
        .ok_or_else(|| LlamaError::format("overflow computing row blocks"))?
        / block_elems;
    let row_size = row_blocks
        .checked_mul(block_bytes)
        .ok_or_else(|| LlamaError::format("overflow computing row size"))?;

    dimensions[1..]
        .iter()
        .try_fold(row_size, |acc, &dim| {
            acc.checked_mul(dim)
                .ok_or_else(|| LlamaError::format("overflow computing tensor size"))
        })
}

fn read_value(reader: &mut impl Read, value_type: GgufType) -> Result<GgufValue> {
    Ok(match value_type {
        GgufType::Uint8 => GgufValue::Uint8(read_u8(reader)?),
        GgufType::Int8 => GgufValue::Int8(read_i8(reader)?),
        GgufType::Uint16 => GgufValue::Uint16(read_u16(reader)?),
        GgufType::Int16 => GgufValue::Int16(read_i16(reader)?),
        GgufType::Uint32 => GgufValue::Uint32(read_u32(reader)?),
        GgufType::Int32 => GgufValue::Int32(read_i32(reader)?),
        GgufType::Float32 => GgufValue::Float32(read_f32(reader)?),
        GgufType::Bool => GgufValue::Bool(read_bool(reader)?),
        GgufType::String => GgufValue::String(read_value_string(reader)?),
        GgufType::Array => {
            let array_type_raw = read_i32(reader)?;
            let array_type = GgufType::from_i32(array_type_raw).ok_or_else(|| {
                LlamaError::format(format!("invalid gguf array type {}", array_type_raw))
            })?;
            if array_type == GgufType::Array {
                return Err(LlamaError::unsupported("nested gguf arrays are not supported"));
            }
            let count = read_u64(reader)?;
            if count > GGUF_MAX_ARRAY_ELEMENTS {
                return Err(LlamaError::format(format!(
                    "gguf array too large: {} elements",
                    count
                )));
            }
            GgufValue::Array(read_array(reader, array_type, count as usize)?)
        }
        GgufType::Uint64 => GgufValue::Uint64(read_u64(reader)?),
        GgufType::Int64 => GgufValue::Int64(read_i64(reader)?),
        GgufType::Float64 => GgufValue::Float64(read_f64(reader)?),
    })
}

fn read_array(reader: &mut impl Read, array_type: GgufType, count: usize) -> Result<GgufArray> {
    Ok(match array_type {
        GgufType::Uint8 => GgufArray::Uint8(read_n(reader, count, read_u8)?),
        GgufType::Int8 => GgufArray::Int8(read_n(reader, count, read_i8)?),
        GgufType::Uint16 => GgufArray::Uint16(read_n(reader, count, read_u16)?),
        GgufType::Int16 => GgufArray::Int16(read_n(reader, count, read_i16)?),
        GgufType::Uint32 => GgufArray::Uint32(read_n(reader, count, read_u32)?),
        GgufType::Int32 => GgufArray::Int32(read_n(reader, count, read_i32)?),
        GgufType::Float32 => GgufArray::Float32(read_n(reader, count, read_f32)?),
        GgufType::Bool => GgufArray::Bool(read_n(reader, count, read_bool)?),
        GgufType::String => GgufArray::String(read_n(reader, count, read_value_string)?),
        GgufType::Uint64 => GgufArray::Uint64(read_n(reader, count, read_u64)?),
        GgufType::Int64 => GgufArray::Int64(read_n(reader, count, read_i64)?),
        GgufType::Float64 => GgufArray::Float64(read_n(reader, count, read_f64)?),
        GgufType::Array => {
            return Err(LlamaError::unsupported("nested gguf arrays are not supported"))
        }
    })
}

fn read_n<R: Read, T>(
    reader: &mut R,
    count: usize,
    mut read_one: impl FnMut(&mut R) -> Result<T>,
) -> Result<Vec<T>> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_one(reader)?);
    }
    Ok(out)
}

fn read_key_string(reader: &mut impl Read) -> Result<String> {
    let bytes = read_value_string(reader)?;
    bytes
        .try_utf8()
        .map(|s| s.to_owned())
        .map_err(|err| LlamaError::format(format!("invalid utf-8 gguf key: {}", err)))
}

fn read_value_string(reader: &mut impl Read) -> Result<GgufString> {
    let len = read_u64(reader)?;
    if len > GGUF_MAX_STRING_LENGTH {
        return Err(LlamaError::format(format!(
            "gguf string too large: {} bytes",
            len
        )));
    }
    let mut bytes = vec![0u8; len as usize];
    reader.read_exact(&mut bytes)?;
    Ok(GgufString::new(bytes))
}

fn read_bool(reader: &mut impl Read) -> Result<bool> {
    Ok(read_i8(reader)? != 0)
}

fn read_u8(reader: &mut impl Read) -> Result<u8> {
    let mut buf = [0u8; 1];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_i8(reader: &mut impl Read) -> Result<i8> {
    Ok(read_u8(reader)? as i8)
}

fn read_u16(reader: &mut impl Read) -> Result<u16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_i16(reader: &mut impl Read) -> Result<i16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(i16::from_le_bytes(buf))
}

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_i32(reader: &mut impl Read) -> Result<i32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_f32(reader: &mut impl Read) -> Result<f32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(f32::from_le_bytes(buf))
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_i64(reader: &mut impl Read) -> Result<i64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(i64::from_le_bytes(buf))
}

fn read_u64_from_i64(reader: &mut impl Read) -> Result<u64> {
    let value = read_i64(reader)?;
    if value < 0 {
        return Err(LlamaError::format(format!(
            "negative tensor dimension {}",
            value
        )));
    }
    Ok(value as u64)
}

fn read_f64(reader: &mut impl Read) -> Result<f64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(f64::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ggml::ggml_type_name;

    #[test]
    fn ggml_tensor_size_for_q5_k_matches_known_shape() {
        let ty = TensorType::Q5K;
        let dims = [2048_u64, 512_u64];
        let size = ggml_tensor_size_bytes(ty, &dims).unwrap();
        assert_eq!(size, 720_896);
    }

    #[test]
    fn align_up_rounds_to_multiple() {
        assert_eq!(align_up(33, 32).unwrap(), 64);
        assert_eq!(align_up(64, 32).unwrap(), 64);
    }

    #[test]
    fn tensor_type_names_are_available() {
        assert_eq!(ggml_type_name(TensorType::Q5K.ggml_type()), "q5_K");
    }
}
