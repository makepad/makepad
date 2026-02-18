use std::fs;
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
    let data = fs::read(&index_path)?;
    parse_index(&data)
}

/// Parse a git index from raw bytes.
pub fn parse_index(data: &[u8]) -> Result<Index, GitError> {
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

    let mut entries = Vec::with_capacity(num_entries);
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
            // v4: prefix-compressed — read varint strip count, then NUL-terminated suffix
            // We don't implement v4 prefix compression yet — just read until NUL
            let start = r.pos;
            while r.pos < r.data.len() && r.data[r.pos] != 0 {
                r.pos += 1;
            }
            let path = std::str::from_utf8(&r.data[start..r.pos])
                .map_err(|_| GitError::InvalidIndex("invalid UTF-8 path".into()))?
                .to_string();
            r.pos += 1; // skip NUL
            path
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

    // We skip extensions and the trailing checksum — not needed for reading

    Ok(Index { version, entries })
}

/// Write the index in v2 format.
pub fn write_index(git_dir: &Path, index: &Index) -> Result<(), GitError> {
    let data = serialize_index(index)?;
    let index_path = git_dir.join("index");
    let lock_path = git_dir.join("index.lock");
    fs::write(&lock_path, &data)?;
    fs::rename(&lock_path, &index_path)?;
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

    #[test]
    fn test_parse_real_index() {
        // Create a test repo and parse its index
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        fs::write(dir.path().join("hello.txt"), "hello\n").unwrap();
        fs::write(dir.path().join("world.txt"), "world\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let git_dir = dir.path().join(".git");
        let index = read_index(&git_dir).unwrap();
        assert_eq!(index.version, 2);
        assert_eq!(index.entries.len(), 2);
        assert_eq!(index.entries[0].path, "hello.txt");
        assert_eq!(index.entries[1].path, "world.txt");
    }

    #[test]
    fn test_serialize_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        fs::write(dir.path().join("a.txt"), "aaa\n").unwrap();
        fs::write(dir.path().join("b.txt"), "bbb\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let git_dir = dir.path().join(".git");
        let index = read_index(&git_dir).unwrap();

        // Serialize and re-parse
        let data = serialize_index(&index).unwrap();
        let index2 = parse_index(&data).unwrap();

        assert_eq!(index2.entries.len(), index.entries.len());
        for (a, b) in index.entries.iter().zip(index2.entries.iter()) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.oid, b.oid);
            assert_eq!(a.mode, b.mode);
        }
    }
}
