//! Makepad Asset Server headless core.
//!
//! The durable, transport-free heart of the Asset Server, built directly on
//! the canonical content contract (`makepad-asset-data`). Everything here
//! is deterministic and fail-closed:
//!
//! - **CAS** ([`cas`]): filesystem SHA-256 store. Streaming hash-while-write
//!   into temp files, fsync + atomic rename commit, dedup by digest,
//!   re-hash-on-read corruption refusal, orphan cleanup on restart.
//! - **Reference blobs** ([`blobrefs`]): content the store catalogues without
//!   copying — the original file stays at its own path and only its path,
//!   size and digest are recorded. Every read re-verifies all three, so such
//!   a blob can become unavailable but never wrong. For libraries of video
//!   too large to duplicate; the store never deletes a referenced file.
//! - **Catalog** ([`catalog`]): SQLite (WAL) rows for blobs, immutable
//!   asset/game revisions keyed by the SHA-256 of their canonical manifest
//!   bytes, staged/published/quarantined candidates, mutable alias heads,
//!   and exact game→asset revision refs.
//! - **Jobs** ([`jobs`]): durable operation graph with dependency edges,
//!   recorded attempts, worker leases + heartbeats, and hierarchical
//!   cancellation.
//! - **Auth** ([`auth`]): principals, hashed-only tokens, and explicit
//!   capability grants scoped by namespace. Secrets never enter a record.
//! - **Search** ([`search`]): mutable annotations (title, description, kind,
//!   categories, tags, creator, generator chain, prompt) plus alias heads
//!   over a lexical posting index. Deterministic integer ranking ordered
//!   score, canonical alias, asset id; opaque generation-carrying keyset
//!   cursors bound to the query shape; privacy-safe snippets; per-viewer
//!   weights that give non-owners zero signal from private fields.
//! - **Seed** ([`seed`]): deterministic stock-content interfaces; identities
//!   derive from content so every deployment seeds identically.
//! - **Imports** ([`imports`]): approved external source collections and the
//!   atomic deterministic pack-import transaction — one manifest becomes
//!   published revisions, aliases, and an immutable entry map, whole or not
//!   at all, idempotent by import revision.
//! - **Deletion** ([`catalog`] retirement + [`gc`]): retiring an asset or a
//!   superseded revision is the terminal quarantine transition plus a
//!   deletion intent, cost proportional to that asset alone; the bytes it
//!   named are reclaimed later by an incremental, restartable, crash-safe
//!   mark-and-sweep over the whole store that never blocks publishes.
//! - **Variants** ([`variants`]): canonical processing recipes, the
//!   single-flight derivation cache keyed by exact content, typed worker job
//!   arming (the server never runs kernels), validated completion, immutable
//!   derived variants, frozen variant sets, and deterministic per-client
//!   resolution.
//!
//! Time never comes from a clock in the core: every mutating API takes an
//! explicit `now_ms`. The HTTP/UDP host ([`host`]) supplies real time,
//! transport auth secrets, and randomness for opaque IDs.

pub mod auth;
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub mod blobrefs;
pub mod budget;
pub mod cas;
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
mod cas_file;
pub mod catalog;
pub mod core;
#[cfg(any(target_arch = "wasm32", feature = "embedded"))]
pub mod embedded;
pub mod error;
pub mod gc;
pub mod imports;
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub mod observe;
pub mod search;
pub mod seed;
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub mod server;
pub mod static_export_core;
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub mod static_export;
pub mod variants;
mod sqlite;
mod synonyms;

pub use auth::{token_hash, Auth, Capability, PrincipalId, Scope};
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub use blobrefs::{BlobRef, BlobRefs, RefScan, RefState};
pub use budget::Budgets;
pub use cas::{BlobCommit, Cas, MemoryCas};
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub use cas_file::{BlobWriter, FsCas};
pub use catalog::{validate_namespace, CandidateState, Catalog, RetireReport};
pub use core::{
    AssetDetail, CatalogCore, PublicAliasHead, PublicAssetRevision, PublicExportAsset,
    PublicExportFilter, PublicExportPage, PublicSearchProjection, PublicSearchTerm, PublishBatchItem,
    PublishBatchOutcome, SERVER_SCHEMA_VERSION,
};
#[cfg(any(target_arch = "wasm32", feature = "embedded"))]
pub use embedded::*;
pub use error::{ServerError, ServerResult};
pub use gc::{Gc, GcConfig, GcDeleteIntent, GcPhase, GcStatus, GcStep, PhysicalDelete};
pub use imports::{ImportEntryRow, ImportReport, Imports};
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub use observe::{ObserveConfig, Outcome as ObserveOutcome};
pub use variants::{
    DerivationOutcome, DerivationStatus, DerivedResult, Variants, MAX_DERIVATION_ROUNDS,
};
pub use search::{
    kind_name, kind_parse, AssetAnnotation, BacklogRow, Search, SearchFilters, SearchHit,
    SearchPage, SearchQuery, SearchViewer, ViewerScope, Visibility, ANNOTATABLE_KINDS,
};
pub use seed::{stock_asset_id, SeedAsset, SeedReport, StockSeedSource};
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub use server::{
    AssetServerCore, BlobRefCommit, RecoverReport, RefRescanPage,
};
pub use static_export_core::{brotli_bytes, ExportEntry, ExportPlan, ExportSink, ExportStep};
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub use static_export::{export_static, StaticExportOptions, StaticExportReport};

/// HTTP/UDP host used by asset-ui / sandbox embed and the standalone bin.
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub mod host;
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub use host::{
    AssetServer, BlobRefPolicy, DiscoveryConfig, LISTEN_FILE, ServerConfig,
    DEFAULT_DISCOVERY_PORT,
};
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub use host::discovery;
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub use host::json;
#[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
pub use host::util;
