//! Explicit admission budgets for the server core.
//!
//! Every limit the core enforces is a named number here, passed in at open
//! time — nothing is an implicit constant buried in a code path. Content
//! document budgets (file counts, dimensions…) live in the content contract;
//! these are the server-side operational budgets layered on top.

use crate::error::{ServerError, ServerResult};

#[derive(Clone, Copy, Debug)]
pub struct Budgets {
    /// Largest single blob the CAS accepts, enforced while streaming.
    pub max_blob_bytes: u64,
    /// Largest canonical manifest document accepted into the catalog.
    pub max_manifest_bytes: u64,
    /// Read/write streaming chunk size.
    pub io_chunk_bytes: usize,
    /// Largest opaque job payload.
    pub max_job_payload_bytes: u64,
    /// Most dependency edges one job may declare.
    pub max_job_deps: u32,
    /// Deepest parent chain a job may sit under (hierarchical cancellation
    /// walks this).
    pub max_job_depth: u32,
    /// Ceiling on any job's max_attempts.
    pub max_attempts: u32,
    /// Longest lease a worker may take or extend to, in milliseconds.
    pub max_lease_ms: u64,
    /// Longest retry backoff a failure report may request, in milliseconds.
    pub max_retry_delay_ms: u64,
    /// SQLite busy timeout, milliseconds.
    pub db_busy_timeout_ms: u32,
    /// Most assets one stock seed source may provide.
    pub max_seed_assets: u32,
    /// Most assets one external-pack import manifest may carry. The content
    /// contract's own `MAX_IMPORT_ASSETS` is the hard ceiling; this budget
    /// may only tighten it.
    pub max_import_assets: u32,
    /// Longest search query text, in bytes.
    pub max_search_query_bytes: u32,
    /// Most distinct lexical terms one search query may carry (each term is
    /// one bound SQL parameter in an IN list).
    pub max_search_query_terms: u32,
    /// Largest page size a search may request.
    pub max_search_results: u32,
    /// Most distinct posting terms one asset's annotation (and, separately,
    /// its alias set) may index.
    pub max_search_index_terms: u32,
    /// Snippet output bound, in bytes.
    pub max_search_snippet_bytes: u32,
    /// Most facet rows (label + count) one search may ask for. Facets are
    /// opt-in per query, so this bounds the extra aggregation, not the
    /// ordinary page.
    pub max_search_facets: u32,
    /// Most exact input bindings one typed operation may pin.
    pub max_operation_inputs: u32,
    /// Largest canonical operation spec document.
    pub max_operation_spec_bytes: u32,
    /// How many rounds of terminal failure one operation may retry through.
    pub max_operation_rounds: u32,
    /// Durable event rows one operation may accumulate (a hard backstop —
    /// lifecycle events are already bounded by the round budget).
    pub max_operation_events: u32,
    /// How recently a worker must have offered an operation's executor kind
    /// for the operation to count as available, in milliseconds.
    pub operation_worker_liveness_ms: u64,
}

/// Ceiling on `io_chunk_bytes`: the chunk is allocated whole, so an absurd
/// value must not become an allocation-of-attacker-chosen-size.
pub const MAX_IO_CHUNK_BYTES: usize = 64 * 1024 * 1024;

/// Ceilings on the search budgets. Each guards a real domain: query terms
/// become bound SQL parameters (SQLite's historical variable floor is 999, so
/// 256 terms plus every filter bind stays far under it), results bound the
/// page allocation, index terms bound per-asset posting rows, and snippet
/// bytes bound the output allocation.
pub const MAX_SEARCH_QUERY_TERMS: u32 = 256;
pub const MAX_SEARCH_RESULTS: u32 = 10_000;
pub const MAX_SEARCH_INDEX_TERMS: u32 = 65_536;
pub const MAX_SEARCH_SNIPPET_BYTES: u32 = 16 * 1024;
/// Hard ceiling on requested facet rows.
pub const MAX_SEARCH_FACETS: u32 = 1_000;

impl Budgets {
    /// The frozen v1 defaults. Tests may shrink individual fields to hit
    /// budget refusals cheaply.
    pub fn default_v1() -> Self {
        Self {
            max_blob_bytes: 256 * 1024 * 1024,
            max_manifest_bytes: 1024 * 1024,
            io_chunk_bytes: 64 * 1024,
            max_job_payload_bytes: 64 * 1024,
            max_job_deps: 64,
            max_job_depth: 8,
            max_attempts: 16,
            max_lease_ms: 10 * 60 * 1000,
            max_retry_delay_ms: 24 * 60 * 60 * 1000,
            db_busy_timeout_ms: 5000,
            max_seed_assets: 4096,
            max_import_assets: 1024,
            max_search_query_bytes: 1024,
            max_search_query_terms: 32,
            max_search_results: 100,
            max_search_index_terms: 16_384,
            max_search_snippet_bytes: 320,
            max_search_facets: 64,
            max_operation_inputs: 8,
            max_operation_spec_bytes: 16 * 1024,
            max_operation_rounds: 8,
            max_operation_events: 256,
            operation_worker_liveness_ms: 120_000,
        }
    }

    /// Refuse budget values that cannot survive the storage domains they gate.
    /// Sizes and timestamps live in SQLite INTEGER (i64) columns, single-bind
    /// byte lengths in `c_int`, and the busy timeout in `c_int`; a budget
    /// permitting values outside those domains would let admission pass bytes
    /// the layers below must then refuse (or worse, truncate). Checked once at
    /// open so every later cast is backed by an enforced invariant.
    pub fn validate(&self) -> ServerResult<()> {
        if self.io_chunk_bytes == 0 || self.io_chunk_bytes > MAX_IO_CHUNK_BYTES {
            return Err(ServerError::InvalidInput { what: "budget io_chunk_bytes" });
        }
        if self.max_blob_bytes == 0 || self.max_blob_bytes > i64::MAX as u64 {
            return Err(ServerError::InvalidInput { what: "budget max_blob_bytes" });
        }
        // Manifests and job payloads are bound whole as single blobs.
        if self.max_manifest_bytes == 0 || self.max_manifest_bytes > i32::MAX as u64 {
            return Err(ServerError::InvalidInput { what: "budget max_manifest_bytes" });
        }
        if self.max_job_payload_bytes > i32::MAX as u64 {
            return Err(ServerError::InvalidInput { what: "budget max_job_payload_bytes" });
        }
        if self.max_lease_ms == 0 || self.max_lease_ms > i64::MAX as u64 {
            return Err(ServerError::InvalidInput { what: "budget max_lease_ms" });
        }
        if self.max_retry_delay_ms > i64::MAX as u64 {
            return Err(ServerError::InvalidInput { what: "budget max_retry_delay_ms" });
        }
        if self.db_busy_timeout_ms > i32::MAX as u32 {
            return Err(ServerError::InvalidInput { what: "budget db_busy_timeout_ms" });
        }
        if self.max_attempts == 0 {
            return Err(ServerError::InvalidInput { what: "budget max_attempts" });
        }
        if self.max_import_assets == 0
            || self.max_import_assets
                > makepad_asset_data::limits::MAX_IMPORT_ASSETS as u32
        {
            return Err(ServerError::InvalidInput { what: "budget max_import_assets" });
        }
        // Search budgets: zero disables the feature it gates (fail closed at
        // open instead of at first use) and the caps above keep every value
        // inside the SQL bind / allocation domain it feeds.
        if self.max_search_query_bytes == 0 || self.max_search_query_bytes > i32::MAX as u32 {
            return Err(ServerError::InvalidInput { what: "budget max_search_query_bytes" });
        }
        if self.max_search_query_terms == 0 || self.max_search_query_terms > MAX_SEARCH_QUERY_TERMS
        {
            return Err(ServerError::InvalidInput { what: "budget max_search_query_terms" });
        }
        if self.max_search_results == 0 || self.max_search_results > MAX_SEARCH_RESULTS {
            return Err(ServerError::InvalidInput { what: "budget max_search_results" });
        }
        if self.max_search_index_terms == 0 || self.max_search_index_terms > MAX_SEARCH_INDEX_TERMS
        {
            return Err(ServerError::InvalidInput { what: "budget max_search_index_terms" });
        }
        if self.max_search_snippet_bytes == 0
            || self.max_search_snippet_bytes > MAX_SEARCH_SNIPPET_BYTES
        {
            return Err(ServerError::InvalidInput { what: "budget max_search_snippet_bytes" });
        }
        if self.max_search_facets == 0 || self.max_search_facets > MAX_SEARCH_FACETS {
            return Err(ServerError::InvalidInput { what: "budget max_search_facets" });
        }
        // Operation budgets: zero disables the feature it gates; spec bytes
        // are bound whole as a single blob (c_int domain); the liveness
        // window feeds i64 comparisons.
        if self.max_operation_inputs == 0 || self.max_operation_inputs > 64 {
            return Err(ServerError::InvalidInput { what: "budget max_operation_inputs" });
        }
        if self.max_operation_spec_bytes == 0 || self.max_operation_spec_bytes > i32::MAX as u32 {
            return Err(ServerError::InvalidInput { what: "budget max_operation_spec_bytes" });
        }
        if self.max_operation_rounds == 0 || self.max_operation_rounds > 64 {
            return Err(ServerError::InvalidInput { what: "budget max_operation_rounds" });
        }
        if self.max_operation_events == 0 || self.max_operation_events > 65_536 {
            return Err(ServerError::InvalidInput { what: "budget max_operation_events" });
        }
        if self.operation_worker_liveness_ms == 0
            || self.operation_worker_liveness_ms > i64::MAX as u64
        {
            return Err(ServerError::InvalidInput { what: "budget operation_worker_liveness_ms" });
        }
        Ok(())
    }
}
