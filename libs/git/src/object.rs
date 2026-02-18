use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::path::Path;

use crate::error::GitError;
use crate::oid::{hash_object, ObjectId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Blob,
    Tree,
    Commit,
    Tag,
}

impl ObjectKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ObjectKind::Blob => "blob",
            ObjectKind::Tree => "tree",
            ObjectKind::Commit => "commit",
            ObjectKind::Tag => "tag",
        }
    }

    pub fn from_str(s: &str) -> Result<Self, GitError> {
        match s {
            "blob" => Ok(ObjectKind::Blob),
            "tree" => Ok(ObjectKind::Tree),
            "commit" => Ok(ObjectKind::Commit),
            "tag" => Ok(ObjectKind::Tag),
            _ => Err(GitError::InvalidObject(format!(
                "unknown object type: {}",
                s
            ))),
        }
    }

    /// From pack file type number
    pub fn from_type_num(n: u8) -> Result<Self, GitError> {
        match n {
            1 => Ok(ObjectKind::Commit),
            2 => Ok(ObjectKind::Tree),
            3 => Ok(ObjectKind::Blob),
            4 => Ok(ObjectKind::Tag),
            _ => Err(GitError::InvalidObject(format!(
                "unknown type number: {}",
                n
            ))),
        }
    }
}

/// A raw git object: its type and uncompressed data.
#[derive(Debug, Clone)]
pub struct Object {
    pub kind: ObjectKind,
    pub data: Vec<u8>,
}

/// Read a loose object from .git/objects/xx/yyyyyy...
pub fn read_loose_object(git_dir: &Path, oid: &ObjectId) -> Result<Object, GitError> {
    let (dir, file) = oid.loose_path_components();
    let path = git_dir.join("objects").join(&dir).join(&file);

    let compressed = fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::ObjectNotFound(oid.to_hex())
        } else {
            GitError::Io(e)
        }
    })?;

    let mut decoder = ZlibDecoder::new(&compressed[..]);
    let mut raw = Vec::new();
    decoder.read_to_end(&mut raw).map_err(|e| {
        GitError::InvalidObject(format!("zlib decompress failed for {}: {}", oid, e))
    })?;

    // Parse header: "<type> <size>\0<data>"
    let null_pos = raw.iter().position(|&b| b == 0).ok_or_else(|| {
        GitError::InvalidObject(format!("no null byte in object header for {}", oid))
    })?;

    let header = std::str::from_utf8(&raw[..null_pos])
        .map_err(|_| GitError::InvalidObject(format!("invalid UTF-8 in header for {}", oid)))?;

    let space_pos = header
        .find(' ')
        .ok_or_else(|| GitError::InvalidObject(format!("no space in header for {}", oid)))?;

    let type_str = &header[..space_pos];
    let size_str = &header[space_pos + 1..];
    let kind = ObjectKind::from_str(type_str)?;
    let size: usize = size_str.parse().map_err(|_| {
        GitError::InvalidObject(format!(
            "invalid size in header for {}: '{}'",
            oid, size_str
        ))
    })?;

    let data = raw[null_pos + 1..].to_vec();
    if data.len() != size {
        return Err(GitError::InvalidObject(format!(
            "size mismatch for {}: header says {} but got {}",
            oid,
            size,
            data.len()
        )));
    }

    Ok(Object { kind, data })
}

/// Write a loose object to .git/objects/xx/yyyyyy...
/// Returns the ObjectId of the written object.
pub fn write_loose_object(
    git_dir: &Path,
    kind: ObjectKind,
    data: &[u8],
) -> Result<ObjectId, GitError> {
    write_loose_object_level(git_dir, kind, data, Compression::default())
}

/// Write a loose object with a specific compression level.
/// Level 1 is ~10x faster than default (6) with only slightly larger output.
pub fn write_loose_object_fast(
    git_dir: &Path,
    kind: ObjectKind,
    data: &[u8],
) -> Result<ObjectId, GitError> {
    write_loose_object_level(git_dir, kind, data, Compression::fast())
}

fn write_loose_object_level(
    git_dir: &Path,
    kind: ObjectKind,
    data: &[u8],
    level: Compression,
) -> Result<ObjectId, GitError> {
    let oid = hash_object(kind.as_str(), data);
    let (dir, file) = oid.loose_path_components();
    let obj_dir = git_dir.join("objects").join(&dir);
    let obj_path = obj_dir.join(&file);

    // Already exists — content-addressable, so we're done
    if obj_path.exists() {
        return Ok(oid);
    }

    // Build raw content: header + data
    let header = format!("{} {}\0", kind.as_str(), data.len());
    let mut encoder = ZlibEncoder::new(Vec::new(), level);
    encoder.write_all(header.as_bytes())?;
    encoder.write_all(data)?;
    let compressed = encoder.finish()?;

    // Write atomically: write to temp file, then rename
    fs::create_dir_all(&obj_dir)?;
    let tmp_path = obj_dir.join(format!("tmp_{}", std::process::id()));
    fs::write(&tmp_path, &compressed)?;
    fs::rename(&tmp_path, &obj_path).or_else(
        |_: std::io::Error| -> Result<(), std::io::Error> {
            // rename can fail if another process wrote the same object
            let _ = fs::remove_file(&tmp_path);
            Ok(())
        },
    )?;

    Ok(oid)
}

/// Copy a loose object file from one git dir to another (raw bytes, no decompress).
pub fn copy_loose_object(
    src_git_dir: &Path,
    dst_git_dir: &Path,
    oid: &ObjectId,
) -> Result<bool, GitError> {
    let (dir, file) = oid.loose_path_components();
    let src_path = src_git_dir.join("objects").join(&dir).join(&file);
    if !src_path.exists() {
        return Ok(false);
    }
    let dst_dir = dst_git_dir.join("objects").join(&dir);
    let dst_path = dst_dir.join(&file);
    if dst_path.exists() {
        return Ok(true);
    }
    fs::create_dir_all(&dst_dir)?;
    fs::copy(&src_path, &dst_path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn make_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        fs::write(dir.path().join("hello.txt"), "hello world\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn test_read_blob() {
        let dir = make_test_repo();
        let git_dir = dir.path().join(".git");

        // "hello world\n" as a blob
        let oid = ObjectId::from_hex("3b18e512dba79e4c8300dd08aeb37f8e728b8dad").unwrap();
        let obj = read_loose_object(&git_dir, &oid).unwrap();
        assert_eq!(obj.kind, ObjectKind::Blob);
        assert_eq!(obj.data, b"hello world\n");
    }

    #[test]
    fn test_write_and_read_blob() {
        let dir = make_test_repo();
        let git_dir = dir.path().join(".git");

        let data = b"test content for writing\n";
        let oid = write_loose_object(&git_dir, ObjectKind::Blob, data).unwrap();

        // Read it back
        let obj = read_loose_object(&git_dir, &oid).unwrap();
        assert_eq!(obj.kind, ObjectKind::Blob);
        assert_eq!(obj.data, data);

        // Verify git can read it too
        let output = Command::new("git")
            .args(["cat-file", "-p", &oid.to_hex()])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "test content for writing\n"
        );
    }
}
