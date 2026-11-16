use std::fs;
use std::io::Write;
use std::path::Path;

use crate::error::GitError;
use crate::oid::ObjectId;

/// A single entry in the git index (staging area).
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub ctime_sec: u32,
    pub ctime_nsec: u32,
    pub mtime_sec: u32,
    pub mtime_nsec: u32,
    pub dev: u32,
    pub ino: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub file_size: u32,
    pub oid: ObjectId,
    pub flags: u16,
    pub path: String,
}

impl IndexEntry {
    /// Stage number (0 = normal, 1-3 = merge conflict stages)
    pub fn stage(&self) -> u8 {
        ((self.flags >> 12) & 0x3) as u8
    }
}

/// The git index file.
#[derive(Debug, Clone)]
pub struct Index {
    pub version: u32,
    pub entries: Vec<IndexEntry>,
}

struct BufReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BufReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BufReader { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn read_u16(&mut self) -> Result<u16, GitError> {
        if self.remaining() < 2 {
            return Err(GitError::InvalidIndex("unexpected end of index".into()));
        }
        let val = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(val)
    }

    fn read_u32(&mut self) -> Result<u32, GitError> {
        if self.remaining() < 4 {
            return Err(GitError::InvalidIndex("unexpected end of index".into()));
        }
        let val = u32::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(val)
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], GitError> {
        if self.remaining() < n {
            return Err(GitError::InvalidIndex("unexpected end of index".into()));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }
}

/// Read and parse the .git/index file.
pub fn read_index(git_dir: &Path) -> Result<Index, GitError> {
    let index_path = git_dir.join("index");
    let data = fs::read(&index_path).map_err(|e| GitError::io_at("read", &index_path, e))?;
    parse_index(&data)
}

/// Parse a git index from raw bytes.
pub fn parse_index(data: &[u8]) -> Result<Index, GitError> {
    parse_index_entries(data).map(|(index, _)| index)
}

/// Read an index that can be rewritten from its entries without losing split
/// or sparse state. Those extensions require information beyond this model.
pub fn read_index_for_update(git_dir: &Path) -> Result<Index, GitError> {
    let index_path = git_dir.join("index");
    let data = fs::read(&index_path).map_err(|e| GitError::io_at("read", &index_path, e))?;
    let (index, end) = parse_index_entries(&data)?;
    if special_index_extensions(&data, end)? {
        return Err(GitError::InvalidIndex("cannot rewrite a split or sparse index".into()));
    }
    if index.entries.iter().any(|e| e.flags & 0x4000 != 0) {
        return Err(GitError::InvalidIndex("cannot rewrite extended index flags".into()));
    }
    Ok(index)
}

/// Whether the index carries split-index or sparse-index state.
pub fn is_split_or_sparse(data: &[u8]) -> Result<bool, GitError> {
    let (_, end) = parse_index_entries(data)?;
    special_index_extensions(data, end)
}

fn special_index_extensions(data: &[u8], end: usize) -> Result<bool, GitError> {
    let payload_end = data.len().checked_sub(20).filter(|n| *n >= end)
        .ok_or_else(|| GitError::InvalidIndex("missing index checksum".into()))?;
    let mut r = BufReader::new(&data[end..payload_end]);
    let mut special = false;
    while r.remaining() != 0 {
        let name = r.read_bytes(4)?;
        special |= name == b"link" || name == b"sdir";
        let len = r.read_u32()? as usize;
        r.read_bytes(len)?;
    }
    Ok(special)
}

fn parse_index_entries(data: &[u8]) -> Result<(Index, usize), GitError> {
    let mut r = BufReader::new(data);

    // Header: "DIRC" + version + entry count
    let sig = r.read_bytes(4)?;
    if sig != b"DIRC" {
        return Err(GitError::InvalidIndex(format!("bad signature: {:?}", sig)));
    }
    let version = r.read_u32()?;
    if version < 2 || version > 4 {
        return Err(GitError::InvalidIndex(format!(
            "unsupported version: {}",
            version
        )));
    }
    let num_entries = r.read_u32()? as usize;

    let mut entries: Vec<IndexEntry> = Vec::new();
    for _ in 0..num_entries {
        let entry_start = r.pos;

        let ctime_sec = r.read_u32()?;
        let ctime_nsec = r.read_u32()?;
        let mtime_sec = r.read_u32()?;
        let mtime_nsec = r.read_u32()?;
        let dev = r.read_u32()?;
        let ino = r.read_u32()?;
        let mode = r.read_u32()?;
        let uid = r.read_u32()?;
        let gid = r.read_u32()?;
        let file_size = r.read_u32()?;
        let sha = r.read_bytes(20)?;
        let oid = ObjectId::from_slice(sha)?;
        let flags = r.read_u16()?;

        // Extended flags for v3+
        if version >= 3 && (flags & 0x4000) != 0 {
            let _ext_flags = r.read_u16()?;
        }

        // Path name
        let name_len = (flags & 0xFFF) as usize;
        let path = if version < 4 {
            // v2/v3: NUL-terminated, padded to 8-byte boundary
            let name_len = if name_len == 0xfff {
                r.data[r.pos..].iter().position(|b| *b == 0)
                    .ok_or_else(|| GitError::InvalidIndex("unterminated index path".into()))?
            } else { name_len };
            let name_bytes = r.read_bytes(name_len)?;
            let path = std::str::from_utf8(name_bytes)
                .map_err(|_| GitError::InvalidIndex("invalid UTF-8 path".into()))?
                .to_string();

            // Skip padding (entry must end on 8-byte boundary, minimum 1 NUL)
            let entry_len = r.pos - entry_start;
            let padded_len = (entry_len + 8) & !7;
            let pad = padded_len - entry_len;
            r.read_bytes(pad)?;

            path
        } else {
            // v4 removes a suffix of the previous path before appending this one.
            let mut b = r.read_bytes(1)?[0];
            let mut strip = (b & 0x7f) as usize;
            while b & 0x80 != 0 {
                b = r.read_bytes(1)?[0];
                strip = strip.checked_add(1).and_then(|n| n.checked_mul(128))
                    .and_then(|n| n.checked_add((b & 0x7f) as usize))
                    .ok_or_else(|| GitError::InvalidIndex("index path prefix overflow".into()))?;
            }
            let previous = entries.last().map_or("", |e| e.path.as_str());
            let keep = previous.len().checked_sub(strip)
                .ok_or_else(|| GitError::InvalidIndex("invalid index path prefix".into()))?;
            let len = r.data[r.pos..].iter().position(|b| *b == 0)
                .ok_or_else(|| GitError::InvalidIndex("unterminated index path".into()))?;
            let mut bytes = previous.as_bytes()[..keep].to_vec();
            bytes.extend_from_slice(r.read_bytes(len)?);
            r.read_bytes(1)?;
            String::from_utf8(bytes).map_err(|_| GitError::InvalidIndex("invalid UTF-8 path".into()))?
        };

        entries.push(IndexEntry {
            ctime_sec,
            ctime_nsec,
            mtime_sec,
            mtime_nsec,
            dev,
            ino,
            mode,
            uid,
            gid,
            file_size,
            oid,
            flags,
            path,
        });
    }

    Ok((Index { version, entries }, r.pos))
}

/// Write the index in v2 format.
/// The lock is taken exclusively; a held lock is refused, never written through.
pub fn write_index(git_dir: &Path, index: &Index) -> Result<(), GitError> {
    write_index_inner(git_dir, index, false)
}

/// Publish a complete index only after its bytes and directory entry are durable.
pub fn write_index_durable(git_dir: &Path, index: &Index) -> Result<(), GitError> {
    write_index_inner(git_dir, index, true)
}

fn write_index_inner(git_dir: &Path, index: &Index, durable: bool) -> Result<(), GitError> {
    let data = serialize_index(index)?;
    let index_path = git_dir.join("index");
    let lock_path = git_dir.join("index.lock");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                GitError::InvalidIndex(format!(
                    "{} is locked: another git writes it ({} exists)",
                    index_path.display(),
                    lock_path.display()
                ))
            } else {
                GitError::io_at("create", &lock_path, e)
            }
        })?;
    let written = file.write_all(&data).and_then(|_| if durable { file.sync_all() } else { Ok(()) });
    drop(file);
    if let Err(e) = written {
        let _ = fs::remove_file(&lock_path);
        return Err(GitError::io_at("write", &lock_path, e));
    }
    if let Err(e) = fs::rename(&lock_path, &index_path) {
        let _ = fs::remove_file(&lock_path);
        return Err(GitError::io_at("rename", &lock_path, e));
    }
    #[cfg(unix)]
    if durable { fs::File::open(git_dir).and_then(|d| d.sync_all()).map_err(|e| GitError::io_at("sync", git_dir, e))?; }
    Ok(())
}

/// Serialize an index to binary v2 format.
pub fn serialize_index(index: &Index) -> Result<Vec<u8>, GitError> {
    use crate::sha1::Sha1;

    let mut buf = Vec::new();

    // Header
    buf.extend_from_slice(b"DIRC");
    buf.extend_from_slice(&2u32.to_be_bytes()); // version 2
    buf.extend_from_slice(&(index.entries.len() as u32).to_be_bytes());

    for entry in &index.entries {
        let entry_start = buf.len();

        buf.extend_from_slice(&entry.ctime_sec.to_be_bytes());
        buf.extend_from_slice(&entry.ctime_nsec.to_be_bytes());
        buf.extend_from_slice(&entry.mtime_sec.to_be_bytes());
        buf.extend_from_slice(&entry.mtime_nsec.to_be_bytes());
        buf.extend_from_slice(&entry.dev.to_be_bytes());
        buf.extend_from_slice(&entry.ino.to_be_bytes());
        buf.extend_from_slice(&entry.mode.to_be_bytes());
        buf.extend_from_slice(&entry.uid.to_be_bytes());
        buf.extend_from_slice(&entry.gid.to_be_bytes());
        buf.extend_from_slice(&entry.file_size.to_be_bytes());
        buf.extend_from_slice(entry.oid.as_bytes());

        // Flags: name length capped at 0xFFF
        let name_len = entry.path.len().min(0xFFF) as u16;
        let flags = (entry.flags & 0xF000) | name_len;
        buf.extend_from_slice(&flags.to_be_bytes());

        // Path name
        buf.extend_from_slice(entry.path.as_bytes());

        // Padding: entry must end on 8-byte boundary (at least 1 NUL)
        let entry_len = buf.len() - entry_start;
        let padded_len = (entry_len + 8) & !7;
        let pad = padded_len - entry_len;
        for _ in 0..pad {
            buf.push(0);
        }
    }

    // Trailing checksum (SHA-1 of everything)
    let mut hasher = Sha1::new();
    hasher.update(&buf);
    let checksum = hasher.finalize();
    buf.extend_from_slice(&checksum);

    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oid::hash_object;

    fn entry(path: &str, content: &[u8]) -> IndexEntry {
        IndexEntry {
            ctime_sec: 1,
            ctime_nsec: 0,
            mtime_sec: 1,
            mtime_nsec: 0,
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: content.len() as u32,
            oid: hash_object("blob", content),
            flags: path.len().min(0xfff) as u16,
            path: path.to_string(),
        }
    }

    #[test]
    fn durable_index_and_extension_detection() {
        let dir = crate::test_support::tempdir().unwrap();
        let index = Index { version: 2, entries: vec![entry("a", b"contents")] };
        write_index_durable(dir.path(), &index).unwrap();
        let read = read_index_for_update(dir.path()).unwrap();
        assert_eq!(serialize_index(&read).unwrap(), serialize_index(&index).unwrap(), "durable index round-trip");
        assert!(!dir.path().join("index.lock").exists(), "the lock was renamed");
        for name in [b"link", b"sdir"] {
            let mut data = serialize_index(&index).unwrap();
            data.truncate(data.len() - 20);
            data.extend_from_slice(name);
            data.extend_from_slice(&0u32.to_be_bytes());
            let mut hash = crate::Sha1::new();
            hash.update(&data);
            data.extend_from_slice(&hash.finalize());
            assert!(is_split_or_sparse(&data).unwrap(), "special extension is detected");
            fs::write(dir.path().join("index"), data).unwrap();
            assert!(read_index_for_update(dir.path()).is_err(), "special index is not rewritten from entries");
        }
    }

    #[test]
    fn compressed_and_long_index_paths_remain_complete() {
        let entries = vec![entry("dir/a", b"a"), entry("dir/b", b"b")];
        let mut data = b"DIRC".to_vec();
        data.extend_from_slice(&4u32.to_be_bytes());
        data.extend_from_slice(&2u32.to_be_bytes());
        for (e, suffix) in entries.iter().zip([&b"\0dir/a\0"[..], &b"\x01b\0"[..]]) {
            let one = serialize_index(&Index { version: 2, entries: vec![e.clone()] }).unwrap();
            data.extend_from_slice(&one[12..74]);
            data.extend_from_slice(suffix);
        }
        let mut hash = crate::Sha1::new();
        hash.update(&data);
        data.extend_from_slice(&hash.finalize());
        let read = parse_index(&data).unwrap();
        assert_eq!(read.entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["dir/a", "dir/b"], "v4 paths expand before rewriting");
        assert!(!is_split_or_sparse(&data).unwrap(), "v4 entries end at the extensions");
        let path = format!("{}a", "d/".repeat(2048));
        let data = serialize_index(&Index { version: 2, entries: vec![entry(&path, b"a")] }).unwrap();
        assert_eq!(parse_index(&data).unwrap().entries[0].path, path, "saturated name length still reads the full path");
    }

    #[test]
    fn test_parse_written_index() {
        // Write an index the way staging does and parse it back.
        let dir = crate::test_support::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let index = Index {
            version: 2,
            entries: vec![entry("hello.txt", b"hello\n"), entry("world.txt", b"world\n")],
        };
        write_index(&git_dir, &index).unwrap();

        let index = read_index(&git_dir).unwrap();
        assert_eq!(index.version, 2);
        assert_eq!(index.entries.len(), 2);
        assert_eq!(index.entries[0].path, "hello.txt");
        assert_eq!(index.entries[1].path, "world.txt");
        assert_eq!(index.entries[0].oid.to_hex(), "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    #[test]
    fn a_held_index_lock_is_refused_and_left_alone() {
        let dir = crate::test_support::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let first = Index {
            version: 2,
            entries: vec![entry("hello.txt", b"hello\n"), entry("world.txt", b"world\n")],
        };
        write_index(&git_dir, &first).unwrap();

        let lock_path = git_dir.join("index.lock");
        fs::write(&lock_path, b"another writer").unwrap();
        let second = Index {
            version: 2,
            entries: vec![entry("changed.txt", b"changed\n")],
        };
        let error = write_index(&git_dir, &second).unwrap_err();
        assert!(error.to_string().contains("is locked"));
        assert_eq!(fs::read(&lock_path).unwrap(), b"another writer");
        let unchanged = read_index(&git_dir).unwrap();
        assert_eq!(
            serialize_index(&unchanged).unwrap(),
            serialize_index(&first).unwrap(),
            "a refused write must preserve the first index's entries"
        );

        fs::remove_file(&lock_path).unwrap();
        write_index(&git_dir, &second).unwrap();
        assert!(!lock_path.exists());
    }

    #[test]
    fn test_serialize_roundtrip() {
        let index = Index {
            version: 2,
            entries: vec![entry("a.txt", b"aaa\n"), entry("b.txt", b"bbb\n")],
        };

        // Serialize and re-parse
        let data = serialize_index(&index).unwrap();
        assert!(data.starts_with(b"DIRC"));
        let index2 = parse_index(&data).unwrap();

        assert_eq!(index2.entries.len(), index.entries.len());
        for (a, b) in index.entries.iter().zip(index2.entries.iter()) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.oid, b.oid);
            assert_eq!(a.mode, b.mode);
        }
    }
}
