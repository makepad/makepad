use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{error::GitError, object::ObjectKind, oid::ObjectId, sha1::Sha1};

static NEXT_TEMP_DIR_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn tempdir() -> io::Result<TempDir> {
    let pid = std::process::id();
    let epoch_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = NEXT_TEMP_DIR_ID.fetch_add(1, Ordering::Relaxed);

    for attempt in 0..64_u32 {
        let path =
            std::env::temp_dir().join(format!("makepad-git-{pid}-{epoch_nanos}-{seq}-{attempt}"));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(TempDir { path }),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "failed to allocate a unique temp directory",
    ))
}

/// One entry of a test pack.
pub enum PackEntry {
    /// A complete object.
    Full(ObjectKind, Vec<u8>),
    /// A ref-delta against `base` (which must be in the same pack or already
    /// in the store); `result` is the reconstructed object the delta yields.
    RefDelta { kind: ObjectKind, base: ObjectId, delta: Vec<u8>, result: Vec<u8> },
    /// An ofs-delta against the entry at index `base_index` of this pack.
    OfsDelta { kind: ObjectKind, base_index: usize, delta: Vec<u8>, result: Vec<u8> },
}

/// A copy instruction of git's delta format: copy `size` bytes from `offset`
/// of the base object.
pub fn delta_copy(offset: u32, size: u32) -> Vec<u8> {
    let mut out = vec![0x80u8];
    let mut flags = 0u8;
    let mut payload = Vec::new();
    for (i, byte) in offset.to_le_bytes().iter().enumerate() {
        if *byte != 0 {
            flags |= 1 << i;
            payload.push(*byte);
        }
    }
    for (i, byte) in size.to_le_bytes().iter().take(3).enumerate() {
        if *byte != 0 {
            flags |= 0x10 << i;
            payload.push(*byte);
        }
    }
    out[0] |= flags;
    out.extend_from_slice(&payload);
    out
}

/// An insert instruction of git's delta format (at most 127 bytes).
pub fn delta_insert(bytes: &[u8]) -> Vec<u8> {
    assert!(!bytes.is_empty() && bytes.len() < 128);
    let mut out = vec![bytes.len() as u8];
    out.extend_from_slice(bytes);
    out
}

/// The delta header: base size and result size as little-endian varints.
pub fn delta_header(base_len: usize, result_len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for mut value in [base_len, result_len] {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }
    out
}

fn entry_header(type_num: u8, mut size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut byte = (type_num << 4) | (size & 0x0f) as u8;
    size >>= 4;
    while size > 0 {
        out.push(byte | 0x80);
        byte = (size & 0x7f) as u8;
        size >>= 7;
    }
    out.push(byte);
    out
}

fn type_num(kind: &ObjectKind) -> u8 {
    match kind {
        ObjectKind::Commit => 1,
        ObjectKind::Tree => 2,
        ObjectKind::Blob => 3,
        ObjectKind::Tag => 4,
    }
}

/// Build the bytes of a version-2 pack file (with its trailing checksum) and
/// return them with the object ids and offsets of its entries, in input order.
pub fn build_pack_bytes(entries: &[PackEntry]) -> (Vec<u8>, Vec<(ObjectId, u64)>) {
    let mut pack = Vec::new();
    pack.extend_from_slice(b"PACK");
    pack.extend_from_slice(&2u32.to_be_bytes());
    pack.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    let mut ids: Vec<(ObjectId, u64)> = Vec::new();
    for entry in entries {
        let offset = pack.len() as u64;
        match entry {
            PackEntry::Full(kind, data) => {
                pack.extend_from_slice(&entry_header(type_num(kind), data.len()));
                pack.extend_from_slice(&makepad_fast_inflate::zlib_compress(data, 6));
                ids.push((crate::oid::hash_object(kind.as_str(), data), offset));
            }
            PackEntry::RefDelta { kind, base, delta, result } => {
                pack.extend_from_slice(&entry_header(7, delta.len()));
                pack.extend_from_slice(base.as_bytes());
                pack.extend_from_slice(&makepad_fast_inflate::zlib_compress(delta, 6));
                ids.push((crate::oid::hash_object(kind.as_str(), result), offset));
            }
            PackEntry::OfsDelta { kind, base_index, delta, result } => {
                pack.extend_from_slice(&entry_header(6, delta.len()));
                let base_offset = ids[*base_index].1;
                let mut distance = offset - base_offset;
                // git's ofs-delta encoding: big-endian 7-bit groups, each
                // continuation adds one.
                let mut bytes = vec![(distance & 0x7f) as u8];
                distance >>= 7;
                while distance > 0 {
                    distance -= 1;
                    bytes.push(0x80 | (distance & 0x7f) as u8);
                    distance >>= 7;
                }
                bytes.reverse();
                pack.extend_from_slice(&bytes);
                pack.extend_from_slice(&makepad_fast_inflate::zlib_compress(delta, 6));
                ids.push((crate::oid::hash_object(kind.as_str(), result), offset));
            }
        }
    }
    let mut hasher = Sha1::new();
    hasher.update(&pack);
    let pack_sha = hasher.finalize();
    pack.extend_from_slice(&pack_sha);
    (pack, ids)
}

/// Write a version-2 pack plus its v2 index into `git_dir/objects/pack`,
/// without a git binary. Returns the object ids in input order.
pub fn write_pack_entries(git_dir: &Path, entries: &[PackEntry]) -> Result<Vec<ObjectId>, GitError> {
    let (pack, ids) = build_pack_bytes(entries);
    let pack_sha: [u8; 20] = pack[pack.len() - 20..].try_into().expect("pack checksum");
    let mut sorted = ids.clone();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut idx = Vec::new();
    idx.extend_from_slice(&[0xff, 0x74, 0x4f, 0x63]);
    idx.extend_from_slice(&2u32.to_be_bytes());
    let mut fanout = [0u32; 256];
    for (oid, _) in &sorted {
        for slot in fanout.iter_mut().skip(oid.as_bytes()[0] as usize) {
            *slot += 1;
        }
    }
    for count in fanout {
        idx.extend_from_slice(&count.to_be_bytes());
    }
    for (oid, _) in &sorted {
        idx.extend_from_slice(oid.as_bytes());
    }
    for _ in &sorted {
        // CRC32 of the packed entry; the reader does not consult it.
        idx.extend_from_slice(&0u32.to_be_bytes());
    }
    for (_, offset) in &sorted {
        idx.extend_from_slice(&(*offset as u32).to_be_bytes());
    }
    idx.extend_from_slice(&pack_sha);
    let mut hasher = Sha1::new();
    hasher.update(&idx);
    let idx_sha = hasher.finalize();
    idx.extend_from_slice(&idx_sha);

    let name = ObjectId::from_slice(&pack_sha)?.to_hex();
    let pack_dir = git_dir.join("objects").join("pack");
    fs::create_dir_all(&pack_dir)?;
    fs::write(pack_dir.join(format!("pack-{name}.pack")), &pack)?;
    fs::write(pack_dir.join(format!("pack-{name}.idx")), &idx)?;
    Ok(ids.into_iter().map(|(oid, _)| oid).collect())
}

/// Write a version-2 pack of undeltified objects plus its v2 index into
/// `git_dir/objects/pack`, without a git binary. Returns the object ids in
/// input order.
pub fn write_pack(git_dir: &Path, objects: &[(ObjectKind, Vec<u8>)]) -> Result<Vec<ObjectId>, GitError> {
    let entries: Vec<PackEntry> = objects
        .iter()
        .map(|(kind, data)| PackEntry::Full(*kind, data.clone()))
        .collect();
    write_pack_entries(git_dir, &entries)
}

/// Move every loose object of `git_dir` into one pack (what `git gc` does
/// for a small repository), leaving no loose objects behind.
pub fn pack_all_loose(git_dir: &Path) -> Result<Vec<ObjectId>, GitError> {
    let objects = git_dir.join("objects");
    let mut entries = Vec::new();
    let mut files = Vec::new();
    for dir in fs::read_dir(&objects)? {
        let dir = dir?;
        let name = dir.file_name();
        let name = name.to_string_lossy();
        if name.len() != 2 || !dir.path().is_dir() {
            continue;
        }
        for file in fs::read_dir(dir.path())? {
            let file = file?;
            let hex = format!("{}{}", name, file.file_name().to_string_lossy());
            let oid = ObjectId::from_hex(&hex)?;
            let object = crate::object::read_loose_object(git_dir, &oid)?;
            entries.push(PackEntry::Full(object.kind, object.data));
            files.push(file.path());
        }
    }
    let ids = write_pack_entries(git_dir, &entries)?;
    for file in files {
        fs::remove_file(file)?;
    }
    Ok(ids)
}
