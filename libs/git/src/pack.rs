use std::fs;
use std::path::{Path, PathBuf};

use crate::error::GitError;
use crate::object::{Object, ObjectKind};
use crate::oid::ObjectId;

/// Trait for looking up object offsets in a pack — used for delta resolution.
pub trait PackLookup {
    fn find_offset(&self, oid: &ObjectId) -> Option<u64>;
}

/// The parsed tables of a pack index. Every reader of the same .idx file in
/// the process shares one copy: a large repository's index (the Linux
/// kernel's 14M objects parse to ~500 MB) is not parsed again per reader
/// thread.
struct IdxTables {
    fanout: [u32; 256],
    oids: Vec<ObjectId>,
    offsets: Vec<u64>,
    sorted_offsets: Vec<u64>,
}

/// Parsed indexes survive reader closes, by canonical .idx path, length and
/// mtime. A new index retires entries whose files have been removed by repack.
type SharedTables = std::collections::HashMap<PathBuf, (u64, Option<std::time::SystemTime>, std::sync::Arc<IdxTables>)>;
static SHARED_TABLES: std::sync::Mutex<Option<SharedTables>> = std::sync::Mutex::new(None);

/// In-memory representation of a pack index (.idx v2) with positional object reads.
pub struct PackIndex {
    pub pack_path: PathBuf,
    tables: std::sync::Arc<IdxTables>,
    read_account: crate::memory::MemoryAccount,
    metadata_lease: Option<crate::memory::Reservation>,
    stats: std::cell::Cell<ReadPhases>,
    /// The pack file kept open for bounded reads and `object_size`.
    header_file: std::cell::RefCell<Option<fs::File>>,
    delta_bases: std::cell::RefCell<crate::memory::ReadLru<u64, (ObjectKind, std::sync::Arc<[u8]>)>>,
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
            self.tables.fanout[first_byte - 1] as usize
        };
        let end = self.tables.fanout[first_byte] as usize;
        let slice = &self.tables.oids[start..end];
        match slice.binary_search_by(|probe| probe.as_bytes().cmp(oid.as_bytes())) {
            Ok(idx) => Some(self.tables.offsets[start + idx]),
            Err(_) => None,
        }
    }
}

/// Read and parse a v2 pack index file (or share the tables of a reader
/// that already has).
pub fn read_pack_index(idx_path: &Path) -> Result<PackIndex, GitError> {
    let path = idx_path.canonicalize()?;
    let idx_path = path.as_path();
    let meta = fs::metadata(idx_path)?;
    let key = (meta.len(), meta.modified().ok());
    // Held while parsing, so readers opening together parse once.
    let mut shared = SHARED_TABLES.lock().unwrap_or_else(|e| e.into_inner());
    let shared = shared.get_or_insert_with(Default::default);
    let tables = match shared.get(idx_path).filter(|(len, time, _)| (*len, *time) == key).map(|(_, _, t)| t.clone()) {
        Some(tables) => tables,
        None => {
            let tables = std::sync::Arc::new(parse_pack_index(idx_path)?);
            shared.retain(|path, _| path.exists());
            shared.insert(idx_path.to_path_buf(), (key.0, key.1, tables.clone()));
            tables
        }
    };
    Ok(PackIndex {
        pack_path: idx_path.with_extension("pack"),
        tables,
        read_account: crate::memory::read_account(),
        metadata_lease: None,
        stats: std::cell::Cell::new(ReadPhases::default()),
        header_file: std::cell::RefCell::new(None),
        delta_bases: std::cell::RefCell::new(crate::memory::ReadLru::new(crate::memory::READ_CACHE_BYTES)),
    })
}

fn parse_pack_index(idx_path: &Path) -> Result<IdxTables, GitError> {
    let data = fs::read(idx_path)?;

    // Check v2 magic: ff744f63
    if data.len() < 8 + 1024 + 40 {
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
    let table = data.get(pos..pos + total_objects * 20).ok_or_else(|| GitError::CorruptPack("truncated OID table".into()))?;
    let oids: Vec<ObjectId> = table.chunks_exact(20).map(|c| ObjectId::from_bytes(c.try_into().unwrap())).collect();
    pos += total_objects * 20;

    // CRC32 table: skip (total_objects x 4 bytes)
    pos += total_objects * 4;

    // Offset table: total_objects x 4 bytes
    let mut offsets = Vec::with_capacity(total_objects);
    let large_offset_start = pos + total_objects * 4;
    if large_offset_start + 40 > data.len() { return Err(GitError::CorruptPack("truncated offset table".into())); }
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

    let sorted_offsets = match reverse_index(idx_path, &data, &offsets) {
        Some(sorted) => sorted,
        None => {
            let mut sorted = offsets.clone();
            sorted.sort_unstable();
            sorted
        }
    };
    Ok(IdxTables { fanout, oids, offsets, sorted_offsets })
}

/// The offsets in pack order from the pack's reverse index (`.rev`, which
/// git writes beside the pack: each object's index position, in pack
/// order), instead of sorting them: a 12M-object pack's sort took half a
/// second of every index parse. `None` without a valid one for this pack.
fn reverse_index(idx_path: &Path, idx: &[u8], offsets: &[u64]) -> Option<Vec<u64>> {
    let data = fs::read(idx_path.with_extension("rev")).ok()?;
    let n = offsets.len();
    // "RIDX", version 1, hash id 1 (SHA-1), n positions, the pack's
    // checksum and the file's own.
    if data.len() != 12 + 4 * n + 40 || data[0..4] != *b"RIDX" || read_u32(&data, 4) != 1 || read_u32(&data, 8) != 1 {
        return None;
    }
    // The pack checksum both files name must agree.
    if idx.len() < 40 || data[12 + 4 * n..12 + 4 * n + 20] != idx[idx.len() - 40..idx.len() - 20] {
        return None;
    }
    let mut sorted = Vec::with_capacity(n);
    for i in 0..n {
        sorted.push(*offsets.get(read_u32(&data, 12 + 4 * i) as usize)?);
    }
    sorted.windows(2).all(|w| w[0] < w[1]).then_some(sorted)
}

impl PackIndex {
    pub fn retained_bytes(&self) -> usize {
        let t = &self.tables;
        t.oids.capacity() * std::mem::size_of::<ObjectId>() + (t.offsets.capacity() + t.sorted_offsets.capacity()) * 8
    }
    pub fn read_phases(&self) -> ReadPhases { self.stats.get() }
    /// Bind optional retention to the caller's existing history account.
    /// Metadata and active objects must remain readable even above its budget;
    /// optional cache entries use `try_reserve` and simply cease being retained.
    pub fn set_read_budget(&mut self, account: crate::memory::MemoryAccount, bytes: usize) -> Result<(), GitError> {
        self.delta_bases.borrow_mut().clear();
        self.delta_bases.borrow_mut().resize(bytes.min(crate::memory::READ_CACHE_BYTES));
        self.metadata_lease = None;
        self.metadata_lease = Some(account.reserve(self.retained_bytes()));
        self.read_account = account;
        Ok(())
    }

    /// Open the pack handle without reading the pack into memory.
    pub fn ensure_loaded(&self) -> Result<(), GitError> {
        let mut slot = self.header_file.borrow_mut();
        if slot.is_none() { *slot = Some(fs::File::open(&self.pack_path)?); }
        Ok(())
    }

    /// An independent bounded reader sharing only immutable parsed tables.
    pub(crate) fn reader(&self) -> Self {
        Self {
            pack_path: self.pack_path.clone(), tables: self.tables.clone(),
            read_account: self.read_account.clone(), metadata_lease: None,
            stats: Default::default(), header_file: Default::default(),
            delta_bases: std::cell::RefCell::new(crate::memory::ReadLru::new(crate::memory::READ_CACHE_BYTES)),
        }
    }

    /// Resolve iteratively: long delta chains do not consume the call stack.
    pub fn read_object(&self, offset: u64) -> Result<Object, GitError> {
        let mut at = offset;
        let mut chain = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let (kind, mut data) = loop {
            if !seen.insert(at) { return Err(GitError::CorruptPack("cyclic delta chain".into())); }
            if let Some(hit) = self.delta_bases.borrow_mut().get(&at).cloned() { break hit; }
            if let Some(base) = self.delta_parent(at)? {
                // Keep only offsets while finding the base. Retaining inflated
                // delta programs here makes memory scale with chain depth.
                chain.push(at);
                at = base;
            } else {
                let (kind, bytes, _) = self.read_window(at)?;
                let kind = ObjectKind::from_type_num(kind)?;
                let data: std::sync::Arc<[u8]> = bytes.into();
                self.cache_base(at, kind, &data);
                break (kind, data);
            }
        };
        let mut _base = self.read_account.reserve(data.len());
        while let Some(at) = chain.pop() {
            let (_, delta, _) = self.read_window(at)?;
            let _delta = self.read_account.reserve(delta.len());
            let (_, n) = read_delta_size(&delta, 0)?;
            let (size, _) = read_delta_size(&delta, n)?;
            let result_lease = self.read_account.reserve(usize::try_from(size).map_err(|_| GitError::CorruptPack("delta result too large".into()))?);
            data = apply_delta(&data, &delta)?.into();
            _base = result_lease;
            self.cache_base(at, kind, &data);
        }
        Ok(Object { kind, data: data.to_vec() })
    }

    fn cache_base(&self, at: u64, kind: ObjectKind, data: &std::sync::Arc<[u8]>) {
        self.delta_bases.borrow_mut().insert(at, data.len().saturating_add(128), &self.read_account, || (kind, data.clone()));
    }

    fn range_end(&self, offset: u64, file: &fs::File) -> Result<u64, GitError> {
        if self.tables.sorted_offsets.binary_search(&offset).is_err() {
            return Err(GitError::CorruptPack("offset absent from index".into()));
        }
        let i = self.tables.sorted_offsets.partition_point(|o| *o <= offset);
        Ok(match self.tables.sorted_offsets.get(i) {
            Some(end) => *end,
            None => file.metadata()?.len().saturating_sub(20),
        })
    }

    /// The largest valid size + delta-base header fits in 30 bytes. Reading
    /// this prefix lets us walk arbitrarily long chains without retaining any
    /// compressed or inflated delta programs from the walk.
    fn delta_parent(&self, offset: u64) -> Result<Option<u64>, GitError> {
        let start = crate::clock::Stopwatch::start();
        self.ensure_loaded()?;
        let slot = self.header_file.borrow();
        let file = slot.as_ref().unwrap();
        let end = self.range_end(offset, file)?;
        let len = end.checked_sub(offset).ok_or_else(|| GitError::CorruptPack("invalid object range".into()))?.min(32) as usize;
        let mut prefix = [0; 32];
        read_exact_at(file, &mut prefix[..len], offset)?;
        let mut stats = self.stats.get();
        stats.pack_load_ms += start.elapsed_ms();
        stats.pack_bytes_read += len as u64;
        self.stats.set(stats);
        Ok(entry_header(&prefix[..len], offset, self)?.2)
    }

    fn read_window(&self, offset: u64) -> Result<(u8, Vec<u8>, Option<u64>), GitError> {
        let account = &self.read_account;
        let start = crate::clock::Stopwatch::start();
        self.ensure_loaded()?;
        let slot = self.header_file.borrow();
        let file = slot.as_ref().unwrap();
        let end = self.range_end(offset, file)?;
        let length = usize::try_from(end.checked_sub(offset).ok_or_else(|| GitError::CorruptPack("invalid object range".into()))?).map_err(|_| GitError::CorruptPack("object too large".into()))?;
        let _compressed = account.reserve(length);
        let mut data = vec![0; length];
        read_exact_at(file, &mut data, offset)?;
        drop(slot);
        let mut stats = self.stats.get(); stats.pack_load_ms += start.elapsed_ms(); stats.pack_bytes_read += length as u64; self.stats.set(stats);
        let (kind, size, base_offset, pos) = entry_header(&data, offset, self)?;
        let _inflated = account.reserve(size);
        let start = crate::clock::Stopwatch::start();
        let inflated = zlib_decompress(&data[pos..], size)?;
        let mut stats = self.stats.get(); stats.inflate_ms += start.elapsed_ms(); stats.objects_inflated += 1; self.stats.set(stats);
        Ok((kind, inflated, base_offset))
    }

    /// The inflated size of the object at `offset`, from its pack entry
    /// header, without reading its content or any delta base. A base
    /// object's size is the header varint. A delta's result size is the
    /// second varint of the delta payload, so the (small) delta payload is
    /// inflated; its base chain is never touched. Uses positional reads from
    /// the same kept-open handle as object reads.
    pub fn object_size(&self, offset: u64) -> Result<u64, GitError> {
        self.ensure_loaded()?;
        let slot = self.header_file.borrow();
        let file = slot.as_ref().unwrap();
        let end = self.range_end(offset, file)?;
        let length = end.checked_sub(offset).ok_or_else(|| GitError::CorruptPack("invalid object range".into()))?;
        let mut owned = vec![0; length.min(32) as usize];
        read_exact_at(file, &mut owned, offset)?;
        if (owned.first().copied().unwrap_or(0) >> 4) & 7 >= 6 {
            owned.resize(usize::try_from(length).map_err(|_| GitError::CorruptPack("object too large".into()))?, 0);
            read_exact_at(file, &mut owned, offset)?;
        }
        let entry = owned.as_slice();
        let mut pos = 0;
        let mut next = || -> Result<u8, GitError> { let b = *entry.get(pos).ok_or_else(|| GitError::CorruptPack("truncated object header".into()))?; pos += 1; Ok(b) };
        let first = next()?;
        let kind = (first >> 4) & 7;
        let mut size = (first & 15) as u64;
        let (mut byte, mut shift) = (first, 4);
        while byte & 128 != 0 {
            byte = next()?;
            if shift > 57 { return Err(GitError::CorruptPack("object size overflow".into())); }
            size |= ((byte & 127) as u64) << shift;
            shift += 7;
        }
        match kind {
            1..=4 => Ok(size),
            6 | 7 => {
                if kind == 6 { while next()? & 128 != 0 {} } else { for _ in 0..20 { next()?; } }
                let delta = zlib_decompress(entry.get(pos..).unwrap_or(&[]), size as usize)?;
                let (_, n) = read_delta_size(&delta, 0)?;
                Ok(read_delta_size(&delta, n)?.0)
            }
            _ => Err(GitError::CorruptPack(format!("unknown pack object type: {kind}"))),
        }
    }

}

fn entry_header(data: &[u8], offset: u64, lookup: &dyn PackLookup) -> Result<(u8, usize, Option<u64>, usize), GitError> {
    let mut pos = 0;
    let next = |pos: &mut usize| -> Result<u8, GitError> { let b = data.get(*pos).copied().ok_or_else(|| GitError::CorruptPack("truncated object header".into()))?; *pos += 1; Ok(b) };
    let first = next(&mut pos)?;
    let kind = (first >> 4) & 7;
    let mut size = (first & 15) as usize;
    let mut byte = first; let mut shift = 4;
    while byte & 128 != 0 { byte = next(&mut pos)?; if shift >= usize::BITS { return Err(GitError::CorruptPack("object size overflow".into())); } size |= ((byte & 127) as usize) << shift; shift += 7; }
    let base_offset = match kind {
        6 => { let mut byte = next(&mut pos)?; let mut back = (byte & 127) as u64; while byte & 128 != 0 { byte = next(&mut pos)?; back = back.checked_add(1).and_then(|b| b.checked_mul(128)).and_then(|b| b.checked_add((byte & 127) as u64)).ok_or_else(|| GitError::CorruptPack("delta offset overflow".into()))?; } Some(offset.checked_sub(back).ok_or_else(|| GitError::CorruptPack("delta before pack".into()))?) }
        7 => { let end = pos + 20; let oid = ObjectId::from_slice(data.get(pos..end).ok_or_else(|| GitError::CorruptPack("truncated delta oid".into()))?)?; pos = end; Some(lookup.find_offset(&oid).ok_or_else(|| GitError::CorruptPack("delta base absent".into()))?) }
        _ => None,
    };
    Ok((kind, size, base_offset, pos))
}

/// Positional I/O does not mutate a shared file cursor.
fn read_exact_at(file: &fs::File, mut data: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !data.is_empty() {
        #[cfg(unix)]
        let result = { use std::os::unix::fs::FileExt; file.read_at(data, offset) };
        #[cfg(windows)]
        let result = { use std::os::windows::fs::FileExt; file.seek_read(data, offset) };
        #[cfg(not(any(unix, windows)))]
        let result = {
            use std::io::{Read, Seek, SeekFrom};
            let mut file = file.try_clone()?;
            file.seek(SeekFrom::Start(offset))?;
            file.read(data)
        };
        match result {
            Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
            Ok(n) => { offset += n as u64; data = &mut data[n..]; }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
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
    fn indexes_survive_closed_readers_and_refresh_after_replacement() {
        let dir = crate::test_support::tempdir().unwrap();
        crate::test_support::write_pack(dir.path(), &[(ObjectKind::Blob, b"one".to_vec())]).unwrap();
        let pack = find_packs(dir.path()).unwrap().pop().unwrap();
        let path = pack.pack_path.with_extension("idx");
        let tables = std::sync::Arc::downgrade(&pack.tables);
        drop(pack);
        let reopened = read_pack_index(&path).unwrap();
        assert!(std::sync::Arc::ptr_eq(&tables.upgrade().unwrap(), &reopened.tables), "closing readers must not trigger another parse");
        let other = crate::test_support::tempdir().unwrap();
        crate::test_support::write_pack(other.path(), &[(ObjectKind::Blob, b"two".to_vec()), (ObjectKind::Blob, b"three".to_vec())]).unwrap();
        let replacement = find_packs(other.path()).unwrap().pop().unwrap();
        fs::copy(replacement.pack_path.with_extension("idx"), &path).unwrap();
        let changed = read_pack_index(&path).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&reopened.tables, &changed.tables), "changed size/mtime must invalidate parsed tables");
        assert_eq!(changed.tables.oids.len(), 2);
    }

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
        let obj = pack.read_object(offset).unwrap();
        assert_eq!(obj.kind, ObjectKind::Blob);
        assert_eq!(obj.data, b"packed content\n");
    }
}

/// Save a verified, self-contained incoming pack and its standard v2 index.
/// Offsets and object identities come from the importer's delta resolution.
pub(crate) fn write_imported_pack(
    git_dir: &Path, data: &[u8], entries: &[(ObjectId, u64, u64)],
) -> Result<(), GitError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let checksum = &data[data.len() - 20..];
    let name = ObjectId::from_slice(checksum)?.to_hex();
    let mut sorted = entries.to_vec();
    sorted.sort_unstable_by_key(|e| *e.0.as_bytes());
    let mut index = Vec::with_capacity(1072 + entries.len() * 28);
    index.extend_from_slice(&[0xff, 0x74, 0x4f, 0x63]);
    index.extend_from_slice(&2u32.to_be_bytes());
    let mut fanout = [0u32; 256];
    for entry in &sorted { fanout[entry.0.as_bytes()[0] as usize] += 1; }
    let mut total = 0u32;
    for count in fanout { total += count; index.extend_from_slice(&total.to_be_bytes()); }
    for entry in &sorted { index.extend_from_slice(entry.0.as_bytes()); }
    for (_, start, end) in &sorted {
        let packed = data.get(*start as usize..*end as usize).ok_or_else(|| GitError::CorruptPack("invalid indexed object range".into()))?;
        index.extend_from_slice(&makepad_fast_inflate::crc32(packed).to_be_bytes());
    }
    let mut large = Vec::new();
    for (_, offset, _) in &sorted {
        let offset = if *offset < 0x80000000 { *offset as u32 } else {
            let entry = 0x80000000 | large.len() as u32;
            large.push(*offset); entry
        };
        index.extend_from_slice(&offset.to_be_bytes());
    }
    for offset in large { index.extend_from_slice(&offset.to_be_bytes()); }
    index.extend_from_slice(checksum);
    let mut hash = crate::sha1::Sha1::new(); hash.update(&index);
    index.extend_from_slice(&hash.finalize());
    let dir = git_dir.join("objects/pack");
    fs::create_dir_all(&dir)?;
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    // Publish the index last; readers discover packs through their index files.
    for (extension, bytes) in [("pack", data), ("idx", index.as_slice())] {
        let destination = dir.join(format!("pack-{name}.{extension}"));
        let temporary = dir.join(format!("tmp-{}-{serial}.{extension}", std::process::id()));
        let result = (|| -> Result<(), GitError> {
            fs::write(&temporary, bytes)?;
            match fs::rename(&temporary, &destination) {
                Ok(()) => Ok(()),
                Err(_) if destination.is_file() => { fs::remove_file(&temporary)?; Ok(()) }
                Err(e) => Err(e.into()),
            }
        })();
        if result.is_err() { let _ = fs::remove_file(&temporary); }
        result?;
    }
    Ok(())
}
