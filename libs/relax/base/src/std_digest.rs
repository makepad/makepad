//! Binary/std compatibility: SHA-256 of sorted UTF-8 relative paths and exact file bytes.
//! Shared with the compiler build script. Length prefixes make the encoding unambiguous;
//! neither checkout location, mtimes, nor the repository's other files affect the digest.
pub fn digest(mut files: Vec<(String, Vec<u8>)>) -> String {
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut bytes = b"relax-std-tree-v1\0".to_vec();
    for (path, data) in files {
        bytes.extend_from_slice(&(path.len() as u64).to_le_bytes());
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&data);
    }
    crate::sha256::sha256_hex(&bytes)
}
