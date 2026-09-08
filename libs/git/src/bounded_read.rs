//! Uncached, admitted metadata reads. Pack indexes are searched on disk so a
//! diff never implicitly loads an entire pack or retains an uncharged index.
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::diff::TreeDiffExhaustion;
use crate::memory::{MemoryAccount, Reservation};
use crate::{GitError, ObjectId, ObjectKind};

pub(crate) struct Scratch {
    pub account: MemoryAccount,
    local: MemoryAccount,
    pub blob_reads: std::cell::Cell<u64>,
}

pub(crate) struct ScratchLease {
    _shared: Reservation,
    _local: Reservation,
}

impl Scratch {
    pub fn new(account: &MemoryAccount, limit: usize) -> Self {
        Self { account: account.clone(), local: MemoryAccount::new(limit), blob_reads: std::cell::Cell::new(0) }
    }

    pub fn reserve(&self, bytes: usize) -> Result<ScratchLease, GitError> {
        let local = self.local.try_reserve(bytes)
            .ok_or(GitError::TreeDiffLimit(TreeDiffExhaustion::ScratchBytes))?;
        let shared = self.account.try_reserve(bytes)
            .ok_or(GitError::TreeDiffLimit(TreeDiffExhaustion::MemoryAccount))?;
        Ok(ScratchLease { _shared: shared, _local: local })
    }

    pub fn peak(&self) -> usize { self.local.peak() }
}

pub(crate) struct Bytes {
    pub data: Vec<u8>,
    _lease: ScratchLease,
}

impl Bytes {
    pub fn zeroed(size: usize, scratch: &Scratch) -> Result<Self, GitError> {
        let lease = scratch.reserve(size)?;
        Ok(Self { data: vec![0; size], _lease: lease })
    }
}

fn invalid(message: &str) -> GitError { GitError::InvalidObject(message.into()) }

fn check_kind(actual: ObjectKind, expected: ObjectKind, scratch: &Scratch) -> Result<(), GitError> {
    if actual == ObjectKind::Blob { scratch.blob_reads.set(scratch.blob_reads.get().saturating_add(1)); }
    if actual != expected { return Err(invalid("unexpected metadata object type")); }
    Ok(())
}

fn read_bytes(file: &mut File, size: usize, scratch: &Scratch) -> Result<Bytes, GitError> {
    let mut bytes = Bytes::zeroed(size, scratch)?;
    file.read_exact(&mut bytes.data)?;
    Ok(bytes)
}

fn inflate(data: &[u8], size: usize, scratch: &Scratch) -> Result<Bytes, GitError> {
    let mut bytes = Bytes::zeroed(size, scratch)?;
    let (_, written) = makepad_fast_inflate::zlib_decompress(data, &mut bytes.data)
        .map_err(|_| invalid("invalid compressed metadata"))?;
    if written != size { return Err(invalid("metadata size mismatch")); }
    Ok(bytes)
}

fn loose(path: &Path, expected: ObjectKind, scratch: &Scratch, bytes_read: &mut u64)
    -> Result<Bytes, GitError>
{
    let mut file = File::open(path)?;
    let size = usize::try_from(file.metadata()?.len()).map_err(|_| invalid("object too large"))?;
    let compressed = read_bytes(&mut file, size, scratch)?;
    *bytes_read = bytes_read.saturating_add(size as u64);
    // Loose headers are inside zlib. Retry with admitted buffers, dropping the
    // previous output before growth; the inflater cannot grow our fixed slice.
    let mut capacity = 128;
    loop {
        let mut raw = Bytes::zeroed(capacity, scratch)?;
        match makepad_fast_inflate::zlib_decompress(&compressed.data, &mut raw.data) {
            Ok((_, written)) => {
                raw.data.truncate(written);
                let end = raw.data.iter().position(|b| *b == 0).ok_or_else(|| invalid("no object header"))?;
                let header = std::str::from_utf8(&raw.data[..end]).map_err(|_| invalid("invalid object header"))?;
                let (kind, length) = header.split_once(' ').ok_or_else(|| invalid("invalid object header"))?;
                check_kind(ObjectKind::from_str(kind)?, expected, scratch)?;
                let length: usize = length.parse().map_err(|_| invalid("invalid object size"))?;
                if length != written - end - 1 { return Err(invalid("object size mismatch")); }
                raw.data.copy_within(end + 1.., 0);
                raw.data.truncate(length);
                return Ok(raw);
            }
            Err(makepad_fast_inflate::DecompressError::InsufficientSpace) => {
                capacity = capacity.checked_mul(2).ok_or(GitError::TreeDiffLimit(TreeDiffExhaustion::ScratchBytes))?;
            }
            Err(_) => return Err(invalid("invalid compressed metadata")),
        }
    }
}

fn at<const N: usize>(file: &mut File, offset: u64) -> Result<[u8; N], GitError> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = [0; N];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

struct DiskIndex {
    file: File,
    count: u64,
}

impl DiskIndex {
    fn open(path: &Path) -> Result<Self, GitError> {
        let mut file = File::open(path)?;
        if at::<8>(&mut file, 0)? != [255, 116, 79, 99, 0, 0, 0, 2] {
            return Err(GitError::CorruptPack("expected v2 index".into()));
        }
        let count = u32::from_be_bytes(at(&mut file, 8 + 255 * 4)?) as u64;
        if file.metadata()?.len() < 1032 + count * 28 + 40 {
            return Err(GitError::CorruptPack("truncated index".into()));
        }
        Ok(Self { file, count })
    }

    fn offset(&mut self, value: u32) -> Result<u64, GitError> {
        if value & 0x80000000 == 0 { return Ok(value as u64); }
        Ok(u64::from_be_bytes(at(&mut self.file, 1032 + self.count * 28 + (value & 0x7fffffff) as u64 * 8)?))
    }

    fn find(&mut self, oid: &ObjectId) -> Result<Option<u64>, GitError> {
        let (mut lo, mut hi) = (0, self.count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match at::<20>(&mut self.file, 1032 + mid * 20)?.cmp(oid.as_bytes()) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    let value = u32::from_be_bytes(at(&mut self.file, 1032 + self.count * 24 + mid * 4)?);
                    return self.offset(value).map(Some);
                }
            }
        }
        Ok(None)
    }

    fn end(&mut self, start: u64, pack_end: u64) -> Result<u64, GitError> {
        let mut end = pack_end;
        // Fixed stack storage, also covered by the reader's frame admission.
        let mut chunk = [0u8; 4096];
        let mut i = 0;
        while i < self.count {
            let count = (self.count - i).min(1024);
            self.file.seek(SeekFrom::Start(1032 + self.count * 24 + i * 4))?;
            self.file.read_exact(&mut chunk[..count as usize * 4])?;
            for value in chunk[..count as usize * 4].chunks_exact(4) {
                let offset = self.offset(u32::from_be_bytes(value.try_into().unwrap()))?;
                if offset > start { end = end.min(offset); }
            }
            i += count;
        }
        if start < 12 || end <= start { return Err(invalid("invalid pack offset")); }
        Ok(end)
    }
}

fn byte(data: &[u8], pos: &mut usize) -> Result<u8, GitError> {
    let b = *data.get(*pos).ok_or_else(|| invalid("truncated pack metadata"))?;
    *pos += 1;
    Ok(b)
}

fn varint(data: &[u8], pos: &mut usize) -> Result<usize, GitError> {
    let mut value = 0usize;
    for shift in (0..usize::BITS).step_by(7) {
        let b = byte(data, pos)?;
        let part = (b & 127) as usize;
        if part > usize::MAX >> shift { return Err(invalid("size overflow")); }
        value |= part << shift;
        if b & 128 == 0 { return Ok(value); }
    }
    Err(invalid("size overflow"))
}

fn packed(file: &mut File, index: &mut DiskIndex, offset: u64, expected: ObjectKind,
    scratch: &Scratch, bytes_read: &mut u64, depth: usize, cancel: &dyn Fn() -> bool)
    -> Result<Bytes, GitError>
{
    if cancel() { return Err(GitError::Cancelled); }
    if depth >= 64 { return Err(invalid("pack delta chain too deep")); }
    let _frame = scratch.reserve(8192)?;
    let end = index.end(offset, file.metadata()?.len().checked_sub(20).ok_or_else(|| invalid("truncated pack"))?)?;
    file.seek(SeekFrom::Start(offset))?;
    let data = read_bytes(file, usize::try_from(end - offset).map_err(|_| invalid("packed object too large"))?, scratch)?;
    *bytes_read = bytes_read.saturating_add(data.data.len() as u64);
    let data_slice = &data.data;
    let mut pos = 0;
    let first = byte(data_slice, &mut pos)?;
    let kind = (first >> 4) & 7;
    let mut size = (first & 15) as usize;
    let mut b = first;
    let mut shift = 4;
    while b & 128 != 0 {
        b = byte(data_slice, &mut pos)?;
        if shift >= usize::BITS || (b & 127) as usize > usize::MAX >> shift { return Err(invalid("pack size overflow")); }
        size |= ((b & 127) as usize) << shift;
        shift += 7;
    }
    let base = match kind {
        6 => {
            let mut b = byte(data_slice, &mut pos)?;
            let mut distance = (b & 127) as u64;
            while b & 128 != 0 {
                b = byte(data_slice, &mut pos)?;
                distance = distance.checked_add(1).and_then(|v| v.checked_mul(128)).and_then(|v| v.checked_add((b & 127) as u64)).ok_or_else(|| invalid("delta offset overflow"))?;
            }
            if distance == 0 { return Err(invalid("self-referencing delta")); }
            Some(offset.checked_sub(distance).ok_or_else(|| invalid("delta before pack"))?)
        }
        7 => {
            let oid = ObjectId::from_slice(data_slice.get(pos..pos + 20).ok_or_else(|| invalid("truncated delta oid"))?)?;
            pos += 20;
            Some(index.find(&oid)?.ok_or_else(|| invalid("delta base absent"))?)
        }
        _ => {
            check_kind(ObjectKind::from_type_num(kind)?, expected, scratch)?;
            None
        }
    };
    let delta = inflate(&data_slice[pos..], size, scratch)?;
    drop(data);
    let Some(base_offset) = base else { return Ok(delta); };
    let mut pos = 0;
    let base_len = varint(&delta.data, &mut pos)?;
    let result_len = varint(&delta.data, &mut pos)?;
    let mut result = Bytes::zeroed(result_len, scratch)?;
    let base = packed(file, index, base_offset, expected, scratch, bytes_read, depth + 1, cancel)?;
    if base_len != base.data.len() { return Err(invalid("delta base size mismatch")); }
    let mut out = 0usize;
    while pos < delta.data.len() {
        let command = byte(&delta.data, &mut pos)?;
        let source = if command & 128 != 0 {
            let mut offset = 0usize;
            let mut size = 0usize;
            for i in 0..4 { if command & (1 << i) != 0 { offset |= (byte(&delta.data, &mut pos)? as usize) << (8 * i); } }
            for i in 0..3 { if command & (16 << i) != 0 { size |= (byte(&delta.data, &mut pos)? as usize) << (8 * i); } }
            if size == 0 { size = 65536; }
            let end = offset.checked_add(size).ok_or_else(|| invalid("delta copy overflow"))?;
            base.data.get(offset..end).ok_or_else(|| invalid("delta copy out of range"))?
        } else {
            if command == 0 { return Err(invalid("reserved delta command")); }
            let end = pos + command as usize;
            let slice = delta.data.get(pos..end).ok_or_else(|| invalid("truncated delta insert"))?;
            pos = end;
            slice
        };
        let end = out.checked_add(source.len()).ok_or_else(|| invalid("delta result overflow"))?;
        result.data.get_mut(out..end).ok_or_else(|| invalid("delta result exceeds header"))?.copy_from_slice(source);
        out = end;
    }
    if out != result_len { return Err(invalid("delta result size mismatch")); }
    Ok(result)
}

fn in_objects(objects: &Path, oid: &ObjectId, expected: ObjectKind, scratch: &Scratch,
    bytes_read: &mut u64, cancel: &dyn Fn() -> bool) -> Result<Option<Bytes>, GitError>
{
    let (dir, name) = oid.loose_path_components();
    match loose(&objects.join(dir).join(name), expected, scratch, bytes_read) {
        Ok(bytes) => return Ok(Some(bytes)),
        Err(GitError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let entries = match fs::read_dir(objects.join("pack")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "idx") { continue; }
        let mut index = DiskIndex::open(&path)?;
        if let Some(offset) = index.find(oid)? {
            let mut file = File::open(path.with_extension("pack"))?;
            return packed(&mut file, &mut index, offset, expected, scratch, bytes_read, 0, cancel).map(Some);
        }
    }
    Ok(None)
}

pub(crate) fn read(common_dir: &Path, oid: &ObjectId, expected: ObjectKind,
    scratch: &Scratch, bytes_read: &mut u64, cancel: &dyn Fn() -> bool) -> Result<Bytes, GitError>
{
    if cancel() { return Err(GitError::Cancelled); }
    // Covers bounded stack decoder/index frames and filesystem path buffers.
    let _frame = scratch.reserve(32768usize.saturating_add(common_dir.as_os_str().len().saturating_mul(4)))?;
    let objects = common_dir.join("objects");
    if let Some(bytes) = in_objects(&objects, oid, expected, scratch, bytes_read, cancel)? { return Ok(bytes); }
    let mut file = match File::open(objects.join("info/alternates")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(GitError::ObjectNotFound(oid.to_hex())),
        Err(e) => return Err(e.into()),
    };
    let size = usize::try_from(file.metadata()?.len()).map_err(|_| invalid("alternates too large"))?;
    let paths = read_bytes(&mut file, size, scratch)?;
    let text = std::str::from_utf8(&paths.data).map_err(|_| invalid("invalid alternate path"))?;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let _path = scratch.reserve(line.len().saturating_add(objects.as_os_str().len()).saturating_mul(4))?;
        if let Some(bytes) = in_objects(&objects.join(line), oid, expected, scratch, bytes_read, cancel)? { return Ok(bytes); }
    }
    Err(GitError::ObjectNotFound(oid.to_hex()))
}
