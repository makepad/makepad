//! Little-endian GGUF v3 writer. Tensor dimensions are innermost first.
//! Metadata order and tensor order are supplied by the caller; padding is zero.

use super::gguf::{GgufArray, GgufError, GgufKeyValue, GgufTensorInfo, GgufValue, Result};
use crate::quant::{block_elements, block_size, TensorType};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct TensorSpec {
    pub name: String,
    pub dimensions: Vec<u64>,
    pub tensor_type: TensorType,
}

#[derive(Debug)]
pub struct GgufWriteLayout {
    pub data_offset: u64,
    pub file_size: u64,
    pub tensors: Vec<GgufTensorInfo>,
}

fn align(n: u64, alignment: u64) -> Result<u64> {
    n.checked_add((alignment - n % alignment) % alignment)
        .ok_or_else(|| GgufError::format("GGUF offset overflow"))
}

fn string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Canonical wire representation, also useful for exact metadata comparisons.
pub fn value_bytes(value: &GgufValue) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(value.value_type() as u32).to_le_bytes());
    macro_rules! scalar {
        ($x:expr) => {
            out.extend_from_slice(&$x.to_le_bytes())
        };
    }
    macro_rules! array {
        ($values:expr) => {
            for x in $values {
                scalar!(x);
            }
        };
    }
    match value {
        GgufValue::Uint8(x) => scalar!(x),
        GgufValue::Int8(x) => scalar!(x),
        GgufValue::Uint16(x) => scalar!(x),
        GgufValue::Int16(x) => scalar!(x),
        GgufValue::Uint32(x) => scalar!(x),
        GgufValue::Int32(x) => scalar!(x),
        GgufValue::Float32(x) => scalar!(x),
        GgufValue::Uint64(x) => scalar!(x),
        GgufValue::Int64(x) => scalar!(x),
        GgufValue::Float64(x) => scalar!(x),
        GgufValue::Bool(x) => out.push(u8::from(*x)),
        GgufValue::String(x) => string(&mut out, x.as_bytes()),
        GgufValue::Array(a) => {
            out.extend_from_slice(&(a.element_type() as u32).to_le_bytes());
            out.extend_from_slice(&(a.len() as u64).to_le_bytes());
            match a {
                GgufArray::Uint8(xs) => array!(xs),
                GgufArray::Int8(xs) => array!(xs),
                GgufArray::Uint16(xs) => array!(xs),
                GgufArray::Int16(xs) => array!(xs),
                GgufArray::Uint32(xs) => array!(xs),
                GgufArray::Int32(xs) => array!(xs),
                GgufArray::Float32(xs) => array!(xs),
                GgufArray::Uint64(xs) => array!(xs),
                GgufArray::Int64(xs) => array!(xs),
                GgufArray::Float64(xs) => array!(xs),
                GgufArray::Bool(xs) => out.extend(xs.iter().map(|x| u8::from(*x))),
                GgufArray::String(xs) => {
                    for x in xs {
                        string(&mut out, x.as_bytes());
                    }
                }
            }
        }
    }
    out
}

/// Creates an exclusive output file, writes the complete header and reserves
/// its tensor payload. Callers may fill disjoint tensor ranges concurrently
/// with independent file handles, then sync before publishing the file.
pub fn create(path: &Path, kv: &[GgufKeyValue], specs: &[TensorSpec]) -> Result<GgufWriteLayout> {
    let alignment = match kv.iter().find(|k| k.key == "general.alignment") {
        Some(k) => k
            .value
            .as_u32()
            .filter(|a| *a > 0)
            .ok_or_else(|| GgufError::format("general.alignment must be a positive u32"))?
            as u64,
        None => 32,
    };
    let mut keys = HashSet::new();
    let mut header = Vec::new();
    header.extend_from_slice(b"GGUF");
    header.extend_from_slice(&3u32.to_le_bytes());
    header.extend_from_slice(&(specs.len() as u64).to_le_bytes());
    header.extend_from_slice(&(kv.len() as u64).to_le_bytes());
    for k in kv {
        if k.key.is_empty() || !keys.insert(k.key.as_str()) {
            return Err(GgufError::format(format!(
                "empty or duplicate metadata key {:?}",
                k.key
            )));
        }
        string(&mut header, k.key.as_bytes());
        header.extend_from_slice(&value_bytes(&k.value));
    }
    let mut tensors = Vec::with_capacity(specs.len());
    let mut names = HashSet::new();
    let mut offset = 0u64;
    for s in specs {
        if s.name.is_empty()
            || !names.insert(s.name.as_str())
            || s.dimensions.is_empty()
            || s.dimensions.len() > 4
            || s.dimensions.contains(&0)
        {
            return Err(GgufError::format(format!(
                "invalid tensor descriptor {}",
                s.name
            )));
        }
        let elements = block_elements(s.tensor_type.ggml_type()) as u64;
        if s.dimensions[0] % elements != 0 {
            return Err(GgufError::format(format!(
                "{} row width is not divisible by block size {elements}",
                s.name
            )));
        }
        let count = s
            .dimensions
            .iter()
            .try_fold(1u64, |n, d| n.checked_mul(*d))
            .ok_or_else(|| GgufError::format("tensor element count overflow"))?;
        let size_bytes = (count / elements)
            .checked_mul(block_size(s.tensor_type.ggml_type()) as u64)
            .ok_or_else(|| GgufError::format("tensor byte count overflow"))?;
        offset = align(offset, alignment)?;
        string(&mut header, s.name.as_bytes());
        header.extend_from_slice(&(s.dimensions.len() as u32).to_le_bytes());
        for d in &s.dimensions {
            header.extend_from_slice(&d.to_le_bytes());
        }
        header.extend_from_slice(&s.tensor_type.ggml_type().to_le_bytes());
        header.extend_from_slice(&offset.to_le_bytes());
        tensors.push(GgufTensorInfo {
            name: s.name.clone(),
            dimensions: s.dimensions.clone(),
            tensor_type: s.tensor_type,
            offset,
            size_bytes,
        });
        offset = offset
            .checked_add(size_bytes)
            .ok_or_else(|| GgufError::format("tensor offset overflow"))?;
    }
    let data_offset = align(header.len() as u64, alignment)?;
    let file_size = data_offset
        .checked_add(align(offset, alignment)?)
        .ok_or_else(|| GgufError::format("file size overflow"))?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.set_len(file_size)?;
    file.write_all(&header)?;
    file.sync_all()?;
    Ok(GgufWriteLayout {
        data_offset,
        file_size,
        tensors,
    })
}

/// Writes bytes inside one tensor's reserved extent. Each concurrent caller
/// must use its own `File::open` handle, since cloned handles share a cursor.
pub fn write_range(
    file: &mut File,
    layout: &GgufWriteLayout,
    index: usize,
    offset: u64,
    bytes: &[u8],
) -> Result<()> {
    let tensor = layout
        .tensors
        .get(index)
        .ok_or_else(|| GgufError::format("tensor index out of range"))?;
    let end = offset
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| GgufError::format("write length overflow"))?;
    if end > tensor.size_bytes {
        return Err(GgufError::format(format!("write past {}", tensor.name)));
    }
    file.seek(SeekFrom::Start(layout.data_offset + tensor.offset + offset))?;
    file.write_all(bytes)?;
    Ok(())
}
