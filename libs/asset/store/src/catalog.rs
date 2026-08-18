//! SQLite catalog: immutable asset/game revisions, candidates, aliases, refs.
//!
//! Rules enforced here, all fail-closed:
//! - Revision rows are immutable and content-addressed: the row key IS the
//!   SHA-256 of the stored canonical manifest bytes, so re-insertion is
//!   idempotent by construction and conflicting content is impossible.
//! - Every blob a manifest references must already be recorded (which means
//!   it was committed to the CAS first); a manifest can never point at bytes
//!   the store does not hold.
//! - Candidate lifecycle: staged -> published, staged/published -> quarantined,
//!   quarantined is terminal. No other transition exists.
//! - Aliases are the only mutable pointers, and may only point at PUBLISHED
//!   revisions whose namespace matches the alias namespace. That invariant is
//!   maintained, not just admitted: quarantining a revision drops every alias
//!   head pointing at it in the same transaction.
//! - Every alias-head mutation (set, clear, quarantine drop) recomputes the
//!   search index's alias state (live flag, canonical alias, alias posting
//!   terms) for the affected assets and advances the search index generation,
//!   all in the mutating transaction.
//! - Game revisions pin exact asset revisions (the refs table); staging one
//!   requires every pinned asset revision to be published.

use crate::budget::Budgets;
use crate::error::{ServerError, ServerResult};
use crate::sqlite::Db;
use makepad_asset_data::limits::MAX_DOCUMENT_BYTES;
use makepad_asset_data::{
    AssetAlias, AssetId, AssetManifest, AssetRevisionId, AssetRevisionRef, BlobId, ContentLock,
    GameAlias, GameId, GameRevisionId, GameRevisionManifest,
};

pub const CATALOG_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS blobs(
    blob_id BLOB PRIMARY KEY,
    size INTEGER NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS assets(
    asset_id BLOB PRIMARY KEY,
    namespace TEXT NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS asset_revisions(
    revision BLOB PRIMARY KEY,
    asset_id BLOB NOT NULL,
    manifest BLOB NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS candidates(
    kind TEXT NOT NULL CHECK(kind IN ('asset','game')),
    owner_id BLOB NOT NULL,
    revision BLOB NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('staged','published','quarantined')),
    staged_ms INTEGER NOT NULL,
    published_ms INTEGER,
    quarantined_ms INTEGER,
    PRIMARY KEY(kind, owner_id, revision)
);
CREATE TABLE IF NOT EXISTS asset_aliases(
    alias TEXT PRIMARY KEY,
    asset_id BLOB NOT NULL,
    head_revision BLOB NOT NULL,
    updated_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS games(
    game_id BLOB PRIMARY KEY,
    namespace TEXT NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS game_revisions(
    revision BLOB PRIMARY KEY,
    game_id BLOB NOT NULL,
    manifest BLOB NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS game_revision_refs(
    game_revision BLOB NOT NULL,
    asset_id BLOB NOT NULL,
    asset_revision BLOB NOT NULL,
    PRIMARY KEY(game_revision, asset_id)
);
CREATE TABLE IF NOT EXISTS game_aliases(
    alias TEXT PRIMARY KEY,
    game_id BLOB NOT NULL,
    head_revision BLOB NOT NULL,
    updated_ms INTEGER NOT NULL
);
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateState {
    Staged,
    Published,
    Quarantined,
}

impl CandidateState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Published => "published",
            Self::Quarantined => "quarantined",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "staged" => Some(Self::Staged),
            "published" => Some(Self::Published),
            "quarantined" => Some(Self::Quarantined),
            _ => None,
        }
    }
}

/// Server-side namespace rule. A namespace is exactly one alias segment as
/// the content contract defines it (an alias's `namespace()` is its first
/// segment), so the charset here mirrors `check_segments`: starts with a
/// lowercase letter or digit, then lowercase/digit/`-`/`_`.
pub fn validate_namespace(ns: &str) -> ServerResult<()> {
    if ns.is_empty() || ns.len() > 64 {
        return Err(ServerError::InvalidInput { what: "namespace length" });
    }
    let bytes = ns.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err(ServerError::InvalidInput { what: "namespace charset" });
    }
    if !bytes.iter().all(|&b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
    }) {
        return Err(ServerError::InvalidInput { what: "namespace charset" });
    }
    Ok(())
}

pub struct Catalog<'a> {
    pub(crate) db: &'a Db,
    pub(crate) budgets: &'a Budgets,
}

impl<'a> Catalog<'a> {
    // ---- blobs -------------------------------------------------------------

    /// Record a CAS-committed blob. Idempotent for the same size; a size
    /// conflict on the same digest is corruption and refuses.
    pub fn record_blob(&self, blob_id: &BlobId, size: u64, now_ms: u64) -> ServerResult<()> {
        if let Some(existing) = self.blob_size(blob_id)? {
            if existing != size {
                return Err(ServerError::SizeMismatch {
                    what: "blob record",
                    expected: existing,
                    found: size,
                });
            }
            return Ok(());
        }
        let mut s = self.db.prepare(
            "record blob",
            "INSERT INTO blobs(blob_id, size, created_ms) VALUES(?1, ?2, ?3)",
        )?;
        s.bind_blob(1, blob_id.as_bytes())?;
        s.bind_u64(2, size)?;
        s.bind_u64(3, now_ms)?;
        s.run()
    }

    pub fn blob_size(&self, blob_id: &BlobId) -> ServerResult<Option<u64>> {
        let mut s = self
            .db
            .prepare("blob size", "SELECT size FROM blobs WHERE blob_id = ?1")?;
        s.bind_blob(1, blob_id.as_bytes())?;
        if s.step()? {
            Ok(Some(s.column_u64(0)))
        } else {
            Ok(None)
        }
    }

    pub fn has_blob(&self, blob_id: &BlobId) -> ServerResult<bool> {
        Ok(self.blob_size(blob_id)?.is_some())
    }

    fn require_blob(&self, blob_id: &BlobId, what: &'static str) -> ServerResult<()> {
        if self.has_blob(blob_id)? {
            Ok(())
        } else {
            Err(ServerError::NotFound { what })
        }
    }

    /// A manifest's declared byte length must equal the size the CAS recorded
    /// for that digest. A manifest that lies about a size it names is refused
    /// at admission, so clients can budget downloads from manifests alone.
    pub(crate) fn require_blob_sized(
        &self,
        blob_id: &BlobId,
        declared: u64,
        missing: &'static str,
        mismatch: &'static str,
    ) -> ServerResult<()> {
        let recorded = self
            .blob_size(blob_id)?
            .ok_or(ServerError::NotFound { what: missing })?;
        if recorded != declared {
            return Err(ServerError::SizeMismatch {
                what: mismatch,
                expected: declared,
                found: recorded,
            });
        }
        Ok(())
    }

    // ---- assets ------------------------------------------------------------

    /// Register an asset identity. Idempotent for the same namespace;
    /// conflicting namespace refuses.
    pub fn register_asset(&self, asset_id: &AssetId, namespace: &str, now_ms: u64) -> ServerResult<()> {
        validate_namespace(namespace)?;
        if let Some(existing) = self.asset_namespace(asset_id)? {
            if existing != namespace {
                return Err(ServerError::Conflict { what: "asset namespace" });
            }
            return Ok(());
        }
        let mut s = self.db.prepare(
            "register asset",
            "INSERT INTO assets(asset_id, namespace, created_ms) VALUES(?1, ?2, ?3)",
        )?;
        s.bind_blob(1, asset_id.as_bytes())?;
        s.bind_text(2, namespace)?;
        s.bind_u64(3, now_ms)?;
        s.run()
    }

    pub fn asset_namespace(&self, asset_id: &AssetId) -> ServerResult<Option<String>> {
        let mut s = self.db.prepare(
            "asset namespace",
            "SELECT namespace FROM assets WHERE asset_id = ?1",
        )?;
        s.bind_blob(1, asset_id.as_bytes())?;
        if s.step()? {
            Ok(Some(s.column_text(0)))
        } else {
            Ok(None)
        }
    }

    /// Admit an asset revision from canonical manifest bytes: decode/validate
    /// through the content contract, require every referenced blob and
    /// dependency revision to exist, then store the immutable revision row and
    /// a staged candidate. Fully idempotent while the candidate is staged.
    pub fn stage_asset_revision(&self, manifest_bytes: &[u8], now_ms: u64) -> ServerResult<AssetRevisionId> {
        self.db
            .tx(|db| self.stage_asset_revision_in_tx(db, manifest_bytes, now_ms))
    }

    /// The body of [`Self::stage_asset_revision`] inside the caller's open
    /// transaction, so a multi-asset operation (pack import) can stage every
    /// revision atomically with its own records.
    pub(crate) fn stage_asset_revision_in_tx(
        &self,
        db: &Db,
        manifest_bytes: &[u8],
        now_ms: u64,
    ) -> ServerResult<AssetRevisionId> {
        if manifest_bytes.len() as u64 > self.budgets.max_manifest_bytes {
            return Err(ServerError::OverBudget {
                what: "asset manifest bytes",
                limit: self.budgets.max_manifest_bytes,
                found: manifest_bytes.len() as u64,
            });
        }
        let manifest = AssetManifest::from_canonical_bytes(manifest_bytes)?;
        let revision = AssetRevisionId::hash_of(manifest_bytes);
        if self.asset_namespace(&manifest.asset_id)?.is_none() {
            return Err(ServerError::NotFound { what: "asset for revision" });
        }
        // Every referenced blob must exist AND be exactly the size the
        // manifest declares for it; a lying byte_len is refused here.
        for f in &manifest.files {
            self.require_blob_sized(&f.blob, f.byte_len, "asset file blob", "asset file blob size")?;
        }
        if let Some(t) = &manifest.thumbnail {
            self.require_blob_sized(
                &t.blob,
                t.byte_len,
                "asset thumbnail blob",
                "asset thumbnail blob size",
            )?;
        }
        for dep in &manifest.dependencies {
            if self.asset_revision_manifest(&dep.revision)?.is_none() {
                return Err(ServerError::NotFound { what: "asset dependency revision" });
            }
        }
        let mut s = db.prepare(
            "insert asset revision",
            "INSERT OR IGNORE INTO asset_revisions(revision, asset_id, manifest, created_ms)
             VALUES(?1, ?2, ?3, ?4)",
        )?;
        s.bind_blob(1, revision.as_bytes())?;
        s.bind_blob(2, manifest.asset_id.as_bytes())?;
        s.bind_blob(3, manifest_bytes)?;
        s.bind_u64(4, now_ms)?;
        s.run()?;
        drop(s);
        self.stage_candidate("asset", manifest.asset_id.as_bytes(), revision.as_bytes(), now_ms)?;
        Ok(revision)
    }

    pub fn asset_revision_manifest(&self, revision: &AssetRevisionId) -> ServerResult<Option<Vec<u8>>> {
        let mut s = self.db.prepare(
            "asset revision manifest",
            "SELECT manifest FROM asset_revisions WHERE revision = ?1",
        )?;
        s.bind_blob(1, revision.as_bytes())?;
        if s.step()? {
            Ok(Some(s.column_blob(0)))
        } else {
            Ok(None)
        }
    }

    // ---- candidates --------------------------------------------------------

    fn stage_candidate(&self, kind: &'static str, owner: &[u8], revision: &[u8], now_ms: u64) -> ServerResult<()> {
        match self.candidate_state_raw(kind, owner, revision)? {
            // Re-staging identical content is a no-op only while staged;
            // published/quarantined content cannot be re-admitted.
            Some(CandidateState::Staged) => Ok(()),
            Some(CandidateState::Published) => Err(ServerError::InvalidState {
                what: "candidate",
                state: "published",
            }),
            Some(CandidateState::Quarantined) => Err(ServerError::InvalidState {
                what: "candidate",
                state: "quarantined",
            }),
            None => {
                let mut s = self.db.prepare(
                    "stage candidate",
                    "INSERT INTO candidates(kind, owner_id, revision, state, staged_ms)
                     VALUES(?1, ?2, ?3, 'staged', ?4)",
                )?;
                s.bind_text(1, kind)?;
                s.bind_blob(2, owner)?;
                s.bind_blob(3, revision)?;
                s.bind_u64(4, now_ms)?;
                s.run()
            }
        }
    }

    fn candidate_state_raw(&self, kind: &str, owner: &[u8], revision: &[u8]) -> ServerResult<Option<CandidateState>> {
        let mut s = self.db.prepare(
            "candidate state",
            "SELECT state FROM candidates WHERE kind = ?1 AND owner_id = ?2 AND revision = ?3",
        )?;
        s.bind_text(1, kind)?;
        s.bind_blob(2, owner)?;
        s.bind_blob(3, revision)?;
        if s.step()? {
            let state = s.column_text(0);
            CandidateState::parse(&state)
                .map(Some)
                .ok_or(ServerError::InvalidState { what: "candidate row", state: "unknown" })
        } else {
            Ok(None)
        }
    }

    /// Apply one candidate state transition inside the caller's transaction,
    /// so lifecycle side effects (alias drops, search liveness) land
    /// atomically with the state change.
    pub(crate) fn transition_in_tx(
        &self,
        db: &Db,
        kind: &'static str,
        owner: &[u8],
        revision: &[u8],
        allowed_from: &[CandidateState],
        to: CandidateState,
        now_ms: u64,
    ) -> ServerResult<()> {
        let cur = self
            .candidate_state_raw(kind, owner, revision)?
            .ok_or(ServerError::NotFound { what: "candidate" })?;
        if cur == to {
            // Idempotent repeat of a landed transition.
            return Ok(());
        }
        if !allowed_from.contains(&cur) {
            return Err(ServerError::InvalidState {
                what: "candidate transition",
                state: cur.as_str(),
            });
        }
        let sql = match to {
            CandidateState::Published => {
                "UPDATE candidates SET state='published', published_ms=?4
                 WHERE kind=?1 AND owner_id=?2 AND revision=?3"
            }
            CandidateState::Quarantined => {
                "UPDATE candidates SET state='quarantined', quarantined_ms=?4
                 WHERE kind=?1 AND owner_id=?2 AND revision=?3"
            }
            CandidateState::Staged => {
                return Err(ServerError::InvalidState {
                    what: "candidate transition",
                    state: "staged is admission-only",
                })
            }
        };
        let mut s = db.prepare("candidate transition", sql)?;
        s.bind_text(1, kind)?;
        s.bind_blob(2, owner)?;
        s.bind_blob(3, revision)?;
        s.bind_u64(4, now_ms)?;
        s.run()
    }

    /// Recompute the search index's alias-derived state (live flag, canonical
    /// alias, alias posting terms) for up to two assets, inside the caller's
    /// transaction, then advance the index generation so every open keyset
    /// cursor fails closed as stale. Assets without an annotation simply have
    /// no rows to update.
    fn refresh_search_live(&self, db: &Db, asset_a: &[u8], asset_b: &[u8]) -> ServerResult<()> {
        crate::search::refresh_alias_state(db, self.budgets, asset_a)?;
        if asset_b != asset_a {
            crate::search::refresh_alias_state(db, self.budgets, asset_b)?;
        }
        crate::search::bump_generation(db)
    }

    pub fn asset_candidate_state(&self, asset_id: &AssetId, revision: &AssetRevisionId) -> ServerResult<Option<CandidateState>> {
        self.candidate_state_raw("asset", asset_id.as_bytes(), revision.as_bytes())
    }

    pub fn publish_asset(&self, asset_id: &AssetId, revision: &AssetRevisionId, now_ms: u64) -> ServerResult<()> {
        self.db.tx(|db| {
            self.transition_in_tx(
                db,
                "asset",
                asset_id.as_bytes(),
                revision.as_bytes(),
                &[CandidateState::Staged],
                CandidateState::Published,
                now_ms,
            )
        })
    }

    /// Quarantine a revision AND tear down every alias head pointing at it,
    /// atomically. Quarantine is the emergency pull: it always succeeds from
    /// staged/published, and no mutable pointer keeps resolving to pulled
    /// content afterwards. Search liveness for the asset flips in the same
    /// transaction.
    pub fn quarantine_asset(&self, asset_id: &AssetId, revision: &AssetRevisionId, now_ms: u64) -> ServerResult<()> {
        self.db.tx(|db| {
            self.transition_in_tx(
                db,
                "asset",
                asset_id.as_bytes(),
                revision.as_bytes(),
                &[CandidateState::Staged, CandidateState::Published],
                CandidateState::Quarantined,
                now_ms,
            )?;
            let mut s = db.prepare(
                "drop quarantined alias heads",
                "DELETE FROM asset_aliases WHERE asset_id = ?1 AND head_revision = ?2",
            )?;
            s.bind_blob(1, asset_id.as_bytes())?;
            s.bind_blob(2, revision.as_bytes())?;
            s.run()?;
            drop(s);
            self.refresh_search_live(db, asset_id.as_bytes(), asset_id.as_bytes())
        })
    }

    // ---- asset aliases -----------------------------------------------------

    /// Point an alias at a published revision. Aliases are the only mutable
    /// heads in the catalog; namespace must match the asset's.
    pub fn set_asset_alias(
        &self,
        alias: &AssetAlias,
        target: &AssetRevisionRef,
        now_ms: u64,
    ) -> ServerResult<()> {
        self.db
            .tx(|db| self.set_asset_alias_in_tx(db, alias, target, now_ms))
    }

    /// The body of [`Self::set_asset_alias`] inside the caller's open
    /// transaction. Every alias-head mutation still funnels through
    /// `refresh_search_live` here, so the search invariant holds for
    /// composite operations too.
    pub(crate) fn set_asset_alias_in_tx(
        &self,
        db: &Db,
        alias: &AssetAlias,
        target: &AssetRevisionRef,
        now_ms: u64,
    ) -> ServerResult<()> {
        {
            let ns = self
                .asset_namespace(&target.asset_id)?
                .ok_or(ServerError::NotFound { what: "alias target asset" })?;
            if alias.namespace() != ns {
                return Err(ServerError::Conflict { what: "alias namespace" });
            }
            match self.asset_candidate_state(&target.asset_id, &target.revision)? {
                Some(CandidateState::Published) => {}
                Some(_) => {
                    return Err(ServerError::InvalidState {
                        what: "alias target",
                        state: "not published",
                    })
                }
                None => return Err(ServerError::NotFound { what: "alias target revision" }),
            }
            // The previous target (if this is a retarget) may lose its last
            // alias here; its search liveness must flip in this transaction.
            let mut s = db.prepare(
                "alias old target",
                "SELECT asset_id FROM asset_aliases WHERE alias = ?1",
            )?;
            s.bind_text(1, alias.as_str())?;
            let old_target = if s.step()? {
                Some(fixed16(&s.column_blob(0), "asset alias row")?)
            } else {
                None
            };
            drop(s);
            let mut s = db.prepare(
                "set asset alias",
                "INSERT INTO asset_aliases(alias, asset_id, head_revision, updated_ms)
                 VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(alias) DO UPDATE
                 SET asset_id=?2, head_revision=?3, updated_ms=?4",
            )?;
            s.bind_text(1, alias.as_str())?;
            s.bind_blob(2, target.asset_id.as_bytes())?;
            s.bind_blob(3, target.revision.as_bytes())?;
            s.bind_u64(4, now_ms)?;
            s.run()?;
            drop(s);
            // Transactional search reindex on alias-head mutation: recompute
            // the live flag for both affected assets from the alias table.
            self.refresh_search_live(
                db,
                target.asset_id.as_bytes(),
                old_target.as_ref().map(|o| &o[..]).unwrap_or(target.asset_id.as_bytes()),
            )
        }
    }

    /// Remove an alias head. Idempotent: returns whether the alias existed.
    /// The unpinned asset's search liveness flips in the same transaction.
    pub fn clear_asset_alias(&self, alias: &AssetAlias) -> ServerResult<bool> {
        self.db.tx(|db| {
            let mut s = db.prepare(
                "clear alias old target",
                "SELECT asset_id FROM asset_aliases WHERE alias = ?1",
            )?;
            s.bind_text(1, alias.as_str())?;
            if !s.step()? {
                return Ok(false);
            }
            let old_target = fixed16(&s.column_blob(0), "asset alias row")?;
            drop(s);
            let mut s = db.prepare(
                "clear asset alias",
                "DELETE FROM asset_aliases WHERE alias = ?1",
            )?;
            s.bind_text(1, alias.as_str())?;
            s.run()?;
            drop(s);
            self.refresh_search_live(db, &old_target, &old_target)?;
            Ok(true)
        })
    }

    pub fn resolve_asset_alias(&self, alias: &AssetAlias) -> ServerResult<Option<AssetRevisionRef>> {
        let mut s = self.db.prepare(
            "resolve asset alias",
            "SELECT asset_id, head_revision FROM asset_aliases WHERE alias = ?1",
        )?;
        s.bind_text(1, alias.as_str())?;
        if !s.step()? {
            return Ok(None);
        }
        Ok(Some(AssetRevisionRef {
            asset_id: AssetId::from_bytes(fixed16(&s.column_blob(0), "asset alias row")?),
            revision: AssetRevisionId::from_bytes(fixed32(&s.column_blob(1), "asset alias row")?),
        }))
    }

    // ---- games -------------------------------------------------------------

    pub fn register_game(&self, game_id: &GameId, namespace: &str, now_ms: u64) -> ServerResult<()> {
        validate_namespace(namespace)?;
        if let Some(existing) = self.game_namespace(game_id)? {
            if existing != namespace {
                return Err(ServerError::Conflict { what: "game namespace" });
            }
            return Ok(());
        }
        let mut s = self.db.prepare(
            "register game",
            "INSERT INTO games(game_id, namespace, created_ms) VALUES(?1, ?2, ?3)",
        )?;
        s.bind_blob(1, game_id.as_bytes())?;
        s.bind_text(2, namespace)?;
        s.bind_u64(3, now_ms)?;
        s.run()
    }

    pub fn game_namespace(&self, game_id: &GameId) -> ServerResult<Option<String>> {
        let mut s = self.db.prepare(
            "game namespace",
            "SELECT namespace FROM games WHERE game_id = ?1",
        )?;
        s.bind_blob(1, game_id.as_bytes())?;
        if s.step()? {
            Ok(Some(s.column_text(0)))
        } else {
            Ok(None)
        }
    }

    /// Admit a game revision. Requires the lock bytes so the pin set can be
    /// verified: the manifest's lock_blob must be the digest of `lock_bytes`,
    /// the lock's game_id must match, and every closure entry must be a
    /// PUBLISHED asset revision. Pins land in the refs table.
    pub fn stage_game_revision(
        &self,
        manifest_bytes: &[u8],
        lock_bytes: &[u8],
        now_ms: u64,
    ) -> ServerResult<GameRevisionId> {
        if manifest_bytes.len() as u64 > self.budgets.max_manifest_bytes {
            return Err(ServerError::OverBudget {
                what: "game manifest bytes",
                limit: self.budgets.max_manifest_bytes,
                found: manifest_bytes.len() as u64,
            });
        }
        let manifest = GameRevisionManifest::from_canonical_bytes(manifest_bytes)?;
        let lock = ContentLock::from_canonical_bytes(lock_bytes)?;
        let lock_digest = BlobId::hash_of(lock_bytes);
        if manifest.lock_blob != lock_digest {
            return Err(ServerError::DigestMismatch {
                what: "game lock blob",
                expected: *manifest.lock_blob.as_bytes(),
                found: *lock_digest.as_bytes(),
            });
        }
        if lock.game_id != manifest.game_id {
            return Err(ServerError::Conflict { what: "lock game_id" });
        }
        let revision = GameRevisionId::hash_of(manifest_bytes);
        self.db.tx(|db| {
            if self.game_namespace(&manifest.game_id)?.is_none() {
                return Err(ServerError::NotFound { what: "game for revision" });
            }
            // Declared byte lengths must equal the recorded CAS sizes. The
            // manifest.toml blob declares no length in the contract, so it is
            // existence-checked only; the lock's exact bytes are in hand, so
            // its recorded size must match them.
            self.require_blob_sized(
                &manifest.splash_blob,
                manifest.splash_byte_len,
                "game splash blob",
                "game splash blob size",
            )?;
            self.require_blob(&manifest.manifest_blob, "game manifest.toml blob")?;
            self.require_blob_sized(
                &manifest.lock_blob,
                lock_bytes.len() as u64,
                "game lock blob",
                "game lock blob size",
            )?;
            self.require_blob_sized(
                &manifest.thumbnail.blob,
                manifest.thumbnail.byte_len,
                "game thumbnail blob",
                "game thumbnail blob size",
            )?;
            // A pinned catalog snapshot must exist and fit the contract's
            // canonical-document bound; it declares no exact length, so the
            // bound is the only size contract available for it.
            if let Some(snapshot) = &manifest.catalog_snapshot {
                let size = self
                    .blob_size(snapshot)?
                    .ok_or(ServerError::NotFound { what: "game catalog snapshot blob" })?;
                if size == 0 {
                    return Err(ServerError::InvalidInput { what: "game catalog snapshot empty" });
                }
                if size > MAX_DOCUMENT_BYTES as u64 {
                    return Err(ServerError::OverBudget {
                        what: "game catalog snapshot bytes",
                        limit: MAX_DOCUMENT_BYTES as u64,
                        found: size,
                    });
                }
            }
            for r in &lock.closure {
                match self.asset_candidate_state(&r.asset_id, &r.revision)? {
                    Some(CandidateState::Published) => {}
                    Some(_) => {
                        return Err(ServerError::InvalidState {
                            what: "pinned asset revision",
                            state: "not published",
                        })
                    }
                    None => return Err(ServerError::NotFound { what: "pinned asset revision" }),
                }
            }
            let mut s = db.prepare(
                "insert game revision",
                "INSERT OR IGNORE INTO game_revisions(revision, game_id, manifest, created_ms)
                 VALUES(?1, ?2, ?3, ?4)",
            )?;
            s.bind_blob(1, revision.as_bytes())?;
            s.bind_blob(2, manifest.game_id.as_bytes())?;
            s.bind_blob(3, manifest_bytes)?;
            s.bind_u64(4, now_ms)?;
            s.run()?;
            drop(s);
            for r in &lock.closure {
                let mut s = db.prepare(
                    "insert game ref",
                    "INSERT OR IGNORE INTO game_revision_refs(game_revision, asset_id, asset_revision)
                     VALUES(?1, ?2, ?3)",
                )?;
                s.bind_blob(1, revision.as_bytes())?;
                s.bind_blob(2, r.asset_id.as_bytes())?;
                s.bind_blob(3, r.revision.as_bytes())?;
                s.run()?;
            }
            self.stage_candidate("game", manifest.game_id.as_bytes(), revision.as_bytes(), now_ms)?;
            Ok(revision)
        })
    }

    pub fn game_candidate_state(&self, game_id: &GameId, revision: &GameRevisionId) -> ServerResult<Option<CandidateState>> {
        self.candidate_state_raw("game", game_id.as_bytes(), revision.as_bytes())
    }

    /// Exact canonical bytes for a stored game revision. Lifecycle filtering
    /// belongs to the transport, just like [`Self::asset_revision_manifest`]:
    /// internal recovery code may need to inspect a quarantined candidate,
    /// while public reads must make quarantined content indistinguishable
    /// from absent content.
    pub fn game_revision_manifest(
        &self,
        revision: &GameRevisionId,
    ) -> ServerResult<Option<Vec<u8>>> {
        let mut s = self.db.prepare(
            "game revision manifest",
            "SELECT manifest FROM game_revisions WHERE revision = ?1",
        )?;
        s.bind_blob(1, revision.as_bytes())?;
        if s.step()? {
            Ok(Some(s.column_blob(0)))
        } else {
            Ok(None)
        }
    }

    pub fn publish_game(&self, game_id: &GameId, revision: &GameRevisionId, now_ms: u64) -> ServerResult<()> {
        self.db.tx(|db| {
            self.transition_in_tx(
                db,
                "game",
                game_id.as_bytes(),
                revision.as_bytes(),
                &[CandidateState::Staged],
                CandidateState::Published,
                now_ms,
            )
        })
    }

    /// Quarantine a game revision AND drop every game alias head pointing at
    /// it, atomically — pulled games stop resolving immediately.
    pub fn quarantine_game(&self, game_id: &GameId, revision: &GameRevisionId, now_ms: u64) -> ServerResult<()> {
        self.db.tx(|db| {
            self.transition_in_tx(
                db,
                "game",
                game_id.as_bytes(),
                revision.as_bytes(),
                &[CandidateState::Staged, CandidateState::Published],
                CandidateState::Quarantined,
                now_ms,
            )?;
            let mut s = db.prepare(
                "drop quarantined game alias heads",
                "DELETE FROM game_aliases WHERE game_id = ?1 AND head_revision = ?2",
            )?;
            s.bind_blob(1, game_id.as_bytes())?;
            s.bind_blob(2, revision.as_bytes())?;
            s.run()
        })
    }

    /// The exact asset revisions a game revision pins, canonical order.
    pub fn game_revision_refs(&self, revision: &GameRevisionId) -> ServerResult<Vec<AssetRevisionRef>> {
        let mut s = self.db.prepare(
            "game revision refs",
            "SELECT asset_id, asset_revision FROM game_revision_refs
             WHERE game_revision = ?1 ORDER BY asset_id",
        )?;
        s.bind_blob(1, revision.as_bytes())?;
        let mut out = Vec::new();
        while s.step()? {
            out.push(AssetRevisionRef {
                asset_id: AssetId::from_bytes(fixed16(&s.column_blob(0), "game ref row")?),
                revision: AssetRevisionId::from_bytes(fixed32(&s.column_blob(1), "game ref row")?),
            });
        }
        Ok(out)
    }

    pub fn set_game_alias(
        &self,
        alias: &GameAlias,
        game_id: &GameId,
        revision: &GameRevisionId,
        now_ms: u64,
    ) -> ServerResult<()> {
        self.db.tx(|db| {
            let ns = self
                .game_namespace(game_id)?
                .ok_or(ServerError::NotFound { what: "alias target game" })?;
            if alias.namespace() != ns {
                return Err(ServerError::Conflict { what: "game alias namespace" });
            }
            match self.game_candidate_state(game_id, revision)? {
                Some(CandidateState::Published) => {}
                Some(_) => {
                    return Err(ServerError::InvalidState {
                        what: "game alias target",
                        state: "not published",
                    })
                }
                None => return Err(ServerError::NotFound { what: "game alias target revision" }),
            }
            let mut s = db.prepare(
                "set game alias",
                "INSERT INTO game_aliases(alias, game_id, head_revision, updated_ms)
                 VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(alias) DO UPDATE
                 SET game_id=?2, head_revision=?3, updated_ms=?4",
            )?;
            s.bind_text(1, alias.as_str())?;
            s.bind_blob(2, game_id.as_bytes())?;
            s.bind_blob(3, revision.as_bytes())?;
            s.bind_u64(4, now_ms)?;
            s.run()
        })
    }

    /// Remove a game alias head. Idempotent: returns whether it existed.
    pub fn clear_game_alias(&self, alias: &GameAlias) -> ServerResult<bool> {
        self.db.tx(|db| {
            let mut s = db.prepare(
                "clear game alias",
                "DELETE FROM game_aliases WHERE alias = ?1",
            )?;
            s.bind_text(1, alias.as_str())?;
            s.run()?;
            drop(s);
            Ok(db.changes() > 0)
        })
    }

    pub fn resolve_game_alias(&self, alias: &GameAlias) -> ServerResult<Option<(GameId, GameRevisionId)>> {
        let mut s = self.db.prepare(
            "resolve game alias",
            "SELECT game_id, head_revision FROM game_aliases WHERE alias = ?1",
        )?;
        s.bind_text(1, alias.as_str())?;
        if !s.step()? {
            return Ok(None);
        }
        Ok(Some((
            GameId::from_bytes(fixed16(&s.column_blob(0), "game alias row")?),
            GameRevisionId::from_bytes(fixed32(&s.column_blob(1), "game alias row")?),
        )))
    }
}

/// A stored ID column that is not exactly its fixed width is catalog
/// corruption; refuse rather than truncate or pad.
pub(crate) fn fixed32(bytes: &[u8], what: &'static str) -> ServerResult<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|_| ServerError::SizeMismatch { what, expected: 32, found: bytes.len() as u64 })
}

pub(crate) fn fixed16(bytes: &[u8], what: &'static str) -> ServerResult<[u8; 16]> {
    bytes
        .try_into()
        .map_err(|_| ServerError::SizeMismatch { what, expected: 16, found: bytes.len() as u64 })
}
