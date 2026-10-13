//! Relax's git client: exactly the read path `run <git url>` needs (bootloader fetch.rs into the
//! store, gitstore.rs). Smart HTTP upload-pack (the ref advertisement from /info/refs, want/have
//! negotiation, the pack out of the response), the pack's objects with ofs/ref deltas resolved in
//! memory, object ids. The HTTP transport is the caller's (it turns a [`smart_http::Request`] into
//! a [`smart_http::Response`]); inflate is crate::inflate. Nothing is written anywhere: no
//! checkout, index, refs, push or local repository.
pub mod sha1;
pub mod smart_http;
pub mod unpack;

pub use smart_http::{
    build_info_refs_request, build_upload_pack_request, extract_pack_from_response, parse_info_refs_response, Method,
    RemoteHead, Request, Response,
};
pub use unpack::unpack_pack;

use std::fmt;

#[derive(Debug)]
pub enum GitError {
    InvalidObjectId(String),
    InvalidObject(String),
    InvalidRef(String),
    RefNotFound(String),
    CorruptPack(String),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::InvalidObjectId(s) => write!(f, "invalid object id: {s}"),
            GitError::InvalidObject(s) => write!(f, "invalid object: {s}"),
            GitError::InvalidRef(s) => write!(f, "invalid ref: {s}"),
            GitError::RefNotFound(s) => write!(f, "ref not found: {s}"),
            GitError::CorruptPack(s) => write!(f, "corrupt pack: {s}"),
        }
    }
}

impl std::error::Error for GitError {}

/// A SHA-1 object id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(pub [u8; 20]);

impl ObjectId {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, GitError> {
        let arr: [u8; 20] = bytes.try_into().map_err(|_| GitError::InvalidObjectId(format!("expected 20 bytes, got {}", bytes.len())))?;
        Ok(ObjectId(arr))
    }

    pub fn from_hex(hex: &str) -> Result<Self, GitError> {
        if hex.len() != 40 || !hex.is_ascii() {
            return Err(GitError::InvalidObjectId(format!("expected 40 hex chars: {hex}")));
        }
        let mut bytes = [0u8; 20];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| GitError::InvalidObjectId(format!("invalid hex: {hex}")))?;
        }
        Ok(ObjectId(bytes))
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(40);
        for b in &self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", self.to_hex())
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// The id git gives `data` as an object of `kind`: SHA-1 of "<kind> <len>\0<data>".
pub fn hash_object(kind: ObjectKind, data: &[u8]) -> ObjectId {
    let mut h = sha1::Sha1::new();
    h.update(format!("{} {}\0", kind.as_str(), data.len()).as_bytes());
    h.update(data);
    ObjectId(h.finalize())
}

/// An object's type; the numbers are the pack's (and the store's: gitstore::COMMIT..TAG).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Commit = 1,
    Tree = 2,
    Blob = 3,
    Tag = 4,
}

impl ObjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ObjectKind::Commit => "commit",
            ObjectKind::Tree => "tree",
            ObjectKind::Blob => "blob",
            ObjectKind::Tag => "tag",
        }
    }

    pub fn from_type_num(n: u8) -> Result<Self, GitError> {
        match n {
            1 => Ok(ObjectKind::Commit),
            2 => Ok(ObjectKind::Tree),
            3 => Ok(ObjectKind::Blob),
            4 => Ok(ObjectKind::Tag),
            _ => Err(GitError::InvalidObject(format!("unknown type number: {n}"))),
        }
    }
}

/// An object: its type and its uncompressed data.
#[derive(Debug, Clone)]
pub struct Object {
    pub kind: ObjectKind,
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_and_blob_id() {
        let hex = "552e2a0e14dd313f5f572118a18fba67ad99699c";
        assert_eq!(ObjectId::from_hex(hex).unwrap().to_hex(), hex);
        assert!(ObjectId::from_hex("zzzz").is_err());
        assert!(ObjectId::from_hex("552e2a0e14dd313f5f572118a18fba67ad99699").is_err());
        // printf "hello world\n" | git hash-object --stdin
        assert_eq!(hash_object(ObjectKind::Blob, b"hello world\n").to_hex(), "3b18e512dba79e4c8300dd08aeb37f8e728b8dad");
    }
}
