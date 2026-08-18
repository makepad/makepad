//! Content lock and immutable game revision manifest.
//!
//! `game.splash` stays readable through aliases; the lock resolves every alias
//! to one exact `{asset_id, revision}` and records the transitive dependency
//! closure, which is what makes a multiplayer game reproducible. The lock is
//! an immutable role of every game revision, not an optional sidecar.

use crate::asset::ThumbnailMeta;
use crate::codec::{check_sorted_unique, dockind, CanonReader, CanonWriter};
use crate::error::AssetDataError;
use crate::id::{
    AssetAlias, AssetId, AssetRevisionId, AssetRevisionRef, BlobId, GameId, GameRevisionId,
    VariantSetId,
};
use crate::limits::*;

/// One alias resolved to one exact revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockEntry {
    pub alias: AssetAlias,
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
}

impl LockEntry {
    pub fn revision_ref(&self) -> AssetRevisionRef {
        AssetRevisionRef {
            asset_id: self.asset_id,
            revision: self.revision,
        }
    }
    fn encode(&self, w: &mut CanonWriter) {
        self.alias.encode(w);
        self.asset_id.encode(w);
        self.revision.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            alias: AssetAlias::decode(r)?,
            asset_id: AssetId::decode(r)?,
            revision: AssetRevisionId::decode(r)?,
        })
    }
}

/// One asset's pinned variant set: release truth for which exact frozen set
/// every peer resolves against. Selected once at release, never per client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockVariantSet {
    pub base: AssetRevisionRef,
    pub set: VariantSetId,
}

impl LockVariantSet {
    fn encode(&self, w: &mut CanonWriter) {
        self.base.encode(w);
        self.set.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            base: AssetRevisionRef::decode(r)?,
            set: VariantSetId::decode(r)?,
        })
    }
}

/// The canonical `content.lock` document. Its blob ID (SHA-256 of canonical
/// bytes) is what a `GameRevisionManifest` pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentLock {
    pub game_id: GameId,
    /// Alias resolutions, canonical order by alias.
    pub entries: Vec<LockEntry>,
    /// Transitive dependency closure: every exact revision this game needs,
    /// including every entry's own revision. Canonical order by asset ID and
    /// exactly one revision per asset — two peers can never resolve the same
    /// game to different bytes.
    pub closure: Vec<AssetRevisionRef>,
    /// The pinned `base -> VariantSet` table, canonical order by base. Every
    /// base must be a closure member; an asset without derived variants
    /// simply has no row. Readiness proves an aggregate resolution against
    /// exactly this table.
    pub variant_sets: Vec<LockVariantSet>,
}

impl ContentLock {
    pub fn canonicalize(&mut self) {
        self.entries.sort_by(|a, b| a.alias.cmp(&b.alias));
        self.closure.sort();
        self.variant_sets.sort_by_key(|v| v.base);
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        if self.entries.len() > MAX_LOCK_ENTRIES {
            return Err(AssetDataError::OverBudget {
                what: "lock entries",
                limit: MAX_LOCK_ENTRIES as u64,
                found: self.entries.len() as u64,
            });
        }
        if self.closure.len() > MAX_LOCK_CLOSURE {
            return Err(AssetDataError::OverBudget {
                what: "lock closure",
                limit: MAX_LOCK_CLOSURE as u64,
                found: self.closure.len() as u64,
            });
        }
        check_sorted_unique(&self.entries, |e| e.alias.clone(), "lock alias")?;
        check_sorted_unique(&self.closure, |c| *c, "lock closure entry")?;
        check_sorted_unique(&self.closure, |c| c.asset_id, "lock closure asset_id")?;
        for e in &self.entries {
            if self
                .closure
                .binary_search_by(|c| c.asset_id.cmp(&e.asset_id))
                .map(|i| self.closure[i].revision != e.revision)
                .unwrap_or(true)
            {
                return Err(AssetDataError::Missing {
                    what: "lock entry revision in closure",
                });
            }
        }
        if self.variant_sets.len() > MAX_LOCK_CLOSURE {
            return Err(AssetDataError::OverBudget {
                what: "lock variant sets",
                limit: MAX_LOCK_CLOSURE as u64,
                found: self.variant_sets.len() as u64,
            });
        }
        check_sorted_unique(&self.variant_sets, |v| v.base, "lock variant set base")?;
        for v in &self.variant_sets {
            // The pinned base must be the closure's exact revision of that
            // asset — a set pinned to a revision the game does not ship is a
            // contradiction.
            if self
                .closure
                .binary_search_by(|c| c.cmp(&v.base))
                .is_err()
            {
                return Err(AssetDataError::Missing {
                    what: "lock variant set base in closure",
                });
            }
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::CONTENT_LOCK);
        self.game_id.encode(&mut w);
        w.u32(self.entries.len() as u32);
        for e in &self.entries {
            e.encode(&mut w);
        }
        w.u32(self.closure.len() as u32);
        for c in &self.closure {
            c.encode(&mut w);
        }
        w.u32(self.variant_sets.len() as u32);
        for v in &self.variant_sets {
            v.encode(&mut w);
        }
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::CONTENT_LOCK)?;
        let game_id = GameId::decode(&mut r)?;
        let n = r.count("lock entries", MAX_LOCK_ENTRIES)?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            entries.push(LockEntry::decode(&mut r)?);
        }
        let n = r.count("lock closure", MAX_LOCK_CLOSURE)?;
        let mut closure = Vec::with_capacity(n);
        for _ in 0..n {
            closure.push(AssetRevisionRef::decode(&mut r)?);
        }
        let n = r.count("lock variant sets", MAX_LOCK_CLOSURE)?;
        let mut variant_sets = Vec::with_capacity(n);
        for _ in 0..n {
            variant_sets.push(LockVariantSet::decode(&mut r)?);
        }
        r.finish()?;
        let lock = Self {
            game_id,
            entries,
            closure,
            variant_sets,
        };
        lock.validate()?;
        Ok(lock)
    }

    /// The lock's identity as a stored blob: SHA-256 of its canonical bytes.
    pub fn blob_id(&self) -> Result<BlobId, AssetDataError> {
        Ok(BlobId::hash_of(&self.to_canonical_bytes()?))
    }

    /// Resolve an alias to its exact revision — the only alias lookup a
    /// running game is allowed to perform.
    pub fn resolve(&self, alias: &AssetAlias) -> Option<AssetRevisionRef> {
        self.entries
            .binary_search_by(|e| e.alias.cmp(alias))
            .ok()
            .map(|i| self.entries[i].revision_ref())
    }
}

/// The immutable game release manifest. Its revision ID is the SHA-256 of its
/// canonical bytes. Runtime facts (health, transforms, scores) never appear
/// here; the room host replicates those.
///
/// The baseline content set is deliberately NOT a field: it is derived
/// deterministically from this revision's lock via
/// [`crate::content_set::ContentSetManifest::baseline`], because the content
/// set names this revision and two digests cannot both contain each other.
#[derive(Clone, Debug, PartialEq)]
pub struct GameRevisionManifest {
    pub game_id: GameId,
    pub name: String,
    pub description: String,
    pub author: String,
    /// The canonical authored scene source.
    pub splash_blob: BlobId,
    /// The game's `manifest.toml` description card.
    pub manifest_blob: BlobId,
    /// Canonical `ContentLock` bytes stored as a blob.
    pub lock_blob: BlobId,
    /// Mandatory primary thumbnail for catalog browsing.
    pub thumbnail: ThumbnailMeta,
    /// Pinned catalog snapshot so construction-time searches are reproducible.
    pub catalog_snapshot: Option<BlobId>,
    pub search_algorithm_version: u32,
    /// Engine/protocol compatibility this revision was validated against.
    pub engine_version: u32,
    pub protocol_version: u32,
    /// Byte length of the Splash source blob, bounded at admission.
    pub splash_byte_len: u64,
}

impl GameRevisionManifest {
    pub fn validate(&self) -> Result<(), AssetDataError> {
        if self.name.is_empty() || self.name.len() > MAX_NAME_BYTES * 2 {
            return Err(AssetDataError::Malformed { what: "game name" });
        }
        if self.description.len() > MAX_DESCRIPTION_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "game description",
                limit: MAX_DESCRIPTION_BYTES as u64,
                found: self.description.len() as u64,
            });
        }
        if self.author.is_empty() || self.author.len() > MAX_NAME_BYTES * 2 {
            return Err(AssetDataError::Malformed { what: "game author" });
        }
        self.thumbnail.validate()?;
        if self.splash_byte_len == 0 {
            return Err(AssetDataError::Malformed {
                what: "splash byte_len",
            });
        }
        if self.splash_byte_len > MAX_SPLASH_SOURCE_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "splash byte_len",
                limit: MAX_SPLASH_SOURCE_BYTES,
                found: self.splash_byte_len,
            });
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::GAME_REVISION);
        self.game_id.encode(&mut w);
        w.str(&self.name);
        w.str(&self.description);
        w.str(&self.author);
        self.splash_blob.encode(&mut w);
        self.manifest_blob.encode(&mut w);
        self.lock_blob.encode(&mut w);
        self.thumbnail.encode(&mut w);
        match &self.catalog_snapshot {
            None => w.bool(false),
            Some(b) => {
                w.bool(true);
                b.encode(&mut w);
            }
        }
        w.u32(self.search_algorithm_version);
        w.u32(self.engine_version);
        w.u32(self.protocol_version);
        w.u64(self.splash_byte_len);
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::GAME_REVISION)?;
        let game_id = GameId::decode(&mut r)?;
        let name = r.str("game name", MAX_NAME_BYTES * 2)?;
        let description = r.str("game description", MAX_DESCRIPTION_BYTES)?;
        let author = r.str("game author", MAX_NAME_BYTES * 2)?;
        let splash_blob = BlobId::decode(&mut r)?;
        let manifest_blob = BlobId::decode(&mut r)?;
        let lock_blob = BlobId::decode(&mut r)?;
        let thumbnail = ThumbnailMeta::decode(&mut r)?;
        let catalog_snapshot = if r.bool("catalog snapshot present")? {
            Some(BlobId::decode(&mut r)?)
        } else {
            None
        };
        let search_algorithm_version = r.u32("search algorithm version")?;
        let engine_version = r.u32("engine version")?;
        let protocol_version = r.u32("protocol version")?;
        let splash_byte_len = r.u64("splash byte_len")?;
        r.finish()?;
        let manifest = Self {
            game_id,
            name,
            description,
            author,
            splash_blob,
            manifest_blob,
            lock_blob,
            thumbnail,
            catalog_snapshot,
            search_algorithm_version,
            engine_version,
            protocol_version,
            splash_byte_len,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// The immutable identity of this exact release.
    pub fn revision(&self) -> Result<GameRevisionId, AssetDataError> {
        Ok(GameRevisionId::hash_of(&self.to_canonical_bytes()?))
    }
}
