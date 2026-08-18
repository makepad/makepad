//! Canonical bounded binary encoding ("canon v1") for every manifest and DTO
//! in this crate.
//!
//! Determinism rules, frozen for schema version 1:
//! - fixed-width big-endian integers; no varints;
//! - length-prefixed UTF-8 strings, bounded before allocation;
//! - no maps anywhere — repeated fields are vectors whose canonical order is
//!   enforced by `validate()`, so identical values always produce identical
//!   bytes regardless of how a producer's map iterated;
//! - `f32` stored as IEEE-754 bits; NaN/Inf are refused and `-0.0` is
//!   normalized to `+0.0` on encode and refused on decode, so every accepted
//!   byte string is the unique encoding of its value;
//! - every document starts with magic + document kind + schema version and
//!   must consume its input exactly.
//!
//! Decoding is total and fail-closed: unknown tags, non-canonical forms,
//! over-budget lengths, and trailing bytes are `AssetDataError`s, never guesses.

use crate::error::AssetDataError;
use crate::limits::MAX_DOCUMENT_BYTES;

/// The canonical-encoding magic. Version is carried separately per document.
pub(crate) const MAGIC: [u8; 4] = *b"MPC1";

/// One shared schema version for the whole frozen contract. Any change to any
/// canonical type bumps this and old decoders refuse rather than misread.
/// v2: activation ready DTOs pin the aggregate realm-resolution digest, the
/// realm descriptor pins the variant policy version, and the content lock
/// pins the per-asset variant-set table.
/// v3: `Rights` becomes the complete rights record — exact license identity
/// and revision, pinned terms digest/URL, upstream source and archive
/// digest, and the source-determined redistribution/derivative policy.
pub const CONTENT_SCHEMA_VERSION: u16 = 3;

/// Document kind tags, so one manifest kind can never be decoded as another.
pub(crate) mod dockind {
    pub const ASSET_MANIFEST: u8 = 1;
    pub const CONTENT_LOCK: u8 = 2;
    pub const GAME_REVISION: u8 = 3;
    pub const CONTENT_SET: u8 = 4;
    pub const SCENE_PLAN: u8 = 5;
    pub const MIGRATION_PLAN: u8 = 6;
    pub const SOURCE_COLLECTION: u8 = 7;
    pub const IMPORT_MANIFEST: u8 = 8;
    pub const PROCESSING_RECIPE: u8 = 9;
    pub const DERIVED_VARIANT: u8 = 10;
    pub const VARIANT_SET: u8 = 11;
    pub const CLIENT_PROFILE: u8 = 12;
    pub const RESOLVED_VARIANT_MAP: u8 = 13;
    pub const REALM_RESOLUTION: u8 = 14;
    pub const REALM_DESCRIPTOR: u8 = 16;
    pub const JOIN_CONTENT_READY: u8 = 17;
    pub const PREPARE_SCENE_CHANGE: u8 = 18;
    pub const SCENE_CONTENT_READY: u8 = 19;
    pub const COMMIT_SCENE_CHANGE: u8 = 20;
    pub const SCENE_APPLIED: u8 = 21;
    pub const PREPARE_CONTENT_CHANGE: u8 = 22;
    pub const CONTENT_CHANGE_READY: u8 = 23;
    pub const COMMIT_CONTENT_CHANGE: u8 = 24;
    pub const PREPARE_REALM: u8 = 25;
    pub const COMMIT_REALM: u8 = 26;
    pub const ROOM_CONTENT_TUPLE: u8 = 27;
    pub const CONTENT_REFUSAL: u8 = 28;
    pub const SNAPSHOT_BEGIN: u8 = 32;
    pub const SNAPSHOT_CHUNK: u8 = 33;
    pub const SNAPSHOT_END: u8 = 34;
    pub const SNAPSHOT_READY: u8 = 35;
}

/// Infallible byte writer. Encoding never runs on an unvalidated value:
/// every `to_canonical_bytes` validates first, so bounds hold by then.
pub(crate) struct CanonWriter {
    buf: Vec<u8>,
}

impl CanonWriter {
    pub fn new(kind: u8) -> Self {
        let mut w = Self { buf: Vec::new() };
        w.buf.extend_from_slice(&MAGIC);
        w.u8(kind);
        w.u16(CONTENT_SCHEMA_VERSION);
        w
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn bool(&mut self, v: bool) {
        self.buf.push(v as u8);
    }
    pub fn f32(&mut self, v: f32) {
        // Validation has already refused non-finite values; -0.0 is legal
        // input but has exactly one canonical encoding.
        let v = if v == 0.0 { 0.0 } else { v };
        self.buf.extend_from_slice(&v.to_bits().to_be_bytes());
    }
    pub fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.buf.extend_from_slice(b);
    }
    pub fn raw32(&mut self, b: &[u8; 32]) {
        self.buf.extend_from_slice(b);
    }
    pub fn raw16(&mut self, b: &[u8; 16]) {
        self.buf.extend_from_slice(b);
    }

    pub fn finish(self) -> Result<Vec<u8>, AssetDataError> {
        if self.buf.len() > MAX_DOCUMENT_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "document bytes",
                limit: MAX_DOCUMENT_BYTES as u64,
                found: self.buf.len() as u64,
            });
        }
        Ok(self.buf)
    }
}

/// Bounded, total reader over untrusted bytes.
pub(crate) struct CanonReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> CanonReader<'a> {
    pub fn new(buf: &'a [u8], kind: u8) -> Result<Self, AssetDataError> {
        if buf.len() > MAX_DOCUMENT_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "document bytes",
                limit: MAX_DOCUMENT_BYTES as u64,
                found: buf.len() as u64,
            });
        }
        let mut r = Self { buf, pos: 0 };
        let magic = r.take(4, "magic")?;
        if magic != MAGIC {
            return Err(AssetDataError::BadMagic);
        }
        let found_kind = r.u8("document kind")?;
        if found_kind != kind {
            return Err(AssetDataError::BadDocKind {
                expected: kind,
                found: found_kind,
            });
        }
        let version = r.u16("schema version")?;
        if version != CONTENT_SCHEMA_VERSION {
            return Err(AssetDataError::UnsupportedSchema { found: version });
        }
        Ok(r)
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], AssetDataError> {
        if self.buf.len() - self.pos < n {
            return Err(AssetDataError::Truncated { what });
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn u8(&mut self, what: &'static str) -> Result<u8, AssetDataError> {
        Ok(self.take(1, what)?[0])
    }
    pub fn u16(&mut self, what: &'static str) -> Result<u16, AssetDataError> {
        let b = self.take(2, what)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    pub fn u32(&mut self, what: &'static str) -> Result<u32, AssetDataError> {
        let b = self.take(4, what)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn u64(&mut self, what: &'static str) -> Result<u64, AssetDataError> {
        let b = self.take(8, what)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    pub fn i64(&mut self, what: &'static str) -> Result<i64, AssetDataError> {
        Ok(self.u64(what)? as i64)
    }
    pub fn bool(&mut self, what: &'static str) -> Result<bool, AssetDataError> {
        match self.u8(what)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(AssetDataError::Malformed { what }),
        }
    }
    pub fn f32(&mut self, what: &'static str) -> Result<f32, AssetDataError> {
        let bits = self.u32(what)?;
        let v = f32::from_bits(bits);
        if !v.is_finite() || bits == 0x8000_0000 {
            return Err(AssetDataError::Malformed { what });
        }
        Ok(v)
    }
    pub fn str(&mut self, what: &'static str, max: usize) -> Result<String, AssetDataError> {
        let len = self.u32(what)? as usize;
        if len > max {
            return Err(AssetDataError::OverBudget {
                what,
                limit: max as u64,
                found: len as u64,
            });
        }
        let bytes = self.take(len, what)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| AssetDataError::Malformed { what })
    }
    pub fn bytes(&mut self, what: &'static str, max: usize) -> Result<Vec<u8>, AssetDataError> {
        let len = self.u32(what)? as usize;
        if len > max {
            return Err(AssetDataError::OverBudget {
                what,
                limit: max as u64,
                found: len as u64,
            });
        }
        Ok(self.take(len, what)?.to_vec())
    }
    pub fn raw32(&mut self, what: &'static str) -> Result<[u8; 32], AssetDataError> {
        let b = self.take(32, what)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(b);
        Ok(out)
    }
    pub fn raw16(&mut self, what: &'static str) -> Result<[u8; 16], AssetDataError> {
        let b = self.take(16, what)?;
        let mut out = [0u8; 16];
        out.copy_from_slice(b);
        Ok(out)
    }

    /// Read a collection count, refusing both over-budget counts and counts
    /// that cannot possibly fit the remaining bytes (allocation bombs).
    pub fn count(&mut self, what: &'static str, max: usize) -> Result<usize, AssetDataError> {
        let n = self.u32(what)? as usize;
        if n > max {
            return Err(AssetDataError::OverBudget {
                what,
                limit: max as u64,
                found: n as u64,
            });
        }
        // Every element encodes to at least one byte.
        if n > self.remaining() {
            return Err(AssetDataError::Truncated { what });
        }
        Ok(n)
    }

    pub fn finish(&self) -> Result<(), AssetDataError> {
        if self.pos != self.buf.len() {
            return Err(AssetDataError::TrailingBytes);
        }
        Ok(())
    }
}

/// Canonical order for a repeated field: strictly increasing by key, which
/// gives sortedness and uniqueness in one check. Both `validate()` (producer
/// side) and `from_canonical_bytes` (consumer side) go through this, so a
/// producer iterating a map in arbitrary order cannot ship non-canonical bytes.
pub(crate) fn check_sorted_unique<T, K: Ord, F: Fn(&T) -> K>(
    items: &[T],
    key: F,
    what: &'static str,
) -> Result<(), AssetDataError> {
    for pair in items.windows(2) {
        match key(&pair[0]).cmp(&key(&pair[1])) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => return Err(AssetDataError::Duplicate { what }),
            std::cmp::Ordering::Greater => return Err(AssetDataError::NotSorted { what }),
        }
    }
    Ok(())
}

/// Implements `tag`/`from_tag`/`decode` for a payload-free enum with frozen
/// canonical tag numbers.
macro_rules! canon_enum {
    ($ty:ident { $($variant:ident = $tag:literal),+ $(,)? }) => {
        impl $ty {
            pub(crate) fn tag(self) -> u8 {
                match self { $($ty::$variant => $tag),+ }
            }
            pub(crate) fn from_tag(t: u8) -> Result<Self, $crate::error::AssetDataError> {
                match t {
                    $($tag => Ok($ty::$variant),)+
                    _ => Err($crate::error::AssetDataError::BadTag {
                        what: stringify!($ty),
                        found: t,
                    }),
                }
            }
            pub(crate) fn decode(
                r: &mut $crate::codec::CanonReader,
            ) -> Result<Self, $crate::error::AssetDataError> {
                Self::from_tag(r.u8(stringify!($ty))?)
            }
        }
    };
}
pub(crate) use canon_enum;
