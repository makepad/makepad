use std::fs;
use std::path::{Path, PathBuf};

use crate::error::GitError;
use crate::object::{Object, ObjectKind};
use crate::oid::ObjectId;

/// Trait for looking up object offsets in a pack — used for delta resolution.
pub trait PackLookup {
    fn find_offset(&self, oid: &ObjectId) -> Option<u64>;
}

/// In-memory representation of a pack index (.idx v2) with cached pack data.
pub struct PackIndex {
    pub pack_path: PathBuf,
    fanout: [u32; 256],
    oids: Vec<ObjectId>,
    offsets: Vec<u64>,
    /// Cached pack file data — loaded once on first read.
    pack_data: std::cell::RefCell<Option<Vec<u8>>>,
    sorted_offsets: Vec<u64>,
    read_account: Option<crate::memory::MemoryAccount>,
    read_budget: usize,
    metadata_lease: Option<crate::memory::Reservation>,
    stats: std::cell::Cell<ReadPhases>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReadPhases {
    pub pack_load_ms: f64,
    pub inflate_ms: f64,
    pub pack_bytes_read: u64,
    pub objects_inflated: u64,
}

impl PackLookup for PackIndex {
    fn find_offset(&self, oid: &ObjectId) -> Option<u64> {
        let first_byte = oid.as_bytes()[0] as usize;
        let start = if first_byte == 0 {
            0
        } else {
            self.fanout[first_byte - 1] as usize
        };
        let end = self.fanout[first_byte] as usize;
        let slice = &self.oids[start..end];
        match slice.binary_search_by(|probe| probe.as_bytes().cmp(oid.as_bytes())) {
            Ok(idx) => Some(self.offsets[start + idx]),
            Err(_) => None,
        }
    }
}

/// Read and parse a v2 pack index file.
pub fn read_pack_index(idx_path: &Path) -> Result<PackIndex, GitError> {
    let data = fs::read(idx_path)?;

    // Check v2 magic: ff744f63
    if data.len() < 8 {
        return Err(GitError::CorruptPack("index too small".into()));
    }
    let magic = &data[0..4];
    if magic != [0xff, 0x74, 0x4f, 0x63] {
        return Err(GitError::CorruptPack("not a v2 pack index".into()));
    }
    let version = read_u32(&data, 4);
    if version != 2 {
        return Err(GitError::CorruptPack(format!(
            "unsupported index version: {}",
            version
        )));
    }
    let mut pos = 8;

    // Fanout table: 256 x 4-byte big-endian
    let mut fanout = [0u32; 256];
    for i in 0..256 {
        fanout[i] = read_u32(&data, pos);
        pos += 4;
    }
    let total_objects = fanout[255] as usize;

    // OID table: total_objects x 20 bytes
    let mut oids = Vec::with_capacity(total_objects);
    for _ in 0..total_objects {
        if pos + 20 > data.len() {
            return Err(GitError::CorruptPack("truncated OID table".into()));
        }
        oids.push(ObjectId::from_slice(&data[pos..pos + 20])?);
        pos += 20;
    }

    // CRC32 table: skip (total_objects x 4 bytes)
    pos += total_objects * 4;

    // Offset table: total_objects x 4 bytes
    let mut offsets = Vec::with_capacity(total_objects);
    let large_offset_start = pos + total_objects * 4;
    for _ in 0..total_objects {
        let off = read_u32(&data, pos);
        pos += 4;
        if off & 0x80000000 != 0 {
            // MSB set: index into large offset table
            let large_idx = (off & 0x7FFFFFFF) as usize;
            let large_pos = large_offset_start + large_idx * 8;
            if large_pos + 8 > data.len() {
                return Err(GitError::CorruptPack("truncated large offset table".into()));
            }
            let hi = read_u32(&data, large_pos) as u64;
            let lo = read_u32(&data, large_pos + 4) as u64;
            offsets.push((hi << 32) | lo);
        } else {
            offsets.push(off as u64);
        }
    }

    // Derive pack path from index path (.idx -> .pack)
    let pack_path = idx_path.with_extension("pack");

    let mut sorted_offsets = offsets.clone();
    sorted_offsets.sort_unstable();
    Ok(PackIndex {
        pack_path,
        fanout,
        oids,
        offsets,
        pack_data: std::cell::RefCell::new(None),
        sorted_offsets,
        read_account: None,
        read_budget: usize::MAX,
        metadata_lease: None,
        stats: std::cell::Cell::new(ReadPhases::default()),
    })
}

impl PackIndex {
    pub fn retained_bytes(&self) -> usize {
        self.oids.capacity() * std::mem::size_of::<ObjectId>() + (self.offsets.capacity() + self.sorted_offsets.capacity()) * 8 + self.pack_data.borrow().as_ref().map_or(0, Vec::capacity)
    }
    pub fn read_phases(&self) -> ReadPhases { self.stats.get() }
    /// Historical readers retain the index, and read one compressed object
    /// range at a time. No whole-pack residency outside the history account.
    pub fn set_read_budget(&mut self, account: crate::memory::MemoryAccount, bytes: usize) -> Result<(), GitError> {
        *self.pack_data.borrow_mut() = None;
        let needed = self.retained_bytes();
        self.metadata_lease = Some(account.try_reserve(needed).ok_or_else(|| GitError::InvalidObject(format!("reader budget exceeded: needed {needed}, available {}", account.available())))?);
        self.read_account = Some(account);
        self.read_budget = bytes;
        Ok(())
    }
    /// Ensure pack data is loaded into memory (read once, cached).
    pub fn ensure_loaded(&self) -> Result<(), GitError> {
        if self.read_account.is_some() { return Err(GitError::InvalidObject("whole-pack loading is disabled for a bounded reader".into())); }
        let mut cache = self.pack_data.borrow_mut();
        if cache.is_none() {
            let start = std::time::Instant::now();
            *cache = Some(fs::read(&self.pack_path)?);
            let mut stats = self.stats.get();
            stats.pack_load_ms += start.elapsed().as_secs_f64() * 1000.0;
            stats.pack_bytes_read += cache.as_ref().unwrap().len() as u64;
            self.stats.set(stats);
        }
        Ok(())
    }

    /// Read an object at the given offset, using cached pack data.
    pub fn read_object(&self, offset: u64) -> Result<Object, GitError> {
        if self.read_account.is_some() { return self.read_window(offset, 0); }
        self.ensure_loaded()?;
        let cache = self.pack_data.borrow();
        let data = cache.as_ref().unwrap();
        let start = std::time::Instant::now();
        let result = read_pack_object_at(data, offset as usize, self, data);
        let mut stats = self.stats.get();
        stats.inflate_ms += start.elapsed().as_secs_f64() * 1000.0;
        stats.objects_inflated += 1;
        self.stats.set(stats);
        result
    }

    fn read_window(&self, offset: u64, depth: usize) -> Result<Object, GitError> {
        use std::io::{Read, Seek, SeekFrom};
        if depth > 128 { return Err(GitError::CorruptPack("delta chain exceeds 128".into())); }
        let account = self.read_account.as_ref().unwrap();
        let start = std::time::Instant::now();
        let mut file = fs::File::open(&self.pack_path)?;
        let i = self.sorted_offsets.partition_point(|o| *o <= offset);
        let end = self.sorted_offsets.get(i).copied().unwrap_or(file.metadata()?.len().saturating_sub(20));
        let length = usize::try_from(end.checked_sub(offset).ok_or_else(|| GitError::CorruptPack("invalid object range".into()))?).map_err(|_| GitError::CorruptPack("object too large".into()))?;
        if length > self.read_budget { return Err(GitError::InvalidObject(format!("compressed object exceeds reader budget: {length}"))); }
        let _compressed = account.try_reserve(length).ok_or_else(|| GitError::InvalidObject("history account cannot admit compressed object".into()))?;
        let mut data = vec![0; length];
        file.seek(SeekFrom::Start(offset))?; file.read_exact(&mut data)?;
        let mut stats = self.stats.get(); stats.pack_load_ms += start.elapsed().as_secs_f64() * 1000.0; stats.pack_bytes_read += length as u64; self.stats.set(stats);
        let mut pos = 0;
        let next = |pos: &mut usize| -> Result<u8, GitError> { let b = data.get(*pos).copied().ok_or_else(|| GitError::CorruptPack("truncated object header".into()))?; *pos += 1; Ok(b) };
        let first = next(&mut pos)?;
        let kind = (first >> 4) & 7;
        let mut size = (first & 15) as usize;
        let mut byte = first; let mut shift = 4;
        while byte & 128 != 0 { byte = next(&mut pos)?; if shift >= usize::BITS { return Err(GitError::CorruptPack("object size overflow".into())); } size |= ((byte & 127) as usize) << shift; shift += 7; }
        if size > self.read_budget { return Err(GitError::InvalidObject(format!("inflated object exceeds reader budget: {size}"))); }
        let _inflated = account.try_reserve(size).ok_or_else(|| GitError::InvalidObject("history account cannot admit inflated object".into()))?;
        let base_offset = match kind {
            6 => { let mut byte = next(&mut pos)?; let mut back = (byte & 127) as u64; while byte & 128 != 0 { byte = next(&mut pos)?; back = back.checked_add(1).and_then(|b| b.checked_mul(128)).and_then(|b| b.checked_add((byte & 127) as u64)).ok_or_else(|| GitError::CorruptPack("delta offset overflow".into()))?; } Some(offset.checked_sub(back).ok_or_else(|| GitError::CorruptPack("delta before pack".into()))?) }
            7 => { let end = pos + 20; let oid = ObjectId::from_slice(data.get(pos..end).ok_or_else(|| GitError::CorruptPack("truncated delta oid".into()))?)?; pos = end; Some(self.find_offset(&oid).ok_or_else(|| GitError::CorruptPack("delta base absent".into()))?) }
            _ => None,
        };
        let start = std::time::Instant::now();
        let inflated = zlib_decompress(&data[pos..], size)?;
        let mut stats = self.stats.get(); stats.inflate_ms += start.elapsed().as_secs_f64() * 1000.0; stats.objects_inflated += 1; self.stats.set(stats);
        if let Some(base_offset) = base_offset {
            let (_, n) = read_delta_size(&inflated, 0)?;
            let (result_size, _) = read_delta_size(&inflated, n)?;
            if result_size > self.read_budget as u64 { return Err(GitError::InvalidObject("delta result exceeds reader budget".into())); }
            let _result = account.try_reserve(result_size as usize).ok_or_else(|| GitError::InvalidObject("history account cannot admit delta result".into()))?;
            let base = self.read_window(base_offset, depth + 1)?;
            let _base = account.try_reserve(base.data.capacity()).ok_or_else(|| GitError::InvalidObject("history account cannot retain delta base".into()))?;
            let result = apply_delta(&base.data, &inflated)?;
            Ok(Object { kind: base.kind, data: result })
        } else {
            Ok(Object { kind: ObjectKind::from_type_num(kind)?, data: inflated })
        }
    }

    /// Get a clone of the cached pack data (for thread-safe sharing).
    pub fn get_cached_data(&self) -> Option<Vec<u8>> {
        self.pack_data.borrow().clone()
    }

    /// Get copies of index metadata for thread-safe use.
    pub fn fanout_copy(&self) -> [u32; 256] {
        self.fanout
    }

    pub fn oids_copy(&self) -> Vec<ObjectId> {
        self.oids.clone()
    }

    pub fn offsets_copy(&self) -> Vec<u64> {
        self.offsets.clone()
    }
}

/// Read an object from a pack file at the given offset.
/// Uses the PackIndex's cached pack data (loaded once on first call).
pub fn read_pack_object(
    _pack_path: &Path,
    offset: u64,
    idx: &PackIndex,
) -> Result<Object, GitError> {
    idx.read_object(offset)
}

/// Read an object from raw pack data with a PackLookup for delta resolution.
/// This is the thread-safe entry point used by the parallel clone.
pub fn read_pack_object_from_data(
    pack_data: &[u8],
    offset: usize,
    lookup: &dyn PackLookup,
) -> Result<Object, GitError> {
    read_pack_object_at(pack_data, offset, lookup, pack_data)
}

fn read_pack_object_at(
    pack_data: &[u8],
    offset: usize,
    lookup: &dyn PackLookup,
    full_pack: &[u8],
) -> Result<Object, GitError> {
    let mut pos = offset;

    if pos >= pack_data.len() {
        return Err(GitError::CorruptPack("offset beyond pack data".into()));
    }

    let first = pack_data[pos];
    pos += 1;
    let type_num = (first >> 4) & 0x7;
    let mut size: u64 = (first & 0x0F) as u64;
    let mut shift = 4;

    let mut byte = first;
    while byte & 0x80 != 0 {
        if pos >= pack_data.len() {
            return Err(GitError::CorruptPack("truncated size header".into()));
        }
        byte = pack_data[pos];
        pos += 1;
        size |= ((byte & 0x7F) as u64) << shift;
        shift += 7;
    }

    match type_num {
        1 | 2 | 3 | 4 => {
            let kind = ObjectKind::from_type_num(type_num)?;
            let decompressed = zlib_decompress(&pack_data[pos..], size as usize)?;
            Ok(Object {
                kind,
                data: decompressed,
            })
        }
        6 => {
            // OFS_DELTA
            let mut byte = pack_data[pos];
            pos += 1;
            let mut delta_offset: u64 = (byte & 0x7F) as u64;
            while byte & 0x80 != 0 {
                if pos >= pack_data.len() {
                    return Err(GitError::CorruptPack("truncated ofs-delta offset".into()));
                }
                byte = pack_data[pos];
                pos += 1;
                delta_offset = ((delta_offset + 1) << 7) | (byte & 0x7F) as u64;
            }

            let base_offset = offset as u64 - delta_offset;
            let base = read_pack_object_at(full_pack, base_offset as usize, lookup, full_pack)?;
            let delta_data = zlib_decompress(&pack_data[pos..], size as usize)?;
            let patched = apply_delta(&base.data, &delta_data)?;
            Ok(Object {
                kind: base.kind,
                data: patched,
            })
        }
        7 => {
            // REF_DELTA
            if pos + 20 > pack_data.len() {
                return Err(GitError::CorruptPack("truncated ref-delta base oid".into()));
            }
            let base_oid = ObjectId::from_slice(&pack_data[pos..pos + 20])?;
            pos += 20;

            let base_offset = lookup.find_offset(&base_oid).ok_or_else(|| {
                GitError::CorruptPack(format!("ref-delta base not in pack: {}", base_oid))
            })?;
            let base = read_pack_object_at(full_pack, base_offset as usize, lookup, full_pack)?;
            let delta_data = zlib_decompress(&pack_data[pos..], size as usize)?;
            let patched = apply_delta(&base.data, &delta_data)?;
            Ok(Object {
                kind: base.kind,
                data: patched,
            })
        }
        _ => Err(GitError::CorruptPack(format!(
            "unknown pack object type: {}",
            type_num
        ))),
    }
}

/// Apply a git delta to a base object.
fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>, GitError> {
    let mut pos = 0;

    let (_base_size, bytes_read) = read_delta_size(delta, pos)?;
    pos += bytes_read;

    let (result_size, bytes_read) = read_delta_size(delta, pos)?;
    pos += bytes_read;

    let mut result = Vec::with_capacity(result_size as usize);

    while pos < delta.len() {
        let cmd = delta[pos];
        pos += 1;

        if cmd & 0x80 != 0 {
            let mut copy_offset: u32 = 0;
            let mut copy_size: u32 = 0;

            if cmd & 0x01 != 0 {
                copy_offset |= delta[pos] as u32;
                pos += 1;
            }
            if cmd & 0x02 != 0 {
                copy_offset |= (delta[pos] as u32) << 8;
                pos += 1;
            }
            if cmd & 0x04 != 0 {
                copy_offset |= (delta[pos] as u32) << 16;
                pos += 1;
            }
            if cmd & 0x08 != 0 {
                copy_offset |= (delta[pos] as u32) << 24;
                pos += 1;
            }
            if cmd & 0x10 != 0 {
                copy_size |= delta[pos] as u32;
                pos += 1;
            }
            if cmd & 0x20 != 0 {
                copy_size |= (delta[pos] as u32) << 8;
                pos += 1;
            }
            if cmd & 0x40 != 0 {
                copy_size |= (delta[pos] as u32) << 16;
                pos += 1;
            }

            if copy_size == 0 {
                copy_size = 0x10000;
            }

            let start = copy_offset as usize;
            let end = start + copy_size as usize;
            if end > base.len() {
                return Err(GitError::CorruptPack(format!(
                    "delta copy out of range: {}..{} but base is {} bytes",
                    start,
                    end,
                    base.len()
                )));
            }
            result.extend_from_slice(&base[start..end]);
        } else if cmd > 0 {
            let n = cmd as usize;
            if pos + n > delta.len() {
                return Err(GitError::CorruptPack("delta insert truncated".into()));
            }
            result.extend_from_slice(&delta[pos..pos + n]);
            pos += n;
        } else {
            return Err(GitError::CorruptPack(
                "delta: reserved instruction 0x00".into(),
            ));
        }
    }

    if result.len() != result_size as usize {
        return Err(GitError::CorruptPack(format!(
            "delta result size mismatch: expected {}, got {}",
            result_size,
            result.len()
        )));
    }

    Ok(result)
}

fn read_delta_size(data: &[u8], start: usize) -> Result<(u64, usize), GitError> {
    let mut pos = start;
    let mut size: u64 = 0;
    let mut shift = 0;
    loop {
        if pos >= data.len() {
            return Err(GitError::CorruptPack("truncated delta size".into()));
        }
        let byte = data[pos];
        pos += 1;
        size |= ((byte & 0x7F) as u64) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            break;
        }
    }
    Ok((size, pos - start))
}

fn zlib_decompress(data: &[u8], expected_size: usize) -> Result<Vec<u8>, GitError> {
    let mut buf = vec![0u8; expected_size];
    let (_, written) = makepad_fast_inflate::zlib_decompress(data, &mut buf)
        .map_err(|e| GitError::CorruptPack(format!("zlib decompress failed: {}", e)))?;
    buf.truncate(written);
    Ok(buf)
}

fn read_u32(data: &[u8], pos: usize) -> u32 {
    u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
}

/// Find all pack files in .git/objects/pack/
pub fn find_packs(git_dir: &Path) -> Result<Vec<PackIndex>, GitError> {
    find_packs_in_objects_dir(&git_dir.join("objects"))
}

/// Find all pack files under an `objects` directory (the repository's own or
/// an alternate's).
pub fn find_packs_in_objects_dir(objects_dir: &Path) -> Result<Vec<PackIndex>, GitError> {
    let pack_dir = objects_dir.join("pack");
    let mut packs = Vec::new();

    let entries = match fs::read_dir(&pack_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(packs),
        Err(e) => return Err(GitError::Io(e)),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "idx").unwrap_or(false) {
            packs.push(read_pack_index(&path)?);
        }
    }

    Ok(packs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_from_pack() {
        let dir = crate::test_support::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(git_dir.join("objects")).unwrap();
        let ids = crate::test_support::write_pack(
            &git_dir,
            &[(ObjectKind::Blob, b"packed content\n".to_vec())],
        )
        .unwrap();

        let packs = find_packs(&git_dir).unwrap();
        assert!(!packs.is_empty(), "should have at least one pack");

        // The id git assigns to this content.
        let blob_oid = ObjectId::from_hex("5e4999f3bfe35be914c4bba7b0a362112cd4474c").unwrap();
        assert_eq!(ids[0], blob_oid);

        let pack = &packs[0];
        let offset = pack.find_offset(&blob_oid).expect("blob should be in pack");
        let obj = read_pack_object(&pack.pack_path, offset, pack).unwrap();
        assert_eq!(obj.kind, ObjectKind::Blob);
        assert_eq!(obj.data, b"packed content\n");
    }
}
