//! Stable identities: content digests, opaque IDs, readable aliases, and
//! stable scene/state keys.
//!
//! Digest newtypes are deliberately distinct so an asset revision can never be
//! passed where a game revision belongs. Display forms are canonical and parse
//! rejects any non-canonical spelling (uppercase hex, padded base32, spare
//! bits set), so a printed ID round-trips to exactly one value.

use crate::codec::{CanonReader, CanonWriter};
use crate::error::AssetDataError;
use crate::limits::*;
use crate::sha256::sha256;

// ---------------------------------------------------------------------------
// hex / base32 primitives
// ---------------------------------------------------------------------------

fn hex64_encode(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn hex64_decode(s: &str, what: &'static str) -> Result<[u8; 32], AssetDataError> {
    let s = s.as_bytes();
    if s.len() != 64 {
        return Err(AssetDataError::Malformed { what });
    }
    let mut out = [0u8; 32];
    for (i, pair) in s.chunks_exact(2).enumerate() {
        let nib = |c: u8| -> Result<u8, AssetDataError> {
            match c {
                b'0'..=b'9' => Ok(c - b'0'),
                // Lowercase only: uppercase would give one value two spellings.
                b'a'..=b'f' => Ok(c - b'a' + 10),
                _ => Err(AssetDataError::Malformed { what }),
            }
        };
        out[i] = (nib(pair[0])? << 4) | nib(pair[1])?;
    }
    Ok(out)
}

const BASE32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

fn base32_encode(bytes: &[u8; 16]) -> String {
    let mut out = String::with_capacity(26);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &b in bytes {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32_ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32_ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn base32_decode(s: &str, what: &'static str) -> Result<[u8; 16], AssetDataError> {
    let s = s.as_bytes();
    if s.len() != 26 {
        return Err(AssetDataError::Malformed { what });
    }
    let mut out = Vec::with_capacity(16);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in s {
        let v = match c {
            b'a'..=b'z' => c - b'a',
            b'2'..=b'7' => c - b'2' + 26,
            _ => return Err(AssetDataError::Malformed { what }),
        };
        acc = (acc << 5) | v as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // 26 chars carry 130 bits; the 2 spare bits must be zero or the same ID
    // would have four spellings.
    if acc & ((1 << bits) - 1) != 0 {
        return Err(AssetDataError::Malformed { what });
    }
    let mut id = [0u8; 16];
    id.copy_from_slice(&out);
    Ok(id)
}

// ---------------------------------------------------------------------------
// digest newtypes
// ---------------------------------------------------------------------------

macro_rules! digest_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 32]);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            pub fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }
            /// Digest of the given canonical bytes.
            pub fn hash_of(bytes: &[u8]) -> Self {
                Self(sha256(bytes))
            }
            pub fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
            // Some digest types are computed identities only and are never
            // embedded in a document, so their codec hooks may be unused.
            #[allow(dead_code)]
            pub(crate) fn encode(&self, w: &mut CanonWriter) {
                w.raw32(&self.0);
            }
            #[allow(dead_code)]
            pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
                Ok(Self(r.raw32(stringify!($name))?))
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}{}", $prefix, hex64_encode(&self.0))
            }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{self}")
            }
        }
        impl std::str::FromStr for $name {
            type Err = AssetDataError;
            fn from_str(s: &str) -> Result<Self, AssetDataError> {
                let hex = s.strip_prefix($prefix).ok_or(AssetDataError::Malformed {
                    what: stringify!($name),
                })?;
                Ok(Self(hex64_decode(hex, stringify!($name))?))
            }
        }
    };
}

digest_id! {
    /// `sha256:<hex>` of exact file bytes. The only identity a blob has.
    BlobId, "sha256:"
}
digest_id! {
    /// SHA-256 of a canonical `AssetManifest` document.
    AssetRevisionId, "arev_"
}
digest_id! {
    /// SHA-256 of a canonical `GameRevisionManifest` document.
    GameRevisionId, "grev_"
}
digest_id! {
    /// SHA-256 of a canonical `ContentSetManifest` document.
    ContentSetId, "cset_"
}
digest_id! {
    /// SHA-256 of a canonical `ScenePlan` document.
    ScenePlanDigest, "splan_"
}
digest_id! {
    /// SHA-256 of a canonical `SceneMigrationPlan` document.
    MigrationPlanDigest, "mplan_"
}
digest_id! {
    /// Digest over one snapshot's ordered chunk stream, computed identically
    /// by the host producer and the installing client.
    SnapshotDigest, "snap_"
}
digest_id! {
    /// SHA-256 of a canonical `SourceCollection` document — one approved
    /// external content source (for example the Kenney pack family).
    SourceCollectionId, "scol_"
}
digest_id! {
    /// SHA-256 of a canonical `ImportManifest` document. The identity of one
    /// deterministic external-pack import, including its exact entry map and
    /// source digests.
    ImportRevisionId, "irev_"
}
digest_id! {
    /// SHA-256 of a canonical `ProcessingRecipe` document. Every
    /// quality-affecting setting and the exact tool closure are inside, so a
    /// processor upgrade changes the digest instead of poisoning old caches.
    RecipeDigest, "recp_"
}
digest_id! {
    /// SHA-256 of a canonical `DerivedVariantManifest` document — one
    /// validated processing result tied to an exact base asset revision.
    DerivedVariantId, "dvar_"
}
digest_id! {
    /// SHA-256 of a canonical `VariantSetManifest` document — the bounded,
    /// frozen set of variants a release may resolve among.
    VariantSetId, "vset_"
}
digest_id! {
    /// The derivation single-flight cache key: a domain-separated digest over
    /// base revision, recipe digest, and ordered input roles/digests. Computed
    /// by [`crate::derived::derivation_key`], never hashed from a document.
    DerivationKey, "dkey_"
}
digest_id! {
    /// SHA-256 of a canonical `ClientProfile` document.
    ClientProfileDigest, "prof_"
}
digest_id! {
    /// SHA-256 of a canonical `ResolvedVariantMap` document — the exact
    /// per-client resolution of ONE asset's variant set.
    ResolvedMapDigest, "rmap_"
}
digest_id! {
    /// SHA-256 of a canonical `RealmResolution` document — the aggregate
    /// per-client resolution over every variant set a content set pins. This
    /// is the digest readiness messages acknowledge.
    RealmResolutionDigest, "rres_"
}

// ---------------------------------------------------------------------------
// opaque stable identities
// ---------------------------------------------------------------------------

macro_rules! opaque_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 16]);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            /// From explicit bytes. This crate never invents or infers an ID;
            /// the issuing authority (Asset Server) generates the bytes.
            pub fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }
            pub fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }
            pub(crate) fn encode(&self, w: &mut CanonWriter) {
                w.raw16(&self.0);
            }
            pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
                Ok(Self(r.raw16(stringify!($name))?))
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}{}", $prefix, base32_encode(&self.0))
            }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{self}")
            }
        }
        impl std::str::FromStr for $name {
            type Err = AssetDataError;
            fn from_str(s: &str) -> Result<Self, AssetDataError> {
                let body = s.strip_prefix($prefix).ok_or(AssetDataError::Malformed {
                    what: stringify!($name),
                })?;
                Ok(Self(base32_decode(body, stringify!($name))?))
            }
        }
    };
}

opaque_id! {
    /// `ast_<base32>` — stable across renames and revisions.
    AssetId, "ast_"
}
opaque_id! {
    /// `gam_<base32>` — stable identity of a Splash game.
    GameId, "gam_"
}
opaque_id! {
    /// `txn_<base32>` — one prepare/commit activation transaction.
    TransactionId, "txn_"
}

/// One exact `{asset_id, revision}` pair — the only way runtime records may
/// name an asset. Aliases are for authoring; refs are for multiplayer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetRevisionRef {
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
}

impl AssetRevisionRef {
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        self.asset_id.encode(w);
        self.revision.encode(w);
    }
    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            asset_id: AssetId::decode(r)?,
            revision: AssetRevisionId::decode(r)?,
        })
    }
}

impl std::fmt::Display for AssetRevisionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.asset_id, self.revision)
    }
}

// ---------------------------------------------------------------------------
// readable aliases
// ---------------------------------------------------------------------------

fn check_segments(
    s: &str,
    what: &'static str,
    max_total: usize,
    min_segments: usize,
    max_segments: usize,
    max_segment: usize,
) -> Result<(), AssetDataError> {
    if s.len() > max_total {
        return Err(AssetDataError::OverBudget {
            what,
            limit: max_total as u64,
            found: s.len() as u64,
        });
    }
    let segments: Vec<&str> = s.split('/').collect();
    if segments.len() < min_segments {
        return Err(AssetDataError::TooSmall {
            what,
            min: min_segments as u64,
            found: segments.len() as u64,
        });
    }
    if segments.len() > max_segments {
        return Err(AssetDataError::OverBudget {
            what,
            limit: max_segments as u64,
            found: segments.len() as u64,
        });
    }
    for seg in segments {
        if seg.is_empty() || seg.len() > max_segment {
            return Err(AssetDataError::Malformed { what });
        }
        // Must start alphanumeric so "-x" or "_x" cannot alias-collide with
        // option-like or hidden-file-like spellings anywhere they surface.
        let bytes = seg.as_bytes();
        if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
            return Err(AssetDataError::Malformed { what });
        }
        for &c in bytes {
            match c {
                b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => {}
                _ => return Err(AssetDataError::Malformed { what }),
            }
        }
    }
    Ok(())
}

macro_rules! alias_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Result<Self, AssetDataError> {
                let s = s.into();
                check_segments(
                    &s,
                    stringify!($name),
                    MAX_ALIAS_BYTES,
                    MIN_ALIAS_SEGMENTS,
                    MAX_ALIAS_SEGMENTS,
                    MAX_ALIAS_SEGMENT_BYTES,
                )?;
                Ok(Self(s))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            /// First segment: the owning namespace, e.g. `rik2`.
            pub fn namespace(&self) -> &str {
                self.0.split('/').next().unwrap_or("")
            }
            // GameAlias is a control-plane ref only, so its codec hooks are
            // currently unused; AssetAlias uses them from the lock.
            #[allow(dead_code)]
            pub(crate) fn encode(&self, w: &mut CanonWriter) {
                w.str(&self.0);
            }
            #[allow(dead_code)]
            pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
                Self::new(r.str(stringify!($name), MAX_ALIAS_BYTES)?)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
        impl std::str::FromStr for $name {
            type Err = AssetDataError;
            fn from_str(s: &str) -> Result<Self, AssetDataError> {
                Self::new(s)
            }
        }
    };
}

alias_type! {
    /// Readable namespaced asset ref, e.g. `rik2/weapons/fancy-rocket-launcher`.
    /// Mutable head pointer material — never a runtime identity.
    AssetAlias
}
alias_type! {
    /// Readable namespaced game ref, e.g. `rik2/games/rocket-arena`.
    GameAlias
}

// ---------------------------------------------------------------------------
// stable scene / state keys
// ---------------------------------------------------------------------------

macro_rules! stable_key {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Result<Self, AssetDataError> {
                let s = s.into();
                check_segments(
                    &s,
                    stringify!($name),
                    MAX_KEY_BYTES,
                    1,
                    MAX_KEY_SEGMENTS,
                    MAX_KEY_SEGMENT_BYTES,
                )?;
                Ok(Self(s))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub(crate) fn encode(&self, w: &mut CanonWriter) {
                w.str(&self.0);
            }
            pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
                Self::new(r.str(stringify!($name), MAX_KEY_BYTES)?)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
        impl std::str::FromStr for $name {
            type Err = AssetDataError;
            fn from_str(s: &str) -> Result<Self, AssetDataError> {
                Self::new(s)
            }
        }
    };
}

stable_key! {
    /// Required stable identity of a durable authored object, e.g.
    /// `arena/main_gate`. Source position, construction order, and inferred
    /// keys are invalid: inserting a line must not rename every door.
    SceneObjectKey
}
stable_key! {
    /// Stable identity of one typed persistent `game.state` slot, e.g.
    /// `match/blue_score`.
    StateKey
}
stable_key! {
    /// Required stable identity of one asset inside an external pack, e.g.
    /// `models/watchtower`. Explicit import data — never guessed from a path
    /// by a runtime client.
    PackEntryKey
}

impl SceneObjectKey {
    /// Deterministic child key from a stable parent and a semantic name —
    /// never from global allocation order.
    pub fn child(&self, name: &str) -> Result<Self, AssetDataError> {
        Self::new(format!("{}/{}", self.0, name))
    }
    /// Deterministic child key for loop/prefab expansion: stable parent plus
    /// semantic name and index.
    pub fn child_indexed(&self, name: &str, index: u32) -> Result<Self, AssetDataError> {
        Self::new(format!("{}/{}-{}", self.0, name, index))
    }
}

/// Bounded lowercase machine name (anchor, component, or parameter name).
pub(crate) fn check_name(s: &str, what: &'static str) -> Result<(), AssetDataError> {
    if s.is_empty() || s.len() > MAX_NAME_BYTES {
        return Err(AssetDataError::Malformed { what });
    }
    let bytes = s.as_bytes();
    if !bytes[0].is_ascii_lowercase() {
        return Err(AssetDataError::Malformed { what });
    }
    for &c in bytes {
        match c {
            b'a'..=b'z' | b'0'..=b'9' | b'_' => {}
            _ => return Err(AssetDataError::Malformed { what }),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_roundtrip_canonical() {
        let id = AssetId::from_bytes([
            0x00, 0x44, 0x32, 0x14, 0xc7, 0x42, 0x54, 0xb6, 0x35, 0xcf, 0x84, 0x65, 0x3a, 0x56,
            0xd7, 0xff,
        ]);
        let s = id.to_string();
        assert!(s.starts_with("ast_"));
        assert_eq!(s.len(), 4 + 26);
        assert_eq!(s.parse::<AssetId>().unwrap(), id);
    }

    #[test]
    fn base32_rejects_noncanonical_spare_bits() {
        let id = GameId::from_bytes([0xff; 16]);
        let mut s = id.to_string();
        // The final char carries 3 data bits + 2 spare bits. For all-ones
        // data the canonical last char is 0b11100 = '4'.
        let last = s.pop().unwrap();
        assert_eq!(last, '4');
        // 0b11101 = '5' decodes to the same 128 data bits with a spare bit
        // set — a second spelling of the same ID, which must be refused.
        s.push('5');
        assert!(s.parse::<GameId>().is_err());
    }

    #[test]
    fn digest_display_roundtrip_and_prefix_isolation() {
        let d = AssetRevisionId::hash_of(b"x");
        let s = d.to_string();
        assert_eq!(s.parse::<AssetRevisionId>().unwrap(), d);
        // The same hex under another prefix must not parse as this type.
        assert!(s.parse::<GameRevisionId>().is_err());
        assert!(s.to_uppercase().parse::<AssetRevisionId>().is_err());
    }

    #[test]
    fn blob_id_uses_doc_spelling() {
        let b = BlobId::hash_of(b"bytes");
        assert!(b.to_string().starts_with("sha256:"));
    }

    #[test]
    fn alias_rules() {
        assert!(AssetAlias::new("rik2/weapons/fancy-rocket-launcher").is_ok());
        assert!(AssetAlias::new("single").is_err()); // too few segments
        assert!(AssetAlias::new("rik2//gap").is_err());
        assert!(AssetAlias::new("rik2/UPPER").is_err());
        assert!(AssetAlias::new("rik2/-lead").is_err());
        assert!(AssetAlias::new(format!("ns/{}", "a".repeat(200))).is_err());
    }

    #[test]
    fn key_rules_and_children() {
        let k = SceneObjectKey::new("arena/main_gate").unwrap();
        assert_eq!(k.child("hinge").unwrap().as_str(), "arena/main_gate/hinge");
        assert_eq!(
            k.child_indexed("bolt", 3).unwrap().as_str(),
            "arena/main_gate/bolt-3"
        );
        assert!(SceneObjectKey::new("solo").is_ok()); // keys allow one segment
        assert!(SceneObjectKey::new("bad key").is_err());
        assert!(StateKey::new("match/blue_score").is_ok());
    }
}
