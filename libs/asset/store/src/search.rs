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
//! - Query expansion (synonyms and plural folds, see [`crate::synonyms`]) is
//!   QUERY-SIDE ONLY: the index and its generation are untouched, so turning
//!   it on or off never needs a reindex. The query's terms become disjoint
//!   GROUPS — one per thing asked for, so two words for one thing ("sniper
//!   rifle") are one demand — each holding the query's own words at full
//!   weight and the table's additions at `weight / 3` (integer). An exact
//!   hit therefore always outranks the same hit reached through a synonym,
//!   and a query with nothing to expand scores bit-for-bit as it did before.
//!   A multi-term query still requires EVERY group to be satisfied.
//!   Expansion is capped per group and per query, is part of the cursor's
//!   query-shape fingerprint, and `SearchQuery::expand = false` turns it off
//!   entirely.
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

#![cfg_attr(any(target_arch = "wasm32", feature = "embedded"), allow(dead_code))]

use crate::auth::PrincipalId;
use crate::budget::Budgets;
use crate::catalog::{fixed16, validate_namespace};
use crate::error::{ServerError, ServerResult};
use crate::sqlite::{Db, Stmt};
use crate::synonyms;
use makepad_asset_data::limits::MAX_ALIAS_BYTES;
use makepad_asset_data::{sha256, AssetId, AssetKind};

use std::collections::{BTreeMap, BTreeSet};

/// The `kind` column's constraint. Must stay identical in the CREATE below
/// and in `SEARCH_KIND_MIGRATION`, which retrofits the column onto tables
/// created before schema v2 (tests/migration.rs proves the parity).
const KIND_DDL: &str = "kind TEXT CHECK(kind IS NULL OR kind IN \
    ('mesh','character','weapon','vehicle','prop','texture','material',\
'audio','video','skybox','world','prefab','billboard','game','vjeffect',\
'data','model-program'))";

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
'audio','video','skybox','world','prefab','billboard','game','vjeffect',\
'data','model-program')),
    visibility TEXT NOT NULL CHECK(visibility IN ('public','private')),
    owner BLOB,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    creator TEXT NOT NULL,
    artist TEXT NOT NULL DEFAULT '',
    artist_url TEXT NOT NULL DEFAULT '',
    album TEXT NOT NULL DEFAULT '',
    source_url TEXT NOT NULL DEFAULT '',
    license TEXT NOT NULL DEFAULT '',
    license_url TEXT NOT NULL DEFAULT '',
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

/// Catalog kinds the vision-annotation pass can describe — the ones an
/// import gives a turntable thumbnail sheet. A billboard sprite sheet or an
/// audio waveform is not sixteen views of one object, and the prompt that
/// reads one would be describing something that is not there.
pub const ANNOTATABLE_KINDS: &[&str] =
    &["mesh", "character", "weapon", "vehicle", "prop", "prefab"];

/// One asset the vision pass still owes a description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacklogRow {
    pub asset_id: AssetId,
    pub namespace: String,
    /// Canonical alias; the pass fetches the turntable sheet by it.
    pub alias: String,
    pub kind: Option<AssetKind>,
}

/// The annotatable-and-live predicate both backlog reads share. Kinds are
/// a compile-time list, never caller input, so they are inlined; the tag
/// and the category are bound.
fn backlog_where(category: Option<&str>, tag_param: i32, cat_param: i32) -> String {
    let kinds = ANNOTATABLE_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    let mut w = format!(
        "live = 1 AND canon_alias <> '' AND kind IN ({kinds}) \
         AND NOT EXISTS (SELECT 1 FROM search_labels l \
             WHERE l.asset_id = search_annotations.asset_id \
             AND l.kind = 'tag' AND l.label = ?{tag_param})"
    );
    if category.is_some() {
        w.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM search_labels c \
               WHERE c.asset_id = search_annotations.asset_id \
               AND c.kind = 'category' AND c.label = ?{cat_param})"
        ));
    }
    w
}

fn backlog_sql(category: Option<&str>, count_only: bool) -> String {
    let w = backlog_where(category, 1, 2);
    if count_only {
        return format!("SELECT COUNT(*) FROM search_annotations WHERE {w}");
    }
    let limit_param = if category.is_some() { 3 } else { 2 };
    format!(
        "SELECT asset_id, namespace, canon_alias, kind FROM search_annotations \
         WHERE {w} ORDER BY canon_alias LIMIT ?{limit_param}"
    )
}

/// The complement: annotatable, live, and already carrying the tag.
fn annotated_sql(category: Option<&str>) -> String {
    let kinds = ANNOTATABLE_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    let mut w = format!(
        "live = 1 AND canon_alias <> '' AND kind IN ({kinds}) \
         AND EXISTS (SELECT 1 FROM search_labels l \
             WHERE l.asset_id = search_annotations.asset_id \
             AND l.kind = 'tag' AND l.label = ?1)"
    );
    if category.is_some() {
        w.push_str(
            " AND EXISTS (SELECT 1 FROM search_labels c \
               WHERE c.asset_id = search_annotations.asset_id \
               AND c.kind = 'category' AND c.label = ?2)",
        );
    }
    format!("SELECT COUNT(*) FROM search_annotations WHERE {w}")
}

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
        AssetKind::Game => "game",
        AssetKind::VjEffect => "vjeffect",
        AssetKind::Data => "data",
        AssetKind::ModelProgram => "model-program",
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
        "game" => AssetKind::Game,
        "vjeffect" => AssetKind::VjEffect,
        "data" => AssetKind::Data,
        "model-program" => AssetKind::ModelProgram,
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

/// The v13 -> v14 attribution columns. Empty defaults preserve annotations
/// written before music credits became a typed catalog projection.
pub(crate) const ATTRIBUTION_MIGRATION_SQL: [&str; 6] = [
    "ALTER TABLE search_annotations ADD COLUMN artist TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE search_annotations ADD COLUMN artist_url TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE search_annotations ADD COLUMN album TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE search_annotations ADD COLUMN source_url TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE search_annotations ADD COLUMN license TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE search_annotations ADD COLUMN license_url TEXT NOT NULL DEFAULT ''",
];

/// The browse-mode total order, verbatim: `canon_alias ASC, asset_id ASC`.
/// Without it every browse page is a full table scan plus a temp b-tree sort
/// of the whole annotation table, and the keyset predicate cannot seek.
///
/// It lives outside `SEARCH_SCHEMA` because that string is also executed by
/// the v1 -> v2 step, where `canon_alias` does not exist yet; the v2 -> v3
/// step runs this immediately after adding the column, and the v7 -> v8 step
/// runs it for roots that predate the index.
pub(crate) const SEARCH_CANON_INDEX_SQL: &str = "
CREATE INDEX IF NOT EXISTS search_annotations_by_canon
    ON search_annotations(canon_alias, asset_id);
";

/// Rebuild `search_annotations` so the kind CHECK matches this build's
/// `KIND_DDL` (v5 -> v6 added `billboard`, v6 -> v7 added `game`,
/// v10 -> v11 added `vjeffect`, v11 -> v12 added `data`, v12 -> v13 added
/// `model-program`). SQLite cannot
/// ALTER a CHECK; copy + rename is the retrofit, and re-running it is
/// harmless, so every kind-widening step reuses this one statement.
pub(crate) const KIND_CHECK_REBUILD_SQL: &str = "
CREATE TABLE search_annotations_rebuild(
    asset_id BLOB PRIMARY KEY,
    namespace TEXT NOT NULL,
    kind TEXT CHECK(kind IS NULL OR kind IN \
    ('mesh','character','weapon','vehicle','prop','texture','material',\
'audio','video','skybox','world','prefab','billboard','game','vjeffect',\
'data','model-program')),
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
INSERT INTO search_annotations_rebuild(
    asset_id, namespace, kind, visibility, owner, title, description,
    creator, generator, backend, model, prompt, provenance, live,
    updated_ms, canon_alias
)
SELECT
    asset_id, namespace, kind, visibility, owner, title, description,
    creator, generator, backend, model, prompt, provenance, live,
    updated_ms, canon_alias
FROM search_annotations;
DROP TABLE search_annotations;
ALTER TABLE search_annotations_rebuild RENAME TO search_annotations;
CREATE INDEX IF NOT EXISTS search_annotations_by_ns ON search_annotations(namespace);
CREATE INDEX IF NOT EXISTS search_annotations_by_kind ON search_annotations(kind);
CREATE INDEX IF NOT EXISTS search_annotations_by_canon
    ON search_annotations(canon_alias, asset_id);
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
    pub artist: String,
    pub artist_url: String,
    pub album: String,
    pub source_url: String,
    pub license: String,
    pub license_url: String,
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
    /// Negative tag filter: assets carrying this tag are excluded, even when
    /// `tag` also matches. Server-side so frontends never post-filter pages.
    pub exclude_tag: Option<&'a str>,
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
    /// Widen each term with its synonyms and plural folds (see
    /// [`crate::synonyms`]). Expansion matches score strictly below exact
    /// ones and never change what an exact-only query returns first; `false`
    /// is the escape hatch for a caller that means the literal words it typed
    /// (the HTTP routes spell it `exact=1`).
    pub expand: bool,
    /// How many facet rows to return with the page; 0 asks for none.
    ///
    /// Facets ride the page rather than a route of their own so they are
    /// counted in the SAME read snapshot as the hits — a separate call
    /// could land either side of a commit and show counts that disagree
    /// with the rows on screen. Callers that do not need them (a game
    /// binding an alias, a pad wall paging through kinds) pay nothing.
    pub facets: u32,
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
    pub creator: String,
    pub artist: String,
    pub artist_url: String,
    pub album: String,
    pub source_url: String,
    pub license: String,
    pub license_url: String,
    pub snippet: String,
    pub score: u64,
    pub live: bool,
    /// Canonical alias: the lexicographically smallest alias head currently
    /// pointing at this asset; `None` when no alias does. Second key of the
    /// result order, after score and before asset id.
    pub alias: Option<String>,
    /// When this asset's search row last changed, epoch ms — the annotation
    /// is rewritten on every publish, so this is "last touched" as the
    /// catalog knows it. A list that shows a date needs one per row, and a
    /// per-row detail request to find it would be a request per row.
    pub updated_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// Total matches under the SAME viewer constraint as the hits.
    pub total: u64,
    /// Present iff more results exist; feed back verbatim to continue.
    pub cursor: Option<Vec<u8>>,
    /// Label counts over the WHOLE result set (not just this page), most
    /// used first. Empty unless the query asked for facets — see
    /// [`SearchQuery::facets`].
    pub facets: Vec<Facet>,
}

/// Which vocabulary a facet label belongs to. Both are annotation labels;
/// they are separate filters (`category` / `tag`) and separate index rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetKind {
    Category,
    Tag,
}

impl FacetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FacetKind::Category => "category",
            FacetKind::Tag => "tag",
        }
    }
}

/// One label of the current result set, and how many of its assets carry it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facet {
    pub kind: FacetKind,
    pub label: String,
    pub count: u64,
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

// ---- query expansion -------------------------------------------------------

/// Most expansion terms one group may contribute, beyond the query's own
/// words for it.
pub const MAX_EXPANSION_PER_GROUP: usize = 12;
/// Most terms — the query's own words plus every expansion — that the
/// index-seek form of the posting source may carry.
///
/// Each term becomes two `term = ?` branches of one compound SELECT, and the
/// SQL engine recurses per branch of a compound select: measured on a worker
/// thread's 2 MB stack, 64 branches are fine and ~100 overflow it. Expansion
/// is budgeted to keep a query inside this limit, and a query whose own terms
/// already exceed it falls back to the flat `term IN (...)` form — one scan,
/// no recursion — so a pathological query is slow, never fatal.
pub const MAX_SEEK_TERMS: usize = 24;
/// Most expansion terms a whole query may contribute, before the seek budget
/// above trims it further. With the per-group cap this bounds the index seeks
/// a synonym query can ask for.
pub const MAX_EXPANSION_TOTAL: usize = 64;
/// A matched expansion term scores `weight * EXPANSION_NUM / EXPANSION_DEN`
/// (integer division). An exact term scores `weight * EXPANSION_DEN /
/// EXPANSION_DEN`, which is the weight unchanged — expansion can only ever
/// add lower-scoring hits below the exact ones, never move an exact hit.
const EXPANSION_NUM: u64 = 1;
const EXPANSION_DEN: u64 = 3;

/// One thing the query asked for, in every word it may be written with.
///
/// `exact` holds the query's own words for this thing — usually one, more
/// when the query spelled the same thing twice ("sniper rifle", "dog puppy"):
/// synonymous words are ONE demand, not two, or a search for a two-word name
/// of one object would require an annotation to contain both halves.
/// `expansion` holds the words the tables added, scored a tier lower.
///
/// Groups are DISJOINT: every query word is claimed by exactly one group and
/// an expansion never takes a word another group needs. That is what lets a
/// single first-match `CASE` map a posting to exactly one group, so
/// `COUNT(DISTINCT group) = groups.len()` still means "every thing asked for
/// was found".
struct TermGroup {
    exact: Vec<String>,
    expansion: Vec<String>,
}

impl TermGroup {
    fn all(&self) -> impl Iterator<Item = &String> {
        self.exact.iter().chain(self.expansion.iter())
    }
}

/// Group the query's terms, in the terms' own (sorted, deduplicated) order.
/// With `expand` false every term is its own group with no expansion, which
/// is the query shape this index has always had.
fn build_groups(terms: &[String], expand: bool) -> Vec<TermGroup> {
    if !expand {
        return terms
            .iter()
            .map(|t| TermGroup { exact: vec![t.clone()], expansion: Vec::new() })
            .collect();
    }
    // Phase 1 — one group per distinct thing asked for. A term joins an
    // existing group when either accepts the other as a synonym; the first
    // such group wins, so the outcome depends only on the sorted term list.
    let mut built: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    for t in terms {
        let candidates = synonyms::expand_term(t);
        let joined = built.iter().position(|(exact, cands)| {
            cands.iter().any(|c| c == t) || exact.iter().any(|e| candidates.contains(e))
        });
        match joined {
            Some(i) => {
                built[i].0.push(t.clone());
                for c in candidates {
                    if !built[i].1.contains(&c) {
                        built[i].1.push(c);
                    }
                }
            }
            None => built.push((vec![t.clone()], candidates)),
        }
    }
    // Phase 2 — hand out expansion words, never a word the query itself used
    // and never one an earlier group already took. The budget is what is left
    // of MAX_SEEK_TERMS after the query's own words, shared evenly between the
    // groups (what one group leaves unused passes to the next), so a widened
    // query still fits the index-seek form. A query with more words than the
    // limit gets no expansion at all — it is already asking for everything.
    let mut claimed: BTreeSet<String> = terms.iter().cloned().collect();
    let mut budget = MAX_SEEK_TERMS.saturating_sub(terms.len()).min(MAX_EXPANSION_TOTAL);
    let group_count = built.len();
    let mut groups = Vec::with_capacity(group_count);
    for (i, (exact, candidates)) in built.into_iter().enumerate() {
        let share = (budget / (group_count - i)).min(MAX_EXPANSION_PER_GROUP);
        let mut expansion = Vec::new();
        for word in candidates {
            if expansion.len() >= share {
                break;
            }
            // Anything the tokenizer could not have produced could not be in
            // the index either; skipping it keeps the term list honest.
            if word.len() > MAX_TERM_BYTES
                || word.is_empty()
                || !word.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                || !claimed.insert(word.clone())
            {
                continue;
            }
            expansion.push(word);
            budget -= 1;
        }
        groups.push(TermGroup { exact, expansion });
    }
    groups
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
/// Longest encoded search cursor: the fixed frame plus a maximal alias.
/// The HTTP routes decode cursors against THIS bound — a smaller one turned
/// every page boundary that fell on a long alias into "malformed cursor".
pub const MAX_SEARCH_CURSOR_BYTES: usize = CURSOR_FIXED_LEN + MAX_ALIAS_BYTES;

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

/// Delete one asset's entire searchable footprint — annotation row, labels,
/// postings, alias postings — inside the caller's transaction, and advance
/// the index generation if anything was there. Retirement uses this: a
/// retired asset is ABSENT from the index rather than filtered out of it, so
/// no query pays a predicate for content that no longer exists. Returns
/// whether an annotation row was removed.
pub(crate) fn clear_annotation_in_tx(db: &Db, asset_id: &[u8]) -> ServerResult<bool> {
    let mut s = db.prepare(
        "clear annotation",
        "DELETE FROM search_annotations WHERE asset_id = ?1",
    )?;
    s.bind_blob(1, asset_id)?;
    s.run()?;
    drop(s);
    let existed = db.changes() > 0;
    for (sql, op) in [
        ("DELETE FROM search_labels WHERE asset_id = ?1", "clear labels"),
        ("DELETE FROM search_postings WHERE asset_id = ?1", "clear postings"),
        ("DELETE FROM search_alias_postings WHERE asset_id = ?1", "clear alias postings"),
    ] {
        let mut s = db.prepare(op, sql)?;
        s.bind_blob(1, asset_id)?;
        s.run()?;
    }
    if existed {
        bump_generation(db)?;
    }
    Ok(existed)
}

pub struct Search<'a> {
    pub(crate) db: &'a Db,
    pub(crate) budgets: &'a Budgets,
}

impl<'a> Search<'a> {
    pub fn generation(&self) -> ServerResult<u64> {
        read_generation(self.db)
    }

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
        self.db
            .tx(|db| self.set_annotation_in_tx(db, asset_id, ann, now_ms))
    }

    /// The body of [`Self::set_annotation`] inside the caller's open
    /// transaction, so a composite operation (batch publish) can land its
    /// annotations atomically with the catalog rows they describe.
    pub(crate) fn set_annotation_in_tx(
        &self,
        db: &Db,
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
        for (s, wl, wc) in [
            (&ann.artist, "annotation artist bytes", "annotation artist charset"),
            (&ann.artist_url, "annotation artist url bytes", "annotation artist url charset"),
            (&ann.album, "annotation album bytes", "annotation album charset"),
            (&ann.source_url, "annotation source url bytes", "annotation source url charset"),
            (&ann.license, "annotation license bytes", "annotation license charset"),
            (&ann.license_url, "annotation license url bytes", "annotation license url charset"),
        ] {
            check_text(s, MAX_PROVENANCE_BYTES, false, wl, wc)?;
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

        {
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
                    title, description, creator, artist, artist_url, album, source_url,
                    license, license_url, generator, backend, model, prompt, provenance,
                    live, updated_ms, canon_alias)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)
                 ON CONFLICT(asset_id) DO UPDATE SET
                    namespace=?2, kind=?3, visibility=?4, owner=?5, title=?6, description=?7,
                    creator=?8, artist=?9, artist_url=?10, album=?11, source_url=?12,
                    license=?13, license_url=?14, generator=?15, backend=?16, model=?17,
                    prompt=?18, provenance=?19, live=?20, updated_ms=?21, canon_alias=?22",
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
            s.bind_text(9, &ann.artist)?;
            s.bind_text(10, &ann.artist_url)?;
            s.bind_text(11, &ann.album)?;
            s.bind_text(12, &ann.source_url)?;
            s.bind_text(13, &ann.license)?;
            s.bind_text(14, &ann.license_url)?;
            s.bind_text(15, &ann.generator)?;
            s.bind_text(16, &ann.backend)?;
            s.bind_text(17, &ann.model)?;
            s.bind_text(18, &ann.prompt)?;
            s.bind_text(19, &ann.provenance)?;
            s.bind_i64(20, live as i64)?;
            s.bind_u64(21, now_ms)?;
            s.bind_text(22, &canon_alias)?;
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
        }
    }

    /// Remove an asset's annotation and every index row. Idempotent; the
    /// index generation advances only when a row was actually removed.
    pub fn clear_annotation(&self, asset_id: &AssetId) -> ServerResult<()> {
        self.db.tx(|db| {
            clear_annotation_in_tx(db, asset_id.as_bytes())?;
            Ok(())
        })
    }

    /// Full-fidelity read of an annotation. This is a trusted internal read
    /// like `read_blob`; per-viewer redaction happens in search, and the
    /// transport layer gates who may call this directly.
    pub fn annotation(&self, asset_id: &AssetId) -> ServerResult<Option<AssetAnnotation>> {
        let mut s = self.db.prepare(
            "read annotation",
            "SELECT visibility, owner, title, description, creator, artist, artist_url,
                    album, source_url, license, license_url, generator, backend, model,
                    prompt, provenance, kind
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
        let kind = read_kind_column(&s, 16)?;
        let mut ann = AssetAnnotation {
            title: s.column_text(2),
            description: s.column_text(3),
            kind,
            categories: Vec::new(),
            tags: Vec::new(),
            creator: s.column_text(4),
            artist: s.column_text(5),
            artist_url: s.column_text(6),
            album: s.column_text(7),
            source_url: s.column_text(8),
            license: s.column_text(9),
            license_url: s.column_text(10),
            owner,
            generator: s.column_text(11),
            backend: s.column_text(12),
            model: s.column_text(13),
            prompt: s.column_text(14),
            provenance: s.column_text(15),
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

    // ---- annotation backlog ------------------------------------------------

    /// Live assets of an annotatable kind that do not carry `version_tag`.
    ///
    /// The vision-annotation queue's admission list: it is a catalog read,
    /// not a search, because "everything still un-described" is a set no
    /// text query can name and the answer has to be exact.
    pub fn annotation_backlog(
        &self,
        version_tag: &str,
        category: Option<&str>,
        limit: u64,
    ) -> ServerResult<Vec<BacklogRow>> {
        let mut s = self.db.prepare("annotation backlog", &backlog_sql(category, false))?;
        s.bind_text(1, version_tag)?;
        let mut next = 2;
        if let Some(c) = category {
            s.bind_text(next, c)?;
            next += 1;
        }
        s.bind_u64(next, limit)?;
        let mut out = Vec::new();
        while s.step()? {
            out.push(BacklogRow {
                asset_id: AssetId::from_bytes(fixed16(&s.column_blob(0), "backlog asset id")?),
                namespace: s.column_text(1),
                alias: s.column_text(2),
                kind: read_kind_column(&s, 3)?,
            });
        }
        Ok(out)
    }

    /// The backlog row for ONE asset, or `None` when it owes nothing: not
    /// live, no alias, not an annotatable kind, or already carrying the
    /// tag. The publish-time enqueue asks exactly this question.
    pub fn backlog_row_for(
        &self,
        asset_id: &AssetId,
        version_tag: &str,
    ) -> ServerResult<Option<BacklogRow>> {
        let mut s = self.db.prepare(
            "annotation backlog row",
            "SELECT namespace, canon_alias, kind, live FROM search_annotations \
             WHERE asset_id = ?1",
        )?;
        s.bind_blob(1, asset_id.as_bytes())?;
        if !s.step()? {
            return Ok(None);
        }
        let namespace = s.column_text(0);
        let alias = s.column_text(1);
        let kind = read_kind_column(&s, 2)?;
        let live = s.column_i64(3) != 0;
        drop(s);
        if !live || alias.is_empty() {
            return Ok(None);
        }
        let Some(k) = kind else {
            return Ok(None);
        };
        if !ANNOTATABLE_KINDS.contains(&kind_name(k)) {
            return Ok(None);
        }
        let mut s = self.db.prepare(
            "annotation tag probe",
            "SELECT 1 FROM search_labels WHERE asset_id = ?1 AND kind = 'tag' AND label = ?2",
        )?;
        s.bind_blob(1, asset_id.as_bytes())?;
        s.bind_text(2, version_tag)?;
        if s.step()? {
            return Ok(None);
        }
        Ok(Some(BacklogRow { asset_id: *asset_id, namespace, alias, kind }))
    }

    /// `(still owed, already annotated)` over the same annotatable set.
    pub fn annotation_backlog_counts(
        &self,
        version_tag: &str,
        category: Option<&str>,
    ) -> ServerResult<(u64, u64)> {
        let count = |sql: String| -> ServerResult<u64> {
            let mut s = self.db.prepare("annotation backlog count", &sql)?;
            s.bind_text(1, version_tag)?;
            if let Some(c) = category {
                s.bind_text(2, c)?;
            }
            if !s.step()? {
                return Ok(0);
            }
            Ok(s.column_u64(0))
        };
        let owed = count(backlog_sql(category, true))?;
        let done = count(annotated_sql(category))?;
        Ok((owed, done))
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
        if query.facets > self.budgets.max_search_facets {
            return Err(ServerError::OverBudget {
                what: "search facets",
                limit: self.budgets.max_search_facets as u64,
                found: query.facets as u64,
            });
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
        for l in [f.category, f.tag, f.exclude_tag] {
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
            return Ok(SearchPage {
                hits: Vec::new(),
                total: 0,
                cursor: None,
                facets: Vec::new(),
            });
        }

        // -- widen each term into its (disjoint) group -----------------------
        // Query-side only: no posting is written, no generation is bumped.
        // The groups ARE the query shape from here on — the fingerprint, the
        // count, the page and the facets all read the same ones, so a cursor
        // cut with expansion on is refused by the same query with `exact=1`.
        let groups = build_groups(&terms, query.expand);
        // Snippets may centre on an expansion match: it is what the row was
        // found by, and hiding it would explain the hit less, not more.
        let snippet_terms: Vec<String> =
            groups.iter().flat_map(|g| g.all().cloned()).collect();

        // -- fingerprint the full query shape for cursor binding -------------
        let fp = fingerprint(&groups, browse, f, viewer, &scope_namespaces, query.page_size);
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
                build_sql(&groups, browse, f, &viewer.principal, &scope_namespaces, None, None);
            let mut s = db.prepare("search count", &count_sql)?;
            apply_binds(&mut s, &count_binds)?;
            s.step()?;
            let total = s.column_u64(0);
            drop(s);

            // Fetch one row beyond the page to learn whether more exist.
            let limit = query.page_size as u64 + 1;
            let (page_sql, page_binds) = build_sql(
                &groups,
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
                let updated_ms = s.column_u64(8);
                let creator = s.column_text(9);
                let artist = s.column_text(10);
                let artist_url = s.column_text(11);
                let album = s.column_text(12);
                let source_url = s.column_text(13);
                let license = s.column_text(14);
                let license_url = s.column_text(15);
                let snippet = build_snippet(
                    &title,
                    &description,
                    &snippet_terms,
                    self.budgets.max_search_snippet_bytes as usize,
                );
                let alias = if canon.is_empty() { None } else { Some(canon) };
                hits.push(SearchHit {
                    asset_id,
                    namespace,
                    kind,
                    title,
                    creator,
                    artist,
                    artist_url,
                    album,
                    source_url,
                    license,
                    license_url,
                    snippet,
                    score,
                    live,
                    alias,
                    updated_ms,
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
            // Facets come out of the SAME snapshot as the page: the counts
            // a user sees can never describe a different generation than
            // the rows under them. The candidate set is the query without
            // its keyset or page limit, so a facet counts every match, not
            // just this page's.
            let facets = if query.facets == 0 {
                Vec::new()
            } else {
                let (facet_sql, facet_binds) = build_facet_sql(
                    &groups,
                    browse,
                    f,
                    &viewer.principal,
                    &scope_namespaces,
                    query.facets as u64,
                );
                // Facets are DECORATION on the page, never the page: a
                // facet failure degrades to no facets instead of failing
                // the whole request. (Found live: the engine could not yet
                // plan the derived-table join this SQL uses, and every
                // catalog browse 500'd for it — hits included.)
                match facet_rows(db, &facet_sql, &facet_binds) {
                    Ok(out) => out,
                    Err(error) => {
                        eprintln!(
                            "[asset-server] search facets failed (page served without them): {error:?}"
                        );
                        Vec::new()
                    }
                }
            };
            Ok(SearchPage { hits, total, cursor, facets })
        })
    }
}

/// SHA-256 over a canonical encoding of everything that shapes a result set.
/// A cursor is only valid against the identical shape.
fn fingerprint(
    groups: &[TermGroup],
    browse: bool,
    f: &SearchFilters<'_>,
    viewer: &SearchViewer<'_>,
    scope: &Option<Vec<&str>>,
    page_size: u32,
) -> [u8; 32] {
    let mut buf = Vec::new();
    buf.push(CURSOR_VERSION);
    buf.push(browse as u8);
    // Groups, not bare terms: two queries that read the same but expand
    // differently (`exact=1`, a changed table) are different shapes, and a
    // cursor from one refuses against the other.
    for g in groups {
        for t in g.all() {
            buf.extend_from_slice(&(t.len() as u16).to_be_bytes());
            buf.extend_from_slice(t.as_bytes());
        }
        buf.push(0xfe);
    }
    buf.push(0xff);
    for opt in [
        f.namespace,
        f.kind.map(kind_name),
        f.category,
        f.tag,
        f.exclude_tag,
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

    /// Expansion is bounded in both directions, and no two groups ever claim
    /// the same term — the HAVING's first-match CASE would miscount if they
    /// did, and a query term could go missing behind another's synonym.
    #[test]
    fn expansion_is_capped_and_groups_stay_disjoint() {
        let terms: Vec<String> = [
            "dog", "cat", "car", "tree", "stone", "water", "fire", "gun", "sword", "house",
            "chair", "lamp",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let groups = build_groups(&terms, true);
        assert_eq!(
            groups.iter().flat_map(|g| g.exact.clone()).collect::<Vec<_>>(),
            terms,
            "these terms are twelve different things"
        );
        let mut claimed = BTreeSet::new();
        let mut total = 0;
        for g in &groups {
            assert!(g.expansion.len() <= MAX_EXPANSION_PER_GROUP, "per-group cap");
            total += g.expansion.len();
            for t in g.all() {
                assert!(claimed.insert(t.clone()), "term {t} claimed by two groups");
                assert!(t.len() <= MAX_TERM_BYTES);
            }
        }
        // This query wants far more expansion than it may have: the seek
        // budget is what is left of MAX_SEEK_TERMS, shared between groups.
        assert!(total > 0);
        assert_eq!(total, MAX_SEEK_TERMS - terms.len(), "the seek budget must bind");
        assert!(total <= MAX_EXPANSION_TOTAL);
        // Without expansion the shape is exactly the pre-expansion one.
        assert!(build_groups(&terms, false).iter().all(|g| g.expansion.is_empty()));
    }

    /// The posting source is one index seek per term while the query fits
    /// MAX_SEEK_TERMS and the flat scanned list beyond it — the engine
    /// recurses per branch of a compound select, so an over-wide query has to
    /// be slow, never fatal. Expansion is budgeted to stay on the fast side.
    #[test]
    fn an_over_wide_query_falls_back_to_the_flat_term_list() {
        let filters = SearchFilters::default();
        let sql_for = |terms: &[String], expand: bool| {
            build_candidate_sql(&build_groups(terms, expand), false, &filters, &None, &None).0
        };
        let narrow: Vec<String> = (0..8).map(|i| format!("w{i}")).collect();
        let sql = sql_for(&narrow, false);
        assert!(sql.contains("FROM search_postings WHERE term = ?"), "{sql}");
        assert!(!sql.contains("p.term IN ("));
        let wide: Vec<String> = (0..MAX_SEEK_TERMS + 1).map(|i| format!("w{i}")).collect();
        let sql = sql_for(&wide, false);
        assert!(sql.contains("p.term IN ("), "{sql}");
        assert!(!sql.contains("WHERE term = ?"));
        // A widened everyday query stays on the seek side of the line.
        for text in [vec!["dog"], vec!["tiny", "dog"], vec!["red", "sports", "car"]] {
            let terms: Vec<String> = text.iter().map(|s| s.to_string()).collect();
            let groups = build_groups(&terms, true);
            let total: usize = groups.iter().map(|g| g.all().count()).sum();
            assert!(total > terms.len(), "{text:?} expanded nothing");
            assert!(total <= MAX_SEEK_TERMS, "{text:?} widened to {total} terms");
            assert!(sql_for(&terms, true).contains("FROM search_postings WHERE term = ?"));
        }
    }

    /// Two words for one thing are one demand: `dog puppy` and `sniper rifle`
    /// each name a single thing, and requiring an annotation to contain both
    /// halves of a name is how those queries used to return nothing.
    #[test]
    fn synonymous_query_words_become_one_group() {
        for pair in [["dog", "puppy"], ["rifle", "sniper"], ["sofa", "couch"]] {
            let terms: Vec<String> = pair.iter().map(|s| s.to_string()).collect();
            let groups = build_groups(&terms, true);
            assert_eq!(groups.len(), 1, "{pair:?} is one thing");
            assert_eq!(groups[0].exact, terms, "both words stay exact, at full weight");
        }
        // Words for different things stay different demands.
        let terms = vec!["dog".to_string(), "red".to_string()];
        assert_eq!(build_groups(&terms, true).len(), 2);
    }

    /// Same parity law for the v2 -> v3 canonical-alias retrofit.
    #[test]
    fn canon_alias_ddl_is_shared_between_create_and_migration() {
        assert!(SEARCH_SCHEMA.contains(CANON_ALIAS_DDL), "schema drifted from CANON_ALIAS_DDL");
        assert!(canon_alias_migration_sql().contains(CANON_ALIAS_DDL));
    }

    #[test]
    fn longest_cursor_fits_the_route_bound() {
        let alias = "a".repeat(MAX_ALIAS_BYTES);
        let bytes = encode_cursor(7, &[3u8; 32], 9, &alias, &AssetId::from_bytes([1u8; 16]));
        assert_eq!(bytes.len(), MAX_SEARCH_CURSOR_BYTES);
        assert!(
            decode_cursor(&bytes, &[3u8; 32]).is_ok(),
            "a maximal alias must round-trip"
        );
        // A realistic pack alias is well past the old 128-byte bound's
        // 53-byte alias room and must round-trip too.
        let long = "kenney/modular-dungeon-kit/wall-doorway-round-cracked-narrow";
        assert!(long.len() > 53);
        let bytes = encode_cursor(1, &[0u8; 32], 1, long, &AssetId::from_bytes([2u8; 16]));
        assert!(bytes.len() > 128, "this cursor exceeds the old bound");
        assert!(bytes.len() <= MAX_SEARCH_CURSOR_BYTES);
        assert!(decode_cursor(&bytes, &[0u8; 32]).is_ok());
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
            AssetKind::Game,
            AssetKind::VjEffect,
            AssetKind::Data,
            AssetKind::ModelProgram,
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
/// Label counts over the same candidate set the page is cut from, most used
/// first. `build_sql`'s counting form is exactly that candidate SELECT
/// wrapped in a COUNT, so the facet query reuses it as a subquery: one
/// definition of "what matches", three uses (count, page, facets).
/// The facet query's rows, isolated so a failure can degrade (see caller).
fn facet_rows(db: &Db, facet_sql: &str, facet_binds: &[Bind]) -> ServerResult<Vec<Facet>> {
    let mut s = db.prepare("search facets", facet_sql)?;
    apply_binds(&mut s, facet_binds)?;
    let mut out = Vec::new();
    while s.step()? {
        let kind = match s.column_text(0).as_str() {
            "category" => FacetKind::Category,
            "tag" => FacetKind::Tag,
            // The label index only ever holds those two kinds (a CHECK
            // constraint says so); anything else is a corrupt row, not a
            // new vocabulary to guess at.
            _ => continue,
        };
        out.push(Facet {
            kind,
            label: s.column_text(1),
            count: s.column_u64(2),
        });
    }
    Ok(out)
}

fn build_facet_sql(
    groups: &[TermGroup],
    browse: bool,
    f: &SearchFilters<'_>,
    principal: &Option<PrincipalId>,
    scope: &Option<Vec<&str>>,
    limit: u64,
) -> (String, Vec<Bind>) {
    let (candidates, mut binds) = build_candidate_sql(groups, browse, f, principal, scope);
    let sql = format!(
        "SELECT l.kind, l.label, COUNT(*) AS n
         FROM search_labels l
         JOIN ({candidates}) c ON c.asset_id = l.asset_id
         GROUP BY l.kind, l.label
         ORDER BY n DESC, l.kind ASC, l.label ASC
         LIMIT ?"
    );
    binds.push(Bind::U64(limit));
    (sql, binds)
}

fn build_sql(
    groups: &[TermGroup],
    browse: bool,
    f: &SearchFilters<'_>,
    principal: &Option<PrincipalId>,
    scope: &Option<Vec<&str>>,
    keyset: Option<&Keyset>,
    limit: Option<u64>,
) -> (String, Vec<Bind>) {
    let counting = limit.is_none();
    let (mut sql, mut binds) = build_candidate_sql(groups, browse, f, principal, scope);
    // Keyset: resume strictly after (score DESC, canon_alias ASC, asset ASC).
    // In browse mode every score is 0, so the score comparison degenerates
    // and only the (alias, asset) tail remains.
    if let Some(k) = keyset {
        if browse {
            sql.push_str(" AND (a.canon_alias > ? OR (a.canon_alias = ? AND a.asset_id > ?))");
            binds.push(Bind::Text(k.alias.clone()));
            binds.push(Bind::Text(k.alias.clone()));
            binds.push(Bind::Blob(k.asset.to_vec()));
        } else {
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
        sql.insert_str(0, "SELECT COUNT(*) FROM (");
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

/// Everything that decides WHETHER an asset matches — the term join or the
/// browse scan, visibility, scope and every filter — with no keyset, order
/// or limit. Counting wraps it, the page appends its keyset and order to it,
/// and the facet aggregation joins the label index against it, so all three
/// answer for exactly the same set.
fn build_candidate_sql(
    groups: &[TermGroup],
    browse: bool,
    f: &SearchFilters<'_>,
    principal: &Option<PrincipalId>,
    scope: &Option<Vec<&str>>,
) -> (String, Vec<Bind>) {
    let mut binds: Vec<Bind> = Vec::new();
    let mut sql = String::with_capacity(1024);
    // With nothing expanded the score expression is the bare posting weight,
    // exactly as it was before expansion existed, and group membership is the
    // term itself; with expansion both become a first-match CASE over the
    // groups. The two forms agree on every exact term (`w * 3 / 3 == w` in
    // integer arithmetic) — one formula written two ways, the short one to
    // keep the common query cheap.
    let expanded = groups.iter().any(|g| !g.expansion.is_empty());
    // Index seeks per term, unless the query carries more terms than that
    // form's recursion budget allows (see MAX_SEEK_TERMS).
    let seek = groups.iter().map(|g| g.all().count()).sum::<usize>() <= MAX_SEEK_TERMS;
    if browse {
        sql.push_str(
            "SELECT a.asset_id, a.namespace, a.title, a.description, a.live, 0 AS score, a.kind,
                    a.canon_alias, a.updated_ms, a.creator, a.artist, a.artist_url,
                    a.album, a.source_url, a.license, a.license_url
             FROM search_annotations a WHERE 1=1",
        );
    } else {
        sql.push_str("SELECT a.asset_id, a.namespace, a.title, a.description, a.live, SUM(");
        if expanded {
            sql.push('(');
            sql.push_str(OWNER_WEIGHT);
            binds.push(principal_bind(principal));
            sql.push_str(") * (CASE WHEN p.term IN (");
            for (i, t) in groups.iter().flat_map(|g| g.exact.iter()).enumerate() {
                if i > 0 {
                    sql.push(',');
                }
                sql.push('?');
                binds.push(Bind::Text(t.clone()));
            }
            sql.push_str(&format!(
                ") THEN {EXPANSION_DEN} ELSE {EXPANSION_NUM} END) / {EXPANSION_DEN}"
            ));
        } else {
            sql.push_str(OWNER_WEIGHT);
            binds.push(principal_bind(principal));
        }
        // Annotation postings and alias postings are one logical index:
        // alias terms are public, so they carry the same weight for both
        // columns of the union.
        //
        // One `term = ?` branch per term per posting table, rather than one
        // `term IN (...)` over the union: the engine plans an equality with
        // the (term, asset_id) primary-key index and a list membership with a
        // full scan, so the branch form is an index seek per term where the
        // list form reads every posting row for every query. Measured on the
        // live catalog (262k postings, 13 terms): 7ms against 313ms. It is
        // also what keeps expansion affordable — a widened term costs one
        // more seek, not another pass over the table.
        //
        // Past MAX_SEEK_TERMS the flat list comes back: the engine recurses
        // per branch of a compound select and a long enough chain overflows a
        // worker stack, so an over-wide query (which the term budget alone
        // could reach, and expansion is kept clear of) is served by the scan
        // instead. Both forms select exactly the same postings.
        sql.push_str(") AS score, a.kind, a.canon_alias, a.updated_ms, a.creator, a.artist, a.artist_url, a.album, a.source_url, a.license, a.license_url FROM (");
        if seek {
            for (i, t) in groups.iter().flat_map(TermGroup::all).enumerate() {
                if i > 0 {
                    sql.push_str(" UNION ALL ");
                }
                sql.push_str(
                    "SELECT term, asset_id, weight_public, weight_owner FROM search_postings \
                     WHERE term = ?",
                );
                binds.push(Bind::Text(t.clone()));
            }
            for t in groups.iter().flat_map(TermGroup::all) {
                sql.push_str(
                    " UNION ALL SELECT term, asset_id, weight, weight FROM \
                     search_alias_postings WHERE term = ?",
                );
                binds.push(Bind::Text(t.clone()));
            }
            sql.push_str(") p JOIN search_annotations a ON a.asset_id = p.asset_id WHERE ");
        } else {
            sql.push_str(
                "SELECT term, asset_id, weight_public, weight_owner FROM search_postings
                 UNION ALL
                 SELECT term, asset_id, weight, weight FROM search_alias_postings
                 ) p JOIN search_annotations a ON a.asset_id = p.asset_id WHERE p.term IN (",
            );
            for (i, t) in groups.iter().flat_map(TermGroup::all).enumerate() {
                if i > 0 {
                    sql.push(',');
                }
                sql.push('?');
                binds.push(Bind::Text(t.clone()));
            }
            sql.push_str(") AND ");
        }
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
    // Exclusion reads the same label index; it is applied after the positive
    // label clauses so an asset carrying both `tag` and `exclude_tag` drops.
    // Being part of the shared builder, it binds identically in the COUNT,
    // the page and the facet form, so they never disagree.
    if let Some(v) = f.exclude_tag {
        sql.push_str(
            " AND NOT EXISTS(SELECT 1 FROM search_labels l WHERE l.asset_id = a.asset_id AND l.kind = 'tag' AND l.label = ?)",
        );
        binds.push(Bind::Text(v.to_string()));
    }
    if let Some(owner) = &f.owner {
        sql.push_str(" AND a.owner IS NOT NULL AND a.owner = ?");
        binds.push(Bind::Blob(owner.0.to_vec()));
    }
    if f.live_only {
        sql.push_str(" AND a.live = 1");
    }
    if !browse {
        // "every query term satisfied" = every GROUP satisfied. Groups are
        // disjoint, so a posting term maps to exactly one of them and the
        // first-match CASE is total on the term list the WHERE admits.
        sql.push_str(" GROUP BY a.asset_id HAVING COUNT(DISTINCT ");
        if expanded {
            sql.push_str("CASE");
            for (i, g) in groups.iter().enumerate() {
                sql.push_str(" WHEN p.term IN (");
                for (j, t) in g.all().enumerate() {
                    if j > 0 {
                        sql.push(',');
                    }
                    sql.push('?');
                    binds.push(Bind::Text(t.clone()));
                }
                sql.push_str(&format!(") THEN {i}"));
            }
            sql.push_str(" END");
        } else {
            sql.push_str("p.term");
        }
        sql.push_str(") = ?");
        binds.push(Bind::U64(groups.len() as u64));
    }
    (sql, binds)
}
