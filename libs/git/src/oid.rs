use crate::error::GitError;
use crate::sha1::Sha1;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId([u8; 20]);

impl ObjectId {
    pub const ZERO: ObjectId = ObjectId([0u8; 20]);

    pub fn from_bytes(bytes: [u8; 20]) -> Self {
        ObjectId(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, GitError> {
        if bytes.len() != 20 {
            return Err(GitError::InvalidObjectId(format!(
                "expected 20 bytes, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 20];
        arr.copy_from_slice(bytes);
        Ok(ObjectId(arr))
    }

    pub fn from_hex(hex: &str) -> Result<Self, GitError> {
        if hex.len() != 40 {
            return Err(GitError::InvalidObjectId(format!(
                "expected 40 hex chars, got {}",
                hex.len()
            )));
        }
        let mut bytes = [0u8; 20];
        for i in 0..20 {
            bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| GitError::InvalidObjectId(format!("invalid hex: {}", hex)))?;
        }
        Ok(ObjectId(bytes))
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(40);
        for b in &self.0 {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// Returns the loose object path components: (first 2 hex chars, remaining 38)
    pub fn loose_path_components(&self) -> (String, String) {
        let hex = self.to_hex();
        (hex[..2].to_string(), hex[2..].to_string())
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", self.to_hex())
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Hash raw content with a git object header to produce an ObjectId.
/// Format: "<type> <size>\0<data>" — SHA-1 of this produces the OID.
pub fn hash_object(kind: &str, data: &[u8]) -> ObjectId {
    let mut hasher = ObjectHasher::new(kind, data.len());
    hasher.update(data);
    hasher.finish()
}

/// Incremental object hashing without a temporary header or content buffer.
/// `size` is the total content byte length supplied to `update`.
pub struct ObjectHasher(Sha1);

impl ObjectHasher {
    pub fn new(kind: &str, mut size: usize) -> Self {
        let mut hash = Sha1::new();
        hash.update(kind.as_bytes());
        hash.update(b" ");
        let mut decimal = [0u8; 20];
        let mut start = decimal.len();
        loop {
            start -= 1;
            decimal[start] = b'0' + (size % 10) as u8;
            size /= 10;
            if size == 0 {
                break;
            }
        }
        hash.update(&decimal[start..]);
        hash.update(&[0]);
        Self(hash)
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn finish(self) -> ObjectId {
        ObjectId(self.0.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_roundtrip() {
        let hex = "552e2a0e14dd313f5f572118a18fba67ad99699c";
        let oid = ObjectId::from_hex(hex).unwrap();
        assert_eq!(oid.to_hex(), hex);
    }

    #[test]
    fn test_hash_blob() {
        // printf "hello world\n" | git hash-object --stdin
        let data = b"hello world\n";
        let oid = hash_object("blob", data);
        assert_eq!(oid.to_hex(), "3b18e512dba79e4c8300dd08aeb37f8e728b8dad");
    }

    #[test]
    fn test_loose_path() {
        let oid = ObjectId::from_hex("552e2a0e14dd313f5f572118a18fba67ad99699c").unwrap();
        let (dir, file) = oid.loose_path_components();
        assert_eq!(dir, "55");
        assert_eq!(file, "2e2a0e14dd313f5f572118a18fba67ad99699c");
    }

    #[test]
    fn test_invalid_hex() {
        assert!(ObjectId::from_hex("zzzz").is_err());
        assert!(ObjectId::from_hex("552e2a0e14dd313f5f572118a18fba67ad99699").is_err());
        // 39 chars
    }
}
