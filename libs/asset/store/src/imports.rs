//! Deterministic external-pack import: approved source collections and the
//! atomic import transaction.
//!
//! Laws enforced here, all fail-closed:
//! - Imports run only under a REGISTERED source collection whose digest the
//!   manifest pins; an unknown or divergent collection refuses.
//! - Every source blob must already be committed to CAS with the exact
//!   declared size; the import writes no blobs itself.
//! - One import is ONE transaction: assets, revisions, publications, aliases,
//!   entry rows, and the import record land together or not at all. A crash
//!   never leaves a partially visible pack.
//! - Re-running the same manifest is idempotent: the row key is the
//!   `ImportRevisionId` (digest of the canonical bytes), so the second run
//!   returns the recorded result without re-doing work.
//! - A changed pack (new version, entry, or rights) is a NEW import revision
//!   producing new asset revisions; prior published records are never edited.

use crate::budget::Budgets;
use crate::catalog::{fixed16, fixed32, CandidateState, Catalog};
use crate::error::{ServerError, ServerResult};
use crate::sqlite::Db;
use makepad_asset_data::{
    AssetId, AssetRevisionId, AssetRevisionRef, ImportManifest, ImportRevisionId, SourceCollection,
    SourceCollectionId,
};

/// Largest internal source-list query. HTTP asks for one row beyond its
/// 512-row legacy ceiling so it can refuse an oversized unpaged request
/// without ever materializing the complete table.
pub const MAX_SOURCE_PAGE_ROWS: u32 = 513;

pub const IMPORT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS import_sources(
    source_id TEXT PRIMARY KEY,
    digest BLOB NOT NULL,
    manifest BLOB NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS imports(
    import_revision BLOB PRIMARY KEY,
    source_id TEXT NOT NULL,
    manifest BLOB NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS import_entries(
    import_revision BLOB NOT NULL,
    entry_key TEXT NOT NULL,
    asset_id BLOB NOT NULL,
    asset_revision BLOB NOT NULL,
    PRIMARY KEY(import_revision, entry_key)
);
";

/// One imported entry as recorded: stable key to exact identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportEntryRow {
    pub key: String,
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
}

/// The recorded outcome of one import run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportReport {
    pub import_revision: ImportRevisionId,
    /// False when this exact import had already run (idempotent replay).
    pub created: bool,
    pub entries: Vec<ImportEntryRow>,
}

pub struct Imports<'a> {
    pub(crate) db: &'a Db,
    pub(crate) budgets: &'a Budgets,
}

impl<'a> Imports<'a> {
    fn catalog(&self) -> Catalog<'a> {
        Catalog {
            db: self.db,
            budgets: self.budgets,
        }
    }

    // ---- approved sources --------------------------------------------------

    /// Register an approved source collection from canonical bytes. Same
    /// digest is idempotent; a DIFFERENT collection under an existing id
    /// refuses — approval is not silently rewritable.
    pub fn register_source(&self, bytes: &[u8], now_ms: u64) -> ServerResult<SourceCollectionId> {
        if bytes.len() as u64 > self.budgets.max_manifest_bytes {
            return Err(ServerError::OverBudget {
                what: "source collection bytes",
                limit: self.budgets.max_manifest_bytes,
                found: bytes.len() as u64,
            });
        }
        let collection = SourceCollection::from_canonical_bytes(bytes)?;
        let digest = SourceCollectionId::hash_of(bytes);
        self.db.tx(|db| {
            if let Some((existing, _)) = self.source_row(&collection.id)? {
                if existing != digest {
                    return Err(ServerError::Conflict { what: "source collection digest" });
                }
                return Ok(digest);
            }
            let mut s = db.prepare(
                "register source",
                "INSERT INTO import_sources(source_id, digest, manifest, created_ms)
                 VALUES(?1, ?2, ?3, ?4)",
            )?;
            s.bind_text(1, &collection.id)?;
            s.bind_blob(2, digest.as_bytes())?;
            s.bind_blob(3, bytes)?;
            s.bind_u64(4, now_ms)?;
            s.run()?;
            Ok(digest)
        })
    }

    fn source_row(&self, source_id: &str) -> ServerResult<Option<(SourceCollectionId, Vec<u8>)>> {
        let mut s = self.db.prepare(
            "source row",
            "SELECT digest, manifest FROM import_sources WHERE source_id = ?1",
        )?;
        s.bind_text(1, source_id)?;
        if !s.step()? {
            return Ok(None);
        }
        Ok(Some((
            SourceCollectionId::from_bytes(fixed32(&s.column_blob(0), "source row")?),
            s.column_blob(1),
        )))
    }

    /// Canonical bytes of one approved collection.
    pub fn source_manifest(&self, source_id: &str) -> ServerResult<Option<Vec<u8>>> {
        Ok(self.source_row(source_id)?.map(|(_, bytes)| bytes))
    }

    /// Every approved collection's canonical bytes, ordered by source id.
    /// Maintenance/testing helper; request paths must use [`Self::sources_page`]
    /// so caller-controlled reads remain bounded at the storage layer.
    pub fn sources(&self) -> ServerResult<Vec<Vec<u8>>> {
        let mut s = self.db.prepare(
            "list sources",
            "SELECT manifest FROM import_sources ORDER BY source_id",
        )?;
        let mut out = Vec::new();
        while s.step()? {
            out.push(s.column_blob(0));
        }
        Ok(out)
    }

    /// A storage-bounded keyset page of approved collections, in canonical
    /// source-id order. `after` is the exact last source id consumed by the
    /// caller; it is validated with the same slug grammar as
    /// [`SourceCollection::id`] before becoming a SQL bind.
    pub fn sources_page(&self, after: Option<&str>, limit: u32) -> ServerResult<Vec<Vec<u8>>> {
        if limit == 0 || limit > MAX_SOURCE_PAGE_ROWS {
            return Err(ServerError::InvalidInput { what: "source page limit" });
        }
        if let Some(cursor) = after {
            validate_source_cursor(cursor)?;
        }

        // Canonical source ids are lowercase ASCII slugs, so SQLite's default
        // BINARY TEXT order is identical to their byte/display order. Keep
        // the two static statements separate: no nullable comparison trick
        // and no caller data ever enters SQL text.
        let mut s = match after {
            Some(_) => self.db.prepare(
                "page sources after",
                "SELECT manifest FROM import_sources
                 WHERE source_id > ?1 ORDER BY source_id LIMIT ?2",
            )?,
            None => self.db.prepare(
                "page sources first",
                "SELECT manifest FROM import_sources ORDER BY source_id LIMIT ?1",
            )?,
        };
        match after {
            Some(cursor) => {
                s.bind_text(1, cursor)?;
                s.bind_i64(2, limit as i64)?;
            }
            None => s.bind_i64(1, limit as i64)?,
        }
        let mut out = Vec::with_capacity(limit as usize);
        while s.step()? {
            out.push(s.column_blob(0));
        }
        Ok(out)
    }

    // ---- the import transaction --------------------------------------------

    /// Run one deterministic pack import from canonical manifest bytes.
    ///
    /// On success every entry's asset exists in the collection's namespace,
    /// its revision is staged AND published, its deterministic alias points
    /// at it, and the import/entry rows record the exact mapping — all in one
    /// transaction. Identical re-submission returns the recorded report.
    pub fn run_import(&self, manifest_bytes: &[u8], now_ms: u64) -> ServerResult<ImportReport> {
        if manifest_bytes.len() as u64 > self.budgets.max_manifest_bytes {
            return Err(ServerError::OverBudget {
                what: "import manifest bytes",
                limit: self.budgets.max_manifest_bytes,
                found: manifest_bytes.len() as u64,
            });
        }
        let manifest = ImportManifest::from_canonical_bytes(manifest_bytes)?;
        if manifest.assets.len() as u64 > self.budgets.max_import_assets as u64 {
            return Err(ServerError::OverBudget {
                what: "import assets",
                limit: self.budgets.max_import_assets as u64,
                found: manifest.assets.len() as u64,
            });
        }
        let import_revision = ImportRevisionId::hash_of(manifest_bytes);
        let catalog = self.catalog();
        self.db.tx(|db| {
            // Idempotent replay: the exact same bytes already ran.
            if self.import_manifest_bytes(&import_revision)?.is_some() {
                return Ok(ImportReport {
                    import_revision,
                    created: false,
                    entries: self.entries(&import_revision)?,
                });
            }
            // The manifest must name a REGISTERED collection, by digest AND
            // by id — a stale or foreign collection digest refuses.
            let (registered_digest, registered_bytes) = self
                .source_row(&manifest.source_id)?
                .ok_or(ServerError::NotFound { what: "source collection" })?;
            if registered_digest != manifest.source_collection {
                return Err(ServerError::Conflict { what: "source collection digest" });
            }
            // The registered collection's terms are AUTHORITATIVE: the
            // manifest must carry exactly them. An importer can never
            // downgrade a CC-BY source to CC0, drop the credits line, unpin
            // the terms digest, or loosen redistribution/derivative policy.
            let registered = SourceCollection::from_canonical_bytes(&registered_bytes)?;
            if manifest.rights != registered.terms {
                return Err(ServerError::Conflict { what: "import rights vs registered source" });
            }

            let mut entries = Vec::with_capacity(manifest.assets.len());
            for asset in &manifest.assets {
                let produced = manifest.asset_manifest_for(asset, &import_revision)?;
                let asset_id = produced.asset_id;
                // The asset identity lives in the collection's namespace.
                // Registration is idempotent; a namespace conflict means the
                // deterministic id collided with a foreign asset — refuse.
                catalog.register_asset(&asset_id, &manifest.source_id, now_ms)?;
                let produced_bytes = produced.to_canonical_bytes()?;
                let revision = match catalog.asset_candidate_state(
                    &asset_id,
                    &AssetRevisionId::hash_of(&produced_bytes),
                )? {
                    // A previous import (e.g. an older pack version re-run)
                    // already published this exact revision: keep it.
                    Some(CandidateState::Published) => AssetRevisionId::hash_of(&produced_bytes),
                    Some(CandidateState::Quarantined) => {
                        // Quarantine is terminal: an import never resurrects
                        // pulled content.
                        return Err(ServerError::InvalidState {
                            what: "imported revision",
                            state: "quarantined",
                        });
                    }
                    _ => {
                        let revision =
                            catalog.stage_asset_revision_in_tx(db, &produced_bytes, now_ms)?;
                        catalog.transition_in_tx(
                            db,
                            "asset",
                            asset_id.as_bytes(),
                            revision.as_bytes(),
                            &[CandidateState::Staged],
                            CandidateState::Published,
                            now_ms,
                        )?;
                        revision
                    }
                };
                // Deterministic alias head. Import is release truth for its
                // own namespace, so the head advances to this revision.
                let alias = manifest.alias_for(&asset.key)?;
                catalog.set_asset_alias_in_tx(
                    db,
                    &alias,
                    &AssetRevisionRef { asset_id, revision },
                    now_ms,
                )?;

                let mut s = db.prepare(
                    "insert import entry",
                    "INSERT INTO import_entries(import_revision, entry_key, asset_id, asset_revision)
                     VALUES(?1, ?2, ?3, ?4)",
                )?;
                s.bind_blob(1, import_revision.as_bytes())?;
                s.bind_text(2, asset.key.as_str())?;
                s.bind_blob(3, asset_id.as_bytes())?;
                s.bind_blob(4, revision.as_bytes())?;
                s.run()?;

                entries.push(ImportEntryRow {
                    key: asset.key.as_str().to_string(),
                    asset_id,
                    revision,
                });
            }

            let mut s = db.prepare(
                "insert import",
                "INSERT INTO imports(import_revision, source_id, manifest, created_ms)
                 VALUES(?1, ?2, ?3, ?4)",
            )?;
            s.bind_blob(1, import_revision.as_bytes())?;
            s.bind_text(2, &manifest.source_id)?;
            s.bind_blob(3, manifest_bytes)?;
            s.bind_u64(4, now_ms)?;
            s.run()?;

            Ok(ImportReport {
                import_revision,
                created: true,
                entries,
            })
        })
    }

    /// Canonical bytes of one recorded import.
    pub fn import_manifest_bytes(
        &self,
        import_revision: &ImportRevisionId,
    ) -> ServerResult<Option<Vec<u8>>> {
        let mut s = self.db.prepare(
            "import manifest",
            "SELECT manifest FROM imports WHERE import_revision = ?1",
        )?;
        s.bind_blob(1, import_revision.as_bytes())?;
        if s.step()? {
            Ok(Some(s.column_blob(0)))
        } else {
            Ok(None)
        }
    }

    /// The recorded entry mapping of one import, ordered by entry key.
    pub fn entries(&self, import_revision: &ImportRevisionId) -> ServerResult<Vec<ImportEntryRow>> {
        let mut s = self.db.prepare(
            "import entries",
            "SELECT entry_key, asset_id, asset_revision FROM import_entries
             WHERE import_revision = ?1 ORDER BY entry_key",
        )?;
        s.bind_blob(1, import_revision.as_bytes())?;
        let mut out = Vec::new();
        while s.step()? {
            out.push(ImportEntryRow {
                key: s.column_text(0),
                asset_id: AssetId::from_bytes(fixed16(&s.column_blob(1), "import entry row")?),
                revision: AssetRevisionId::from_bytes(fixed32(
                    &s.column_blob(2),
                    "import entry row",
                )?),
            });
        }
        Ok(out)
    }
}

fn validate_source_cursor(cursor: &str) -> ServerResult<()> {
    if cursor.is_empty()
        || cursor.len() > makepad_asset_data::limits::MAX_ALIAS_SEGMENT_BYTES
    {
        return Err(ServerError::InvalidInput { what: "source page cursor" });
    }
    let bytes = cursor.as_bytes();
    if (!bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit())
        || !cursor
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
    {
        return Err(ServerError::InvalidInput { what: "source page cursor" });
    }
    Ok(())
}

#[cfg(all(
    test,
    feature = "native",
    not(any(target_arch = "wasm32", feature = "embedded"))
))]
mod tests {
    use super::*;
    use crate::{AssetServerCore, Budgets};
    use makepad_asset_data::{
        sha256, DerivativePolicy, Redistribution, Rights, SourceOrigin,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_ROOT: AtomicU64 = AtomicU64::new(0);

    fn core(name: &str) -> AssetServerCore {
        let n = TEST_ROOT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "mp_asset_import_page_{}_{}_{}",
            std::process::id(),
            n,
            name
        ));
        AssetServerCore::open(&root, Budgets::default_v1()).unwrap()
    }

    fn collection(id: &str) -> Vec<u8> {
        SourceCollection {
            id: id.into(),
            title: format!("{id} source"),
            origin: SourceOrigin::Upload,
            terms: Rights {
                license: "CC0-1.0".into(),
                license_revision: String::new(),
                terms_digest: Some(sha256(b"CC0-1.0 legal text")),
                terms_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
                credits: "Paging fixture".into(),
                source: "https://example.invalid/assets".into(),
                source_archive: Some(sha256(b"paging-fixture")),
                redistribution: Redistribution::Allowed,
                derivatives: DerivativePolicy::Allowed,
            },
        }
        .to_canonical_bytes()
        .unwrap()
    }

    fn ids(page: Vec<Vec<u8>>) -> Vec<String> {
        page.into_iter()
            .map(|bytes| SourceCollection::from_canonical_bytes(&bytes).unwrap().id)
            .collect()
    }

    #[test]
    fn source_pages_are_storage_bounded_ordered_and_keyset_exact() {
        let core = core("order");
        for id in ["delta", "alpha", "echo", "bravo", "charlie"] {
            core.imports().register_source(&collection(id), 1).unwrap();
        }

        assert_eq!(
            ids(core.imports().sources_page(None, 2).unwrap()),
            vec!["alpha", "bravo"]
        );
        assert_eq!(
            ids(core.imports().sources_page(Some("bravo"), 2).unwrap()),
            vec!["charlie", "delta"]
        );
        assert_eq!(
            ids(core.imports().sources_page(Some("delta"), 2).unwrap()),
            vec!["echo"]
        );
        assert!(core
            .imports()
            .sources_page(Some("echo"), MAX_SOURCE_PAGE_ROWS)
            .unwrap()
            .is_empty());

        for bad in ["", "Bravo", "has/slash", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"] {
            assert!(matches!(
                core.imports().sources_page(Some(bad), 1),
                Err(ServerError::InvalidInput { what: "source page cursor" })
            ));
        }
        for bad in [0, MAX_SOURCE_PAGE_ROWS + 1] {
            assert!(matches!(
                core.imports().sources_page(None, bad),
                Err(ServerError::InvalidInput { what: "source page limit" })
            ));
        }
    }
}
