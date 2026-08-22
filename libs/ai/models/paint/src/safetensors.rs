//! Bounded, dependency-free safetensors indexer for the pinned DINOv2-Giant
//! checkpoint. The 4.55 GB payload is never copied wholesale: only the JSON
//! header is parsed, then tensors are sought and read one at a time in archive
//! offset order.
//!
//! The parser intentionally accepts only the scalar types the native Paint
//! graph consumes. Unsupported dtypes, malformed shapes, overlapping ranges,
//! duplicate names, and out-of-file offsets fail closed before any upload.

use crate::torch_bin::TorchDtype;
use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};

const MAX_HEADER_BYTES: usize = 64 * 1024 * 1024;
const MAX_TENSORS: usize = 16_384;
const MAX_TENSOR_RANK: usize = 16;
const MAX_TENSOR_NAME_BYTES: usize = 4_096;
const MAX_JSON_STRING_BYTES: usize = 1024 * 1024;
const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_VALUES: usize = 1_000_000;
const MAX_TENSOR_BYTES: usize = 8 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SafeTensorError {
    Io(String),
    Header(String),
    Unsupported(String),
    Tensor(String),
}

impl std::fmt::Display for SafeTensorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SafeTensorError::Io(message) => write!(f, "io: {message}"),
            SafeTensorError::Header(message) => write!(f, "header: {message}"),
            SafeTensorError::Unsupported(message) => write!(f, "unsupported: {message}"),
            SafeTensorError::Tensor(message) => write!(f, "tensor: {message}"),
        }
    }
}

impl std::error::Error for SafeTensorError {}

impl From<std::io::Error> for SafeTensorError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeTensorRecord {
    pub name: String,
    pub dtype: TorchDtype,
    pub shape: Vec<usize>,
    pub numel: usize,
    /// Relative to the safetensors data section (immediately after header).
    pub data_start: usize,
    pub data_end: usize,
}

#[derive(Debug)]
pub struct SafeTensorIndex {
    pub tensors: Vec<SafeTensorRecord>,
    data_offset: usize,
    file_len: usize,
}

impl SafeTensorIndex {
    pub fn find(&self, name: &str) -> Option<&SafeTensorRecord> {
        self.tensors.iter().find(|record| record.name == name)
    }

    pub fn archive_offset(&self, record: &SafeTensorRecord) -> Result<usize, SafeTensorError> {
        self.checked_record_range(record).map(|(start, _)| start)
    }

    fn checked_record_range(
        &self,
        record: &SafeTensorRecord,
    ) -> Result<(usize, usize), SafeTensorError> {
        if record.name.len() > MAX_TENSOR_NAME_BYTES || record.shape.len() > MAX_TENSOR_RANK {
            return Err(SafeTensorError::Tensor(format!(
                "{} name or rank exceeds bounded limits",
                record.name
            )));
        }
        if record.data_end < record.data_start {
            return Err(SafeTensorError::Tensor(format!(
                "{} has reversed data range",
                record.name
            )));
        }
        let numel = record.shape.iter().try_fold(1usize, |product, dimension| {
            product.checked_mul(*dimension).ok_or_else(|| {
                SafeTensorError::Tensor(format!("{} shape product overflow", record.name))
            })
        })?;
        if numel != record.numel {
            return Err(SafeTensorError::Tensor(format!(
                "{} numel does not match shape",
                record.name
            )));
        }
        let expected = numel.checked_mul(record.dtype.elem_size()).ok_or_else(|| {
            SafeTensorError::Tensor(format!("{} byte length overflow", record.name))
        })?;
        let len = record
            .data_end
            .checked_sub(record.data_start)
            .ok_or_else(|| SafeTensorError::Tensor(format!("{} range underflow", record.name)))?;
        if len != expected {
            return Err(SafeTensorError::Tensor(format!(
                "{} range length does not match shape/dtype",
                record.name
            )));
        }
        if len > MAX_TENSOR_BYTES {
            return Err(SafeTensorError::Unsupported(format!(
                "{} tensor is {len} bytes (limit {MAX_TENSOR_BYTES})",
                record.name
            )));
        }
        let start = self
            .data_offset
            .checked_add(record.data_start)
            .ok_or_else(|| SafeTensorError::Tensor(format!("{} offset overflow", record.name)))?;
        if start.checked_add(len).is_none_or(|end| end > self.file_len) {
            return Err(SafeTensorError::Tensor(format!(
                "{} range outside archive",
                record.name
            )));
        }
        Ok((start, len))
    }

    pub fn read_tensor_into<R: Read + Seek>(
        &self,
        archive: &mut R,
        record: &SafeTensorRecord,
        out: &mut Vec<u8>,
    ) -> Result<(), SafeTensorError> {
        let (start, len) = self.checked_record_range(record)?;
        archive.seek(SeekFrom::Start(start as u64))?;
        out.clear();
        out.try_reserve_exact(len)
            .map_err(|_| SafeTensorError::Tensor(format!("{} allocation failed", record.name)))?;
        out.resize(len, 0);
        archive.read_exact(out)?;
        Ok(())
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
    values_seen: usize,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            values_seen: 0,
        }
    }

    fn error(&self, message: impl AsRef<str>) -> SafeTensorError {
        SafeTensorError::Header(format!("{} at byte {}", message.as_ref(), self.at))
    }

    fn whitespace(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            self.at += 1;
        }
    }

    fn take(&mut self, expected: u8) -> Result<(), SafeTensorError> {
        self.whitespace();
        if self.bytes.get(self.at) != Some(&expected) {
            return Err(self.error(format!("expected '{}'", expected as char)));
        }
        self.at += 1;
        Ok(())
    }

    fn consume(&mut self, byte: u8) -> bool {
        self.whitespace();
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn string(&mut self) -> Result<String, SafeTensorError> {
        self.take(b'"')?;
        let mut out = Vec::new();
        loop {
            let byte = *self
                .bytes
                .get(self.at)
                .ok_or_else(|| self.error("unterminated string"))?;
            self.at += 1;
            match byte {
                b'"' => {
                    return String::from_utf8(out)
                        .map_err(|_| self.error("string is not valid UTF-8"));
                }
                b'\\' => {
                    let escaped = *self
                        .bytes
                        .get(self.at)
                        .ok_or_else(|| self.error("unterminated escape"))?;
                    self.at += 1;
                    match escaped {
                        b'"' | b'\\' | b'/' => out.push(escaped),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let end = self
                                .at
                                .checked_add(4)
                                .ok_or_else(|| self.error("unicode escape overflow"))?;
                            let digits = self
                                .bytes
                                .get(self.at..end)
                                .ok_or_else(|| self.error("short unicode escape"))?;
                            let text = std::str::from_utf8(digits)
                                .map_err(|_| self.error("bad unicode escape"))?;
                            let value = u16::from_str_radix(text, 16)
                                .map_err(|_| self.error("bad unicode escape"))?;
                            let ch = char::from_u32(value as u32)
                                .ok_or_else(|| self.error("surrogate escape is unsupported"))?;
                            let mut encoded = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
                            self.at = end;
                        }
                        _ => return Err(self.error("unknown string escape")),
                    }
                }
                0..=31 => return Err(self.error("control byte in string")),
                _ => out.push(byte),
            }
            if out.len() > MAX_JSON_STRING_BYTES {
                return Err(self.error("JSON string exceeds bounded length"));
            }
        }
    }

    fn usize(&mut self) -> Result<usize, SafeTensorError> {
        self.whitespace();
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            self.at += 1;
        }
        if start == self.at {
            return Err(self.error("expected unsigned integer"));
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at])
            .map_err(|_| self.error("invalid integer"))?;
        text.parse::<usize>()
            .map_err(|_| self.error("integer exceeds address space"))
    }

    fn usize_array(&mut self) -> Result<Vec<usize>, SafeTensorError> {
        self.take(b'[')?;
        let mut out = Vec::new();
        if self.consume(b']') {
            return Ok(out);
        }
        loop {
            if out.len() >= MAX_TENSOR_RANK {
                return Err(self.error("integer array exceeds rank/value limit"));
            }
            out.push(self.usize()?);
            if self.consume(b']') {
                return Ok(out);
            }
            self.take(b',')?;
        }
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), SafeTensorError> {
        self.whitespace();
        let end = self
            .at
            .checked_add(literal.len())
            .ok_or_else(|| self.error("JSON literal offset overflow"))?;
        if self.bytes.get(self.at..end) != Some(literal) {
            return Err(self.error("bad JSON literal"));
        }
        self.at = end;
        Ok(())
    }

    fn skip_number(&mut self) -> Result<(), SafeTensorError> {
        self.whitespace();
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(|byte| {
            byte.is_ascii_digit() || matches!(*byte, b'-' | b'+' | b'.' | b'e' | b'E')
        }) {
            self.at += 1;
        }
        if start == self.at {
            return Err(self.error("expected JSON number"));
        }
        Ok(())
    }

    fn skip_value(&mut self, depth: usize) -> Result<(), SafeTensorError> {
        if depth > MAX_JSON_DEPTH {
            return Err(self.error("JSON nesting exceeds bounded depth"));
        }
        self.values_seen = self
            .values_seen
            .checked_add(1)
            .ok_or_else(|| self.error("JSON value count overflow"))?;
        if self.values_seen > MAX_JSON_VALUES {
            return Err(self.error("JSON value count exceeds limit"));
        }
        self.whitespace();
        match self.bytes.get(self.at).copied() {
            Some(b'"') => {
                self.string()?;
                Ok(())
            }
            Some(b'{') => {
                self.take(b'{')?;
                if self.consume(b'}') {
                    return Ok(());
                }
                loop {
                    self.string()?;
                    self.take(b':')?;
                    self.skip_value(depth + 1)?;
                    if self.consume(b'}') {
                        return Ok(());
                    }
                    self.take(b',')?;
                }
            }
            Some(b'[') => {
                self.take(b'[')?;
                if self.consume(b']') {
                    return Ok(());
                }
                loop {
                    self.skip_value(depth + 1)?;
                    if self.consume(b']') {
                        return Ok(());
                    }
                    self.take(b',')?;
                }
            }
            Some(b't') => self.literal(b"true"),
            Some(b'f') => self.literal(b"false"),
            Some(b'n') => self.literal(b"null"),
            Some(_) => self.skip_number(),
            None => Err(self.error("expected JSON value")),
        }
    }

    fn tensor(&mut self, name: String) -> Result<SafeTensorRecord, SafeTensorError> {
        if name.len() > MAX_TENSOR_NAME_BYTES {
            return Err(self.error("tensor name exceeds bounded length"));
        }
        self.take(b'{')?;
        let mut dtype = None;
        let mut shape = None;
        let mut offsets = None;
        if !self.consume(b'}') {
            loop {
                let field = self.string()?;
                self.take(b':')?;
                match field.as_str() {
                    "dtype" => {
                        if dtype.is_some() {
                            return Err(self.error(format!("{name}: duplicate dtype")));
                        }
                        let text = self.string()?;
                        dtype = Some(match text.as_str() {
                            "F32" => TorchDtype::F32,
                            "F16" => TorchDtype::F16,
                            "BF16" => TorchDtype::BF16,
                            "I64" => TorchDtype::I64,
                            _ => {
                                return Err(SafeTensorError::Unsupported(format!(
                                    "{name}: safetensors dtype {text}"
                                )))
                            }
                        });
                    }
                    "shape" => {
                        if shape.is_some() {
                            return Err(self.error(format!("{name}: duplicate shape")));
                        }
                        shape = Some(self.usize_array()?);
                    }
                    "data_offsets" => {
                        if offsets.is_some() {
                            return Err(self.error(format!("{name}: duplicate data_offsets")));
                        }
                        let values = self.usize_array()?;
                        if values.len() != 2 {
                            return Err(self.error(format!(
                                "{name}: data_offsets needs two integers"
                            )));
                        }
                        offsets = Some((values[0], values[1]));
                    }
                    _ => self.skip_value(0)?,
                }
                if self.consume(b'}') {
                    break;
                }
                self.take(b',')?;
            }
        }
        let dtype = dtype.ok_or_else(|| self.error(format!("{name}: missing dtype")))?;
        let shape = shape.ok_or_else(|| self.error(format!("{name}: missing shape")))?;
        let (data_start, data_end) =
            offsets.ok_or_else(|| self.error(format!("{name}: missing data_offsets")))?;
        if data_end < data_start {
            return Err(self.error(format!("{name}: reversed data_offsets")));
        }
        let numel = shape.iter().try_fold(1usize, |product, dim| {
            product.checked_mul(*dim).ok_or_else(|| {
                SafeTensorError::Tensor(format!("{name}: shape product overflow"))
            })
        })?;
        let expected = numel
            .checked_mul(dtype.elem_size())
            .ok_or_else(|| SafeTensorError::Tensor(format!("{name}: byte length overflow")))?;
        if data_end - data_start != expected {
            return Err(SafeTensorError::Tensor(format!(
                "{name}: range is {} bytes, shape/dtype require {expected}",
                data_end - data_start
            )));
        }
        Ok(SafeTensorRecord {
            name,
            dtype,
            shape,
            numel,
            data_start,
            data_end,
        })
    }

    fn header(mut self) -> Result<Vec<SafeTensorRecord>, SafeTensorError> {
        self.take(b'{')?;
        let mut tensors = Vec::new();
        let mut names = BTreeSet::new();
        if !self.consume(b'}') {
            loop {
                let name = self.string()?;
                self.take(b':')?;
                if name == "__metadata__" {
                    self.skip_value(0)?;
                } else {
                    if !names.insert(name.clone()) {
                        return Err(self.error(format!("duplicate tensor {name}")));
                    }
                    if tensors.len() >= MAX_TENSORS {
                        return Err(self.error("tensor count exceeds bounded limit"));
                    }
                    tensors.push(self.tensor(name)?);
                }
                if self.consume(b'}') {
                    break;
                }
                self.take(b',')?;
            }
        }
        self.whitespace();
        if self.at != self.bytes.len() {
            return Err(self.error("trailing header data"));
        }
        Ok(tensors)
    }
}

pub fn read_index_from<R: Read + Seek>(archive: &mut R) -> Result<SafeTensorIndex, SafeTensorError> {
    let file_len_u64 = archive.seek(SeekFrom::End(0))?;
    let file_len = usize::try_from(file_len_u64)
        .map_err(|_| SafeTensorError::Unsupported("archive exceeds address space".to_string()))?;
    if file_len < 8 {
        return Err(SafeTensorError::Header("file shorter than length prefix".to_string()));
    }
    archive.seek(SeekFrom::Start(0))?;
    let mut prefix = [0u8; 8];
    archive.read_exact(&mut prefix)?;
    let header_len_u64 = u64::from_le_bytes(prefix);
    let header_len = usize::try_from(header_len_u64)
        .map_err(|_| SafeTensorError::Unsupported("header exceeds address space".to_string()))?;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err(SafeTensorError::Unsupported(format!(
            "safetensors header is {header_len} bytes (limit {MAX_HEADER_BYTES})"
        )));
    }
    let data_offset = 8usize
        .checked_add(header_len)
        .ok_or_else(|| SafeTensorError::Header("header offset overflow".to_string()))?;
    if data_offset > file_len {
        return Err(SafeTensorError::Header("header extends beyond file".to_string()));
    }
    let mut header = Vec::new();
    header
        .try_reserve_exact(header_len)
        .map_err(|_| SafeTensorError::Header("header allocation failed".to_string()))?;
    header.resize(header_len, 0);
    archive.read_exact(&mut header)?;
    let tensors = Parser::new(&header).header()?;
    let data_len = file_len - data_offset;
    let mut ranges: Vec<(&str, usize, usize)> = tensors
        .iter()
        .map(|record| (record.name.as_str(), record.data_start, record.data_end))
        .collect();
    ranges.sort_by_key(|(_, start, end)| (*start, *end));
    let mut prior_end = 0usize;
    for (name, start, end) in ranges {
        if end > data_len {
            return Err(SafeTensorError::Tensor(format!(
                "{name}: data range {start}..{end} exceeds payload {data_len}"
            )));
        }
        if start < prior_end && start != end {
            return Err(SafeTensorError::Tensor(format!(
                "{name}: data range overlaps a previous tensor"
            )));
        }
        prior_end = prior_end.max(end);
    }
    Ok(SafeTensorIndex {
        tensors,
        data_offset,
        file_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(header: &str, data: &[u8]) -> Vec<u8> {
        let mut header = header.as_bytes().to_vec();
        while header.len() % 8 != 0 {
            header.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(&(header.len() as u64).to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn indexes_and_streams_in_data_offset_order() {
        let bytes = archive(
            r#"{"z.weight":{"dtype":"F32","shape":[2],"data_offsets":[4,12]},"__metadata__":{"format":"pt"},"a.bias":{"dtype":"F16","shape":[2],"data_offsets":[0,4]}}"#,
            &[0x00, 0x3c, 0x00, 0xc0, 0, 0, 0x80, 0x3f, 0, 0, 0, 0xc0],
        );
        let mut cursor = std::io::Cursor::new(bytes);
        let index = read_index_from(&mut cursor).unwrap();
        assert_eq!(index.tensors.len(), 2);
        assert_eq!(index.find("z.weight").unwrap().shape, vec![2]);
        assert_eq!(index.find("a.bias").unwrap().dtype, TorchDtype::F16);
        let mut data = Vec::new();
        index
            .read_tensor_into(&mut cursor, index.find("a.bias").unwrap(), &mut data)
            .unwrap();
        assert_eq!(data, vec![0x00, 0x3c, 0x00, 0xc0]);
        assert!(
            index.archive_offset(index.find("a.bias").unwrap()).unwrap()
                < index.archive_offset(index.find("z.weight").unwrap()).unwrap()
        );
    }

    #[test]
    fn rejects_shape_range_mismatch() {
        let bytes = archive(
            r#"{"bad":{"dtype":"F32","shape":[2],"data_offsets":[0,4]}}"#,
            &[0; 4],
        );
        let error = read_index_from(&mut std::io::Cursor::new(bytes)).unwrap_err();
        assert!(matches!(error, SafeTensorError::Tensor(message) if message.contains("require 8")));
    }

    #[test]
    fn rejects_overlapping_ranges() {
        let bytes = archive(
            r#"{"a":{"dtype":"F32","shape":[2],"data_offsets":[0,8]},"b":{"dtype":"F32","shape":[2],"data_offsets":[4,12]}}"#,
            &[0; 12],
        );
        let error = read_index_from(&mut std::io::Cursor::new(bytes)).unwrap_err();
        assert!(matches!(error, SafeTensorError::Tensor(message) if message.contains("overlaps")));
    }

    #[test]
    fn rejects_unsupported_dtype() {
        let bytes = archive(
            r#"{"bad":{"dtype":"F64","shape":[1],"data_offsets":[0,8]}}"#,
            &[0; 8],
        );
        let error = read_index_from(&mut std::io::Cursor::new(bytes)).unwrap_err();
        assert!(matches!(error, SafeTensorError::Unsupported(message) if message.contains("F64")));
    }

    #[test]
    fn rejects_deep_metadata_and_excess_tensor_rank() {
        let nested = format!(
            "{{\"__metadata__\":{}0{}}}",
            "[".repeat(MAX_JSON_DEPTH + 2),
            "]".repeat(MAX_JSON_DEPTH + 2)
        );
        let error = read_index_from(&mut std::io::Cursor::new(archive(&nested, &[]))).unwrap_err();
        assert!(matches!(error, SafeTensorError::Header(message) if message.contains("depth")));

        let dimensions = std::iter::repeat_n("1", MAX_TENSOR_RANK + 1)
            .collect::<Vec<_>>()
            .join(",");
        let header = format!(
            "{{\"bad\":{{\"dtype\":\"F16\",\"shape\":[{dimensions}],\"data_offsets\":[0,2]}}}}"
        );
        let error =
            read_index_from(&mut std::io::Cursor::new(archive(&header, &[0; 2]))).unwrap_err();
        assert!(matches!(error, SafeTensorError::Header(message) if message.contains("limit")));
    }

    #[test]
    fn public_record_ranges_are_revalidated_before_reading() {
        let bytes = archive(
            r#"{"a":{"dtype":"F16","shape":[2],"data_offsets":[0,4]}}"#,
            &[0; 4],
        );
        let mut cursor = std::io::Cursor::new(bytes);
        let index = read_index_from(&mut cursor).unwrap();
        let mut hostile = index.find("a").unwrap().clone();
        hostile.data_start = 4;
        hostile.data_end = 1;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            index.read_tensor_into(&mut cursor, &hostile, &mut Vec::new())
        }));
        assert!(matches!(result, Ok(Err(SafeTensorError::Tensor(_)))));
    }
}
