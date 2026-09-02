//! Portable, transport-free catalog core.
//!
//! This layer owns only the synchronous SQLite working set and budgets. It
//! contains no filesystem, HTTP, thread, watcher, or browser-storage code.

use crate::auth::Auth;
use crate::budget::Budgets;
use crate::catalog::{CandidateRow, CandidateState, Catalog, CATALOG_SCHEMA};
use crate::error::{ServerError, ServerResult};
use crate::gc::{Gc, GC_SCHEMA};
use crate::imports::{Imports, IMPORT_SCHEMA};
use crate::search::{kind_name, kind_parse, AssetAnnotation, Search, SEARCH_SCHEMA};
use crate::sqlite::Db;
use crate::variants::{Variants, VARIANT_SCHEMA};
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, AssetManifest, AssetRevisionId, AssetRevisionRef,
};

pub const SERVER_SCHEMA_VERSION: i64 = 13;

// GC keeps reference-blob handling fail-closed even though embedded mode can
// never create one. Keeping the empty table in the portable schema avoids a
// target-specific SQL branch in the collector.
const EMPTY_BLOBREF_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS blob_refs(
    blob_id BLOB PRIMARY KEY,
    path TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime_ms INTEGER,
    recorded_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS blob_refs_by_path ON blob_refs(path);
";

pub struct CatalogCore {
    pub(crate) db: Db,
    pub(crate) budgets: Budgets,
}

impl CatalogCore {
    /// Open a fresh in-memory catalog through SQ1's `MemoryStoreSet`.
    pub fn open_memory(budgets: Budgets) -> ServerResult<Self> {
        budgets.validate()?;
        let timeout = budgets.db_busy_timeout_ms;
        let db = Db::open_memory(timeout)?;
        Self::initialize(db, budgets)
    }

    /// Open a catalog on a caller-supplied SQ1 page-store set.
    pub fn open_with<S: makepad_sqlite::PageStoreSet + 'static>(
        stores: S,
        budgets: Budgets,
    ) -> ServerResult<Self> {
        budgets.validate()?;
        let timeout = budgets.db_busy_timeout_ms;
        let db = Db::open_with(stores, timeout)?;
        Self::initialize(db, budgets)
    }

    #[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
    pub(crate) fn from_parts(db: Db, budgets: Budgets) -> Self {
        Self { db, budgets }
    }

    fn initialize(db: Db, budgets: Budgets) -> ServerResult<Self> {
        // Embedded catalogs have one synchronous owner and use rollback
        // journal mode. Browser durability is outside the pager in E2.
        db.exec("set rollback journal", "PRAGMA journal_mode=DELETE")?;
        db.exec("set synchronous", "PRAGMA synchronous=FULL")?;
        db.exec("set foreign keys", "PRAGMA foreign_keys=ON")?;
        let version = {
            let mut stmt = db.prepare("get user_version", "PRAGMA user_version")?;
            if stmt.step()? { stmt.column_i64(0) } else { 0 }
        };
        if version != 0 && version != SERVER_SCHEMA_VERSION {
            return Err(ServerError::UnsupportedSchema { found: version });
        }
        db.tx(|db| {
            db.exec("create catalog schema", CATALOG_SCHEMA)?;
            db.exec("create auth schema", crate::auth::AUTH_SCHEMA)?;
            db.exec("create search schema", SEARCH_SCHEMA)?;
            db.exec("create import schema", IMPORT_SCHEMA)?;
            db.exec("create variant schema", VARIANT_SCHEMA)?;
            db.exec("create gc schema", GC_SCHEMA)?;
            db.exec("create empty blob-ref schema", EMPTY_BLOBREF_SCHEMA)?;
            if version == 0 {
                db.exec(
                    "set user_version",
                    &format!("PRAGMA user_version={SERVER_SCHEMA_VERSION}"),
                )?;
            }
            Ok(())
        })?;
        Ok(Self { db, budgets })
    }

    pub fn budgets(&self) -> &Budgets {
        &self.budgets
    }

    pub fn catalog(&self) -> Catalog<'_> {
        Catalog { db: &self.db, budgets: &self.budgets }
    }

    pub fn auth(&self) -> Auth<'_> {
        Auth { db: &self.db }
    }

    pub fn search(&self) -> Search<'_> {
        Search { db: &self.db, budgets: &self.budgets }
    }

    pub fn imports(&self) -> Imports<'_> {
        Imports { db: &self.db, budgets: &self.budgets }
    }

    pub fn variants(&self) -> Variants<'_> {
        Variants { db: &self.db, budgets: &self.budgets }
    }

    pub fn gc(&self) -> Gc<'_> {
        Gc { db: &self.db, budgets: &self.budgets }
    }

    pub fn detail(&self, asset_id: &AssetId) -> ServerResult<Option<AssetDetail>> {
        let Some(namespace) = self.catalog().asset_namespace(asset_id)? else {
            return Ok(None);
        };
        let annotation = self.search().annotation(asset_id)?;
        let candidates = self.catalog().asset_candidates(asset_id, 512)?;
        let mut stmt = self.db.prepare(
            "asset detail aliases",
            "SELECT alias, head_revision FROM asset_aliases
             WHERE asset_id=?1 ORDER BY alias",
        )?;
        stmt.bind_blob(1, asset_id.as_bytes())?;
        let mut aliases = Vec::new();
        while stmt.step()? {
            aliases.push((
                AssetAlias::new(stmt.column_text(0))?,
                AssetRevisionRef {
                    asset_id: *asset_id,
                    revision: AssetRevisionId::from_bytes(crate::catalog::fixed32(
                        &stmt.column_blob(1),
                        "asset detail alias revision",
                    )?),
                },
            ));
        }
        Ok(Some(AssetDetail { asset_id: *asset_id, namespace, annotation, candidates, aliases }))
    }

    /// Enumerate a bounded, public-only page for a sink-independent export
    /// planner. No filesystem path or transport value crosses this boundary.
    pub fn public_export_page(
        &self,
        filter: PublicExportFilter<'_>,
    ) -> ServerResult<PublicExportPage> {
        if filter.limit == 0 || filter.limit > self.budgets.max_search_results {
            return Err(ServerError::OverBudget {
                what: "public export page size",
                limit: self.budgets.max_search_results as u64,
                found: filter.limit as u64,
            });
        }
        if let Some(namespace) = filter.namespace {
            crate::catalog::validate_namespace(namespace)?;
        }
        let mut stmt = self.db.prepare(
            "public export page",
            "SELECT a.asset_id, a.namespace, a.created_ms,
                    sa.title, sa.description, sa.kind, sa.creator,
                    sa.generator, sa.backend, sa.model, sa.updated_ms
             FROM assets a
             JOIN search_annotations sa ON sa.asset_id = a.asset_id
             WHERE a.retired_ms IS NULL
               AND sa.visibility = 'public' AND sa.live = 1
               AND (?1 IS NULL OR a.namespace = ?1)
               AND (?2 IS NULL OR sa.kind = ?2)
               AND (?3 IS NULL OR a.asset_id > ?3)
               AND EXISTS(
                    SELECT 1 FROM asset_aliases aa
                    JOIN candidates c ON c.kind='asset'
                       AND c.owner_id=aa.asset_id AND c.revision=aa.head_revision
                    WHERE aa.asset_id=a.asset_id AND c.state='published'
                      AND c.retired_ms IS NULL)
             ORDER BY a.asset_id LIMIT ?4",
        )?;
        match filter.namespace {
            Some(namespace) => stmt.bind_text(1, namespace)?,
            None => stmt.bind_null(1)?,
        }
        match filter.kind {
            Some(kind) => stmt.bind_text(2, kind_name(kind))?,
            None => stmt.bind_null(2)?,
        }
        match filter.after {
            Some(asset_id) => stmt.bind_blob(3, asset_id.as_bytes())?,
            None => stmt.bind_null(3)?,
        }
        stmt.bind_u64(4, filter.limit as u64 + 1)?;

        let mut base = Vec::new();
        while stmt.step()? {
            base.push((
                AssetId::from_bytes(crate::catalog::fixed16(
                    &stmt.column_blob(0),
                    "public export asset id",
                )?),
                stmt.column_text(1),
                stmt.column_u64(2),
                stmt.column_text(3),
                stmt.column_text(4),
                if stmt.column_is_null(5) {
                    None
                } else {
                    Some(kind_parse(&stmt.column_text(5)).ok_or(
                        ServerError::InvalidState {
                            what: "public export annotation kind",
                            state: "unknown",
                        },
                    )?)
                },
                stmt.column_text(6),
                stmt.column_text(7),
                stmt.column_text(8),
                stmt.column_text(9),
                stmt.column_u64(10),
            ));
        }
        drop(stmt);
        let more = base.len() > filter.limit as usize;
        base.truncate(filter.limit as usize);
        let next = if more { base.last().map(|row| row.0) } else { None };
        let mut assets = Vec::with_capacity(base.len());
        for (
            asset_id,
            namespace,
            created_ms,
            title,
            description,
            kind,
            creator,
            generator,
            backend,
            model,
            updated_ms,
        ) in base
        {
            let mut aliases = Vec::new();
            let mut alias_stmt = self.db.prepare(
                "public export aliases",
                "SELECT aa.alias, aa.head_revision, aa.updated_ms, c.published_ms
                 FROM asset_aliases aa
                 JOIN candidates c ON c.kind='asset' AND c.owner_id=aa.asset_id
                    AND c.revision=aa.head_revision
                 WHERE aa.asset_id=?1 AND c.state='published' AND c.retired_ms IS NULL
                 ORDER BY aa.alias LIMIT ?2",
            )?;
            alias_stmt.bind_blob(1, asset_id.as_bytes())?;
            alias_stmt.bind_u64(2, self.budgets.max_search_index_terms as u64 + 1)?;
            while alias_stmt.step()? {
                aliases.push(PublicAliasHead {
                    alias: AssetAlias::new(alias_stmt.column_text(0))?,
                    target: AssetRevisionRef {
                        asset_id,
                        revision: AssetRevisionId::from_bytes(crate::catalog::fixed32(
                            &alias_stmt.column_blob(1),
                            "public export alias revision",
                        )?),
                    },
                    updated_ms: alias_stmt.column_u64(2),
                    published_ms: alias_stmt.column_u64(3),
                });
            }
            drop(alias_stmt);
            if aliases.len() > self.budgets.max_search_index_terms as usize {
                return Err(ServerError::OverBudget {
                    what: "public export aliases per asset",
                    limit: self.budgets.max_search_index_terms as u64,
                    found: aliases.len() as u64,
                });
            }

            let mut categories = Vec::new();
            let mut tags = Vec::new();
            let mut labels = self.db.prepare(
                "public export labels",
                "SELECT kind, label FROM search_labels
                 WHERE asset_id=?1 ORDER BY kind, label",
            )?;
            labels.bind_blob(1, asset_id.as_bytes())?;
            while labels.step()? {
                if labels.column_text(0) == "category" {
                    categories.push(labels.column_text(1));
                } else {
                    tags.push(labels.column_text(1));
                }
            }
            drop(labels);

            let mut terms = Vec::new();
            let mut postings = self.db.prepare(
                "public export postings",
                "SELECT term, SUM(weight) FROM (
                     SELECT term, weight_public AS weight FROM search_postings
                     WHERE asset_id=?1 AND weight_public>0
                     UNION ALL
                     SELECT term, weight FROM search_alias_postings
                     WHERE asset_id=?1 AND weight>0
                 ) GROUP BY term ORDER BY term",
            )?;
            postings.bind_blob(1, asset_id.as_bytes())?;
            while postings.step()? {
                terms.push(PublicSearchTerm {
                    term: postings.column_text(0),
                    weight: postings.column_u64(1),
                });
            }
            assets.push(PublicExportAsset {
                asset_id,
                namespace,
                created_ms,
                aliases,
                search: PublicSearchProjection {
                    title,
                    description,
                    kind,
                    categories,
                    tags,
                    creator,
                    generator,
                    backend,
                    model,
                    updated_ms,
                    terms,
                },
            });
        }
        Ok(PublicExportPage { assets, next })
    }

    /// Atomic catalog half of publication. Blob durability/admission must
    /// complete before this method is called.
    pub fn publish_batch(
        &self,
        items: &[PublishBatchItem],
        now_ms: u64,
    ) -> ServerResult<Vec<PublishBatchOutcome>> {
        let mut decoded: Vec<(AssetManifest, AssetRevisionId)> = Vec::with_capacity(items.len());
        for item in items {
            if item.manifest_bytes.len() as u64 > self.budgets.max_manifest_bytes {
                return Err(ServerError::OverBudget {
                    what: "asset manifest bytes",
                    limit: self.budgets.max_manifest_bytes,
                    found: item.manifest_bytes.len() as u64,
                });
            }
            let manifest = AssetManifest::from_canonical_bytes(&item.manifest_bytes)?;
            let revision = AssetRevisionId::hash_of(&item.manifest_bytes);
            if let Some(alias) = &item.alias {
                if alias.namespace() != item.namespace {
                    return Err(ServerError::Conflict { what: "alias namespace" });
                }
            }
            let candidates = self.catalog().asset_candidates(&manifest.asset_id, 512)?;
            let previous = candidates
                .iter()
                .filter(|row| {
                    row.state == CandidateState::Published && row.revision != revision
                })
                .max_by_key(|row| row.published_ms.unwrap_or(0))
                .map(|row| row.revision);
            if let Some(previous) = previous {
                if let Some(bytes) = self.catalog().asset_revision_manifest(&previous)? {
                    if let Ok(previous) = AssetManifest::from_canonical_bytes(&bytes) {
                        if previous.rights != manifest.rights {
                            return Err(ServerError::Conflict {
                                what: "published asset rights would change",
                            });
                        }
                    }
                }
            }
            decoded.push((manifest, revision));
        }

        let catalog = self.catalog();
        let search = self.search();
        self.db.tx(|db| {
            let mut outcomes = Vec::with_capacity(items.len());
            for (item, (manifest, revision)) in items.iter().zip(&decoded) {
                catalog.register_asset(&manifest.asset_id, &item.namespace, now_ms)?;
                let already_published = match catalog
                    .asset_candidate_state(&manifest.asset_id, revision)?
                {
                    Some(CandidateState::Published) => true,
                    Some(CandidateState::Quarantined) => {
                        return Err(ServerError::InvalidState {
                            what: "publish batch revision",
                            state: "quarantined",
                        });
                    }
                    Some(CandidateState::Staged) => {
                        catalog.transition_in_tx(
                            db,
                            "asset",
                            manifest.asset_id.as_bytes(),
                            revision.as_bytes(),
                            &[CandidateState::Staged],
                            CandidateState::Published,
                            now_ms,
                        )?;
                        false
                    }
                    None => {
                        let staged = catalog.stage_asset_revision_in_tx(
                            db,
                            &item.manifest_bytes,
                            now_ms,
                        )?;
                        catalog.transition_in_tx(
                            db,
                            "asset",
                            manifest.asset_id.as_bytes(),
                            staged.as_bytes(),
                            &[CandidateState::Staged],
                            CandidateState::Published,
                            now_ms,
                        )?;
                        false
                    }
                };
                search.set_annotation_in_tx(db, &manifest.asset_id, &item.annotation, now_ms)?;
                if let Some(alias) = &item.alias {
                    catalog.set_asset_alias_in_tx(
                        db,
                        alias,
                        &AssetRevisionRef {
                            asset_id: manifest.asset_id,
                            revision: *revision,
                        },
                        now_ms,
                    )?;
                }
                outcomes.push(PublishBatchOutcome {
                    asset_id: manifest.asset_id,
                    revision: *revision,
                    already_published,
                });
            }
            Ok(outcomes)
        })
    }
}

#[derive(Clone, Debug)]
pub struct AssetDetail {
    pub asset_id: AssetId,
    pub namespace: String,
    pub annotation: Option<AssetAnnotation>,
    pub candidates: Vec<CandidateRow>,
    pub aliases: Vec<(AssetAlias, AssetRevisionRef)>,
}

#[derive(Clone, Debug)]
pub struct PublishBatchItem {
    pub namespace: String,
    pub manifest_bytes: Vec<u8>,
    pub annotation: AssetAnnotation,
    pub alias: Option<AssetAlias>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublishBatchOutcome {
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
    pub already_published: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicAliasHead {
    pub alias: AssetAlias,
    pub target: AssetRevisionRef,
    pub updated_ms: u64,
    pub published_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicSearchTerm {
    pub term: String,
    pub weight: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicSearchProjection {
    pub title: String,
    pub description: String,
    pub kind: Option<AssetKind>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub creator: String,
    pub generator: String,
    pub backend: String,
    pub model: String,
    pub updated_ms: u64,
    pub terms: Vec<PublicSearchTerm>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicExportAsset {
    pub asset_id: AssetId,
    pub namespace: String,
    pub created_ms: u64,
    pub aliases: Vec<PublicAliasHead>,
    pub search: PublicSearchProjection,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PublicExportPage {
    pub assets: Vec<PublicExportAsset>,
    pub next: Option<AssetId>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PublicExportFilter<'a> {
    pub namespace: Option<&'a str>,
    pub kind: Option<AssetKind>,
    pub after: Option<AssetId>,
    pub limit: u32,
}
