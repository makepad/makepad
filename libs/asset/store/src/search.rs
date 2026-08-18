//! Persistent catalog search: mutable annotations + a lexical posting index,
//! fully separate from the immutable manifest store.
//!
//! Design laws, all fail-closed and deterministic:
//! - Annotations are the ONLY searchable text. Manifests stay immutable and
//!   are never scanned; there is no fallback scan path anywhere — every match
//!   comes from the indexed `search_postings` table via SQL. The declared
//!   asset kind is typed (`AssetKind`), stored as its canonical lowercase
//!   name, exact-filterable, and indexed as a public term.
//! - Postings carry two weights per (term, asset): `weight_public` counts
//!   contributions from public fields only, `weight_owner` from all fields.
//!   Non-owners match and rank exclusively on `weight_public`, so private
//!   fields (generation prompt, provenance) contribute exactly zero to their
//!   hits, ranking, snippets AND counts — absence of signal, not filtering
//!   after the fact.
//! - Alias heads are indexed too: every alias pointing at an annotated asset
//!   contributes its tokenized segments as public terms (`search_alias_postings`,
//!   maintained in the alias transaction), and the asset's canonical alias —
//!   the lexicographically smallest head, `''` when none — is stored on the
//!   annotation row for ordering and returned on every hit.
//! - Ranking is integer arithmetic: score = Σ field_weight × min(tf, 15),
//!   ordered `score DESC, canon_alias ASC, asset_id ASC` (browse mode is the
//!   same total order with every score 0). No floats, no clock, no randomness.
//! - Pagination is keyset-based over that total order. A cursor is opaque and
//!   versioned; it embeds the index generation, a fingerprint of the full
//!   query shape (terms, filters, viewer, page size), the keyset position and
//!   an integrity check. Replaying it under another query shape or after ANY
//!   index mutation refuses as stale; any bit flip refuses as tampered.
//! - Reindexing is transactional: annotation writes rebuild the asset's
//!   postings in the same transaction, alias-head mutations rebuild the alias
//!   postings, canonical alias and `live` flag in the alias transaction (see
//!   `Catalog::set_asset_alias`), and both bump the index generation.
//! - Snippets are built from title/description only — never prompt or
//!   provenance, not even for the owner — normalized (control bytes stripped,
//!   whitespace collapsed) and byte-bounded at char boundaries.

use crate::auth::PrincipalId;
use crate::budget::Budgets;
use crate::catalog::{fixed16, validate_namespace};
use crate::error::{ServerError, ServerResult};
use crate::sqlite::{Db, Stmt};
use makepad_asset_data::limits::MAX_ALIAS_BYTES;
use makepad_asset_data::{sha256, AssetId, AssetKind};
use std::collections::BTreeMap;

/// The `kind` column's constraint. Must stay identical in the CREATE below
/// and in `SEARCH_KIND_MIGRATION`, which retrofits the column onto tables
/// created before schema v2 (tests/migration.rs proves the parity).
const KIND_DDL: &str = "kind TEXT CHECK(kind IS NULL OR kind IN \
    ('mesh','character','weapon','vehicle','prop','texture','material',\
'audio','video','skybox','world','prefab','billboard'))";

/// The canonical-alias column's definition. Must stay identical in the CREATE
/// below and in `canon_alias_migration_sql()`, which retrofits the column onto
/// tables created before schema v3 (tests prove the parity). `''` means "no
/// alias head points at this asset" and sorts before every real alias, so the
/// order law needs no NULL handling.
const CANON_ALIAS_DDL: &str = "canon_alias TEXT NOT NULL DEFAULT ''";

pub const SEARCH_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS search_annotations(
    asset_id BLOB PRIMARY KEY,
    namespace TEXT NOT NULL,
    kind TEXT CHECK(kind IS NULL OR kind IN \
    ('mesh','character','weapon','vehicle','prop','texture','material',\
'audio','video','skybox','world','prefab','billboard')),
    visibility TEXT NOT NULL CHECK(visibility IN ('public','private')),
    owner BLOB,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    creator TEXT NOT NULL,
    generator TEXT NOT NULL,
    backend TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    provenance TEXT NOT NULL,
    live INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL,
    canon_alias TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS search_annotations_by_ns ON search_annotations(namespace);
CREATE INDEX IF NOT EXISTS search_annotations_by_kind ON search_annotations(kind);
CREATE TABLE IF NOT EXISTS search_labels(
    asset_id BLOB NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('category','tag')),
    label TEXT NOT NULL,
    PRIMARY KEY(asset_id, kind, label)
);
CREATE INDEX IF NOT EXISTS search_labels_by_label ON search_labels(kind, label);
CREATE TABLE IF NOT EXISTS search_postings(
    term TEXT NOT NULL,
    asset_id BLOB NOT NULL,
    weight_public INTEGER NOT NULL,
    weight_owner INTEGER NOT NULL,
    PRIMARY KEY(term, asset_id)
);
CREATE INDEX IF NOT EXISTS search_postings_by_asset ON search_postings(asset_id);
CREATE TABLE IF NOT EXISTS search_alias_postings(
    term TEXT NOT NULL,
    asset_id BLOB NOT NULL,
    weight INTEGER NOT NULL,
    PRIMARY KEY(term, asset_id)
);
CREATE INDEX IF NOT EXISTS search_alias_postings_by_asset ON search_alias_postings(asset_id);
CREATE TABLE IF NOT EXISTS search_state(
    id INTEGER PRIMARY KEY CHECK(id = 1),
    generation INTEGER NOT NULL
);
INSERT OR IGNORE INTO search_state(id, generation) VALUES(1, 1);
";

// ---- annotation bounds (contract-like, not operational tuning) -------------

pub const MAX_TITLE_BYTES: usize = 200;
pub const MAX_DESCRIPTION_BYTES: usize = 4096;
pub const MAX_ANNOTATION_NAME_BYTES: usize = 128;
pub const MAX_PROMPT_BYTES: usize = 8192;
pub const MAX_PROVENANCE_BYTES: usize = 4096;
pub const MAX_LABELS: usize = 24;
pub const MAX_LABEL_BYTES: usize = 48;
pub const MAX_VIEWER_NAMESPACES: usize = 32;
/// A lexical term is the first 32 bytes of an ASCII-alphanumeric run,
/// lowercased; the remainder of an overlong run is ignored (identically at
/// index and query time, so overlong words still match themselves).
pub const MAX_TERM_BYTES: usize = 32;
/// Per-field term frequency cap folded into the weight.
const TF_CAP: u64 = 15;

const W_TITLE: u64 = 100;
/// Alias segments are curated public names — stronger than labels, weaker
/// than the title itself.
const W_ALIAS: u64 = 80;
const W_LABEL: u64 = 60;
const W_CREATOR: u64 = 40;
const W_GEN: u64 = 30;
const W_DESCRIPTION: u64 = 20;
const W_PROMPT: u64 = 10;
const W_PROVENANCE: u64 = 5;

// ---- types -----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

impl Visibility {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "public" => Some(Self::Public),
            "private" => Some(Self::Private),
            _ => None,
        }
    }
}

/// Canonical lowercase name for an asset kind, used as the stored column
/// value, the exact-filter value, and a public posting term. Exhaustive on
/// purpose: a new content-contract kind fails compilation here instead of
/// silently becoming unsearchable.
pub fn kind_name(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Mesh => "mesh",
        AssetKind::Character => "character",
        AssetKind::Weapon => "weapon",
        AssetKind::Vehicle => "vehicle",
        AssetKind::Prop => "prop",
        AssetKind::Texture => "texture",
        AssetKind::Material => "material",
        AssetKind::Audio => "audio",
        AssetKind::Video => "video",
        AssetKind::Skybox => "skybox",
        AssetKind::World => "world",
        AssetKind::Prefab => "prefab",
        AssetKind::Billboard => "billboard",
    }
}

pub fn kind_parse(s: &str) -> Option<AssetKind> {
    Some(match s {
        "mesh" => AssetKind::Mesh,
        "character" => AssetKind::Character,
        "weapon" => AssetKind::Weapon,
        "vehicle" => AssetKind::Vehicle,
        "prop" => AssetKind::Prop,
        "texture" => AssetKind::Texture,
        "material" => AssetKind::Material,
        "audio" => AssetKind::Audio,
        "video" => AssetKind::Video,
        "skybox" => AssetKind::Skybox,
        "world" => AssetKind::World,
        "prefab" => AssetKind::Prefab,
        "billboard" => AssetKind::Billboard,
        _ => return None,
    })
}

/// The v1 -> v2 retrofit for `search_annotations` created before the kind
/// column existed. Built from the same `KIND_DDL` the CREATE embeds.
pub(crate) fn kind_migration_sql() -> String {
    format!("ALTER TABLE search_annotations ADD COLUMN {KIND_DDL}")
}

/// The v2 -> v3 retrofit for `search_annotations` created before the
/// canonical-alias column existed. Built from the same `CANON_ALIAS_DDL` the
/// CREATE embeds.
pub(crate) fn canon_alias_migration_sql() -> String {
    format!("ALTER TABLE search_annotations ADD COLUMN {CANON_ALIAS_DDL}")
}

/// v5 -> v6: rebuild `search_annotations` so the kind CHECK accepts
/// `billboard`. SQLite cannot ALTER a CHECK; copy + rename is the retrofit.
pub(crate) const KIND_CHECK_REBUILD_SQL: &str = "
CREATE TABLE search_annotations_v6(
    asset_id BLOB PRIMARY KEY,
    namespace TEXT NOT NULL,
    kind TEXT CHECK(kind IS NULL OR kind IN \
    ('mesh','character','weapon','vehicle','prop','texture','material',\
'audio','video','skybox','world','prefab','billboard')),
    visibility TEXT NOT NULL CHECK(visibility IN ('public','private')),
    owner BLOB,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    creator TEXT NOT NULL,
    generator TEXT NOT NULL,
    backend TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    provenance TEXT NOT NULL,
    live INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL,
    canon_alias TEXT NOT NULL DEFAULT ''
);
INSERT INTO search_annotations_v6 SELECT * FROM search_annotations;
DROP TABLE search_annotations;
ALTER TABLE search_annotations_v6 RENAME TO search_annotations;
CREATE INDEX IF NOT EXISTS search_annotations_by_ns ON search_annotations(namespace);
CREATE INDEX IF NOT EXISTS search_annotations_by_kind ON search_annotations(kind);
";

/// Mutable, searchable metadata for one asset. Entirely separate from the
/// immutable manifest; the asset's namespace is taken from the catalog record,
/// never from here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetAnnotation {
    pub title: String,
    pub description: String,
    /// Declared asset kind, mirroring the manifest's `AssetKind` for search.
    /// `None` on annotations written before the kind column existed (and by
    /// annotators that do not know it); such rows match no kind filter.
    pub kind: Option<AssetKind>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub creator: String,
    /// Owning principal. Required when visibility is Private; grants the
    /// owner private-field search and private-annotation visibility.
    pub owner: Option<PrincipalId>,
    pub generator: String,
    pub backend: String,
    pub model: String,
    /// Generation prompt: indexed owner-only, never snippeted.
    pub prompt: String,
    /// Free-form provenance notes: indexed owner-only, never snippeted.
    pub provenance: String,
    pub visibility: Visibility,
}

/// Exact structured filters; every field is an equality constraint.
#[derive(Clone, Copy, Debug, Default)]
pub struct SearchFilters<'a> {
    pub namespace: Option<&'a str>,
    pub kind: Option<AssetKind>,
    pub category: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub creator: Option<&'a str>,
    pub generator: Option<&'a str>,
    pub backend: Option<&'a str>,
    pub model: Option<&'a str>,
    pub owner: Option<PrincipalId>,
    /// Only assets currently referenced by at least one alias head.
    pub live_only: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct SearchQuery<'a> {
    /// Lexical query text. Empty text is browse mode: filters only, ordered
    /// by asset_id. Non-empty text must yield at least one term.
    pub text: &'a str,
    pub filters: SearchFilters<'a>,
    pub page_size: u32,
}

/// What the caller may see. The transport layer resolves credentials into
/// this; the core enforces it on every hit, snippet and count.
#[derive(Clone, Copy, Debug)]
pub enum ViewerScope<'a> {
    All,
    Namespaces(&'a [&'a str]),
}

#[derive(Clone, Copy, Debug)]
pub struct SearchViewer<'a> {
    pub principal: Option<PrincipalId>,
    pub scope: ViewerScope<'a>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub asset_id: AssetId,
    pub namespace: String,
    pub kind: Option<AssetKind>,
    pub title: String,
    pub snippet: String,
    pub score: u64,
    pub live: bool,
    /// Canonical alias: the lexicographically smallest alias head currently
    /// pointing at this asset; `None` when no alias does. Second key of the
    /// result order, after score and before asset id.
    pub alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// Total matches under the SAME viewer constraint as the hits.
    pub total: u64,
    /// Present iff more results exist; feed back verbatim to continue.
    pub cursor: Option<Vec<u8>>,
}

// ---- validation helpers ----------------------------------------------------

fn check_text(
    s: &str,
    max: usize,
    multiline: bool,
    what_len: &'static str,
    what_ch: &'static str,
) -> ServerResult<()> {
    if s.len() > max {
        return Err(ServerError::OverBudget {
            what: what_len,
            limit: max as u64,
            found: s.len() as u64,
        });
    }
    for ch in s.chars() {
        let ok_ws = multiline && (ch == '\n' || ch == '\r' || ch == '\t');
        if ch.is_control() && !ok_ws {
            return Err(ServerError::InvalidInput { what: what_ch });
        }
    }
    Ok(())
}

/// Labels share the namespace charset so exact filters are byte-deterministic.
fn check_label(label: &str) -> ServerResult<()> {
    if label.is_empty() || label.len() > MAX_LABEL_BYTES {
        return Err(ServerError::InvalidInput { what: "annotation label length" });
    }
    let b = label.as_bytes();
    if !b[0].is_ascii_lowercase() && !b[0].is_ascii_digit() {
        return Err(ServerError::InvalidInput { what: "annotation label charset" });
    }
    if !b.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_') {
        return Err(ServerError::InvalidInput { what: "annotation label charset" });
    }
    Ok(())
}

/// Read an optional stored kind name, refusing unknown values as corruption.
fn read_kind_column(s: &Stmt<'_>, i: i32) -> ServerResult<Option<AssetKind>> {
    if s.column_is_null(i) {
        return Ok(None);
    }
    kind_parse(&s.column_text(i))
        .map(Some)
        .ok_or(ServerError::InvalidState { what: "annotation row", state: "unknown kind" })
}

fn check_labels(labels: &[String], what: &'static str) -> ServerResult<Vec<String>> {
    if labels.len() > MAX_LABELS {
        return Err(ServerError::OverBudget {
            what,
            limit: MAX_LABELS as u64,
            found: labels.len() as u64,
        });
    }
    for l in labels {
        check_label(l)?;
    }
    let mut sorted: Vec<String> = labels.to_vec();
    sorted.sort();
    sorted.dedup();
    Ok(sorted)
}

// ---- tokenizer -------------------------------------------------------------

/// Fold `text` into `tf`: lowercase ASCII-alphanumeric runs, each capped at
/// MAX_TERM_BYTES (remainder of an overlong run ignored). Identical for
/// indexing and querying — matching is by construction, not normalization
/// tables that could drift.
fn tokenize_into(text: &str, tf: &mut BTreeMap<String, u64>) {
    let mut cur = String::new();
    let mut skipping = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            if skipping {
                continue;
            }
            cur.push(ch.to_ascii_lowercase());
            if cur.len() == MAX_TERM_BYTES {
                *tf.entry(std::mem::take(&mut cur)).or_insert(0) += 1;
                skipping = true;
            }
        } else {
            if !cur.is_empty() {
                *tf.entry(std::mem::take(&mut cur)).or_insert(0) += 1;
            }
            skipping = false;
        }
    }
    if !cur.is_empty() {
        *tf.entry(cur).or_insert(0) += 1;
    }
}

// ---- snippets --------------------------------------------------------------

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Strip every control char (escape sequences included) to a space and
/// collapse whitespace runs: the snippet can never smuggle terminal or markup
/// control bytes.
fn normalize_snippet_source(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(1024));
    let mut pending_space = false;
    for ch in s.chars() {
        if ch.is_control() || ch.is_whitespace() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(ch);
        }
    }
    out
}

/// Snippet: a bounded window of the normalized description around the first
/// matched term (or its head if nothing matches), falling back to the title.
/// Prompt and provenance are never inputs here, for anyone.
fn build_snippet(title: &str, description: &str, terms: &[String], max_bytes: usize) -> String {
    let source = if description.is_empty() { title } else { description };
    let norm = normalize_snippet_source(source);
    if norm.is_empty() {
        return String::new();
    }
    // ASCII-lowercased copy: byte offsets and char boundaries stay identical
    // to `norm`, and terms are ASCII by construction.
    let lower: String = norm.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut at = None;
    for t in terms {
        if let Some(p) = lower.find(t.as_str()) {
            at = Some(at.map_or(p, |a: usize| a.min(p)));
        }
    }
    let start = floor_char(&norm, at.map_or(0, |p| p.saturating_sub(max_bytes / 4)));
    let end = floor_char(&norm, (start + max_bytes).min(norm.len()));
    norm[start..end].to_string()
}

// ---- cursors ---------------------------------------------------------------
//
// Opaque versioned keyset cursor, v2 layout (all integers big-endian):
//   [0]         version
//   [1..9]      index generation the page was cut from
//   [9..41]     query-shape fingerprint (sha256)
//   [41..49]    last hit's score
//   [49..51]    canonical-alias length n (u16, <= MAX_ALIAS_BYTES)
//   [51..51+n]  last hit's canonical alias bytes ('' when unaliased)
//   [..+16]     last hit's asset id
//   [..+8]      integrity check: first 8 bytes of sha256 over everything above
//
// Fail-closed decode order: structure -> integrity -> fingerprint. The
// generation is compared inside the search read snapshot, so a cursor from
// before ANY index mutation refuses as stale rather than skipping or
// duplicating rows under a changed total order.

const CURSOR_VERSION: u8 = 2;
/// Everything except the variable alias bytes.
const CURSOR_FIXED_LEN: usize = 1 + 8 + 32 + 8 + 2 + 16 + 8;
const CURSOR_CHECK_LEN: usize = 8;

/// The keyset a cursor carries: resume strictly after this position in the
/// total order `score DESC, canon_alias ASC, asset_id ASC`.
struct Keyset {
    generation: u64,
    score: u64,
    alias: String,
    asset: [u8; 16],
}

fn cursor_check(body: &[u8]) -> [u8; CURSOR_CHECK_LEN] {
    let mut out = [0u8; CURSOR_CHECK_LEN];
    out.copy_from_slice(&sha256(body)[..CURSOR_CHECK_LEN]);
    out
}

fn encode_cursor(
    generation: u64,
    fp: &[u8; 32],
    score: u64,
    alias: &str,
    asset: &AssetId,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(CURSOR_FIXED_LEN + alias.len());
    out.push(CURSOR_VERSION);
    out.extend_from_slice(&generation.to_be_bytes());
    out.extend_from_slice(fp);
    out.extend_from_slice(&score.to_be_bytes());
    // Alias length always fits u16: the catalog admits at most
    // MAX_ALIAS_BYTES (128) bytes per alias.
    out.extend_from_slice(&(alias.len() as u16).to_be_bytes());
    out.extend_from_slice(alias.as_bytes());
    out.extend_from_slice(asset.as_bytes());
    let check = cursor_check(&out);
    out.extend_from_slice(&check);
    out
}

fn decode_cursor(bytes: &[u8], expect_fp: &[u8; 32]) -> ServerResult<Keyset> {
    let malformed = ServerError::InvalidInput { what: "search cursor malformed" };
    if bytes.len() < CURSOR_FIXED_LEN
        || bytes.len() > CURSOR_FIXED_LEN + MAX_ALIAS_BYTES
        || bytes[0] != CURSOR_VERSION
    {
        return Err(malformed);
    }
    let mut n = [0u8; 2];
    n.copy_from_slice(&bytes[49..51]);
    let n = u16::from_be_bytes(n) as usize;
    if n > MAX_ALIAS_BYTES || bytes.len() != CURSOR_FIXED_LEN + n {
        return Err(malformed);
    }
    let (body, check) = bytes.split_at(bytes.len() - CURSOR_CHECK_LEN);
    if cursor_check(body) != check {
        return Err(ServerError::InvalidInput { what: "search cursor tampered" });
    }
    if &bytes[9..41] != expect_fp {
        return Err(ServerError::InvalidInput { what: "stale search cursor" });
    }
    let mut generation = [0u8; 8];
    generation.copy_from_slice(&bytes[1..9]);
    let mut score = [0u8; 8];
    score.copy_from_slice(&bytes[41..49]);
    // The check passed, so these are bytes we encoded: the alias is valid
    // UTF-8 by construction. Refuse (not panic) on the impossible case.
    let alias =
        String::from_utf8(bytes[51..51 + n].to_vec()).map_err(|_| malformed)?;
    let mut asset = [0u8; 16];
    asset.copy_from_slice(&bytes[51 + n..51 + n + 16]);
    Ok(Keyset {
        generation: u64::from_be_bytes(generation),
        score: u64::from_be_bytes(score),
        alias,
        asset,
    })
}

// ---- SQL assembly ----------------------------------------------------------

enum Bind {
    Text(String),
    Blob(Vec<u8>),
    U64(u64),
    Null,
}

fn apply_binds(s: &mut Stmt<'_>, binds: &[Bind]) -> ServerResult<()> {
    for (i, b) in binds.iter().enumerate() {
        let idx = (i + 1) as i32;
        match b {
            Bind::Text(t) => s.bind_text(idx, t)?,
            Bind::Blob(bl) => s.bind_blob(idx, bl)?,
            Bind::U64(v) => s.bind_u64(idx, *v)?,
            Bind::Null => s.bind_null(idx)?,
        }
    }
    Ok(())
}

fn principal_bind(p: &Option<PrincipalId>) -> Bind {
    match p {
        Some(p) => Bind::Blob(p.0.to_vec()),
        None => Bind::Null,
    }
}

/// The viewer-dependent per-posting weight: owners of an annotation score on
/// all fields, everyone else on public fields only. A NULL principal bind
/// makes the comparison NULL (false), selecting the public weight.
const OWNER_WEIGHT: &str =
    "(CASE WHEN a.owner IS NOT NULL AND a.owner = ? THEN p.weight_owner ELSE p.weight_public END)";
const VISIBLE: &str = "(a.visibility = 'public' OR (a.owner IS NOT NULL AND a.owner = ?))";

// ---- transactional index maintenance (annotation and alias mutations) ------

/// Advance the index generation. Every transaction that changes what a search
/// can return (annotation upsert/clear, alias-head set/retarget/clear/
/// quarantine-drop) calls this, so keyset cursors cut before the change fail
/// closed as stale. A missing state row is schema corruption, not zero.
pub(crate) fn bump_generation(db: &Db) -> ServerResult<()> {
    let mut s = db.prepare(
        "bump search generation",
        "UPDATE search_state SET generation = generation + 1 WHERE id = 1",
    )?;
    s.run()?;
    drop(s);
    if db.changes() == 0 {
        return Err(ServerError::InvalidState { what: "search state", state: "missing" });
    }
    Ok(())
}

fn read_generation(db: &Db) -> ServerResult<u64> {
    let mut s = db.prepare(
        "read search generation",
        "SELECT generation FROM search_state WHERE id = 1",
    )?;
    if !s.step()? {
        return Err(ServerError::InvalidState { what: "search state", state: "missing" });
    }
    Ok(s.column_u64(0))
}

/// Rebuild `search_alias_postings` for one asset from its current alias
/// heads, inside the caller's transaction: delete the asset's rows, then —
/// only if the asset is annotated — re-tokenize every alias pointing at it.
/// Assets without an annotation index nothing (they can never be a hit).
pub(crate) fn rebuild_alias_postings(
    db: &Db,
    budgets: &Budgets,
    asset_id: &[u8],
) -> ServerResult<()> {
    let mut s = db.prepare(
        "clear alias postings",
        "DELETE FROM search_alias_postings WHERE asset_id = ?1",
    )?;
    s.bind_blob(1, asset_id)?;
    s.run()?;
    drop(s);
    let mut s = db.prepare(
        "alias postings annotated",
        "SELECT EXISTS(SELECT 1 FROM search_annotations WHERE asset_id = ?1)",
    )?;
    s.bind_blob(1, asset_id)?;
    s.step()?;
    let annotated = s.column_i64(0) != 0;
    drop(s);
    if !annotated {
        return Ok(());
    }
    let mut s = db.prepare(
        "read alias heads",
        "SELECT alias FROM asset_aliases WHERE asset_id = ?1 ORDER BY alias",
    )?;
    s.bind_blob(1, asset_id)?;
    let mut tf: BTreeMap<String, u64> = BTreeMap::new();
    while s.step()? {
        tokenize_into(&s.column_text(0), &mut tf);
    }
    drop(s);
    if tf.len() as u64 > budgets.max_search_index_terms as u64 {
        return Err(ServerError::OverBudget {
            what: "search alias index terms",
            limit: budgets.max_search_index_terms as u64,
            found: tf.len() as u64,
        });
    }
    for (term, n) in &tf {
        let mut s = db.prepare(
            "insert alias posting",
            "INSERT INTO search_alias_postings(term, asset_id, weight) VALUES(?1,?2,?3)",
        )?;
        s.bind_text(1, term)?;
        s.bind_blob(2, asset_id)?;
        s.bind_u64(3, W_ALIAS * (*n).min(TF_CAP))?;
        s.run()?;
    }
    Ok(())
}

/// Recompute an asset's alias-derived search state (live flag, canonical
/// alias, alias postings) from the alias table, inside the caller's
/// transaction. Assets without an annotation simply have no row to update.
pub(crate) fn refresh_alias_state(
    db: &Db,
    budgets: &Budgets,
    asset_id: &[u8],
) -> ServerResult<()> {
    let mut s = db.prepare(
        "refresh alias state",
        "UPDATE search_annotations SET
            live = EXISTS(SELECT 1 FROM asset_aliases WHERE asset_id = ?1),
            canon_alias = COALESCE(
                (SELECT MIN(alias) FROM asset_aliases WHERE asset_id = ?1), '')
         WHERE asset_id = ?1",
    )?;
    s.bind_blob(1, asset_id)?;
    s.run()?;
    drop(s);
    rebuild_alias_postings(db, budgets, asset_id)
}

/// One-time v3 migration backfill, inside the migration transaction:
/// recompute the live flag and canonical alias for every annotation row, and
/// build alias postings for every annotated asset an alias points at.
pub(crate) fn backfill_alias_index(db: &Db, budgets: &Budgets) -> ServerResult<()> {
    db.prepare(
        "backfill canon alias",
        "UPDATE search_annotations SET
            live = EXISTS(SELECT 1 FROM asset_aliases
                          WHERE asset_aliases.asset_id = search_annotations.asset_id),
            canon_alias = COALESCE(
                (SELECT MIN(alias) FROM asset_aliases
                 WHERE asset_aliases.asset_id = search_annotations.asset_id), '')",
    )?
    .run()?;
    let mut s = db.prepare(
        "backfill alias assets",
        "SELECT DISTINCT aa.asset_id FROM asset_aliases aa
         JOIN search_annotations sa ON sa.asset_id = aa.asset_id
         ORDER BY aa.asset_id",
    )?;
    let mut assets: Vec<Vec<u8>> = Vec::new();
    while s.step()? {
        assets.push(s.column_blob(0));
    }
    drop(s);
    for asset in &assets {
        rebuild_alias_postings(db, budgets, asset)?;
    }
    Ok(())
}

pub struct Search<'a> {
    pub(crate) db: &'a Db,
    pub(crate) budgets: &'a Budgets,
}

impl<'a> Search<'a> {
    // ---- annotations -------------------------------------------------------

    /// Create or replace the annotation for a registered asset and rebuild its
    /// postings, all in one transaction. The previous index state for the
    /// asset is fully superseded — no stale terms survive.
    pub fn set_annotation(
        &self,
        asset_id: &AssetId,
        ann: &AssetAnnotation,
        now_ms: u64,
    ) -> ServerResult<()> {
        if ann.title.is_empty() {
            return Err(ServerError::InvalidInput { what: "annotation title empty" });
        }
        check_text(&ann.title, MAX_TITLE_BYTES, false, "annotation title bytes", "annotation title charset")?;
        check_text(
            &ann.description,
            MAX_DESCRIPTION_BYTES,
            true,
            "annotation description bytes",
            "annotation description charset",
        )?;
        for (s, wl, wc) in [
            (&ann.creator, "annotation creator bytes", "annotation creator charset"),
            (&ann.generator, "annotation generator bytes", "annotation generator charset"),
            (&ann.backend, "annotation backend bytes", "annotation backend charset"),
            (&ann.model, "annotation model bytes", "annotation model charset"),
        ] {
            check_text(s, MAX_ANNOTATION_NAME_BYTES, false, wl, wc)?;
        }
        check_text(&ann.prompt, MAX_PROMPT_BYTES, true, "annotation prompt bytes", "annotation prompt charset")?;
        check_text(
            &ann.provenance,
            MAX_PROVENANCE_BYTES,
            true,
            "annotation provenance bytes",
            "annotation provenance charset",
        )?;
        let categories = check_labels(&ann.categories, "annotation categories")?;
        let tags = check_labels(&ann.tags, "annotation tags")?;
        if ann.visibility == Visibility::Private && ann.owner.is_none() {
            return Err(ServerError::InvalidInput { what: "private annotation requires owner" });
        }

        // Postings: term -> (weight_public, weight_owner), deterministic order.
        let mut postings: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        {
            let mut add = |text: &str, weight: u64, private: bool| {
                let mut tf: BTreeMap<String, u64> = BTreeMap::new();
                tokenize_into(text, &mut tf);
                for (term, n) in tf {
                    let w = weight * n.min(TF_CAP);
                    let e = postings.entry(term).or_insert((0, 0));
                    if !private {
                        e.0 += w;
                    }
                    e.1 += w;
                }
            };
            add(&ann.title, W_TITLE, false);
            if let Some(kind) = ann.kind {
                add(kind_name(kind), W_LABEL, false);
            }
            for c in &categories {
                add(c, W_LABEL, false);
            }
            for t in &tags {
                add(t, W_LABEL, false);
            }
            add(&ann.creator, W_CREATOR, false);
            add(&ann.generator, W_GEN, false);
            add(&ann.backend, W_GEN, false);
            add(&ann.model, W_GEN, false);
            add(&ann.description, W_DESCRIPTION, false);
            add(&ann.prompt, W_PROMPT, true);
            add(&ann.provenance, W_PROVENANCE, true);
        }
        if postings.len() as u64 > self.budgets.max_search_index_terms as u64 {
            return Err(ServerError::OverBudget {
                what: "search index terms",
                limit: self.budgets.max_search_index_terms as u64,
                found: postings.len() as u64,
            });
        }

        self.db.tx(|db| {
            // Namespace comes from the catalog record — the single source of
            // truth an annotation can never contradict.
            let mut s = db.prepare(
                "annotation asset ns",
                "SELECT namespace FROM assets WHERE asset_id = ?1",
            )?;
            s.bind_blob(1, asset_id.as_bytes())?;
            if !s.step()? {
                return Err(ServerError::NotFound { what: "asset for annotation" });
            }
            let namespace = s.column_text(0);
            drop(s);

            let mut s = db.prepare(
                "annotation live",
                "SELECT EXISTS(SELECT 1 FROM asset_aliases WHERE asset_id = ?1),
                        COALESCE((SELECT MIN(alias) FROM asset_aliases
                                  WHERE asset_id = ?1), '')",
            )?;
            s.bind_blob(1, asset_id.as_bytes())?;
            s.step()?;
            let live = s.column_i64(0) != 0;
            let canon_alias = s.column_text(1);
            drop(s);

            let mut s = db.prepare(
                "upsert annotation",
                "INSERT INTO search_annotations(asset_id, namespace, kind, visibility, owner,
                    title, description, creator, generator, backend, model,
                    prompt, provenance, live, updated_ms, canon_alias)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
                 ON CONFLICT(asset_id) DO UPDATE SET
                    namespace=?2, kind=?3, visibility=?4, owner=?5, title=?6, description=?7,
                    creator=?8, generator=?9, backend=?10, model=?11,
                    prompt=?12, provenance=?13, live=?14, updated_ms=?15, canon_alias=?16",
            )?;
            s.bind_blob(1, asset_id.as_bytes())?;
            s.bind_text(2, &namespace)?;
            match ann.kind {
                Some(k) => s.bind_text(3, kind_name(k))?,
                None => s.bind_null(3)?,
            }
            s.bind_text(4, ann.visibility.as_str())?;
            match &ann.owner {
                Some(p) => s.bind_blob(5, &p.0)?,
                None => s.bind_null(5)?,
            }
            s.bind_text(6, &ann.title)?;
            s.bind_text(7, &ann.description)?;
            s.bind_text(8, &ann.creator)?;
            s.bind_text(9, &ann.generator)?;
            s.bind_text(10, &ann.backend)?;
            s.bind_text(11, &ann.model)?;
            s.bind_text(12, &ann.prompt)?;
            s.bind_text(13, &ann.provenance)?;
            s.bind_i64(14, live as i64)?;
            s.bind_u64(15, now_ms)?;
            s.bind_text(16, &canon_alias)?;
            s.run()?;
            drop(s);

            for (sql, op) in [
                ("DELETE FROM search_labels WHERE asset_id = ?1", "clear labels"),
                ("DELETE FROM search_postings WHERE asset_id = ?1", "clear postings"),
            ] {
                let mut s = db.prepare(op, sql)?;
                s.bind_blob(1, asset_id.as_bytes())?;
                s.run()?;
            }
            for (kind, labels) in [("category", &categories), ("tag", &tags)] {
                for label in labels.iter() {
                    let mut s = db.prepare(
                        "insert label",
                        "INSERT INTO search_labels(asset_id, kind, label) VALUES(?1,?2,?3)",
                    )?;
                    s.bind_blob(1, asset_id.as_bytes())?;
                    s.bind_text(2, kind)?;
                    s.bind_text(3, label)?;
                    s.run()?;
                }
            }
            for (term, (w_pub, w_own)) in &postings {
                let mut s = db.prepare(
                    "insert posting",
                    "INSERT INTO search_postings(term, asset_id, weight_public, weight_owner)
                     VALUES(?1,?2,?3,?4)",
                )?;
                s.bind_text(1, term)?;
                s.bind_blob(2, asset_id.as_bytes())?;
                s.bind_u64(3, *w_pub)?;
                s.bind_u64(4, *w_own)?;
                s.run()?;
            }
            // The asset may have gained its annotation after its aliases
            // existed: (re)build the alias postings in the same transaction,
            // and retire every open cursor.
            rebuild_alias_postings(db, self.budgets, asset_id.as_bytes())?;
            bump_generation(db)
        })
    }

    /// Remove an asset's annotation and every index row. Idempotent; the
    /// index generation advances only when a row was actually removed.
    pub fn clear_annotation(&self, asset_id: &AssetId) -> ServerResult<()> {
        self.db.tx(|db| {
            let mut s = db.prepare(
                "clear annotation",
                "DELETE FROM search_annotations WHERE asset_id = ?1",
            )?;
            s.bind_blob(1, asset_id.as_bytes())?;
            s.run()?;
            drop(s);
            let existed = db.changes() > 0;
            for (sql, op) in [
                ("DELETE FROM search_labels WHERE asset_id = ?1", "clear labels"),
                ("DELETE FROM search_postings WHERE asset_id = ?1", "clear postings"),
                ("DELETE FROM search_alias_postings WHERE asset_id = ?1", "clear alias postings"),
            ] {
                let mut s = db.prepare(op, sql)?;
                s.bind_blob(1, asset_id.as_bytes())?;
                s.run()?;
            }
            if existed {
                bump_generation(db)?;
            }
            Ok(())
        })
    }

    /// Full-fidelity read of an annotation. This is a trusted internal read
    /// like `read_blob`; per-viewer redaction happens in search, and the
    /// transport layer gates who may call this directly.
    pub fn annotation(&self, asset_id: &AssetId) -> ServerResult<Option<AssetAnnotation>> {
        let mut s = self.db.prepare(
            "read annotation",
            "SELECT visibility, owner, title, description, creator, generator,
                    backend, model, prompt, provenance, kind
             FROM search_annotations WHERE asset_id = ?1",
        )?;
        s.bind_blob(1, asset_id.as_bytes())?;
        if !s.step()? {
            return Ok(None);
        }
        let visibility = Visibility::parse(&s.column_text(0))
            .ok_or(ServerError::InvalidState { what: "annotation row", state: "unknown visibility" })?;
        let owner = if s.column_is_null(1) {
            None
        } else {
            Some(PrincipalId(fixed16(&s.column_blob(1), "annotation owner")?))
        };
        let kind = read_kind_column(&s, 10)?;
        let mut ann = AssetAnnotation {
            title: s.column_text(2),
            description: s.column_text(3),
            kind,
            categories: Vec::new(),
            tags: Vec::new(),
            creator: s.column_text(4),
            owner,
            generator: s.column_text(5),
            backend: s.column_text(6),
            model: s.column_text(7),
            prompt: s.column_text(8),
            provenance: s.column_text(9),
            visibility,
        };
        drop(s);
        let mut s = self.db.prepare(
            "read labels",
            "SELECT kind, label FROM search_labels WHERE asset_id = ?1 ORDER BY kind, label",
        )?;
        s.bind_blob(1, asset_id.as_bytes())?;
        while s.step()? {
            let kind = s.column_text(0);
            let label = s.column_text(1);
            if kind == "category" {
                ann.categories.push(label);
            } else {
                ann.tags.push(label);
            }
        }
        Ok(Some(ann))
    }

    // ---- search ------------------------------------------------------------

    pub fn search(
        &self,
        query: &SearchQuery<'_>,
        viewer: &SearchViewer<'_>,
        cursor: Option<&[u8]>,
    ) -> ServerResult<SearchPage> {
        // -- validate the query shape, fail closed on every bound ------------
        if query.text.len() as u64 > self.budgets.max_search_query_bytes as u64 {
            return Err(ServerError::OverBudget {
                what: "search query bytes",
                limit: self.budgets.max_search_query_bytes as u64,
                found: query.text.len() as u64,
            });
        }
        if query.page_size == 0 {
            return Err(ServerError::InvalidInput { what: "search page size zero" });
        }
        if query.page_size > self.budgets.max_search_results {
            return Err(ServerError::OverBudget {
                what: "search page size",
                limit: self.budgets.max_search_results as u64,
                found: query.page_size as u64,
            });
        }
        let mut tf = BTreeMap::new();
        tokenize_into(query.text, &mut tf);
        let terms: Vec<String> = tf.into_keys().collect();
        let browse = query.text.trim().is_empty();
        if !browse && terms.is_empty() {
            return Err(ServerError::InvalidInput { what: "search query has no terms" });
        }
        if terms.len() as u64 > self.budgets.max_search_query_terms as u64 {
            return Err(ServerError::OverBudget {
                what: "search query terms",
                limit: self.budgets.max_search_query_terms as u64,
                found: terms.len() as u64,
            });
        }
        let f = &query.filters;
        if let Some(ns) = f.namespace {
            validate_namespace(ns)?;
        }
        for l in [f.category, f.tag] {
            if let Some(l) = l {
                check_label(l)?;
            }
        }
        for v in [f.creator, f.generator, f.backend, f.model] {
            if let Some(v) = v {
                if v.is_empty() {
                    return Err(ServerError::InvalidInput { what: "search filter empty" });
                }
                check_text(v, MAX_ANNOTATION_NAME_BYTES, false, "search filter bytes", "search filter charset")?;
            }
        }
        let scope_namespaces: Option<Vec<&str>> = match viewer.scope {
            ViewerScope::All => None,
            ViewerScope::Namespaces(list) => {
                if list.len() > MAX_VIEWER_NAMESPACES {
                    return Err(ServerError::OverBudget {
                        what: "search viewer namespaces",
                        limit: MAX_VIEWER_NAMESPACES as u64,
                        found: list.len() as u64,
                    });
                }
                for ns in list {
                    validate_namespace(ns)?;
                }
                let mut sorted: Vec<&str> = list.to_vec();
                sorted.sort_unstable();
                sorted.dedup();
                Some(sorted)
            }
        };
        // A viewer scoped to zero namespaces sees nothing, by construction.
        if matches!(&scope_namespaces, Some(v) if v.is_empty()) {
            return Ok(SearchPage { hits: Vec::new(), total: 0, cursor: None });
        }

        // -- fingerprint the full query shape for cursor binding -------------
        let fp = fingerprint(&terms, browse, f, viewer, &scope_namespaces, query.page_size);
        let keyset = match cursor {
            None => None,
            Some(bytes) => Some(decode_cursor(bytes, &fp)?),
        };

        // -- one snapshot for generation + count + page -----------------------
        self.db.read_tx(|db| {
            // The index generation is read inside the same snapshot the page
            // is cut from: a cursor from before any index mutation is stale.
            let generation = read_generation(db)?;
            if let Some(k) = &keyset {
                if k.generation != generation {
                    return Err(ServerError::InvalidInput { what: "stale search cursor" });
                }
            }
            let (count_sql, count_binds) =
                build_sql(&terms, browse, f, &viewer.principal, &scope_namespaces, None, None);
            let mut s = db.prepare("search count", &count_sql)?;
            apply_binds(&mut s, &count_binds)?;
            s.step()?;
            let total = s.column_u64(0);
            drop(s);

            // Fetch one row beyond the page to learn whether more exist.
            let limit = query.page_size as u64 + 1;
            let (page_sql, page_binds) = build_sql(
                &terms,
                browse,
                f,
                &viewer.principal,
                &scope_namespaces,
                keyset.as_ref(),
                Some(limit),
            );
            let mut s = db.prepare("search page", &page_sql)?;
            apply_binds(&mut s, &page_binds)?;
            let mut hits = Vec::new();
            let mut more = false;
            while s.step()? {
                if hits.len() as u64 == query.page_size as u64 {
                    more = true;
                    break;
                }
                let asset_id = AssetId::from_bytes(fixed16(&s.column_blob(0), "search hit row")?);
                let namespace = s.column_text(1);
                let title = s.column_text(2);
                let description = s.column_text(3);
                let live = s.column_i64(4) != 0;
                let score = s.column_u64(5);
                let kind = read_kind_column(&s, 6)?;
                let canon = s.column_text(7);
                let snippet = build_snippet(
                    &title,
                    &description,
                    &terms,
                    self.budgets.max_search_snippet_bytes as usize,
                );
                let alias = if canon.is_empty() { None } else { Some(canon) };
                hits.push(SearchHit {
                    asset_id,
                    namespace,
                    kind,
                    title,
                    snippet,
                    score,
                    live,
                    alias,
                });
            }
            let cursor = if more {
                let last = hits.last().expect("page_size >= 1");
                Some(encode_cursor(
                    generation,
                    &fp,
                    last.score,
                    last.alias.as_deref().unwrap_or(""),
                    &last.asset_id,
                ))
            } else {
                None
            };
            Ok(SearchPage { hits, total, cursor })
        })
    }
}

/// SHA-256 over a canonical encoding of everything that shapes a result set.
/// A cursor is only valid against the identical shape.
fn fingerprint(
    terms: &[String],
    browse: bool,
    f: &SearchFilters<'_>,
    viewer: &SearchViewer<'_>,
    scope: &Option<Vec<&str>>,
    page_size: u32,
) -> [u8; 32] {
    let mut buf = Vec::new();
    buf.push(CURSOR_VERSION);
    buf.push(browse as u8);
    for t in terms {
        buf.extend_from_slice(&(t.len() as u16).to_be_bytes());
        buf.extend_from_slice(t.as_bytes());
    }
    buf.push(0xff);
    for opt in [
        f.namespace,
        f.kind.map(kind_name),
        f.category,
        f.tag,
        f.creator,
        f.generator,
        f.backend,
        f.model,
    ] {
        match opt {
            None => buf.push(0),
            Some(v) => {
                buf.push(1);
                buf.extend_from_slice(&(v.len() as u16).to_be_bytes());
                buf.extend_from_slice(v.as_bytes());
            }
        }
    }
    match &f.owner {
        None => buf.push(0),
        Some(p) => {
            buf.push(1);
            buf.extend_from_slice(&p.0);
        }
    }
    buf.push(f.live_only as u8);
    match &viewer.principal {
        None => buf.push(0),
        Some(p) => {
            buf.push(1);
            buf.extend_from_slice(&p.0);
        }
    }
    match scope {
        None => buf.push(0xff),
        Some(list) => {
            buf.push(list.len() as u8);
            for ns in list {
                buf.extend_from_slice(&(ns.len() as u16).to_be_bytes());
                buf.extend_from_slice(ns.as_bytes());
            }
        }
    }
    buf.extend_from_slice(&page_size.to_be_bytes());
    sha256(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CREATE in `SEARCH_SCHEMA` and the v1 -> v2 ALTER must define the
    /// kind column identically: both embed `KIND_DDL` byte-for-byte.
    #[test]
    fn kind_ddl_is_shared_between_create_and_migration() {
        assert!(SEARCH_SCHEMA.contains(KIND_DDL), "schema drifted from KIND_DDL");
        assert!(kind_migration_sql().contains(KIND_DDL));
    }

    /// Same parity law for the v2 -> v3 canonical-alias retrofit.
    #[test]
    fn canon_alias_ddl_is_shared_between_create_and_migration() {
        assert!(SEARCH_SCHEMA.contains(CANON_ALIAS_DDL), "schema drifted from CANON_ALIAS_DDL");
        assert!(canon_alias_migration_sql().contains(CANON_ALIAS_DDL));
    }

    #[test]
    fn kind_names_round_trip_and_are_all_in_the_check() {
        let all = [
            AssetKind::Mesh,
            AssetKind::Character,
            AssetKind::Weapon,
            AssetKind::Vehicle,
            AssetKind::Prop,
            AssetKind::Texture,
            AssetKind::Material,
            AssetKind::Audio,
            AssetKind::Video,
            AssetKind::Skybox,
            AssetKind::World,
            AssetKind::Prefab,
            AssetKind::Billboard,
        ];
        for kind in all {
            assert_eq!(kind_parse(kind_name(kind)), Some(kind));
            assert!(KIND_DDL.contains(&format!("'{}'", kind_name(kind))));
        }
        assert_eq!(kind_parse("sword"), None);
        assert_eq!(kind_parse(""), None);
        assert_eq!(kind_parse("Mesh"), None, "stored names are lowercase-exact");
    }
}

/// Assemble the search SQL and its bind list in strict `?` order. With
/// `limit: None` the statement is the COUNT form (no keyset, no order); with
/// a limit it is the page form. All caller strings travel as binds — SQL text
/// varies only by clause structure, never by content.
fn build_sql(
    terms: &[String],
    browse: bool,
    f: &SearchFilters<'_>,
    principal: &Option<PrincipalId>,
    scope: &Option<Vec<&str>>,
    keyset: Option<&Keyset>,
    limit: Option<u64>,
) -> (String, Vec<Bind>) {
    let mut binds: Vec<Bind> = Vec::new();
    let mut sql = String::with_capacity(1024);
    let counting = limit.is_none();
    if counting {
        sql.push_str("SELECT COUNT(*) FROM (");
    }
    if browse {
        sql.push_str(
            "SELECT a.asset_id, a.namespace, a.title, a.description, a.live, 0 AS score, a.kind,
                    a.canon_alias
             FROM search_annotations a WHERE 1=1",
        );
    } else {
        sql.push_str("SELECT a.asset_id, a.namespace, a.title, a.description, a.live, SUM(");
        sql.push_str(OWNER_WEIGHT);
        binds.push(principal_bind(principal));
        // Annotation postings and alias postings are one logical index:
        // alias terms are public, so they carry the same weight for both
        // columns of the union.
        sql.push_str(
            ") AS score, a.kind, a.canon_alias FROM (
                SELECT term, asset_id, weight_public, weight_owner FROM search_postings
                UNION ALL
                SELECT term, asset_id, weight, weight FROM search_alias_postings
             ) p JOIN search_annotations a ON a.asset_id = p.asset_id WHERE p.term IN (",
        );
        for (i, t) in terms.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            sql.push('?');
            binds.push(Bind::Text(t.clone()));
        }
        sql.push_str(") AND ");
        sql.push_str(OWNER_WEIGHT);
        binds.push(principal_bind(principal));
        sql.push_str(" > 0");
    }
    sql.push_str(" AND ");
    sql.push_str(VISIBLE);
    binds.push(principal_bind(principal));
    if let Some(list) = scope {
        sql.push_str(" AND a.namespace IN (");
        for (i, ns) in list.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            sql.push('?');
            binds.push(Bind::Text(ns.to_string()));
        }
        sql.push(')');
    }
    for (clause, value) in [
        (" AND a.namespace = ?", f.namespace),
        (" AND a.kind = ?", f.kind.map(kind_name)),
        (" AND a.creator = ?", f.creator),
        (" AND a.generator = ?", f.generator),
        (" AND a.backend = ?", f.backend),
        (" AND a.model = ?", f.model),
    ] {
        if let Some(v) = value {
            sql.push_str(clause);
            binds.push(Bind::Text(v.to_string()));
        }
    }
    for (kind, value) in [("category", f.category), ("tag", f.tag)] {
        if let Some(v) = value {
            sql.push_str(
                " AND EXISTS(SELECT 1 FROM search_labels l WHERE l.asset_id = a.asset_id AND l.kind = ",
            );
            sql.push('?');
            sql.push_str(" AND l.label = ?)");
            binds.push(Bind::Text(kind.to_string()));
            binds.push(Bind::Text(v.to_string()));
        }
    }
    if let Some(owner) = &f.owner {
        sql.push_str(" AND a.owner IS NOT NULL AND a.owner = ?");
        binds.push(Bind::Blob(owner.0.to_vec()));
    }
    if f.live_only {
        sql.push_str(" AND a.live = 1");
    }
    // Keyset: resume strictly after (score DESC, canon_alias ASC, asset ASC).
    // In browse mode every score is 0, so the score comparison degenerates
    // and only the (alias, asset) tail remains.
    if browse {
        if let Some(k) = keyset {
            sql.push_str(
                " AND (a.canon_alias > ? OR (a.canon_alias = ? AND a.asset_id > ?))",
            );
            binds.push(Bind::Text(k.alias.clone()));
            binds.push(Bind::Text(k.alias.clone()));
            binds.push(Bind::Blob(k.asset.to_vec()));
        }
    } else {
        sql.push_str(" GROUP BY a.asset_id HAVING COUNT(DISTINCT p.term) = ?");
        binds.push(Bind::U64(terms.len() as u64));
        if let Some(k) = keyset {
            sql.push_str(
                " AND (score < ? OR (score = ? AND (a.canon_alias > ? OR (a.canon_alias = ? AND a.asset_id > ?))))",
            );
            binds.push(Bind::U64(k.score));
            binds.push(Bind::U64(k.score));
            binds.push(Bind::Text(k.alias.clone()));
            binds.push(Bind::Text(k.alias.clone()));
            binds.push(Bind::Blob(k.asset.to_vec()));
        }
    }
    if counting {
        sql.push(')');
    } else {
        if browse {
            sql.push_str(" ORDER BY a.canon_alias ASC, a.asset_id ASC");
        } else {
            sql.push_str(" ORDER BY score DESC, a.canon_alias ASC, a.asset_id ASC");
        }
        sql.push_str(" LIMIT ?");
        binds.push(Bind::U64(limit.unwrap_or(1)));
    }
    (sql, binds)
}
