//! The Asset Server wire contract as this client speaks it, in one place.
//!
//! Everything here mirrors the server transport (`libs/asset/store`): the
//! UDP discovery beacon layout, the bearer-token shape, the v1 route paths,
//! and the request-target charset the server accepts (no percent-encoding, no
//! dot segments, restricted bytes). Centralizing it means a server-side route
//! rename is a one-line change here, and the rest of the crate cannot drift.
//!
//! Two deliberate contract choices, coordinated with the server owner:
//! - IDs travel in their canonical display spellings (`ast_…`, `arev_…`,
//!   `grev_…`, `sha256:…`), which are exactly target-charset safe.
//! - Catalog search is a POST with a JSON body. The server transport refuses
//!   percent-encoding and restricts target bytes, so free search text cannot
//!   travel in a query string; JSON bodies are the one bounded channel that
//!   carries arbitrary text.

use makepad_asset_data::{
    AssetAlias, AssetId, AssetRevisionId, BlobId, DerivedVariantId, GameAlias, GameId,
    GameRevisionId, ImportRevisionId, VariantSetId,
};

// ---- discovery beacon ------------------------------------------------------

/// Catalog-event vocabulary this build speaks (see [`path_events`]). v1 is
/// the original kind set; v2 adds `asset_retired` / `revision_retired`.
pub const EVENT_VOCABULARY: u32 = 2;

pub const DISCOVERY_MAGIC: [u8; 8] = *b"MPASDIS1";
pub const BEACON_LEN: usize = 36;
pub const PROTOCOL_VERSION: u16 = 1;
/// Default UDP LAN discovery port (matches the server's default).
pub const DEFAULT_DISCOVERY_PORT: u16 = 41871;

pub const FLAG_AUTH_REQUIRED: u16 = 1 << 0;
pub const FLAG_TLS: u16 = 1 << 1;

/// Coarse service-class bits a beacon may advertise. A bitset, never a
/// string: capability text is unrepresentable on the wire.
pub mod caps {
    pub const CATALOG: u32 = 1 << 0;
    pub const BLOBS: u32 = 1 << 1;
    pub const JOBS: u32 = 1 << 2;
    pub const GAMES: u32 = 1 << 3;
    pub const ALL_V1: u32 = CATALOG | BLOBS | JOBS | GAMES;
}

// ---- bearer token ----------------------------------------------------------

/// Transport token prefix; the full shape is `mpat_<64 lowercase hex>`.
pub const TOKEN_PREFIX: &str = "mpat_";
pub const TOKEN_SECRET_HEX_LEN: usize = 64;

/// True when `token` is exactly the transport token shape. The client
/// validates before sending so a malformed credential fails locally and
/// loudly instead of as an opaque 401.
pub fn token_shape_ok(token: &str) -> bool {
    match token.strip_prefix(TOKEN_PREFIX) {
        Some(hex) => crate::util::from_hex_exact::<32>(hex).is_some(),
        None => false,
    }
}

// ---- request-target charset ------------------------------------------------

/// The exact byte set the server accepts in a request target. Anything the
/// client would send outside this set is refused locally at request build.
pub fn target_byte_ok(b: u8) -> bool {
    matches!(b,
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9'
        | b'-' | b'_' | b'.' | b'/' | b':' | b'?' | b'=' | b'&')
}

pub const MAX_TARGET_BYTES: usize = 2048;

// ---- v1 routes -------------------------------------------------------------

pub fn path_health() -> String {
    "/v1/health".to_string()
}

/// POST body route for text search over the catalog.
pub fn path_catalog_search() -> String {
    "/v1/catalog".to_string()
}

/// Keyset asset listing; `ns`, `cursor` and `limit` ride as query params.
pub fn path_assets(ns: Option<&str>, cursor: Option<&str>, limit: u64) -> String {
    let mut p = format!("/v1/assets?limit={limit}");
    if let Some(ns) = ns {
        p.push_str("&ns=");
        p.push_str(ns);
    }
    if let Some(c) = cursor {
        p.push_str("&cursor=");
        p.push_str(c);
    }
    p
}

pub fn path_asset(id: &AssetId) -> String {
    format!("/v1/assets/{id}")
}

pub fn path_alias(alias: &AssetAlias) -> String {
    format!("/v1/aliases/{alias}")
}

pub fn path_revision(rev: &AssetRevisionId) -> String {
    format!("/v1/revisions/{rev}")
}

pub fn path_game_revision(rev: &GameRevisionId) -> String {
    format!("/v1/game-revisions/{rev}")
}

pub fn path_game_register() -> String {
    "/v1/games".to_string()
}

pub fn path_game_stage(game: &GameId) -> String {
    format!("/v1/games/{game}/revisions")
}

pub fn path_game_publish(game: &GameId, rev: &GameRevisionId) -> String {
    format!("/v1/games/{game}/revisions/{rev}/publish")
}

pub fn path_game_alias(alias: &GameAlias) -> String {
    format!("/v1/game-aliases/{alias}")
}

pub fn path_blob(blob: &BlobId) -> String {
    format!("/v1/blobs/{blob}")
}

// ---- write routes (artifact publication) -----------------------------------

/// Content-addressed blob upload on the DATA plane; the namespace names the
/// authorization context, the body is the bytes.
/// Ordered batch pull of many blobs in ONE request (see the server's
/// `blob_batch`). Absent on older servers, which answer 404 — callers fall
/// back to single GETs.
pub fn path_blobs_batch() -> String {
    "/v1/blobs/fetch".to_string()
}

/// Media type of the framed batch body. A response that does not carry
/// EXACTLY this type is not parsed as frames.
pub const BLOB_BATCH_CONTENT_TYPE: &str = "application/vnd.makepad.blob-batch.v1";

/// Fixed frame header: status(1) + digest(32) + length(8, big-endian).
pub const BLOB_BATCH_FRAME_HEADER: usize = 41;

/// Ceilings this client accepts from a batch response, independent of what
/// the server claims: most frames it will parse, and the largest single item
/// it will buffer.
pub const MAX_BLOB_BATCH_ITEMS: usize = 64;
pub const MAX_BLOB_BATCH_ITEM_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_BLOB_BATCH_BYTES: u64 = 64 * 1024 * 1024;

pub fn path_blob_upload(ns: &str) -> String {
    format!("/v1/blobs?ns={ns}")
}

pub fn path_asset_register() -> String {
    "/v1/assets".to_string()
}

pub fn path_asset_stage(asset: &AssetId) -> String {
    format!("/v1/assets/{asset}/revisions")
}

pub fn path_asset_publish(asset: &AssetId, rev: &AssetRevisionId) -> String {
    format!("/v1/assets/{asset}/revisions/{rev}/publish")
}

pub fn path_annotation(asset: &AssetId) -> String {
    format!("/v1/assets/{asset}/annotation")
}

// ---- deletion --------------------------------------------------------------

/// Retire (delete) a whole asset: every revision leaves the catalog's
/// listings, aliases and search index, and its bytes become collectable.
pub fn path_asset_retire(asset: &AssetId) -> String {
    format!("/v1/assets/{asset}/retire")
}

/// Retire one revision (a superseded one, typically); the asset stays live.
pub fn path_revision_retire(asset: &AssetId, rev: &AssetRevisionId) -> String {
    format!("/v1/assets/{asset}/revisions/{rev}/retire")
}

/// Blob garbage collection: POST advances a run (starting one if needed),
/// GET reports the newest run, and the cancel path abandons an active one.
pub fn path_gc() -> String {
    "/v1/gc".to_string()
}

pub fn path_gc_cancel() -> String {
    "/v1/gc/cancel".to_string()
}

// ---- jobs (generation scheduling) ------------------------------------------

/// Advertised generation capabilities; `domain` filters (`video`, `audio`…).
pub fn path_job_profiles(domain: Option<&str>) -> String {
    match domain {
        Some(d) => format!("/v1/job-profiles?domain={d}"),
        None => "/v1/job-profiles".to_string(),
    }
}

pub fn path_jobs() -> String {
    "/v1/jobs".to_string()
}

/// `job` is the canonical `job_<32 lowercase hex>` display spelling.
pub fn path_job(job: &str) -> String {
    format!("/v1/jobs/{job}")
}

pub fn path_job_cancel(job: &str) -> String {
    format!("/v1/jobs/{job}/cancel")
}

/// Scoped job listing. With `ns` the server requires a job capability on
/// that namespace; without it the caller's own jobs are listed.
pub fn path_jobs_list(ns: Option<&str>, limit: u64) -> String {
    let mut p = format!("/v1/jobs?limit={limit}");
    if let Some(ns) = ns {
        p.push_str("&ns=");
        p.push_str(ns);
    }
    p
}

/// Resumable long-poll over the committed-catalog event journal. Every
/// parameter travels in the query string, so every value must already be
/// query-charset safe (cursors are validated on receipt, kind names are a
/// fixed lowercase vocabulary).
pub fn path_events(
    cursor: Option<&str>,
    wait_ms: u64,
    limit: u32,
    kind: Option<&str>,
) -> String {
    // `ev` is the event VOCABULARY this build understands. A server that
    // only speaks v1 ignores it; a newer server uses it to decide whether it
    // may send kinds this client knows (retirement), and downgrades them to
    // their v1 spelling for clients that do not ask.
    let mut p = format!("/v1/events?wait={wait_ms}&limit={limit}&ev={EVENT_VOCABULARY}");
    if let Some(k) = kind {
        p.push_str("&kind=");
        p.push_str(k);
    }
    if let Some(c) = cursor {
        p.push_str("&cursor=");
        p.push_str(c);
    }
    p
}

// ---- typed asset operations -------------------------------------------------

/// The versioned operation registry with truthful availability.
pub fn path_operation_types() -> String {
    "/v1/operation-types".to_string()
}

pub fn path_operations() -> String {
    "/v1/operations".to_string()
}

/// `op` is the canonical `op_<32 lowercase hex>` display spelling.
pub fn path_operation(op: &str) -> String {
    format!("/v1/operations/{op}")
}

/// Bounded long-poll over one operation's durable event log; `after` is the
/// last consumed sequence number (0 = from the start).
pub fn path_operation_events(op: &str, after: u64, wait_ms: u64, limit: u32) -> String {
    format!("/v1/operations/{op}/events?after={after}&wait={wait_ms}&limit={limit}")
}

pub fn path_operation_cancel(op: &str) -> String {
    format!("/v1/operations/{op}/cancel")
}

pub fn path_operation_retry(op: &str) -> String {
    format!("/v1/operations/{op}/retry")
}

pub fn path_operation_finalize(op: &str) -> String {
    format!("/v1/operations/{op}/finalize")
}

/// Longest operation event wait this client requests (mirrors the server's
/// event-wait ceiling).
pub const MAX_OPERATION_WAIT_MS: u64 = 30_000;

// ---- chat broker -----------------------------------------------------------

pub fn path_chat_providers() -> String {
    "/v1/chat/providers".to_string()
}

pub fn path_chat_sessions() -> String {
    "/v1/chat/sessions".to_string()
}

/// `id` is the canonical `chat_<16 lowercase hex>` spelling.
pub fn path_chat_session(id: &str) -> String {
    format!("/v1/chat/sessions/{id}")
}

pub fn path_chat_send(id: &str) -> String {
    format!("/v1/chat/sessions/{id}/send")
}

pub fn path_chat_events(id: &str, after: u64, wait_ms: u64, limit: u32) -> String {
    format!("/v1/chat/sessions/{id}/events?after={after}&wait={wait_ms}&limit={limit}")
}

pub fn path_chat_cancel(id: &str) -> String {
    format!("/v1/chat/sessions/{id}/cancel")
}

pub fn path_chat_tool_result(id: &str) -> String {
    format!("/v1/chat/sessions/{id}/tool-result")
}

pub const MAX_CHAT_WAIT_MS: u64 = 30_000;
pub const MAX_CHAT_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_CHAT_ATTACHMENTS: usize = 8;

// ---- import + immutable derived variants -----------------------------------

pub fn path_import_sources() -> String {
    "/v1/import-sources".to_string()
}

/// Explicit source-collection page. Always includes `limit` so the request
/// never falls into the legacy no-query listing (512-row / 413 contract).
pub fn path_import_sources_page(cursor: Option<&str>, limit: u64) -> String {
    let mut p = format!("/v1/import-sources?limit={limit}");
    if let Some(c) = cursor {
        p.push_str("&cursor=");
        p.push_str(c);
    }
    p
}

/// Default page size for an explicit `GET /v1/import-sources` (server).
pub const DEFAULT_SOURCE_PAGE_LIMIT: u64 = 100;
/// Largest explicit source-collection page (server clamps here; this client
/// refuses a larger request locally).
pub const MAX_SOURCE_PAGE_LIMIT: u64 = 500;

/// Closed JSON-response budget for one explicit `GET /v1/import-sources`
/// page. Derived from the content-contract field maxima, the 500-row page
/// cap, JSON framing, and a 6× `\u00XX` expansion on `credits`. Saturating,
/// not unbounded. Ordinary JSON routes stay at [`MAX_JSON_RESPONSE_BYTES`].
pub const MAX_SOURCE_PAGE_JSON_RESPONSE_BYTES: u64 = source_page_json_response_ceiling();

const fn source_page_json_response_ceiling() -> u64 {
    use makepad_asset_data::limits as L;
    const SOURCE_ID: u64 = L::MAX_ALIAS_SEGMENT_BYTES as u64;
    const TITLE: u64 = (L::MAX_NAME_BYTES * 2) as u64;
    const LICENSE: u64 = L::MAX_LICENSE_BYTES as u64;
    const CREDITS_JSON: u64 = (L::MAX_STRING_BYTES as u64).saturating_mul(6);
    const DIGEST: u64 = 5 + 64; // `scol_` + hex
    const ROW_FRAMING: u64 = 96;
    const ROW: u64 = SOURCE_ID + TITLE + LICENSE + CREDITS_JSON + DIGEST + ROW_FRAMING;
    const N: u64 = MAX_SOURCE_PAGE_LIMIT;
    const ROWS: u64 = ROW.saturating_mul(N).saturating_add(N);
    const ENVELOPE: u64 = 64 + SOURCE_ID;
    ROWS.saturating_add(ENVELOPE)
}

pub fn path_imports() -> String {
    "/v1/imports".to_string()
}

pub fn path_import(rev: &ImportRevisionId) -> String {
    format!("/v1/imports/{rev}")
}

pub fn path_derived_variant(id: &DerivedVariantId) -> String {
    format!("/v1/derived-variants/{id}")
}

pub fn path_variant_sets() -> String {
    "/v1/variant-sets".to_string()
}

pub fn path_variant_set(id: &VariantSetId) -> String {
    format!("/v1/variant-sets/{id}")
}

pub fn path_variant_resolutions() -> String {
    "/v1/variant-resolutions".to_string()
}

// ---- client-side response budgets ------------------------------------------

/// Largest JSON response body this client will buffer (mirrors the server's
/// JSON body cap). Ordinary routes stay here; import report/status use
/// [`MAX_IMPORT_JSON_RESPONSE_BYTES`].
pub const MAX_JSON_RESPONSE_BYTES: u64 = 256 * 1024;

/// Closed JSON-response budget for `POST /v1/imports` and
/// `GET /v1/imports/{irev}`. Derived from the content-contract maxima
/// (1024 entries, key/alias/license/credits bounds) plus JSON framing
/// and a 6× `\u00XX` expansion on the one free-text field (`credits`).
/// Not unbounded: saturating arithmetic over named field ceilings.
pub const MAX_IMPORT_JSON_RESPONSE_BYTES: u64 = import_json_response_ceiling();

const fn import_json_response_ceiling() -> u64 {
    use makepad_asset_data::limits as L;
    const KEY: u64 = L::MAX_KEY_BYTES as u64;
    const ASSET_ID: u64 = 4 + 26; // `ast_` + base32
    const REV: u64 = 5 + 64; // `arev_` / `irev_` + hex
    const ALIAS: u64 = L::MAX_ALIAS_BYTES as u64;
    const ENTRY_FRAMING: u64 = 64;
    const ENTRY: u64 = KEY + ASSET_ID + REV + ALIAS + ENTRY_FRAMING;
    const N: u64 = L::MAX_IMPORT_ASSETS as u64;
    const ENTRIES: u64 = ENTRY.saturating_mul(N).saturating_add(N);
    const SOURCE: u64 = L::MAX_NAME_BYTES as u64;
    const PACK: u64 = L::MAX_NAME_BYTES as u64;
    const VER: u64 = L::MAX_PACK_VERSION_BYTES as u64;
    const LICENSE: u64 = L::MAX_LICENSE_BYTES as u64;
    const CREDITS_JSON: u64 = (L::MAX_STRING_BYTES as u64).saturating_mul(6);
    const ENVELOPE: u64 = 256 + REV + SOURCE + PACK + VER + LICENSE + CREDITS_JSON;
    ENTRIES.saturating_add(ENVELOPE)
}
/// Largest canonical manifest response (content contract document budget).
pub const MAX_MANIFEST_RESPONSE_BYTES: u64 =
    makepad_asset_data::limits::MAX_DOCUMENT_BYTES as u64;
/// Bounded server `error` detail retained in [`crate::ClientError::Server`].
pub const MAX_ERROR_DETAIL_BYTES: usize = 128;
/// Most hits/rows one page may carry; the server pages well under this.
pub const MAX_PAGE_ENTRIES: usize = 512;
/// Longest opaque pagination cursor accepted from a response.
pub const MAX_CURSOR_BYTES: usize = 1024;
/// Longest namespace string accepted in a DTO.
pub const MAX_NAMESPACE_BYTES: usize = 64;
/// Longest title accepted in a DTO.
pub const MAX_TITLE_BYTES: usize = 512;
/// Longest snippet accepted in a DTO (server budget is 320).
pub const MAX_SNIPPET_BYTES: usize = 1024;
/// Longest search text the client will send (server budget is 1024).
pub const MAX_QUERY_TEXT_BYTES: usize = 1024;
/// Longest filter value (namespace/category/tag/creator…) the client sends.
pub const MAX_FILTER_VALUE_BYTES: usize = 128;
/// Longest event-feed resume cursor (`<16 hex epoch>-<decimal seq>`).
pub const MAX_EVENT_CURSOR_BYTES: usize = 64;
/// Longest server-side long-poll wait this client will request.
pub const MAX_EVENT_WAIT_MS: u64 = 30_000;
/// Most events one `/v1/events` response may carry (mirrors the server's
/// batch cap; well under [`MAX_PAGE_ENTRIES`]).
pub const MAX_EVENT_BATCH: u32 = 256;
/// Most advertised job profiles accepted from one response (server config
/// caps at 64).
pub const MAX_JOB_PROFILES: usize = 64;
/// Longest progress note retained from a job status (server cap is 200).
pub const MAX_PROGRESS_NOTE_BYTES: usize = 256;

/// Source-collection id / browse cursor: the collection slug grammar
/// (`MAX_ALIAS_SEGMENT_BYTES`, `[a-z0-9_-]`, starting alphanumeric) and the
/// query-value charset. Matches the server's `source_cursor` check.
pub fn source_cursor_ok(s: &str) -> bool {
    if s.is_empty() || s.len() > makepad_asset_data::limits::MAX_ALIAS_SEGMENT_BYTES {
        return false;
    }
    let bytes = s.as_bytes();
    (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && s.bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
        && query_value_ok(s)
}

/// True when every byte is legal inside a query parameter VALUE the server
/// will parse back out: the target charset minus the structural bytes
/// (`?`, `=`, `&`, `/`).
pub fn query_value_ok(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| target_byte_ok(b) && !matches!(b, b'?' | b'=' | b'&' | b'/'))
}

/// Strictly validate an event-feed cursor and return its sequence. Event
/// cursors are not generic search cursors: accepting any target-safe string
/// here would turn a corrupt resume position into an avoidable server round
/// trip and would prevent DTOs from checking that a response cursor covers
/// every returned event.
pub fn event_cursor_seq(s: &str) -> Option<u64> {
    let (epoch, seq) = s.split_once('-')?;
    if epoch.len() != 16
        || !epoch.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        || seq.is_empty()
        || seq.len() > 20
        || !seq.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    seq.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn token_shape() {
        let good = format!("mpat_{}", "a1".repeat(32));
        assert!(token_shape_ok(&good));
        assert!(!token_shape_ok("mpat_short"));
        assert!(!token_shape_ok(&format!("mpat_{}", "A1".repeat(32)))); // uppercase
        assert!(!token_shape_ok(&format!("mpxt_{}", "a1".repeat(32)))); // prefix
        assert!(!token_shape_ok(""));
    }

    #[test]
    fn paths_are_target_charset_safe() {
        let asset = AssetId::from_bytes([7u8; 16]);
        let rev = AssetRevisionId::from_bytes([9u8; 32]);
        let game_revision = GameRevisionId::from_bytes([1u8; 32]);
        let game = GameId::from_bytes([3u8; 16]);
        let blob = BlobId::from_bytes([2u8; 32]);
        let alias = AssetAlias::from_str("stock/rocket-launcher").unwrap();
        let game_alias = GameAlias::from_str("stock/games/arena").unwrap();
        let irev = ImportRevisionId::from_bytes([4u8; 32]);
        let dvar = DerivedVariantId::from_bytes([5u8; 32]);
        let vset = VariantSetId::from_bytes([6u8; 32]);
        for p in [
            path_health(),
            path_catalog_search(),
            path_assets(Some("stock"), Some("abcd"), 50),
            path_asset(&asset),
            path_alias(&alias),
            path_revision(&rev),
            path_game_revision(&game_revision),
            path_game_register(),
            path_game_stage(&game),
            path_game_publish(&game, &game_revision),
            path_game_alias(&game_alias),
            path_blob(&blob),
            path_import_sources(),
            path_import_sources_page(None, 100),
            path_import_sources_page(Some("kenney"), 500),
            path_imports(),
            path_import(&irev),
            path_derived_variant(&dvar),
            path_variant_sets(),
            path_variant_set(&vset),
            path_variant_resolutions(),
        ] {
            assert!(p.len() <= MAX_TARGET_BYTES, "{p}");
            assert!(p.bytes().all(target_byte_ok), "{p}");
            assert!(p.starts_with('/'), "{p}");
        }
    }

    #[test]
    fn query_value_charset() {
        assert!(query_value_ok("stock"));
        assert!(query_value_ok("abc-def_2.x:z"));
        assert!(!query_value_ok(""));
        assert!(!query_value_ok("a b"));
        assert!(!query_value_ok("a&b"));
        assert!(!query_value_ok("a/b"));
        assert!(!query_value_ok("a%20b"));
    }

    #[test]
    fn event_cursor_shape_is_strict() {
        assert_eq!(event_cursor_seq("0123456789abcdef-42"), Some(42));
        for bad in [
            "",
            "0123-1",
            "0123456789ABCDEF-1",
            "0123456789abcdef-",
            "0123456789abcdef-x",
            "0123456789abcdef-18446744073709551616",
            "0123456789abcdef-1-more",
        ] {
            assert_eq!(event_cursor_seq(bad), None, "{bad}");
        }
    }

    #[test]
    fn import_json_ceiling_is_derived_and_strictly_bounded() {
        assert!(MAX_IMPORT_JSON_RESPONSE_BYTES > MAX_JSON_RESPONSE_BYTES);
        assert!(MAX_IMPORT_JSON_RESPONSE_BYTES < 2 * 1024 * 1024);
        assert_eq!(MAX_IMPORT_JSON_RESPONSE_BYTES, import_json_response_ceiling());
        let n = makepad_asset_data::limits::MAX_IMPORT_ASSETS as u64;
        // Key + asset id + revision + alias at contract maxima already
        // overflow the ordinary 256 KiB cap at 1024 entries.
        let min_maxed_entry = makepad_asset_data::limits::MAX_KEY_BYTES as u64
            + 30
            + 69
            + makepad_asset_data::limits::MAX_ALIAS_BYTES as u64;
        assert!(min_maxed_entry.saturating_mul(n) > MAX_JSON_RESPONSE_BYTES);
        assert!(MAX_IMPORT_JSON_RESPONSE_BYTES >= min_maxed_entry.saturating_mul(n));
    }

    #[test]
    fn source_page_json_ceiling_is_derived_and_strictly_bounded() {
        assert!(MAX_SOURCE_PAGE_JSON_RESPONSE_BYTES > MAX_JSON_RESPONSE_BYTES);
        assert_eq!(
            MAX_SOURCE_PAGE_JSON_RESPONSE_BYTES,
            source_page_json_response_ceiling()
        );
        let n = MAX_SOURCE_PAGE_LIMIT;
        let min_maxed_row = makepad_asset_data::limits::MAX_STRING_BYTES as u64;
        assert!(min_maxed_row.saturating_mul(n) > MAX_JSON_RESPONSE_BYTES);
        assert!(MAX_SOURCE_PAGE_JSON_RESPONSE_BYTES >= min_maxed_row.saturating_mul(n));
    }

    #[test]
    fn source_cursor_shape() {
        assert!(source_cursor_ok("kenney"));
        assert!(source_cursor_ok("s000"));
        assert!(source_cursor_ok("a-b_1"));
        assert!(!source_cursor_ok(""));
        assert!(!source_cursor_ok("Kenney"));
        assert!(!source_cursor_ok("a/b"));
        assert!(!source_cursor_ok("a b"));
        let long = "a".repeat(makepad_asset_data::limits::MAX_ALIAS_SEGMENT_BYTES + 1);
        assert!(!source_cursor_ok(&long));
    }
}
