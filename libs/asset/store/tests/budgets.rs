//! Budget validation: invalid budgets refuse `open` before anything touches
//! disk, so every later integer-domain cast (c_int binds, i64 columns) is
//! backed by an enforced invariant instead of a hope.

mod common;
use common::*;
use makepad_asset_store::{budget::MAX_IO_CHUNK_BYTES, AssetServerCore, Budgets, ServerError};

fn refuse(name: &str, budgets: Budgets, expect_what: &'static str) {
    let root = test_root(name);
    let err = match AssetServerCore::open(&root, budgets) {
        Ok(_) => panic!("{name}: invalid budget accepted"),
        Err(e) => e,
    };
    match err {
        ServerError::InvalidInput { what } => assert_eq!(what, expect_what, "{name}"),
        other => panic!("{name}: expected InvalidInput, got {other}"),
    }
    assert!(!root.exists(), "{name}: refused open must not create the root");
}

#[test]
fn invalid_budgets_refuse_open_before_touching_disk() {
    let d = Budgets::default_v1;
    refuse("chunk0", Budgets { io_chunk_bytes: 0, ..d() }, "budget io_chunk_bytes");
    refuse(
        "chunkbig",
        Budgets { io_chunk_bytes: MAX_IO_CHUNK_BYTES + 1, ..d() },
        "budget io_chunk_bytes",
    );
    refuse("blob0", Budgets { max_blob_bytes: 0, ..d() }, "budget max_blob_bytes");
    refuse(
        "blobbig",
        Budgets { max_blob_bytes: i64::MAX as u64 + 1, ..d() },
        "budget max_blob_bytes",
    );
    refuse("manifest0", Budgets { max_manifest_bytes: 0, ..d() }, "budget max_manifest_bytes");
    refuse(
        "manifestbig",
        Budgets { max_manifest_bytes: i32::MAX as u64 + 1, ..d() },
        "budget max_manifest_bytes",
    );
    refuse(
        "payloadbig",
        Budgets { max_job_payload_bytes: i32::MAX as u64 + 1, ..d() },
        "budget max_job_payload_bytes",
    );
    refuse("lease0", Budgets { max_lease_ms: 0, ..d() }, "budget max_lease_ms");
    refuse(
        "leasebig",
        Budgets { max_lease_ms: i64::MAX as u64 + 1, ..d() },
        "budget max_lease_ms",
    );
    refuse(
        "retrybig",
        Budgets { max_retry_delay_ms: i64::MAX as u64 + 1, ..d() },
        "budget max_retry_delay_ms",
    );
    refuse(
        "busybig",
        Budgets { db_busy_timeout_ms: i32::MAX as u32 + 1, ..d() },
        "budget db_busy_timeout_ms",
    );
    refuse("attempts0", Budgets { max_attempts: 0, ..d() }, "budget max_attempts");
    // Search budgets: zero and beyond-domain both refuse for all five.
    refuse(
        "search_qbytes0",
        Budgets { max_search_query_bytes: 0, ..d() },
        "budget max_search_query_bytes",
    );
    refuse(
        "search_qbytes_big",
        Budgets { max_search_query_bytes: i32::MAX as u32 + 1, ..d() },
        "budget max_search_query_bytes",
    );
    refuse(
        "search_terms0",
        Budgets { max_search_query_terms: 0, ..d() },
        "budget max_search_query_terms",
    );
    refuse(
        "search_terms_big",
        Budgets { max_search_query_terms: 257, ..d() },
        "budget max_search_query_terms",
    );
    refuse(
        "search_results0",
        Budgets { max_search_results: 0, ..d() },
        "budget max_search_results",
    );
    refuse(
        "search_results_big",
        Budgets { max_search_results: 10_001, ..d() },
        "budget max_search_results",
    );
    refuse(
        "search_index0",
        Budgets { max_search_index_terms: 0, ..d() },
        "budget max_search_index_terms",
    );
    refuse(
        "search_index_big",
        Budgets { max_search_index_terms: 65_537, ..d() },
        "budget max_search_index_terms",
    );
    refuse(
        "search_snippet0",
        Budgets { max_search_snippet_bytes: 0, ..d() },
        "budget max_search_snippet_bytes",
    );
    refuse(
        "search_snippet_big",
        Budgets { max_search_snippet_bytes: 16 * 1024 + 1, ..d() },
        "budget max_search_snippet_bytes",
    );
    // The frozen v1 defaults themselves are valid.
    let (_root, _core) = open_core("valid_defaults");
}
