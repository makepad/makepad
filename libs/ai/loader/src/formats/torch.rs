//! Staged copy of libs/pbr_paint/src/torch_bin.rs (lane T2, /aiarch.md §1) —
//! chosen as the best-hardened of the tree's torch-pickle readers. Pure
//! staging: pbr_paint's original stays in place and consumers are re-pointed
//! in a later lane when pbr_paint itself moves under libs/ai/models/paint.
//!
//! Minimal reader for PyTorch zip-format checkpoints (the pinned Hunyuan
//! `diffusion_pytorch_model.bin` files): a ZIP archive holding
//! `<prefix>/data.pkl` (a pickle building `{name: tensor}` via
//! `torch._utils._rebuild_tensor_v2`) plus one raw little-endian storage blob
//! per `<prefix>/data/<key>`.
//!
//! Scope is deliberately narrow and fail-closed:
//! * STORED zip entries only (torch's writer does not compress storages);
//!   a compressed or zip64-marked entry is a precise error, never a guess;
//! * the pickle VM implements exactly the opcode subset torch emits for
//!   state dicts (protocol 2, plus the proto-4 string/memo ops for safety);
//!   an unknown opcode errors with its stream position;
//! * only contiguous tensors resolve to byte ranges; exotic strides keep
//!   their metadata but refuse `tensor_bytes`.
//!
//! This is the paint lane's own loader so the executor does not depend on
//! the shared GGML/diffusion layers for checkpoint I/O.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

const MAX_ZIP_ENTRIES: usize = 16_384;
const MAX_CENTRAL_DIRECTORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRY_NAME_BYTES: usize = 4_096;
const MAX_PICKLE_BYTES: usize = 128 * 1024 * 1024;
const MAX_PICKLE_STACK: usize = 65_536;
const MAX_PICKLE_MEMO: usize = 65_536;
const MAX_PICKLE_CONTAINER_ITEMS: usize = 65_536;
const MAX_PICKLE_CLONE_NODES: usize = 1_000_000;
/// Cumulative heap bytes the pickle VM may request while decoding and
/// cloning values. This is deliberately a cumulative allocation budget, not
/// merely a live-set estimate: repeated memo GETs of one large value must
/// fail before cloning can amplify a small pickle into an unbounded heap.
const MAX_PICKLE_HEAP_BYTES: usize = 32 * 1024 * 1024;
const MAX_PICKLE_STRING_BYTES: usize = 1024 * 1024;
const MAX_TENSOR_RANK: usize = 16;
const MAX_TENSORS: usize = 16_384;
const MAX_TENSOR_BYTES: usize =
    if usize::BITS >= 64 { 8_u64 * 1024 * 1024 * 1024 } else { usize::MAX as u64 } as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TorchDtype {
    F32,
    F16,
    BF16,
    I64,
}

impl TorchDtype {
    pub fn elem_size(self) -> usize {
        match self {
            TorchDtype::F32 => 4,
            TorchDtype::F16 | TorchDtype::BF16 => 2,
            TorchDtype::I64 => 8,
        }
    }

    fn from_storage_class(name: &str) -> Option<Self> {
        match name {
            "FloatStorage" => Some(TorchDtype::F32),
            "HalfStorage" => Some(TorchDtype::F16),
            "BFloat16Storage" => Some(TorchDtype::BF16),
            "LongStorage" => Some(TorchDtype::I64),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TensorRecord {
    pub name: String,
    pub dtype: TorchDtype,
    pub shape: Vec<usize>,
    pub stride: Vec<usize>,
    pub storage_key: String,
    /// Total elements in the referenced storage blob.
    pub storage_numel: usize,
    /// Offset into the storage, in elements.
    pub storage_offset: usize,
    pub numel: usize,
}

impl TensorRecord {
    pub fn is_contiguous(&self) -> bool {
        if self.shape.len() != self.stride.len() {
            return false;
        }
        let mut expect = 1usize;
        for (dim, stride) in self.shape.iter().zip(self.stride.iter()).rev() {
            if *dim > 1 && *stride != expect {
                return false;
            }
            let Some(next) = expect.checked_mul(*dim) else {
                return false;
            };
            expect = next;
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TorchBinError {
    Io(String),
    Zip(String),
    Pickle(String),
    Unsupported(String),
    Tensor(String),
}

impl std::fmt::Display for TorchBinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TorchBinError::Io(m) => write!(f, "io: {m}"),
            TorchBinError::Zip(m) => write!(f, "zip: {m}"),
            TorchBinError::Pickle(m) => write!(f, "pickle: {m}"),
            TorchBinError::Unsupported(m) => write!(f, "unsupported: {m}"),
            TorchBinError::Tensor(m) => write!(f, "tensor: {m}"),
        }
    }
}

impl std::error::Error for TorchBinError {}

impl From<std::io::Error> for TorchBinError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

#[derive(Clone, Copy, Debug)]
struct ZipEntry {
    data_offset: usize,
    size: usize,
}

#[derive(Debug)]
pub struct TorchBinIndex {
    pub tensors: Vec<TensorRecord>,
    entries: HashMap<String, ZipEntry>,
    /// Exact archive directory containing the uniquely selected `data.pkl`.
    /// Storage lookup is rooted here; suffix matches across other embedded
    /// archives are never accepted.
    archive_root: String,
}

impl TorchBinIndex {
    pub fn find(&self, name: &str) -> Option<&TensorRecord> {
        self.tensors.iter().find(|t| t.name == name)
    }

    /// Exact little-endian bytes of a contiguous tensor within `archive`.
    pub fn tensor_bytes<'a>(
        &self,
        archive: &'a [u8],
        record: &TensorRecord,
    ) -> Result<&'a [u8], TorchBinError> {
        let (start, len) = self.tensor_byte_range(record)?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| TorchBinError::Tensor(format!("{}: archive range overflow", record.name)))?;
        if end > archive.len() {
            return Err(TorchBinError::Tensor(format!(
                "{}: byte range outside archive ({} + {} > {})",
                record.name,
                start,
                len,
                archive.len()
            )));
        }
        Ok(&archive[start..end])
    }

    /// Read exactly one tensor from a seekable checkpoint without mapping or
    /// copying the multi-gigabyte archive. `out` is reused across calls, so a
    /// load pass holds at most one tensor payload in host memory.
    pub fn read_tensor_into<R: Read + Seek>(
        &self,
        archive: &mut R,
        record: &TensorRecord,
        out: &mut Vec<u8>,
    ) -> Result<(), TorchBinError> {
        let (start, len) = self.tensor_byte_range(record)?;
        archive.seek(SeekFrom::Start(start as u64))?;
        out.clear();
        if len > MAX_TENSOR_BYTES {
            return Err(TorchBinError::Unsupported(format!(
                "{} tensor is {len} bytes (limit {MAX_TENSOR_BYTES})",
                record.name
            )));
        }
        out.try_reserve_exact(len)
            .map_err(|_| TorchBinError::Tensor(format!("{}: allocation failed", record.name)))?;
        out.resize(len, 0);
        archive.read_exact(out)?;
        Ok(())
    }

    pub fn read_tensor<R: Read + Seek>(
        &self,
        archive: &mut R,
        record: &TensorRecord,
    ) -> Result<Vec<u8>, TorchBinError> {
        let mut out = Vec::new();
        self.read_tensor_into(archive, record, &mut out)?;
        Ok(out)
    }

    /// Absolute archive byte offset of a contiguous tensor's first byte —
    /// the sort key for sequential-read upload ordering.
    pub fn archive_offset(&self, record: &TensorRecord) -> Result<usize, TorchBinError> {
        self.tensor_byte_range(record).map(|(offset, _)| offset)
    }

    fn exact_storage_entry(&self, record: &TensorRecord) -> Result<ZipEntry, TorchBinError> {
        if record.storage_key.len() > MAX_ENTRY_NAME_BYTES
            || record.storage_key.is_empty()
            || record.storage_key.contains('/')
            || record.storage_key.contains('\\')
            || record.storage_key == "."
            || record.storage_key == ".."
        {
            return Err(TorchBinError::Tensor(format!(
                "{} has an invalid bounded storage key",
                record.name
            )));
        }
        let storage_name = if self.archive_root.is_empty() {
            format!("data/{}", record.storage_key)
        } else {
            format!("{}/data/{}", self.archive_root, record.storage_key)
        };
        let entry = self.entries.get(&storage_name).copied().ok_or_else(|| {
            TorchBinError::Tensor(format!(
                "{}: storage {} is not present at exact data.pkl root {}",
                record.name, record.storage_key, self.archive_root
            ))
        })?;
        let storage_bytes = record
            .storage_numel
            .checked_mul(record.dtype.elem_size())
            .ok_or_else(|| {
                TorchBinError::Tensor(format!("{}: storage length overflow", record.name))
            })?;
        if storage_bytes != entry.size {
            return Err(TorchBinError::Tensor(format!(
                "{}: storage metadata is {storage_bytes} bytes but archive entry is {}",
                record.name, entry.size
            )));
        }
        Ok(entry)
    }

    fn tensor_byte_range(&self, record: &TensorRecord) -> Result<(usize, usize), TorchBinError> {
        if record.name.len() > MAX_PICKLE_STRING_BYTES
            || record.storage_key.len() > MAX_ENTRY_NAME_BYTES
            || record.storage_key.is_empty()
            || record.storage_key.contains('/')
            || record.storage_key.contains('\\')
            || record.storage_key == "."
            || record.storage_key == ".."
            || record.shape.len() > MAX_TENSOR_RANK
        {
            return Err(TorchBinError::Tensor(format!(
                "{} name, storage key, or rank exceeds bounded limits",
                record.name
            )));
        }
        if !record.is_contiguous() {
            return Err(TorchBinError::Tensor(format!(
                "{}: non-contiguous stride {:?} for shape {:?}",
                record.name, record.stride, record.shape
            )));
        }
        if record.shape.len() != record.stride.len() {
            return Err(TorchBinError::Tensor(format!(
                "{}: shape rank {} differs from stride rank {}",
                record.name,
                record.shape.len(),
                record.stride.len()
            )));
        }
        let computed_numel = record.shape.iter().try_fold(1usize, |product, dimension| {
            product.checked_mul(*dimension).ok_or_else(|| {
                TorchBinError::Tensor(format!("{}: shape product overflow", record.name))
            })
        })?;
        if computed_numel != record.numel {
            return Err(TorchBinError::Tensor(format!(
                "{}: numel does not match shape",
                record.name
            )));
        }
        let entry = self.exact_storage_entry(record)?;
        let elem = record.dtype.elem_size();
        let byte_offset = record
            .storage_offset
            .checked_mul(elem)
            .ok_or_else(|| TorchBinError::Tensor(format!("{}: storage offset overflow", record.name)))?;
        let len = record
            .numel
            .checked_mul(elem)
            .ok_or_else(|| TorchBinError::Tensor(format!("{}: tensor length overflow", record.name)))?;
        if len > MAX_TENSOR_BYTES {
            return Err(TorchBinError::Unsupported(format!(
                "{} tensor is {len} bytes (limit {MAX_TENSOR_BYTES})",
                record.name
            )));
        }
        let storage_end = byte_offset
            .checked_add(len)
            .ok_or_else(|| TorchBinError::Tensor(format!("{}: tensor range overflow", record.name)))?;
        if storage_end > entry.size {
            return Err(TorchBinError::Tensor(format!(
                "{}: byte range outside storage ({} + {} > {})",
                record.name, byte_offset, len, entry.size
            )));
        }
        let start = entry
            .data_offset
            .checked_add(byte_offset)
            .ok_or_else(|| TorchBinError::Tensor(format!("{}: archive offset overflow", record.name)))?;
        Ok((start, len))
    }
}

// ---------------------------------------------------------------------------
// ZIP (classic, STORED only)
// ---------------------------------------------------------------------------

fn checked_end(start: usize, len: usize, limit: usize, what: &str) -> Result<usize, TorchBinError> {
    let end = start
        .checked_add(len)
        .ok_or_else(|| TorchBinError::Zip(format!("{what} offset overflow")))?;
    if end > limit {
        return Err(TorchBinError::Zip(format!("{what} out of range")));
    }
    Ok(end)
}

fn read_u16(bytes: &[u8], at: usize, what: &str) -> Result<usize, TorchBinError> {
    let end = checked_end(at, 2, bytes.len(), what)?;
    let raw: [u8; 2] = bytes[at..end]
        .try_into()
        .map_err(|_| TorchBinError::Zip(format!("truncated {what}")))?;
    Ok(u16::from_le_bytes(raw) as usize)
}

fn read_u32(bytes: &[u8], at: usize, what: &str) -> Result<usize, TorchBinError> {
    let end = checked_end(at, 4, bytes.len(), what)?;
    let raw: [u8; 4] = bytes[at..end]
        .try_into()
        .map_err(|_| TorchBinError::Zip(format!("truncated {what}")))?;
    Ok(u32::from_le_bytes(raw) as usize)
}

#[derive(Debug)]
struct CentralRecord {
    name: String,
    flags: usize,
    method: usize,
    size: usize,
    local_offset: usize,
}

fn is_absolute_zip_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    matches!(bytes.first(), Some(b'/') | Some(b'\\'))
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

fn parse_central(
    central: &[u8],
    entry_count: usize,
    absolute_offset: usize,
) -> Result<Vec<CentralRecord>, TorchBinError> {
    const CENTRAL_SIG: usize = 0x0201_4b50;
    let mut records = Vec::new();
    records
        .try_reserve_exact(entry_count)
        .map_err(|_| TorchBinError::Zip("central metadata allocation failed".to_string()))?;
    let mut at = 0usize;
    for _ in 0..entry_count {
        let fixed_end = checked_end(at, 46, central.len(), "central directory entry")?;
        if read_u32(central, at, "central signature")? != CENTRAL_SIG {
            return Err(TorchBinError::Zip(format!(
                "bad central directory entry at {}",
                absolute_offset.saturating_add(at)
            )));
        }
        let flags = read_u16(central, at + 8, "central flags")?;
        let method = read_u16(central, at + 10, "central method")?;
        let comp_size = read_u32(central, at + 20, "central compressed size")?;
        let uncomp_size = read_u32(central, at + 24, "central uncompressed size")?;
        let name_len = read_u16(central, at + 28, "central name length")?;
        let extra_len = read_u16(central, at + 30, "central extra length")?;
        let comment_len = read_u16(central, at + 32, "central comment length")?;
        let local_offset = read_u32(central, at + 42, "central local offset")?;
        if name_len == 0 || name_len > MAX_ENTRY_NAME_BYTES {
            return Err(TorchBinError::Unsupported(format!(
                "zip entry name length {name_len} outside 1..={MAX_ENTRY_NAME_BYTES}"
            )));
        }
        let record_len = 46usize
            .checked_add(name_len)
            .and_then(|value| value.checked_add(extra_len))
            .and_then(|value| value.checked_add(comment_len))
            .ok_or_else(|| TorchBinError::Zip("central record length overflow".to_string()))?;
        let record_end = checked_end(at, record_len, central.len(), "central record")?;
        let name_end = checked_end(fixed_end, name_len, record_end, "central entry name")?;
        let name = std::str::from_utf8(&central[fixed_end..name_end])
            .map_err(|_| TorchBinError::Zip("non-utf8 entry name".to_string()))?
            .to_string();
        if is_absolute_zip_name(&name) {
            return Err(TorchBinError::Unsupported(format!(
                "absolute ZIP entry name is forbidden: {name}"
            )));
        }
        if comp_size == 0xffff_ffff || uncomp_size == 0xffff_ffff || local_offset == 0xffff_ffff {
            return Err(TorchBinError::Unsupported(format!("zip64 sizes on entry {name}")));
        }
        if method != 0 {
            return Err(TorchBinError::Unsupported(format!(
                "compressed entry {name} (method {method}); torch checkpoints use STORED"
            )));
        }
        if comp_size != uncomp_size {
            return Err(TorchBinError::Zip(format!(
                "stored entry {name} with mismatched sizes"
            )));
        }
        if flags & !(0x0008 | 0x0800) != 0 {
            return Err(TorchBinError::Unsupported(format!(
                "entry {name} uses unsupported ZIP flags 0x{flags:04x}"
            )));
        }
        records.push(CentralRecord {
            name,
            flags,
            method,
            size: comp_size,
            local_offset,
        });
        at = record_end;
    }
    if at != central.len() {
        return Err(TorchBinError::Zip(format!(
            "central directory has {} trailing bytes",
            central.len() - at
        )));
    }
    Ok(records)
}

fn eocd_fields(
    bytes: &[u8],
    archive_origin: usize,
) -> Result<(usize, usize, usize), TorchBinError> {
    const EOCD_SIG: usize = 0x0605_4b50;
    if bytes.len() < 22 {
        return Err(TorchBinError::Zip("archive shorter than EOCD".to_string()));
    }
    let scan_from = bytes.len().saturating_sub(22 + 65_535);
    let mut found = None;
    for at in (scan_from..=bytes.len() - 22).rev() {
        if read_u32(bytes, at, "EOCD signature")? != EOCD_SIG {
            continue;
        }
        let comment_len = read_u16(bytes, at + 20, "EOCD comment length")?;
        if at
            .checked_add(22)
            .and_then(|value| value.checked_add(comment_len))
            == Some(bytes.len())
        {
            found = Some(at);
            break;
        }
    }
    let eocd = found.ok_or_else(|| TorchBinError::Zip("no end-of-central-directory".to_string()))?;
    if read_u16(bytes, eocd + 4, "EOCD disk")? != 0
        || read_u16(bytes, eocd + 6, "EOCD central disk")? != 0
    {
        return Err(TorchBinError::Unsupported("multi-disk zip archive".to_string()));
    }
    let entries_on_disk = read_u16(bytes, eocd + 8, "EOCD disk entries")?;
    let entry_count = read_u16(bytes, eocd + 10, "EOCD entries")?;
    let central_size = read_u32(bytes, eocd + 12, "EOCD central size")?;
    let central_offset = read_u32(bytes, eocd + 16, "EOCD central offset")?;
    if entries_on_disk == 0xffff
        || entry_count == 0xffff
        || central_size == 0xffff_ffff
        || central_offset == 0xffff_ffff
    {
        return Err(TorchBinError::Unsupported("zip64 archive".to_string()));
    }
    if entries_on_disk != entry_count {
        return Err(TorchBinError::Unsupported("multi-disk zip entries".to_string()));
    }
    if entry_count > MAX_ZIP_ENTRIES {
        return Err(TorchBinError::Unsupported(format!(
            "ZIP has {entry_count} entries (limit {MAX_ZIP_ENTRIES})"
        )));
    }
    if central_size > MAX_CENTRAL_DIRECTORY_BYTES {
        return Err(TorchBinError::Unsupported(format!(
            "central directory is {central_size} bytes (limit {MAX_CENTRAL_DIRECTORY_BYTES})"
        )));
    }
    let central_end = central_offset
        .checked_add(central_size)
        .ok_or_else(|| TorchBinError::Zip("central directory offset overflow".to_string()))?;
    let eocd_absolute = archive_origin
        .checked_add(eocd)
        .ok_or_else(|| TorchBinError::Zip("EOCD absolute offset overflow".to_string()))?;
    if central_end > eocd_absolute {
        return Err(TorchBinError::Zip("central directory overlaps EOCD".to_string()));
    }
    Ok((entry_count, central_offset, central_size))
}

fn validate_local_memory(
    bytes: &[u8],
    record: &CentralRecord,
    central_offset: usize,
) -> Result<ZipEntry, TorchBinError> {
    const LOCAL_SIG: usize = 0x0403_4b50;
    let fixed_end = checked_end(record.local_offset, 30, bytes.len(), "local header")?;
    if read_u32(bytes, record.local_offset, "local signature")? != LOCAL_SIG {
        return Err(TorchBinError::Zip(format!("bad local header for {}", record.name)));
    }
    let flags = read_u16(bytes, record.local_offset + 6, "local flags")?;
    let method = read_u16(bytes, record.local_offset + 8, "local method")?;
    let local_comp = read_u32(bytes, record.local_offset + 18, "local compressed size")?;
    let local_uncomp = read_u32(bytes, record.local_offset + 22, "local uncompressed size")?;
    let name_len = read_u16(bytes, record.local_offset + 26, "local name length")?;
    let extra_len = read_u16(bytes, record.local_offset + 28, "local extra length")?;
    if flags != record.flags || method != record.method {
        return Err(TorchBinError::Zip(format!(
            "local/central flags or method mismatch for {}",
            record.name
        )));
    }
    if flags & 0x0008 == 0 {
        if local_comp != record.size || local_uncomp != record.size {
            return Err(TorchBinError::Zip(format!(
                "local/central size mismatch for {}",
                record.name
            )));
        }
    } else if !((local_comp == 0 || local_comp == record.size)
        && (local_uncomp == 0 || local_uncomp == record.size))
    {
        return Err(TorchBinError::Zip(format!(
            "invalid data-descriptor sizes for {}",
            record.name
        )));
    }
    if name_len == 0 || name_len > MAX_ENTRY_NAME_BYTES {
        return Err(TorchBinError::Unsupported(format!(
            "local entry name length {name_len} outside 1..={MAX_ENTRY_NAME_BYTES}"
        )));
    }
    let name_end = checked_end(fixed_end, name_len, bytes.len(), "local entry name")?;
    if bytes.get(fixed_end..name_end) != Some(record.name.as_bytes()) {
        return Err(TorchBinError::Zip(format!(
            "local/central name mismatch for {}",
            record.name
        )));
    }
    let data_offset = name_end
        .checked_add(extra_len)
        .ok_or_else(|| TorchBinError::Zip(format!("entry offset overflow for {}", record.name)))?;
    let data_end = checked_end(data_offset, record.size, bytes.len(), "entry payload")?;
    if data_end > central_offset {
        return Err(TorchBinError::Zip(format!(
            "entry {} overlaps central directory",
            record.name
        )));
    }
    Ok(ZipEntry {
        data_offset,
        size: record.size,
    })
}

fn parse_zip(bytes: &[u8]) -> Result<HashMap<String, ZipEntry>, TorchBinError> {
    let (entry_count, central_offset, central_size) = eocd_fields(bytes, 0)?;
    let central_end = checked_end(
        central_offset,
        central_size,
        bytes.len(),
        "central directory",
    )?;
    let records = parse_central(
        &bytes[central_offset..central_end],
        entry_count,
        central_offset,
    )?;
    let mut entries = HashMap::new();
    entries
        .try_reserve(entry_count)
        .map_err(|_| TorchBinError::Zip("ZIP entry map allocation failed".to_string()))?;
    for record in records {
        let entry = validate_local_memory(bytes, &record, central_offset)?;
        if entries.insert(record.name.clone(), entry).is_some() {
            return Err(TorchBinError::Zip(format!("duplicate entry {}", record.name)));
        }
    }
    Ok(entries)
}

/// Parse only the ZIP metadata from a seekable archive. Unlike `parse_zip`,
/// this reads a bounded tail, the central directory, and 30 bytes per local
/// header; storage payloads remain untouched until their tensor is uploaded.
fn parse_zip_from<R: Read + Seek>(archive: &mut R) -> Result<HashMap<String, ZipEntry>, TorchBinError> {
    const LOCAL_SIG: u32 = 0x0403_4b50;
    const MAX_EOCD_TAIL: usize = 22 + 65_535;

    let file_len_u64 = archive.seek(SeekFrom::End(0))?;
    let file_len = usize::try_from(file_len_u64)
        .map_err(|_| TorchBinError::Unsupported("archive exceeds address space".to_string()))?;
    if file_len < 22 {
        return Err(TorchBinError::Zip("archive shorter than EOCD".to_string()));
    }
    let tail_len = file_len.min(MAX_EOCD_TAIL);
    archive.seek(SeekFrom::Start((file_len - tail_len) as u64))?;
    let mut tail = Vec::new();
    tail.try_reserve_exact(tail_len)
        .map_err(|_| TorchBinError::Zip("EOCD tail allocation failed".to_string()))?;
    tail.resize(tail_len, 0);
    archive.read_exact(&mut tail)?;
    let tail_origin = file_len - tail_len;
    let (entry_count, central_offset, central_size) = eocd_fields(&tail, tail_origin)?;
    let central_end = central_offset
        .checked_add(central_size)
        .ok_or_else(|| TorchBinError::Zip("central directory offset overflow".to_string()))?;
    if central_end > file_len - 22 {
        return Err(TorchBinError::Zip("central directory outside archive".to_string()));
    }

    archive.seek(SeekFrom::Start(central_offset as u64))?;
    let mut central = Vec::new();
    central
        .try_reserve_exact(central_size)
        .map_err(|_| TorchBinError::Zip("central directory allocation failed".to_string()))?;
    central.resize(central_size, 0);
    archive.read_exact(&mut central)?;
    let metadata = parse_central(&central, entry_count, central_offset)?;

    let mut entries = HashMap::with_capacity(metadata.len());
    for record in metadata {
        let local_end = record
            .local_offset
            .checked_add(30)
            .ok_or_else(|| TorchBinError::Zip(format!("local header overflow for {}", record.name)))?;
        if local_end > file_len {
            return Err(TorchBinError::Zip(format!("bad local header for {}", record.name)));
        }
        archive.seek(SeekFrom::Start(record.local_offset as u64))?;
        let mut local = [0u8; 30];
        archive.read_exact(&mut local)?;
        if read_u32(&local, 0, "local signature")? as u32 != LOCAL_SIG {
            return Err(TorchBinError::Zip(format!("bad local header for {}", record.name)));
        }
        let local_flags = read_u16(&local, 6, "local flags")?;
        let local_method = read_u16(&local, 8, "local method")?;
        let local_comp = read_u32(&local, 18, "local compressed size")?;
        let local_uncomp = read_u32(&local, 22, "local uncompressed size")?;
        let local_name_len = read_u16(&local, 26, "local name length")?;
        let local_extra_len = read_u16(&local, 28, "local extra length")?;
        if local_flags != record.flags || local_method != record.method {
            return Err(TorchBinError::Zip(format!(
                "local/central flags or method mismatch for {}",
                record.name
            )));
        }
        if local_flags & 0x0008 == 0 {
            if local_comp != record.size || local_uncomp != record.size {
                return Err(TorchBinError::Zip(format!(
                    "local/central size mismatch for {}",
                    record.name
                )));
            }
        } else if !((local_comp == 0 || local_comp == record.size)
            && (local_uncomp == 0 || local_uncomp == record.size))
        {
            return Err(TorchBinError::Zip(format!(
                "invalid data-descriptor sizes for {}",
                record.name
            )));
        }
        if local_name_len == 0 || local_name_len > MAX_ENTRY_NAME_BYTES {
            return Err(TorchBinError::Unsupported(format!(
                "local entry name length {local_name_len} outside 1..={MAX_ENTRY_NAME_BYTES}"
            )));
        }
        let name_offset = record.local_offset
            .checked_add(30)
            .ok_or_else(|| TorchBinError::Zip(format!("entry offset overflow for {}", record.name)))?;
        let name_end = name_offset
            .checked_add(local_name_len)
            .ok_or_else(|| TorchBinError::Zip(format!("entry name overflow for {}", record.name)))?;
        if name_end > file_len {
            return Err(TorchBinError::Zip(format!("entry name out of range for {}", record.name)));
        }
        archive.seek(SeekFrom::Start(name_offset as u64))?;
        let mut local_name = Vec::new();
        local_name
            .try_reserve_exact(local_name_len)
            .map_err(|_| TorchBinError::Zip("local name allocation failed".to_string()))?;
        local_name.resize(local_name_len, 0);
        archive.read_exact(&mut local_name)?;
        if local_name != record.name.as_bytes() {
            return Err(TorchBinError::Zip(format!(
                "local/central name mismatch for {}",
                record.name
            )));
        }
        let data_offset = name_end
            .checked_add(local_extra_len)
            .ok_or_else(|| TorchBinError::Zip(format!("entry offset overflow for {}", record.name)))?;
        if data_offset
            .checked_add(record.size)
            .is_none_or(|end| end > central_offset)
        {
            return Err(TorchBinError::Zip(format!("entry {} data out of range", record.name)));
        }
        if entries
            .insert(
                record.name.clone(),
                ZipEntry {
                    data_offset,
                    size: record.size,
                },
            )
            .is_some()
        {
            return Err(TorchBinError::Zip(format!("duplicate entry {}", record.name)));
        }
    }
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Pickle subset VM
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
// The subset VM must preserve ignored pickle values on the stack even when
// their payload is not semantically inspected by the tensor-index reader.
#[allow(dead_code)]
enum Value {
    None,
    Bool(bool),
    Int(i64),
    Str(String),
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Dict(Vec<(Value, Value)>),
    Global(String, String),
    Persistent(Box<Value>),
    Tensor(TensorRecord),
    Opaque(&'static str),
    Mark,
}

#[derive(Clone, Copy, Debug, Default)]
struct ValueCost {
    nodes: usize,
    heap_bytes: usize,
    max_depth: usize,
}

impl Value {
    fn bounded_cost(&self) -> Result<ValueCost, TorchBinError> {
        const MAX_VALUE_DEPTH: usize = 64;
        fn visit(
            value: &Value,
            depth: usize,
            cost: &mut ValueCost,
        ) -> Result<(), TorchBinError> {
            if depth > MAX_VALUE_DEPTH {
                return Err(TorchBinError::Pickle(
                    "memo value nesting exceeds bounded depth".to_string(),
                ));
            }
            cost.nodes = cost.nodes.checked_add(1).ok_or_else(|| {
                TorchBinError::Pickle("memo clone cost overflow".to_string())
            })?;
            if cost.nodes > MAX_PICKLE_CLONE_NODES {
                return Err(TorchBinError::Pickle(
                    "memo value exceeds bounded clone size".to_string(),
                ));
            }
            cost.max_depth = cost.max_depth.max(depth);
            cost.heap_bytes = cost
                .heap_bytes
                .checked_add(std::mem::size_of::<Value>())
                .ok_or_else(|| TorchBinError::Pickle("memo heap cost overflow".to_string()))?;
            match value {
                Value::Tuple(items) | Value::List(items) => {
                    for item in items {
                        visit(item, depth + 1, cost)?;
                    }
                }
                Value::Dict(items) => {
                    for (key, value) in items {
                        visit(key, depth + 1, cost)?;
                        visit(value, depth + 1, cost)?;
                    }
                }
                Value::Persistent(value) => visit(value, depth + 1, cost)?,
                Value::Str(value) => {
                    cost.heap_bytes = cost.heap_bytes.checked_add(value.len()).ok_or_else(|| {
                        TorchBinError::Pickle("memo string cost overflow".to_string())
                    })?;
                }
                Value::Global(module, name) => {
                    cost.heap_bytes = cost
                        .heap_bytes
                        .checked_add(module.len())
                        .and_then(|bytes| bytes.checked_add(name.len()))
                        .ok_or_else(|| {
                            TorchBinError::Pickle("memo global cost overflow".to_string())
                        })?;
                }
                Value::Tensor(record) => {
                    let vector_bytes = record
                        .shape
                        .len()
                        .checked_add(record.stride.len())
                        .and_then(|items| items.checked_mul(std::mem::size_of::<usize>()))
                        .ok_or_else(|| {
                            TorchBinError::Pickle("memo tensor vector cost overflow".to_string())
                        })?;
                    cost.heap_bytes = cost
                        .heap_bytes
                        .checked_add(record.name.len())
                        .and_then(|bytes| bytes.checked_add(record.storage_key.len()))
                        .and_then(|bytes| bytes.checked_add(vector_bytes))
                        .ok_or_else(|| {
                            TorchBinError::Pickle("memo tensor cost overflow".to_string())
                        })?;
                }
                _ => {}
            }
            Ok(())
        }

        let mut cost = ValueCost::default();
        visit(self, 0, &mut cost)?;
        Ok(cost)
    }

    fn bounded_depth(&self) -> Result<usize, TorchBinError> {
        self.bounded_cost().map(|cost| cost.max_depth)
    }
}

fn charge_pickle_heap(
    heap_bytes: &mut usize,
    amount: usize,
    pos: usize,
    what: &str,
) -> Result<(), TorchBinError> {
    let next = heap_bytes
        .checked_add(amount)
        .ok_or_else(|| pickle_err(pos, "pickle heap allocation budget overflow"))?;
    if next > MAX_PICKLE_HEAP_BYTES {
        return Err(pickle_err(
            pos,
            &format!("pickle heap allocation budget exceeded while {what}"),
        ));
    }
    *heap_bytes = next;
    Ok(())
}

fn charge_value_clone(
    value: &Value,
    clone_nodes: &mut usize,
    heap_bytes: &mut usize,
    pos: usize,
) -> Result<(), TorchBinError> {
    let cost = value.bounded_cost()?;
    let next_nodes = clone_nodes
        .checked_add(cost.nodes)
        .ok_or_else(|| pickle_err(pos, "memo clone budget overflow"))?;
    if next_nodes > MAX_PICKLE_CLONE_NODES {
        return Err(pickle_err(pos, "memo clone budget exceeded"));
    }
    // Preflight the byte budget before mutating either counter or cloning.
    let next_heap = heap_bytes
        .checked_add(cost.heap_bytes)
        .ok_or_else(|| pickle_err(pos, "pickle heap allocation budget overflow"))?;
    if next_heap > MAX_PICKLE_HEAP_BYTES {
        return Err(pickle_err(pos, "pickle heap clone budget exceeded"));
    }
    *clone_nodes = next_nodes;
    *heap_bytes = next_heap;
    Ok(())
}

fn pickle_err(pos: usize, msg: &str) -> TorchBinError {
    TorchBinError::Pickle(format!("{msg} at byte {pos}"))
}

fn pickle_u32(pkl: &[u8], at: usize) -> Result<usize, TorchBinError> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| pickle_err(at, "integer offset overflow"))?;
    let bytes: [u8; 4] = pkl
        .get(at..end)
        .ok_or_else(|| pickle_err(at, "truncated integer"))?
        .try_into()
        .map_err(|_| pickle_err(at, "truncated integer"))?;
    Ok(u32::from_le_bytes(bytes) as usize)
}

fn parse_state_dict(pkl: &[u8]) -> Result<Vec<TensorRecord>, TorchBinError> {
    let mut stack: Vec<Value> = Vec::new();
    let mut memo: HashMap<usize, Value> = HashMap::new();
    let mut memo_seq = 0usize;
    let mut i = 0usize;
    let mut clone_nodes = 0usize;
    let mut heap_bytes = 0usize;

    macro_rules! need {
        ($n:expr) => {
            if i.checked_add($n).is_none_or(|end| end > pkl.len()) {
                return Err(pickle_err(i, "truncated stream"));
            }
        };
    }
    let pop = |stack: &mut Vec<Value>, i: usize| -> Result<Value, TorchBinError> {
        stack.pop().ok_or_else(|| pickle_err(i, "stack underflow"))
    };

    while i < pkl.len() {
        let op = pkl[i];
        i += 1;
        match op {
            0x80 => {
                need!(1);
                i += 1; // PROTO version
            }
            b'.' => {
                let top = pop(&mut stack, i)?;
                let mut tensors = Vec::new();
                let mut names = std::collections::HashSet::new();
                if let Value::Dict(items) = top {
                    for (key, value) in items {
                        if let (Value::Str(name), Value::Tensor(mut record)) = (key, value) {
                            let uniqueness_bytes = name
                                .len()
                                .checked_add(std::mem::size_of::<String>())
                                .ok_or_else(|| pickle_err(i, "tensor-name clone size overflow"))?;
                            charge_pickle_heap(
                                &mut heap_bytes,
                                uniqueness_bytes,
                                i,
                                "cloning a tensor name into the uniqueness set",
                            )?;
                            if !names.insert(name.clone()) {
                                return Err(pickle_err(i, "duplicate tensor name"));
                            }
                            if tensors.len() >= MAX_TENSORS {
                                return Err(pickle_err(i, "tensor count exceeds limit"));
                            }
                            record.name = name;
                            charge_pickle_heap(
                                &mut heap_bytes,
                                std::mem::size_of::<TensorRecord>(),
                                i,
                                "growing the tensor index",
                            )?;
                            tensors.push(record);
                        }
                    }
                } else {
                    return Err(pickle_err(i, "top of stack is not a dict at STOP"));
                }
                return Ok(tensors);
            }
            b'}' => stack.push(Value::Dict(Vec::new())),
            b']' => stack.push(Value::List(Vec::new())),
            b')' => stack.push(Value::Tuple(Vec::new())),
            b'(' => stack.push(Value::Mark),
            b'N' => stack.push(Value::None),
            0x88 => stack.push(Value::Bool(true)),
            0x89 => stack.push(Value::Bool(false)),
            b'K' => {
                need!(1);
                stack.push(Value::Int(pkl[i] as i64));
                i += 1;
            }
            b'M' => {
                need!(2);
                stack.push(Value::Int(u16::from_le_bytes([pkl[i], pkl[i + 1]]) as i64));
                i += 2;
            }
            b'J' => {
                need!(4);
                stack.push(Value::Int(
                    i32::from_le_bytes([pkl[i], pkl[i + 1], pkl[i + 2], pkl[i + 3]]) as i64,
                ));
                i += 4;
            }
            0x8a => {
                // LONG1
                need!(1);
                let n = pkl[i] as usize;
                i += 1;
                if n > 8 {
                    return Err(pickle_err(i - 1, "LONG1 wider than signed 64 bits"));
                }
                need!(n);
                let mut value = 0i64;
                for (k, byte) in pkl[i..i + n].iter().enumerate() {
                    value |= (*byte as i64) << (8 * k);
                }
                if n > 0 && pkl[i + n - 1] & 0x80 != 0 && n < 8 {
                    value -= 1i64 << (8 * n);
                }
                stack.push(Value::Int(value));
                i += n;
            }
            b'X' => {
                need!(4);
                let n = pickle_u32(pkl, i)?;
                i += 4;
                if n > MAX_PICKLE_STRING_BYTES {
                    return Err(pickle_err(i, "string exceeds bounded length"));
                }
                need!(n);
                let s = std::str::from_utf8(&pkl[i..i + n])
                    .map_err(|_| pickle_err(i, "non-utf8 string"))?;
                charge_pickle_heap(&mut heap_bytes, n, i, "allocating a string")?;
                stack.push(Value::Str(s.to_string()));
                i += n;
            }
            0x8c => {
                // SHORT_BINUNICODE
                need!(1);
                let n = pkl[i] as usize;
                i += 1;
                need!(n);
                let s = std::str::from_utf8(&pkl[i..i + n])
                    .map_err(|_| pickle_err(i, "non-utf8 string"))?;
                charge_pickle_heap(&mut heap_bytes, n, i, "allocating a short string")?;
                stack.push(Value::Str(s.to_string()));
                i += n;
            }
            b'c' => {
                // GLOBAL: two newline-terminated lines
                let start = i;
                let mut lines = Vec::new();
                for _ in 0..2 {
                    let end = pkl[i..]
                        .iter()
                        .position(|b| *b == b'\n')
                        .ok_or_else(|| pickle_err(start, "unterminated GLOBAL"))?;
                    if end > MAX_PICKLE_STRING_BYTES {
                        return Err(pickle_err(i, "GLOBAL field exceeds bounded length"));
                    }
                    charge_pickle_heap(
                        &mut heap_bytes,
                        end,
                        i,
                        "allocating a GLOBAL field",
                    )?;
                    lines.push(
                        std::str::from_utf8(&pkl[i..i + end])
                            .map_err(|_| pickle_err(i, "non-utf8 GLOBAL"))?
                            .to_string(),
                    );
                    i += end + 1;
                }
                let name = lines
                    .pop()
                    .ok_or_else(|| pickle_err(i, "GLOBAL name missing"))?;
                let module = lines
                    .pop()
                    .ok_or_else(|| pickle_err(i, "GLOBAL module missing"))?;
                stack.push(Value::Global(module, name));
            }
            0x93 => {
                // STACK_GLOBAL
                let name = pop(&mut stack, i)?;
                let module = pop(&mut stack, i)?;
                match (module, name) {
                    (Value::Str(m), Value::Str(n)) => stack.push(Value::Global(m, n)),
                    _ => return Err(pickle_err(i, "STACK_GLOBAL needs two strings")),
                }
            }
            b'q' => {
                need!(1);
                let key = pkl[i] as usize;
                i += 1;
                let top = stack.last().ok_or_else(|| pickle_err(i, "BINPUT on empty stack"))?;
                if memo.len() >= MAX_PICKLE_MEMO || memo.contains_key(&key) {
                    return Err(pickle_err(i, "memo limit or duplicate memo key"));
                }
                charge_value_clone(top, &mut clone_nodes, &mut heap_bytes, i)?;
                memo.insert(key, top.clone());
            }
            b'r' => {
                need!(4);
                let key = pickle_u32(pkl, i)?;
                i += 4;
                let top = stack.last().ok_or_else(|| pickle_err(i, "LONG_BINPUT on empty stack"))?;
                if memo.len() >= MAX_PICKLE_MEMO || memo.contains_key(&key) {
                    return Err(pickle_err(i, "memo limit or duplicate memo key"));
                }
                charge_value_clone(top, &mut clone_nodes, &mut heap_bytes, i)?;
                memo.insert(key, top.clone());
            }
            0x94 => {
                // MEMOIZE
                let top = stack.last().ok_or_else(|| pickle_err(i, "MEMOIZE on empty stack"))?;
                if memo.len() >= MAX_PICKLE_MEMO || memo.contains_key(&memo_seq) {
                    return Err(pickle_err(i, "memo limit or duplicate memo key"));
                }
                charge_value_clone(top, &mut clone_nodes, &mut heap_bytes, i)?;
                memo.insert(memo_seq, top.clone());
                memo_seq = memo_seq
                    .checked_add(1)
                    .ok_or_else(|| pickle_err(i, "memo sequence overflow"))?;
            }
            b'h' => {
                need!(1);
                let key = pkl[i] as usize;
                i += 1;
                let value = memo
                    .get(&key)
                    .ok_or_else(|| pickle_err(i, "BINGET of unknown memo"))?;
                charge_value_clone(value, &mut clone_nodes, &mut heap_bytes, i)?;
                stack.push(value.clone());
            }
            b'j' => {
                need!(4);
                let key = pickle_u32(pkl, i)?;
                i += 4;
                let value = memo
                    .get(&key)
                    .ok_or_else(|| pickle_err(i, "LONG_BINGET of unknown memo"))?;
                charge_value_clone(value, &mut clone_nodes, &mut heap_bytes, i)?;
                stack.push(value.clone());
            }
            0x85 | 0x86 | 0x87 => {
                let n = (op - 0x84) as usize;
                let allocation = n
                    .checked_mul(std::mem::size_of::<Value>())
                    .ok_or_else(|| pickle_err(i, "tuple allocation overflow"))?;
                charge_pickle_heap(&mut heap_bytes, allocation, i, "allocating a tuple")?;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(pop(&mut stack, i)?);
                }
                items.reverse();
                if items.len() > MAX_PICKLE_CONTAINER_ITEMS {
                    return Err(pickle_err(i, "tuple exceeds container limit"));
                }
                let value = Value::Tuple(items);
                value.bounded_depth()?;
                stack.push(value);
            }
            b't' => {
                let mut items = Vec::new();
                loop {
                    match pop(&mut stack, i)? {
                        Value::Mark => break,
                        value => {
                            charge_pickle_heap(
                                &mut heap_bytes,
                                std::mem::size_of::<Value>(),
                                i,
                                "growing a tuple",
                            )?;
                            items.push(value);
                        }
                    }
                }
                items.reverse();
                if items.len() > MAX_PICKLE_CONTAINER_ITEMS {
                    return Err(pickle_err(i, "tuple exceeds container limit"));
                }
                let value = Value::Tuple(items);
                value.bounded_depth()?;
                stack.push(value);
            }
            b'Q' => {
                let pid = pop(&mut stack, i)?;
                charge_pickle_heap(
                    &mut heap_bytes,
                    std::mem::size_of::<Value>(),
                    i,
                    "boxing a persistent id",
                )?;
                let value = Value::Persistent(Box::new(pid));
                value.bounded_depth()?;
                stack.push(value);
            }
            b'R' => {
                let args = pop(&mut stack, i)?;
                let callable = pop(&mut stack, i)?;
                let value = reduce(callable, args, i, &mut heap_bytes)?;
                value.bounded_depth()?;
                stack.push(value);
            }
            b'b' => {
                // BUILD: pop the state, keep the object as-is.
                let _state = pop(&mut stack, i)?;
            }
            b's' => {
                let value = pop(&mut stack, i)?;
                let key = pop(&mut stack, i)?;
                if key.bounded_depth()? >= 64 || value.bounded_depth()? >= 64 {
                    return Err(pickle_err(i, "dict value nesting exceeds limit"));
                }
                match stack.last_mut() {
                    Some(Value::Dict(items)) if items.len() < MAX_PICKLE_CONTAINER_ITEMS => {
                        charge_pickle_heap(
                            &mut heap_bytes,
                            std::mem::size_of::<(Value, Value)>(),
                            i,
                            "growing a dictionary",
                        )?;
                        items.push((key, value));
                    }
                    Some(Value::Dict(_)) => {
                        return Err(pickle_err(i, "dict exceeds container limit"));
                    }
                    _ => return Err(pickle_err(i, "SETITEM on non-dict")),
                }
            }
            b'u' => {
                let mut pairs = Vec::new();
                loop {
                    let value = pop(&mut stack, i)?;
                    if matches!(value, Value::Mark) {
                        break;
                    }
                    let key = pop(&mut stack, i)?;
                    if key.bounded_depth()? >= 64 || value.bounded_depth()? >= 64 {
                        return Err(pickle_err(i, "dict value nesting exceeds limit"));
                    }
                    charge_pickle_heap(
                        &mut heap_bytes,
                        std::mem::size_of::<(Value, Value)>(),
                        i,
                        "collecting dictionary entries",
                    )?;
                    pairs.push((key, value));
                }
                pairs.reverse();
                if pairs.len() > MAX_PICKLE_CONTAINER_ITEMS {
                    return Err(pickle_err(i, "SETITEMS exceeds container limit"));
                }
                match stack.last_mut() {
                    Some(Value::Dict(items))
                        if items
                            .len()
                            .checked_add(pairs.len())
                            .is_some_and(|total| total <= MAX_PICKLE_CONTAINER_ITEMS) =>
                    {
                        let allocation = pairs
                            .len()
                            .checked_mul(std::mem::size_of::<(Value, Value)>())
                            .ok_or_else(|| pickle_err(i, "dictionary allocation overflow"))?;
                        charge_pickle_heap(
                            &mut heap_bytes,
                            allocation,
                            i,
                            "extending a dictionary",
                        )?;
                        items.extend(pairs);
                    }
                    Some(Value::Dict(_)) => {
                        return Err(pickle_err(i, "dict exceeds container limit"));
                    }
                    _ => return Err(pickle_err(i, "SETITEMS on non-dict")),
                }
            }
            b'a' => {
                let value = pop(&mut stack, i)?;
                if value.bounded_depth()? >= 64 {
                    return Err(pickle_err(i, "list value nesting exceeds limit"));
                }
                match stack.last_mut() {
                    Some(Value::List(items)) if items.len() < MAX_PICKLE_CONTAINER_ITEMS => {
                        charge_pickle_heap(
                            &mut heap_bytes,
                            std::mem::size_of::<Value>(),
                            i,
                            "growing a list",
                        )?;
                        items.push(value);
                    }
                    Some(Value::List(_)) => {
                        return Err(pickle_err(i, "list exceeds container limit"));
                    }
                    _ => return Err(pickle_err(i, "APPEND on non-list")),
                }
            }
            b'e' => {
                let mut items = Vec::new();
                loop {
                    let value = pop(&mut stack, i)?;
                    if matches!(value, Value::Mark) {
                        break;
                    }
                    if value.bounded_depth()? >= 64 {
                        return Err(pickle_err(i, "list value nesting exceeds limit"));
                    }
                    charge_pickle_heap(
                        &mut heap_bytes,
                        std::mem::size_of::<Value>(),
                        i,
                        "collecting list entries",
                    )?;
                    items.push(value);
                }
                items.reverse();
                if items.len() > MAX_PICKLE_CONTAINER_ITEMS {
                    return Err(pickle_err(i, "APPENDS exceeds container limit"));
                }
                match stack.last_mut() {
                    Some(Value::List(list))
                        if list
                            .len()
                            .checked_add(items.len())
                            .is_some_and(|total| total <= MAX_PICKLE_CONTAINER_ITEMS) =>
                    {
                        let allocation = items
                            .len()
                            .checked_mul(std::mem::size_of::<Value>())
                            .ok_or_else(|| pickle_err(i, "list allocation overflow"))?;
                        charge_pickle_heap(
                            &mut heap_bytes,
                            allocation,
                            i,
                            "extending a list",
                        )?;
                        list.extend(items);
                    }
                    Some(Value::List(_)) => {
                        return Err(pickle_err(i, "list exceeds container limit"));
                    }
                    _ => return Err(pickle_err(i, "APPENDS on non-list")),
                }
            }
            other => {
                return Err(pickle_err(
                    i - 1,
                    &format!("unsupported pickle opcode 0x{other:02x}"),
                ));
            }
        }
        if stack.len() > MAX_PICKLE_STACK {
            return Err(pickle_err(i, "pickle stack exceeds limit"));
        }
    }
    Err(pickle_err(i, "stream ended without STOP"))
}

fn as_usize(value: &Value, i: usize, what: &str) -> Result<usize, TorchBinError> {
    match value {
        Value::Int(v) if *v >= 0 => Ok(*v as usize),
        _ => Err(pickle_err(i, &format!("{what}: expected non-negative int"))),
    }
}

fn reduce(
    callable: Value,
    args: Value,
    i: usize,
    heap_bytes: &mut usize,
) -> Result<Value, TorchBinError> {
    let Value::Tuple(args) = args else {
        return Err(pickle_err(i, "REDUCE args must be a tuple"));
    };
    match &callable {
        Value::Global(module, name)
            if module == "torch._utils" && name == "_rebuild_tensor_v2" =>
        {
            if args.len() < 4 {
                return Err(pickle_err(i, "_rebuild_tensor_v2 needs 4+ args"));
            }
            let Value::Persistent(pid) = &args[0] else {
                return Err(pickle_err(i, "_rebuild_tensor_v2 arg0 must be persistent storage"));
            };
            let Value::Tuple(pid) = pid.as_ref() else {
                return Err(pickle_err(i, "persistent id must be a tuple"));
            };
            // ("storage", Global(torch, XStorage), key, device, numel)
            if pid.len() < 5 {
                return Err(pickle_err(i, "storage persistent id needs 5 fields"));
            }
            let Value::Global(_, storage_class) = &pid[1] else {
                return Err(pickle_err(i, "storage class missing"));
            };
            let dtype = TorchDtype::from_storage_class(storage_class).ok_or_else(|| {
                TorchBinError::Unsupported(format!("storage class {storage_class}"))
            })?;
            let Value::Str(storage_key) = &pid[2] else {
                return Err(pickle_err(i, "storage key must be a string"));
            };
            let storage_numel = as_usize(&pid[4], i, "storage numel")?;
            let storage_offset = as_usize(&args[1], i, "storage_offset")?;
            let Value::Tuple(shape_values) = &args[2] else {
                return Err(pickle_err(i, "tensor size must be a tuple"));
            };
            let Value::Tuple(stride_values) = &args[3] else {
                return Err(pickle_err(i, "tensor stride must be a tuple"));
            };
            if shape_values.len() > MAX_TENSOR_RANK || stride_values.len() > MAX_TENSOR_RANK {
                return Err(pickle_err(i, "tensor rank exceeds limit"));
            }
            if shape_values.len() != stride_values.len() {
                return Err(pickle_err(i, "tensor shape/stride rank mismatch"));
            }
            let shape_bytes = shape_values
                .len()
                .checked_mul(std::mem::size_of::<usize>())
                .ok_or_else(|| pickle_err(i, "tensor shape allocation overflow"))?;
            charge_pickle_heap(heap_bytes, shape_bytes, i, "allocating a tensor shape")?;
            let mut shape = Vec::with_capacity(shape_values.len());
            for value in shape_values {
                shape.push(as_usize(value, i, "size")?);
            }
            let stride_bytes = stride_values
                .len()
                .checked_mul(std::mem::size_of::<usize>())
                .ok_or_else(|| pickle_err(i, "tensor stride allocation overflow"))?;
            charge_pickle_heap(heap_bytes, stride_bytes, i, "allocating a tensor stride")?;
            let mut stride = Vec::with_capacity(stride_values.len());
            for value in stride_values {
                stride.push(as_usize(value, i, "stride")?);
            }
            let numel = shape.iter().try_fold(1usize, |product, dim| {
                product
                    .checked_mul(*dim)
                    .ok_or_else(|| pickle_err(i, "tensor shape product overflow"))
            })?;
            let storage_end = storage_offset
                .checked_add(numel)
                .ok_or_else(|| pickle_err(i, "tensor storage range overflow"))?;
            if storage_end > storage_numel {
                return Err(pickle_err(i, "tensor exceeds its storage"));
            }
            numel
                .checked_mul(dtype.elem_size())
                .filter(|bytes| *bytes <= MAX_TENSOR_BYTES)
                .ok_or_else(|| pickle_err(i, "tensor byte length exceeds limit"))?;
            charge_pickle_heap(
                heap_bytes,
                storage_key.len(),
                i,
                "cloning a tensor storage key",
            )?;
            Ok(Value::Tensor(TensorRecord {
                name: String::new(),
                dtype,
                shape,
                stride,
                storage_key: storage_key.clone(),
                storage_numel,
                storage_offset,
                numel,
            }))
        }
        Value::Global(module, name) if module == "collections" && name == "OrderedDict" => {
            Ok(Value::Dict(Vec::new()))
        }
        Value::Global(..) => Ok(Value::Opaque("reduce")),
        _ => Err(pickle_err(i, "REDUCE of non-global")),
    }
}

fn data_pickle_root(name: &str) -> Option<&str> {
    if name == "data.pkl" {
        Some("")
    } else {
        name.strip_suffix("/data.pkl")
    }
}

/// Parse the archive and return the tensor index. `bytes` is the whole file.
pub fn read_index(bytes: &[u8]) -> Result<TorchBinIndex, TorchBinError> {
    let entries = parse_zip(bytes)?;
    let mut pickle_entries = entries
        .iter()
        .filter(|(name, _)| data_pickle_root(name).is_some());
    let (pkl_name, pkl_entry) = pickle_entries
        .next()
        .ok_or_else(|| TorchBinError::Zip("no data.pkl in archive".to_string()))?;
    if pickle_entries.next().is_some() {
        return Err(TorchBinError::Zip("multiple data.pkl entries in archive".to_string()));
    }
    if pkl_entry.size > MAX_PICKLE_BYTES {
        return Err(TorchBinError::Unsupported(format!(
            "data.pkl is {} bytes (limit {MAX_PICKLE_BYTES})",
            pkl_entry.size
        )));
    }
    let pkl_end = pkl_entry
        .data_offset
        .checked_add(pkl_entry.size)
        .ok_or_else(|| TorchBinError::Zip("data.pkl range overflow".to_string()))?;
    let pkl = bytes
        .get(pkl_entry.data_offset..pkl_end)
        .ok_or_else(|| TorchBinError::Zip("data.pkl outside archive".to_string()))?;
    let tensors = parse_state_dict(pkl)?;
    let archive_root = data_pickle_root(pkl_name)
        .ok_or_else(|| TorchBinError::Zip("selected data.pkl has no archive root".to_string()))?
        .to_string();
    let index = TorchBinIndex {
        tensors,
        entries,
        archive_root,
    };
    for record in &index.tensors {
        index.exact_storage_entry(record)?;
    }
    Ok(index)
}

/// Streaming counterpart of [`read_index`]. Only ZIP metadata and
/// `data.pkl` are read; the storage blobs stay on disk and are consumed one
/// tensor at a time through [`TorchBinIndex::read_tensor_into`].
pub fn read_index_from<R: Read + Seek>(archive: &mut R) -> Result<TorchBinIndex, TorchBinError> {
    let entries = parse_zip_from(archive)?;
    let mut pickle_entries = entries
        .iter()
        .filter(|(name, _)| data_pickle_root(name).is_some());
    let (pkl_name, pkl_entry) = pickle_entries
        .next()
        .map(|(name, entry)| (name.clone(), *entry))
        .ok_or_else(|| TorchBinError::Zip("no data.pkl in archive".to_string()))?;
    if pickle_entries.next().is_some() {
        return Err(TorchBinError::Zip("multiple data.pkl entries in archive".to_string()));
    }
    // A state-dict pickle is metadata, not a tensor payload. Bound it so a
    // corrupt archive cannot turn the streaming path into an unbounded read.
    if pkl_entry.size > MAX_PICKLE_BYTES {
        return Err(TorchBinError::Unsupported(format!(
            "data.pkl is {} bytes (limit {MAX_PICKLE_BYTES})",
            pkl_entry.size
        )));
    }
    archive.seek(SeekFrom::Start(pkl_entry.data_offset as u64))?;
    let mut pickle = Vec::new();
    pickle
        .try_reserve_exact(pkl_entry.size)
        .map_err(|_| TorchBinError::Pickle("data.pkl allocation failed".to_string()))?;
    pickle.resize(pkl_entry.size, 0);
    archive.read_exact(&mut pickle)?;
    let tensors = parse_state_dict(&pickle)?;
    let archive_root = data_pickle_root(&pkl_name)
        .ok_or_else(|| TorchBinError::Zip("selected data.pkl has no archive root".to_string()))?
        .to_string();
    let index = TorchBinIndex {
        tensors,
        entries,
        archive_root,
    };
    for record in &index.tensors {
        index.exact_storage_entry(record)?;
    }
    Ok(index)
}

/// Minimal torch-format archive builder shared by this crate's tests.
#[cfg(test)]
pub(crate) mod test_fixture {
    /// Builds a synthetic torch zip checkpoint in memory.
    pub(crate) struct FixtureWriter {
        pickle: Vec<u8>,
        storages: Vec<(String, Vec<u8>)>,
        memo: u8,
    }

    impl FixtureWriter {
        pub(crate) fn new() -> Self {
            let mut pickle = vec![0x80, 2, b'}', b'q', 0, b'('];
            pickle.pop(); // keep dict + memo, add MARK when finishing
            pickle.push(b'(');
            Self {
                pickle,
                storages: Vec::new(),
                memo: 1,
            }
        }

        fn put_str(&mut self, s: &str) {
            self.pickle.push(b'X');
            self.pickle
                .extend_from_slice(&(s.len() as u32).to_le_bytes());
            self.pickle.extend_from_slice(s.as_bytes());
            self.pickle.push(b'q');
            self.pickle.push(self.memo);
            self.memo += 1;
        }

        fn put_int(&mut self, v: usize) {
            self.pickle.push(b'J');
            self.pickle
                .extend_from_slice(&(v as i32).to_le_bytes());
        }

        fn put_global(&mut self, module: &str, name: &str) {
            self.pickle.push(b'c');
            self.pickle.extend_from_slice(module.as_bytes());
            self.pickle.push(b'\n');
            self.pickle.extend_from_slice(name.as_bytes());
            self.pickle.push(b'\n');
            self.pickle.push(b'q');
            self.pickle.push(self.memo);
            self.memo += 1;
        }

        #[allow(clippy::too_many_arguments)]
        pub(crate) fn add_tensor(
            &mut self,
            name: &str,
            storage_class: &str,
            key: &str,
            data: &[u8],
            elem: usize,
            shape: &[usize],
            stride: &[usize],
            offset: usize,
        ) {
            if !self.storages.iter().any(|(k, _)| k == key) {
                self.storages.push((key.to_string(), data.to_vec()));
            }
            self.put_str(name);
            // _rebuild_tensor_v2(
            self.put_global("torch._utils", "_rebuild_tensor_v2");
            self.pickle.push(b'(');
            // persistent id tuple: ("storage", Storage, key, "cpu", numel)
            self.pickle.push(b'(');
            self.put_str("storage");
            self.put_global("torch", storage_class);
            self.put_str(key);
            self.put_str("cpu");
            self.put_int(data.len() / elem);
            self.pickle.push(b't');
            self.pickle.push(b'Q');
            self.put_int(offset);
            self.pickle.push(b'(');
            for dim in shape {
                self.put_int(*dim);
            }
            self.pickle.push(b't');
            self.pickle.push(b'(');
            for s in stride {
                self.put_int(*s);
            }
            self.pickle.push(b't');
            self.pickle.push(0x89); // requires_grad = False
            self.put_global("collections", "OrderedDict");
            self.pickle.push(b')');
            self.pickle.push(b'R'); // OrderedDict()
            self.pickle.push(b't'); // args tuple (to the MARK opened above)
            self.pickle.push(b'R'); // _rebuild_tensor_v2(...)
        }

        pub(crate) fn finish(mut self, compress_marker: bool) -> Vec<u8> {
            self.pickle.push(b'u'); // SETITEMS to MARK
            self.pickle.push(b'.');
            let mut files: Vec<(String, Vec<u8>)> =
                vec![("archive/data.pkl".to_string(), self.pickle.clone())];
            for (key, data) in &self.storages {
                files.push((format!("archive/data/{key}"), data.clone()));
            }
            files.push(("archive/version".to_string(), b"3\n".to_vec()));
            build_zip(&files, compress_marker)
        }
    }

    pub(crate) fn build_zip(files: &[(String, Vec<u8>)], mark_compressed: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let local_offset = out.len();
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&[20, 0, 0, 0]); // version, flags
            let method: u16 = if mark_compressed { 8 } else { 0 };
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 4]); // time/date
            out.extend_from_slice(&[0; 4]); // crc (unchecked by the reader)
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 4]);
            central.extend_from_slice(&[0; 4]);
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 2]); // extra len
            central.extend_from_slice(&[0; 2]); // comment len
            central.extend_from_slice(&[0; 2]); // disk
            central.extend_from_slice(&[0; 2]); // internal attrs
            central.extend_from_slice(&[0; 4]); // external attrs
            central.extend_from_slice(&(local_offset as u32).to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let central_offset = out.len();
        out.extend_from_slice(&central);
        let central_size = out.len() - central_offset;
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central_size as u32).to_le_bytes());
        out.extend_from_slice(&(central_offset as u32).to_le_bytes());
        out.extend_from_slice(&[0; 2]);
        out
    }

    pub(crate) fn f32_bytes(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_fixture::{build_zip, f32_bytes, FixtureWriter};
    use super::*;

    #[test]
    fn roundtrip_two_tensors() {
        let mut fixture = FixtureWriter::new();
        let a: Vec<f32> = (0..6).map(|v| v as f32).collect();
        fixture.add_tensor("a.weight", "FloatStorage", "0", &f32_bytes(&a), 4, &[2, 3], &[3, 1], 0);
        let b_raw: Vec<u8> = vec![0, 0x3c, 0, 0x40, 0, 0x44, 0, 0x48]; // f16 1,2,4,8
        fixture.add_tensor("b.bias", "HalfStorage", "1", &b_raw, 2, &[4], &[1], 0);
        let archive = fixture.finish(false);

        let index = read_index(&archive).unwrap();
        assert_eq!(index.tensors.len(), 2);
        let a_rec = index.find("a.weight").unwrap();
        assert_eq!(a_rec.dtype, TorchDtype::F32);
        assert_eq!(a_rec.shape, vec![2, 3]);
        assert!(a_rec.is_contiguous());
        assert_eq!(index.tensor_bytes(&archive, a_rec).unwrap(), &f32_bytes(&a)[..]);
        let b_rec = index.find("b.bias").unwrap();
        assert_eq!(b_rec.dtype, TorchDtype::F16);
        assert_eq!(index.tensor_bytes(&archive, b_rec).unwrap(), &b_raw[..]);
    }

    #[test]
    fn streaming_index_and_tensor_reads_match_memory_path() {
        let mut fixture = FixtureWriter::new();
        let first = f32_bytes(&[1.25, -2.5, 4.0, 8.0]);
        let second = vec![0x00, 0x3c, 0x00, 0xc0];
        fixture.add_tensor("z.weight", "FloatStorage", "0", &first, 4, &[2, 2], &[2, 1], 0);
        fixture.add_tensor("a.bias", "HalfStorage", "1", &second, 2, &[2], &[1], 0);
        let archive = fixture.finish(false);

        let memory = read_index(&archive).unwrap();
        let mut cursor = std::io::Cursor::new(archive.clone());
        let streamed = read_index_from(&mut cursor).unwrap();
        assert_eq!(streamed.tensors.len(), memory.tensors.len());
        for expected in &memory.tensors {
            let actual = streamed.find(&expected.name).unwrap();
            assert_eq!(actual.dtype, expected.dtype);
            assert_eq!(actual.shape, expected.shape);
            assert_eq!(actual.stride, expected.stride);
            let mut bytes = Vec::new();
            streamed
                .read_tensor_into(&mut cursor, actual, &mut bytes)
                .unwrap();
            assert_eq!(bytes, memory.tensor_bytes(&archive, expected).unwrap());
        }
    }

    #[test]
    fn shared_storage_with_offsets() {
        let mut fixture = FixtureWriter::new();
        let data: Vec<f32> = (0..8).map(|v| v as f32).collect();
        let bytes = f32_bytes(&data);
        fixture.add_tensor("first", "FloatStorage", "0", &bytes, 4, &[4], &[1], 0);
        fixture.add_tensor("second", "FloatStorage", "0", &bytes, 4, &[4], &[1], 4);
        let archive = fixture.finish(false);
        let index = read_index(&archive).unwrap();
        let second = index.find("second").unwrap();
        assert_eq!(
            index.tensor_bytes(&archive, second).unwrap(),
            &f32_bytes(&data[4..8])[..]
        );
    }

    #[test]
    fn non_contiguous_refuses_bytes_but_keeps_metadata() {
        let mut fixture = FixtureWriter::new();
        let data: Vec<f32> = (0..6).map(|v| v as f32).collect();
        fixture.add_tensor("t", "FloatStorage", "0", &f32_bytes(&data), 4, &[2, 3], &[1, 2], 0);
        let archive = fixture.finish(false);
        let index = read_index(&archive).unwrap();
        let record = index.find("t").unwrap();
        assert!(!record.is_contiguous());
        assert!(matches!(
            index.tensor_bytes(&archive, record),
            Err(TorchBinError::Tensor(_))
        ));
    }

    #[test]
    fn compressed_entries_refused_precisely() {
        let mut fixture = FixtureWriter::new();
        fixture.add_tensor("t", "FloatStorage", "0", &f32_bytes(&[1.0]), 4, &[1], &[1], 0);
        let archive = fixture.finish(true);
        match read_index(&archive) {
            Err(TorchBinError::Unsupported(msg)) => assert!(msg.contains("compressed")),
            other => panic!("expected unsupported-compression error, got {other:?}"),
        }
    }

    #[test]
    fn truncated_archive_refused() {
        let mut fixture = FixtureWriter::new();
        fixture.add_tensor("t", "FloatStorage", "0", &f32_bytes(&[1.0, 2.0]), 4, &[2], &[1], 0);
        let archive = fixture.finish(false);
        assert!(read_index(&archive[..archive.len() / 2]).is_err());
    }

    #[test]
    fn unknown_opcode_reports_position() {
        let files = [("archive/data.pkl".to_string(), vec![0x80, 2, 0x01])];
        let archive = build_zip(&files, false);
        match read_index(&archive) {
            Err(TorchBinError::Pickle(msg)) => assert!(msg.contains("0x01")),
            other => panic!("expected pickle error, got {other:?}"),
        }
    }

    #[test]
    fn long1_stack_and_value_depth_limits_fail_closed() {
        let mut too_wide = vec![0x80, 2, 0x8a, 9];
        too_wide.extend_from_slice(&[0; 9]);
        let archive = build_zip(&[("archive/data.pkl".to_string(), too_wide)], false);
        assert!(matches!(read_index(&archive), Err(TorchBinError::Pickle(message)) if message.contains("LONG1")));

        let mut too_deep = vec![0x80, 2, b'N'];
        too_deep.extend(std::iter::repeat_n(0x85, 66));
        let archive = build_zip(&[("archive/data.pkl".to_string(), too_deep)], false);
        assert!(matches!(read_index(&archive), Err(TorchBinError::Pickle(message)) if message.contains("depth")));

        let mut too_tall = vec![0x80, 2];
        too_tall.extend(std::iter::repeat_n(b'N', MAX_PICKLE_STACK + 1));
        let archive = build_zip(&[("archive/data.pkl".to_string(), too_tall)], false);
        assert!(matches!(read_index(&archive), Err(TorchBinError::Pickle(message)) if message.contains("stack")));
    }

    #[test]
    fn repeated_get_of_one_mib_memo_string_hits_byte_budget_before_clone() {
        let payload = vec![b'x'; 1024 * 1024];
        let mut pickle = vec![0x80, 2, b'X'];
        pickle.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        pickle.extend_from_slice(&payload);
        pickle.extend_from_slice(&[b'q', 0]);
        for _ in 0..64 {
            pickle.extend_from_slice(&[b'h', 0]);
        }
        pickle.push(b'.');
        let archive = build_zip(&[("archive/data.pkl".to_string(), pickle)], false);
        assert!(matches!(
            read_index(&archive),
            Err(TorchBinError::Pickle(message)) if message.contains("heap clone budget")
        ));
    }

    #[test]
    fn shape_stride_and_storage_arithmetic_is_checked() {
        let persistent = |storage_numel: i64| {
            Value::Persistent(Box::new(Value::Tuple(vec![
                Value::Str("storage".to_string()),
                Value::Global("torch".to_string(), "HalfStorage".to_string()),
                Value::Str("0".to_string()),
                Value::Str("cpu".to_string()),
                Value::Int(storage_numel),
            ])))
        };
        let callable = Value::Global(
            "torch._utils".to_string(),
            "_rebuild_tensor_v2".to_string(),
        );
        let mismatch = Value::Tuple(vec![
            persistent(8),
            Value::Int(0),
            Value::Tuple(vec![Value::Int(2), Value::Int(2)]),
            Value::Tuple(vec![Value::Int(1)]),
        ]);
        let mut heap_bytes = 0;
        assert!(matches!(
            reduce(callable.clone(), mismatch, 0, &mut heap_bytes),
            Err(TorchBinError::Pickle(message)) if message.contains("rank mismatch")
        ));

        let overflow = Value::Tuple(vec![
            persistent(i64::MAX),
            Value::Int(0),
            Value::Tuple(vec![Value::Int(i64::MAX), Value::Int(3)]),
            Value::Tuple(vec![Value::Int(2), Value::Int(1)]),
        ]);
        let mut heap_bytes = 0;
        assert!(matches!(
            reduce(callable, overflow, 0, &mut heap_bytes),
            Err(TorchBinError::Pickle(message))
                if message.contains("overflow")
                    || message.contains("exceeds limit")
                    || message.contains("exceeds its storage")
        ));
    }

    #[test]
    fn duplicate_names_and_cross_root_storages_are_rejected() {
        let files = [
            ("archive/data.pkl".to_string(), vec![0x80, 2, b'}', b'.']),
            ("other/data.pkl".to_string(), vec![0x80, 2, b'}', b'.']),
        ];
        let archive = build_zip(&files, false);
        assert!(matches!(read_index(&archive), Err(TorchBinError::Zip(message)) if message.contains("multiple data.pkl")));

        let duplicate = [
            ("archive/data.pkl".to_string(), vec![0x80, 2, b'}', b'.']),
            ("archive/data.pkl".to_string(), vec![0x80, 2, b'}', b'.']),
        ];
        let archive = build_zip(&duplicate, false);
        assert!(matches!(read_index(&archive), Err(TorchBinError::Zip(message)) if message.contains("duplicate entry")));

        let mut fixture = FixtureWriter::new();
        fixture.add_tensor("t", "FloatStorage", "0", &f32_bytes(&[1.0]), 4, &[1], &[1], 0);
        let mut wrong_root = fixture.finish(false);
        let from = b"archive/data/0";
        let to = b"otherxx/data/0";
        assert_eq!(from.len(), to.len());
        let mut replacements = 0;
        for at in 0..=wrong_root.len() - from.len() {
            if wrong_root[at..at + from.len()] == *from {
                wrong_root[at..at + to.len()].copy_from_slice(to);
                replacements += 1;
            }
        }
        assert_eq!(replacements, 2, "local and central names must both be rewritten");
        assert!(matches!(
            read_index(&wrong_root),
            Err(TorchBinError::Tensor(message)) if message.contains("exact data.pkl root")
        ));
        assert!(matches!(
            read_index_from(&mut std::io::Cursor::new(wrong_root)),
            Err(TorchBinError::Tensor(message)) if message.contains("exact data.pkl root")
        ));
    }

    #[test]
    fn absolute_data_pickle_root_is_rejected_by_memory_and_streaming_readers() {
        let mut fixture = FixtureWriter::new();
        fixture.add_tensor("t", "FloatStorage", "0", &f32_bytes(&[1.0]), 4, &[1], &[1], 0);
        let canonical = fixture.finish(false);
        let canonical_entries = parse_zip(&canonical).unwrap();
        let payload = |name: &str| {
            let entry = canonical_entries.get(name).copied().unwrap();
            canonical[entry.data_offset..entry.data_offset + entry.size].to_vec()
        };
        let archive = build_zip(
            &[
                ("/data.pkl".to_string(), payload("archive/data.pkl")),
                ("data/0".to_string(), payload("archive/data/0")),
            ],
            false,
        );

        assert!(matches!(
            read_index(&archive),
            Err(TorchBinError::Unsupported(message)) if message.contains("absolute ZIP entry")
        ));
        assert!(matches!(
            read_index_from(&mut std::io::Cursor::new(archive)),
            Err(TorchBinError::Unsupported(message)) if message.contains("absolute ZIP entry")
        ));
    }

    #[test]
    fn zip_bounds_and_local_central_cross_checks_cover_both_paths() {
        let files = [("archive/data.pkl".to_string(), vec![0x80, 2, b'}', b'.'])];
        let archive = build_zip(&files, false);

        let mut name_mismatch = archive.clone();
        name_mismatch[30] ^= 1;
        assert!(matches!(read_index(&name_mismatch), Err(TorchBinError::Zip(message)) if message.contains("name mismatch")));
        assert!(matches!(
            read_index_from(&mut std::io::Cursor::new(name_mismatch)),
            Err(TorchBinError::Zip(message)) if message.contains("name mismatch")
        ));

        let mut huge_central = archive.clone();
        let eocd = huge_central.len() - 22;
        huge_central[eocd + 12..eocd + 16]
            .copy_from_slice(&((MAX_CENTRAL_DIRECTORY_BYTES as u32) + 1).to_le_bytes());
        assert!(matches!(read_index(&huge_central), Err(TorchBinError::Unsupported(message)) if message.contains("central directory")));
        assert!(matches!(
            read_index_from(&mut std::io::Cursor::new(huge_central)),
            Err(TorchBinError::Unsupported(message)) if message.contains("central directory")
        ));

        let mut too_many = archive;
        let eocd = too_many.len() - 22;
        let count = (MAX_ZIP_ENTRIES as u16 + 1).to_le_bytes();
        too_many[eocd + 8..eocd + 10].copy_from_slice(&count);
        too_many[eocd + 10..eocd + 12].copy_from_slice(&count);
        assert!(matches!(read_index(&too_many), Err(TorchBinError::Unsupported(message)) if message.contains("entries")));
    }

    #[test]
    fn duplicate_tensor_names_and_mutated_ranges_fail_closed() {
        let mut fixture = FixtureWriter::new();
        fixture.add_tensor("same", "FloatStorage", "0", &f32_bytes(&[1.0]), 4, &[1], &[1], 0);
        fixture.add_tensor("same", "FloatStorage", "1", &f32_bytes(&[2.0]), 4, &[1], &[1], 0);
        let archive = fixture.finish(false);
        assert!(matches!(read_index(&archive), Err(TorchBinError::Pickle(message)) if message.contains("duplicate tensor")));

        let mut fixture = FixtureWriter::new();
        fixture.add_tensor("t", "FloatStorage", "0", &f32_bytes(&[1.0]), 4, &[1], &[1], 0);
        let archive = fixture.finish(false);
        let mut index = read_index(&archive).unwrap();
        index.tensors[0].numel = usize::MAX;
        assert!(matches!(
            index.tensor_bytes(&archive, &index.tensors[0]),
            Err(TorchBinError::Tensor(_))
        ));
    }
}
