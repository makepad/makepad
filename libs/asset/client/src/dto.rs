//! Bounded typed projections of server JSON responses.
//!
//! Every parser here is fail-closed: a missing required field, an
//! out-of-shape ID, an over-budget string, an unknown enum value, or an
//! oversized collection refuses the WHOLE response with a
//! [`ClientError::Protocol`]. Unknown *fields* are ignored (additive server
//! evolution stays compatible); unknown *values* of closed vocabularies are
//! not (a state string this client cannot interpret must never be rendered as
//! something it isn't).
//!
//! IDs are parsed through the content contract's own strict `FromStr`
//! spellings, so a response can never inject an uppercase/padded/truncated
//! identifier deeper into the system.

use crate::error::{ClientError, ClientResult};
use crate::json::Value;
use crate::util::{from_hex_exact, sanitize_text};
use crate::wire::{
    MAX_CURSOR_BYTES, MAX_ERROR_DETAIL_BYTES, MAX_EVENT_CURSOR_BYTES, MAX_NAMESPACE_BYTES,
    MAX_PAGE_ENTRIES, MAX_SNIPPET_BYTES, MAX_SOURCE_PAGE_LIMIT, MAX_TITLE_BYTES,
};
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, AssetRevisionId, BlobId, ClientProfileDigest, DerivedVariantId,
    FileRole, GameAlias, GameId, GameRevisionId, ImportRevisionId, PackEntryKey, ResolvedEntry,
    ResolvedMapDigest, SourceCollectionId, VariantRole, VariantSetId,
};
use std::str::FromStr;

// ---- shared field helpers --------------------------------------------------

fn need<'a>(v: &'a Value, key: &'static str, what: &'static str) -> ClientResult<&'a Value> {
    v.get(key).ok_or(ClientError::Protocol { what })
}

fn need_str<'a>(
    v: &'a Value,
    key: &'static str,
    max: usize,
    what: &'static str,
) -> ClientResult<&'a str> {
    let s = need(v, key, what)?.as_str().ok_or(ClientError::Protocol { what })?;
    if s.len() > max {
        return Err(ClientError::Protocol { what });
    }
    Ok(s)
}

fn need_u64(v: &Value, key: &'static str, what: &'static str) -> ClientResult<u64> {
    need(v, key, what)?.as_u64().ok_or(ClientError::Protocol { what })
}

fn need_bool(v: &Value, key: &'static str, what: &'static str) -> ClientResult<bool> {
    need(v, key, what)?.as_bool().ok_or(ClientError::Protocol { what })
}

/// Optional u64: absent or null are both "no value"; any other non-integer
/// shape refuses.
fn opt_u64(v: &Value, key: &'static str, what: &'static str) -> ClientResult<Option<u64>> {
    match v.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(None),
        Some(x) => x.as_u64().map(Some).ok_or(ClientError::Protocol { what }),
    }
}

fn parse_asset_id(s: &str) -> ClientResult<AssetId> {
    AssetId::from_str(s).map_err(|_| ClientError::Protocol { what: "malformed asset_id" })
}

fn parse_revision(s: &str) -> ClientResult<AssetRevisionId> {
    AssetRevisionId::from_str(s).map_err(|_| ClientError::Protocol { what: "malformed revision" })
}

/// Display-text hygiene: refuse control characters instead of stripping
/// them — a title with embedded controls is hostile, not sloppy.
fn check_display(s: &str, what: &'static str) -> ClientResult<()> {
    if s.chars().any(char::is_control) {
        return Err(ClientError::Protocol { what });
    }
    Ok(())
}

/// The canonical lowercase kind vocabulary (mirrors the server's search
/// column values and browse projections).
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
        _ => return None,
    })
}

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
    }
}

/// Wire names of the content file roles (the server's canonical vocabulary).
pub fn role_name(role: makepad_asset_data::FileRole) -> &'static str {
    use makepad_asset_data::FileRole as R;
    match role {
        R::RenderGlb => "render_glb",
        R::Lod1Glb => "lod1_glb",
        R::Lod2Glb => "lod2_glb",
        R::Collider => "collider",
        R::AoMesh => "ao_mesh",
        R::ShadowSdf => "shadow_sdf",
        R::Albedo => "albedo",
        R::Normal => "normal",
        R::Orm => "orm",
        R::Texture => "texture",
        R::PreviewFront => "preview_front",
        R::PreviewSide => "preview_side",
        R::Turntable => "turntable",
        R::Audio => "audio",
        R::Video => "video",
        R::Source => "source",
        R::Depth => "depth",
        R::Splat => "splat",
        R::AoTexture => "ao_texture",
        R::StemDrums => "stem_drums",
        R::StemBass => "stem_bass",
        R::StemVocals => "stem_vocals",
        R::StemOther => "stem_other",
        R::Lyrics => "lyrics",
    }
}

/// Inverse of [`role_name`]. Unknown role strings refuse.
pub fn role_parse(s: &str) -> Option<FileRole> {
    use FileRole as R;
    Some(match s {
        "render_glb" => R::RenderGlb,
        "lod1_glb" => R::Lod1Glb,
        "lod2_glb" => R::Lod2Glb,
        "collider" => R::Collider,
        "ao_mesh" => R::AoMesh,
        "shadow_sdf" => R::ShadowSdf,
        "albedo" => R::Albedo,
        "normal" => R::Normal,
        "orm" => R::Orm,
        "texture" => R::Texture,
        "preview_front" => R::PreviewFront,
        "preview_side" => R::PreviewSide,
        "turntable" => R::Turntable,
        "audio" => R::Audio,
        "video" => R::Video,
        "source" => R::Source,
        "depth" => R::Depth,
        "splat" => R::Splat,
        "ao_texture" => R::AoTexture,
        "stem_drums" => R::StemDrums,
        "stem_bass" => R::StemBass,
        "stem_vocals" => R::StemVocals,
        "stem_other" => R::StemOther,
        "lyrics" => R::Lyrics,
        _ => return None,
    })
}

/// Wire name of a resolution role (`thumbnail` or a file-role name).
pub fn variant_role_name(role: VariantRole) -> &'static str {
    match role {
        VariantRole::Thumbnail => "thumbnail",
        VariantRole::File(r) => role_name(r),
    }
}

/// Inverse of [`variant_role_name`]. Unknown strings refuse.
pub fn variant_role_parse(s: &str) -> Option<VariantRole> {
    if s == "thumbnail" {
        Some(VariantRole::Thumbnail)
    } else {
        role_parse(s).map(VariantRole::File)
    }
}

pub fn tier_name(tier: makepad_asset_data::DeviceTier) -> &'static str {
    use makepad_asset_data::DeviceTier as T;
    match tier {
        T::Any => "any",
        T::Low => "low",
        T::Medium => "medium",
        T::High => "high",
    }
}

pub fn media_name(media: makepad_asset_data::MediaType) -> &'static str {
    use makepad_asset_data::MediaType as M;
    match media {
        M::Png => "png",
        M::Jpeg => "jpeg",
        M::Glb => "glb",
        M::Wav => "wav",
        M::Ogg => "ogg",
        M::Mp4 => "mp4",
        M::Bin => "bin",
        M::Text => "text",
        M::Ply => "ply",
        M::Mp3 => "mp3",
        M::Json => "json",
    }
}

/// Bounded, sanitized refusal text out of an error body; `None` when the body
/// is not a well-formed error object. Never fails: refusal rendering must not
/// depend on the refusal body being honest.
///
/// Both halves, when the server sent both. `error` is the CATEGORY — "content
/// contract violation" — and `detail` is the reason, which is the half that
/// tells anyone what to do about it. Reporting only the category turned every
/// refusal into "422 content contract violation", which is a sentence with no
/// information in it.
pub fn parse_error_detail(bytes: &[u8]) -> Option<String> {
    if bytes.len() as u64 > crate::wire::MAX_JSON_RESPONSE_BYTES {
        return None;
    }
    let v = crate::json::parse(bytes).ok()?;
    let msg = v.get("error")?.as_str()?;
    let clean = sanitize_text(msg, MAX_ERROR_DETAIL_BYTES);
    if clean.is_empty() {
        return None;
    }
    let detail = v
        .get("detail")
        .and_then(|d| d.as_str())
        .map(|d| sanitize_text(d, MAX_ERROR_DETAIL_BYTES))
        .filter(|d| !d.is_empty() && *d != clean);
    Some(match detail {
        Some(detail) => sanitize_text(&format!("{clean}: {detail}"), MAX_ERROR_DETAIL_BYTES),
        None => clean,
    })
}

// ---- health ---------------------------------------------------------------

/// `GET /v1/health` — the identity handshake. `server_id` must match the
/// discovery beacon (or the caller's pinned identity) before a server is
/// selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealthDto {
    pub server_id: [u8; 16],
    pub protocol_version: u16,
}

pub fn parse_health(v: &Value) -> ClientResult<HealthDto> {
    let id_hex = need_str(v, "server_id", 32, "health server_id")?;
    let server_id =
        from_hex_exact::<16>(id_hex).ok_or(ClientError::Protocol { what: "health server_id" })?;
    let pv = need_u64(v, "protocol_version", "health protocol_version")?;
    if pv == 0 || pv > u16::MAX as u64 {
        return Err(ClientError::Protocol { what: "health protocol_version" });
    }
    Ok(HealthDto { server_id, protocol_version: pv as u16 })
}

// ---- catalog search --------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogHit {
    pub asset_id: AssetId,
    pub namespace: String,
    pub kind: Option<AssetKind>,
    pub title: String,
    pub snippet: String,
    pub score: u64,
    pub live: bool,
    /// Canonical alias head pointing at this asset, when one exists.
    /// Optional on the wire for compatibility with older servers.
    pub alias: Option<AssetAlias>,
    /// When the asset's search row last changed, epoch ms. Absent from a
    /// server too old to send it, which reads as 0 — "not recorded" — and
    /// never as a date in 1970 on screen.
    pub updated_ms: u64,
}

/// Which vocabulary a facet label came from — the two label filters a
/// catalog query understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetKind {
    Category,
    Tag,
}

/// One label of the result set and how many of its assets carry it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogFacet {
    pub kind: FacetKind,
    pub label: String,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPageDto {
    pub hits: Vec<CatalogHit>,
    /// Total matches under the same viewer constraint as the hits.
    pub total: u64,
    /// Opaque continuation token; present iff more results exist.
    pub cursor: Option<String>,
    /// Label counts over the WHOLE result set, most used first. Empty when
    /// the query did not ask for facets, and empty from a server too old to
    /// count them — never a parse failure.
    pub facets: Vec<CatalogFacet>,
}

pub fn parse_catalog_page(v: &Value) -> ClientResult<CatalogPageDto> {
    let hits_v = need(v, "hits", "catalog hits")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "catalog hits" })?;
    if hits_v.len() > MAX_PAGE_ENTRIES {
        return Err(ClientError::Protocol { what: "catalog page too large" });
    }
    let mut hits = Vec::with_capacity(hits_v.len());
    for h in hits_v {
        let asset_id = parse_asset_id(need_str(h, "asset_id", 64, "hit asset_id")?)?;
        let namespace =
            need_str(h, "namespace", MAX_NAMESPACE_BYTES, "hit namespace")?.to_string();
        check_display(&namespace, "hit namespace")?;
        let kind = match h.get("kind") {
            None | Some(Value::Null) => None,
            Some(k) => {
                let name = k.as_str().ok_or(ClientError::Protocol { what: "hit kind" })?;
                Some(kind_parse(name).ok_or(ClientError::Protocol { what: "hit kind" })?)
            }
        };
        let title = need_str(h, "title", MAX_TITLE_BYTES, "hit title")?.to_string();
        check_display(&title, "hit title")?;
        let snippet = need_str(h, "snippet", MAX_SNIPPET_BYTES, "hit snippet")?.to_string();
        check_display(&snippet, "hit snippet")?;
        let score = need_u64(h, "score", "hit score")?;
        let live = need_bool(h, "live", "hit live")?;
        let alias = match h.get("alias") {
            None | Some(Value::Null) => None,
            Some(a) => {
                let text = a.as_str().ok_or(ClientError::Protocol { what: "hit alias" })?;
                if text.len() > 128 {
                    return Err(ClientError::Protocol { what: "hit alias" });
                }
                Some(
                    AssetAlias::from_str(text)
                        .map_err(|_| ClientError::Protocol { what: "hit alias" })?,
                )
            }
        };
        let updated_ms = match h.get("updated_ms") {
            None | Some(Value::Null) => 0,
            Some(_) => need_u64(h, "updated_ms", "hit updated_ms")?,
        };
        hits.push(CatalogHit {
            asset_id,
            namespace,
            kind,
            title,
            snippet,
            score,
            live,
            alias,
            updated_ms,
        });
    }
    let total = need_u64(v, "total", "catalog total")?;
    let cursor = parse_cursor_field(v)?;
    let facets = parse_facets(v)?;
    Ok(CatalogPageDto { hits, total, cursor, facets })
}

/// Facets are optional on the wire (absent from a server that does not count
/// them, and from every response that was not asked to), but a facet that IS
/// present is typed strictly: an unknown kind is a protocol error, not a
/// label to guess at.
fn parse_facets(v: &Value) -> ClientResult<Vec<CatalogFacet>> {
    let Some(list) = v.get("facets") else {
        return Ok(Vec::new());
    };
    if matches!(list, Value::Null) {
        return Ok(Vec::new());
    }
    let list = list.as_arr().ok_or(ClientError::Protocol { what: "catalog facets" })?;
    if list.len() > MAX_PAGE_ENTRIES {
        return Err(ClientError::Protocol { what: "catalog facets too many" });
    }
    let mut out = Vec::with_capacity(list.len());
    for f in list {
        let kind = match need_str(f, "kind", 16, "facet kind")? {
            "category" => FacetKind::Category,
            "tag" => FacetKind::Tag,
            _ => return Err(ClientError::Protocol { what: "facet kind" }),
        };
        let label = need_str(f, "label", crate::wire::MAX_FILTER_VALUE_BYTES, "facet label")?.to_string();
        check_display(&label, "facet label")?;
        out.push(CatalogFacet { kind, label, count: need_u64(f, "count", "facet count")? });
    }
    Ok(out)
}

/// A cursor out of a response: absent/null = end of results; a string must
/// be non-empty, bounded, and query-charset safe (it travels back inside a
/// request target).
fn parse_cursor_field(v: &Value) -> ClientResult<Option<String>> {
    match v.get("cursor") {
        None => Ok(None),
        Some(Value::Null) => Ok(None),
        Some(c) => {
            let s = c.as_str().ok_or(ClientError::Protocol { what: "cursor" })?;
            if s.is_empty() || s.len() > MAX_CURSOR_BYTES || !crate::wire::query_value_ok(s) {
                return Err(ClientError::Protocol { what: "cursor" });
            }
            Ok(Some(s.to_string()))
        }
    }
}

// ---- asset listing ---------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRow {
    pub asset_id: AssetId,
    pub namespace: String,
    pub created_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetsPageDto {
    pub assets: Vec<AssetRow>,
    pub cursor: Option<String>,
}

pub fn parse_assets_page(v: &Value) -> ClientResult<AssetsPageDto> {
    let rows = need(v, "assets", "assets rows")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "assets rows" })?;
    if rows.len() > MAX_PAGE_ENTRIES {
        return Err(ClientError::Protocol { what: "assets page too large" });
    }
    let mut assets = Vec::with_capacity(rows.len());
    for r in rows {
        let asset_id = parse_asset_id(need_str(r, "asset_id", 64, "asset row id")?)?;
        let namespace =
            need_str(r, "namespace", MAX_NAMESPACE_BYTES, "asset row namespace")?.to_string();
        check_display(&namespace, "asset row namespace")?;
        let created_ms = need_u64(r, "created_ms", "asset row created_ms")?;
        assets.push(AssetRow { asset_id, namespace, created_ms });
    }
    let cursor = parse_cursor_field(v)?;
    Ok(AssetsPageDto { assets, cursor })
}

// ---- asset detail ----------------------------------------------------------

/// Candidate lifecycle vocabulary. Closed: an unknown state string refuses
/// the response instead of being displayed as something it is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateStateDto {
    Staged,
    Published,
    Quarantined,
    /// Deleted: the revision left every listing, alias and search row, and
    /// its bytes are collectable. Terminal, like `Quarantined`, and reported
    /// instead of it once the deletion intent is recorded.
    Retired,
}

impl CandidateStateDto {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Published => "published",
            Self::Quarantined => "quarantined",
            Self::Retired => "retired",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "staged" => Self::Staged,
            "published" => Self::Published,
            "quarantined" => Self::Quarantined,
            "retired" => Self::Retired,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateDto {
    pub revision: AssetRevisionId,
    pub state: CandidateStateDto,
    pub staged_ms: u64,
    pub published_ms: Option<u64>,
    pub quarantined_ms: Option<u64>,
    /// When this revision was deleted; `None` while it is live.
    pub retired_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetDetailDto {
    pub asset_id: AssetId,
    pub namespace: String,
    /// The asset itself was deleted: it is gone from listings, aliases and
    /// search, and every revision below reads `Retired`. Detail still
    /// answers so a UI can say "deleted" instead of "vanished".
    pub retired: bool,
    pub retired_ms: Option<u64>,
    pub candidates: Vec<CandidateDto>,
}

impl AssetDetailDto {
    /// Latest published revision: newest `published_ms`, ties broken by
    /// revision bytes (mirrors the server's head query ordering).
    pub fn latest_published(&self) -> Option<&CandidateDto> {
        self.candidates
            .iter()
            .filter(|c| c.state == CandidateStateDto::Published)
            .max_by(|a, b| {
                a.published_ms
                    .cmp(&b.published_ms)
                    .then_with(|| a.revision.as_bytes().cmp(b.revision.as_bytes()))
            })
    }
}

pub fn parse_asset_detail(v: &Value) -> ClientResult<AssetDetailDto> {
    let asset_id = parse_asset_id(need_str(v, "asset_id", 64, "asset detail id")?)?;
    let namespace =
        need_str(v, "namespace", MAX_NAMESPACE_BYTES, "asset detail namespace")?.to_string();
    check_display(&namespace, "asset detail namespace")?;
    let cands = need(v, "candidates", "asset candidates")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "asset candidates" })?;
    if cands.len() > MAX_PAGE_ENTRIES {
        return Err(ClientError::Protocol { what: "asset candidates too large" });
    }
    let mut candidates = Vec::with_capacity(cands.len());
    for c in cands {
        let revision = parse_revision(need_str(c, "revision", 80, "candidate revision")?)?;
        let state_s = need_str(c, "state", 16, "candidate state")?;
        let state = CandidateStateDto::parse(state_s)
            .ok_or(ClientError::Protocol { what: "candidate state" })?;
        let staged_ms = need_u64(c, "staged_ms", "candidate staged_ms")?;
        let published_ms = opt_u64(c, "published_ms", "candidate published_ms")?;
        let quarantined_ms = opt_u64(c, "quarantined_ms", "candidate quarantined_ms")?;
        // A published state must carry its timestamp; refusing here keeps
        // `latest_published` honest.
        if state == CandidateStateDto::Published && published_ms.is_none() {
            return Err(ClientError::Protocol { what: "published candidate without timestamp" });
        }
        let retired_ms = opt_u64(c, "retired_ms", "candidate retired_ms")?;
        candidates.push(CandidateDto {
            revision,
            state,
            staged_ms,
            published_ms,
            quarantined_ms,
            retired_ms,
        });
    }
    // Older servers do not send these; absent means "not deleted", which is
    // exactly what such a server means.
    let retired = match v.get("retired") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ClientError::Protocol { what: "asset detail retired" }),
    };
    let retired_ms = opt_u64(v, "retired_ms", "asset detail retired_ms")?;
    Ok(AssetDetailDto { asset_id, namespace, retired, retired_ms, candidates })
}

// ---- deletion --------------------------------------------------------------

/// What one retire call removed. Idempotent by design: a repeat reports
/// `already_retired` with zero counts instead of failing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetireDto {
    pub asset_id: AssetId,
    /// Present for a single-revision retirement.
    pub revision: Option<AssetRevisionId>,
    pub already_retired: bool,
    pub revisions_retired: u64,
    pub aliases_dropped: u64,
    pub annotation_cleared: bool,
}

pub fn parse_retire(v: &Value) -> ClientResult<RetireDto> {
    let asset_id = parse_asset_id(need_str(v, "asset_id", 64, "retire asset id")?)?;
    let revision = opt_id_field(v, "revision", "retire revision", AssetRevisionId::from_str)?;
    let already_retired = match v.get("already_retired") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ClientError::Protocol { what: "retire already_retired" }),
    };
    let annotation_cleared = match v.get("annotation_cleared") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ClientError::Protocol { what: "retire annotation_cleared" }),
    };
    Ok(RetireDto {
        asset_id,
        revision,
        already_retired,
        revisions_retired: opt_u64(v, "revisions_retired", "retire revisions")?.unwrap_or(0),
        aliases_dropped: opt_u64(v, "aliases_dropped", "retire aliases")?.unwrap_or(0),
        annotation_cleared,
    })
}

// ---- live game rooms -------------------------------------------------------

/// One live room: somebody is playing `game` right now, and `invite` is how
/// to reach them. The invite is opaque to this crate — it is the game app's
/// own address-plus-key spelling, and it carries capability data, so it is
/// carried, never interpreted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomDto {
    pub room: String,
    pub game: String,
    pub invite: String,
    pub host: String,
    pub created_ms: u64,
    pub expires_ms: u64,
}

/// The answer to "I am about to host this game; is anybody already?".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomClaimDto {
    /// The claim is yours. `token` is the only proof of that and is issued
    /// once — heartbeat and retire need it.
    Claimed { room: RoomDto, token: String },
    /// Somebody was already hosting. Join `room` instead of standing up a
    /// second world nobody would find.
    Occupied { room: RoomDto },
}

impl RoomClaimDto {
    pub fn room(&self) -> &RoomDto {
        match self {
            Self::Claimed { room, .. } | Self::Occupied { room } => room,
        }
    }
}

fn room_text<'a>(v: &'a Value, key: &'static str, max: usize, what: &'static str) -> ClientResult<&'a str> {
    let s = need_str(v, key, max, what)?;
    if s.is_empty() || s.chars().any(char::is_control) {
        return Err(ClientError::Protocol { what });
    }
    Ok(s)
}

pub fn parse_room(v: &Value) -> ClientResult<RoomDto> {
    Ok(RoomDto {
        room: room_text(v, "room", 64, "room id")?.to_string(),
        game: room_text(v, "game", crate::wire::MAX_ROOM_GAME_BYTES, "room game")?.to_string(),
        invite: room_text(v, "invite", crate::wire::MAX_ROOM_INVITE_BYTES, "room invite")?
            .to_string(),
        host: room_text(v, "host", crate::wire::MAX_ROOM_HOST_BYTES, "room host")?.to_string(),
        created_ms: need_u64(v, "created_ms", "room created_ms")?,
        expires_ms: need_u64(v, "expires_ms", "room expires_ms")?,
    })
}

pub fn parse_rooms(v: &Value) -> ClientResult<Vec<RoomDto>> {
    let arr = need(v, "rooms", "rooms list")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "rooms list" })?;
    if arr.len() > crate::wire::MAX_ROOMS_PAGE {
        return Err(ClientError::OverBudget {
            what: "rooms",
            limit: crate::wire::MAX_ROOMS_PAGE as u64,
            found: arr.len() as u64,
        });
    }
    arr.iter().map(parse_room).collect()
}

/// A heartbeat answers with the room as the server now holds it — the
/// renewed lease included, so a host never has to guess when to beat again.
pub fn parse_room_envelope(v: &Value) -> ClientResult<RoomDto> {
    parse_room(need(v, "room", "room envelope")?)
}

pub fn parse_room_claim(v: &Value) -> ClientResult<RoomClaimDto> {
    let room = parse_room(need(v, "room", "room claim room")?)?;
    // A closed vocabulary: an outcome this client does not understand must
    // refuse, never fall through to "somebody else is hosting" — that would
    // silently drop a claim the caller actually holds.
    match need_str(v, "outcome", 16, "room claim outcome")? {
        "claimed" => Ok(RoomClaimDto::Claimed {
            room,
            token: room_text(v, "token", 64, "room claim token")?.to_string(),
        }),
        "occupied" => Ok(RoomClaimDto::Occupied { room }),
        _ => Err(ClientError::Protocol { what: "room claim outcome" }),
    }
}

// ---- blob garbage collection -----------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcPhaseDto {
    Retain,
    Mark,
    Sweep,
    Done,
    Cancelled,
}

impl GcPhaseDto {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Retain => "retain",
            Self::Mark => "mark",
            Self::Sweep => "sweep",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "retain" => Self::Retain,
            "mark" => Self::Mark,
            "sweep" => Self::Sweep,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

/// What one GC run has done so far. A run advances in bounded steps, so a
/// caller polls this until `done`; the counters are durable, not a snapshot
/// of one request's work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GcStatusDto {
    /// `None` when the server has never run a collection.
    pub run_id: Option<u64>,
    pub phase: GcPhaseDto,
    pub done: bool,
    pub dry_run: bool,
    pub started_ms: u64,
    pub updated_ms: u64,
    /// Blobs recorded after this timestamp are protected by the grace window.
    pub horizon_ms: u64,
    pub retain_keep: Option<u64>,
    pub retired_revisions: u64,
    pub scanned_revisions: u64,
    pub marked_blobs: u64,
    pub examined_blobs: u64,
    /// Blobs proven unreferenced (what a dry run reports as reclaimable).
    pub unreferenced_blobs: u64,
    pub unreferenced_bytes: u64,
    pub deleted_blobs: u64,
    pub deleted_bytes: u64,
}

pub fn parse_gc_status(v: &Value) -> ClientResult<GcStatusDto> {
    let run_id = opt_u64(v, "run_id", "gc run id")?;
    let done = match v.get("done") {
        Some(Value::Bool(b)) => *b,
        None | Some(Value::Null) => true,
        Some(_) => return Err(ClientError::Protocol { what: "gc done" }),
    };
    // A server that has never collected answers with a null run: everything
    // else is absent and reads as a finished, empty run.
    if run_id.is_none() {
        return Ok(GcStatusDto {
            run_id: None,
            phase: GcPhaseDto::Done,
            done: true,
            dry_run: false,
            started_ms: 0,
            updated_ms: 0,
            horizon_ms: 0,
            retain_keep: None,
            retired_revisions: 0,
            scanned_revisions: 0,
            marked_blobs: 0,
            examined_blobs: 0,
            unreferenced_blobs: 0,
            unreferenced_bytes: 0,
            deleted_blobs: 0,
            deleted_bytes: 0,
        });
    }
    let phase = GcPhaseDto::parse(need_str(v, "phase", 16, "gc phase")?)
        .ok_or(ClientError::Protocol { what: "gc phase" })?;
    let dry_run = match v.get("dry_run") {
        Some(Value::Bool(b)) => *b,
        None | Some(Value::Null) => false,
        Some(_) => return Err(ClientError::Protocol { what: "gc dry_run" }),
    };
    let field = |key: &'static str, what: &'static str| -> ClientResult<u64> {
        Ok(opt_u64(v, key, what)?.unwrap_or(0))
    };
    Ok(GcStatusDto {
        run_id,
        phase,
        done,
        dry_run,
        started_ms: field("started_ms", "gc started_ms")?,
        updated_ms: field("updated_ms", "gc updated_ms")?,
        horizon_ms: field("horizon_ms", "gc horizon_ms")?,
        retain_keep: opt_u64(v, "retain_keep", "gc retain_keep")?,
        retired_revisions: field("retired_revisions", "gc retired_revisions")?,
        scanned_revisions: field("scanned_revisions", "gc scanned_revisions")?,
        marked_blobs: field("marked_blobs", "gc marked_blobs")?,
        examined_blobs: field("examined_blobs", "gc examined_blobs")?,
        unreferenced_blobs: field("unreferenced_blobs", "gc unreferenced_blobs")?,
        unreferenced_bytes: field("unreferenced_bytes", "gc unreferenced_bytes")?,
        deleted_blobs: field("deleted_blobs", "gc deleted_blobs")?,
        deleted_bytes: field("deleted_bytes", "gc deleted_bytes")?,
    })
}

// ---- catalog event feed ----------------------------------------------------

/// Catalog-event vocabulary. The client asks for exactly the vocabulary it
/// understands (`wire::EVENT_VOCABULARY` travels on every poll), so a server
/// only ever sends kinds this build knows — and a server NEWER than this
/// build, which may have kinds beyond that, is still readable because an
/// unrecognised kind parses as [`CatalogEventKind::Other`] instead of
/// refusing the whole page. A subscriber that cannot interpret `Other`
/// treats it as "something changed" and resyncs; nothing is ever silently
/// misread as a known kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogEventKind {
    AssetPublished,
    AssetQuarantined,
    /// The whole asset was deleted: every revision retired, aliases gone,
    /// search rows removed. Subscribers must drop it from their view.
    AssetRetired,
    /// One revision was deleted; the asset may still be live.
    RevisionRetired,
    AliasSet,
    AliasCleared,
    AnnotationSet,
    AnnotationCleared,
    GamePublished,
    GameQuarantined,
    GameAliasSet,
    GameAliasCleared,
    /// A kind this build does not know (a newer server). Carries no
    /// interpretation on purpose.
    Other,
}

impl CatalogEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AssetPublished => "asset_published",
            Self::AssetQuarantined => "asset_quarantined",
            Self::AssetRetired => "asset_retired",
            Self::RevisionRetired => "revision_retired",
            Self::AliasSet => "alias_set",
            Self::AliasCleared => "alias_cleared",
            Self::AnnotationSet => "annotation_set",
            Self::AnnotationCleared => "annotation_cleared",
            Self::GamePublished => "game_published",
            Self::GameQuarantined => "game_quarantined",
            Self::GameAliasSet => "game_alias_set",
            Self::GameAliasCleared => "game_alias_cleared",
            Self::Other => "other",
        }
    }

    /// True when the event means "this content is gone": quarantine and both
    /// retirement kinds. Subscribers drop the asset on any of them.
    pub fn removes_content(self) -> bool {
        matches!(self, Self::AssetQuarantined | Self::AssetRetired | Self::RevisionRetired)
    }

    fn parse(s: &str) -> Self {
        match s {
            "asset_published" => Self::AssetPublished,
            "asset_quarantined" => Self::AssetQuarantined,
            "asset_retired" => Self::AssetRetired,
            "revision_retired" => Self::RevisionRetired,
            "alias_set" => Self::AliasSet,
            "alias_cleared" => Self::AliasCleared,
            "annotation_set" => Self::AnnotationSet,
            "annotation_cleared" => Self::AnnotationCleared,
            "game_published" => Self::GamePublished,
            "game_quarantined" => Self::GameQuarantined,
            "game_alias_set" => Self::GameAliasSet,
            "game_alias_cleared" => Self::GameAliasCleared,
            _ => Self::Other,
        }
    }
}

/// One committed catalog change from `/v1/events`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEventDto {
    pub seq: u64,
    pub kind: CatalogEventKind,
    pub namespace: String,
    pub asset_id: Option<AssetId>,
    pub revision: Option<AssetRevisionId>,
    pub game_id: Option<GameId>,
    pub game_revision: Option<GameRevisionId>,
    /// Asset or game alias in display spelling, for events that involve one.
    pub alias: Option<String>,
    /// The asset's declared content kind at emit time, when the server knew
    /// it. `None` means unknown, not "no kind" — kind-filtered subscribers
    /// still receive such events.
    pub content_kind: Option<AssetKind>,
    pub ts_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventsPageDto {
    pub events: Vec<CatalogEventDto>,
    /// Resume cursor covering everything this page scanned.
    pub cursor: String,
    /// Events were lost to retention (or the journal restarted): the
    /// subscriber must resync its catalog view before trusting the feed.
    pub gap: bool,
}

fn opt_id_field<T, E>(
    v: &Value,
    key: &'static str,
    what: &'static str,
    parse: impl Fn(&str) -> Result<T, E>,
) -> ClientResult<Option<T>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(x) => {
            let s = x.as_str().ok_or(ClientError::Protocol { what })?;
            if s.len() > 80 {
                return Err(ClientError::Protocol { what });
            }
            Ok(Some(parse(s).map_err(|_| ClientError::Protocol { what })?))
        }
    }
}

pub fn parse_events_page(v: &Value) -> ClientResult<EventsPageDto> {
    let rows = need(v, "events", "events rows")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "events rows" })?;
    if rows.len() > MAX_PAGE_ENTRIES {
        return Err(ClientError::Protocol { what: "events page too large" });
    }
    let mut events = Vec::with_capacity(rows.len());
    let mut last_seq = 0u64;
    for r in rows {
        let seq = need_u64(r, "seq", "event seq")?;
        // The journal is strictly monotonic; a server that repeats or
        // reorders sequence numbers cannot be resumed against.
        if seq <= last_seq {
            return Err(ClientError::Protocol { what: "event seq not increasing" });
        }
        last_seq = seq;
        let kind_s = need_str(r, "kind", 32, "event kind")?;
        let kind = CatalogEventKind::parse(kind_s);
        let namespace = need_str(r, "ns", MAX_NAMESPACE_BYTES, "event ns")?.to_string();
        check_display(&namespace, "event ns")?;
        let asset_id = opt_id_field(r, "asset_id", "event asset_id", AssetId::from_str)?;
        let revision =
            opt_id_field(r, "revision", "event revision", AssetRevisionId::from_str)?;
        let game_id = opt_id_field(r, "game_id", "event game_id", GameId::from_str)?;
        let game_revision = opt_id_field(
            r,
            "game_revision",
            "event game_revision",
            GameRevisionId::from_str,
        )?;
        let alias = match r.get("alias") {
            None | Some(Value::Null) => None,
            Some(a) => {
                let s = a.as_str().ok_or(ClientError::Protocol { what: "event alias" })?;
                if s.is_empty() || s.len() > 128 {
                    return Err(ClientError::Protocol { what: "event alias" });
                }
                check_display(s, "event alias")?;
                Some(s.to_string())
            }
        };
        let content_kind = match r.get("content_kind") {
            None | Some(Value::Null) => None,
            Some(k) => {
                let name =
                    k.as_str().ok_or(ClientError::Protocol { what: "event content_kind" })?;
                Some(kind_parse(name).ok_or(ClientError::Protocol { what: "event content_kind" })?)
            }
        };
        let ts_ms = need_u64(r, "ts_ms", "event ts_ms")?;
        events.push(CatalogEventDto {
            seq,
            kind,
            namespace,
            asset_id,
            revision,
            game_id,
            game_revision,
            alias,
            content_kind,
            ts_ms,
        });
    }
    let cursor = need_str(v, "cursor", MAX_EVENT_CURSOR_BYTES, "events cursor")?.to_string();
    let cursor_seq = crate::wire::event_cursor_seq(&cursor)
        .ok_or(ClientError::Protocol { what: "events cursor" })?;
    if last_seq > cursor_seq {
        return Err(ClientError::Protocol { what: "events cursor" });
    }
    let gap = need_bool(v, "gap", "events gap")?;
    Ok(EventsPageDto { events, cursor, gap })
}

// ---- jobs (generation scheduling) ------------------------------------------

/// Client-side job identity: the transport's `job_<32 lowercase hex>`
/// spelling, parsed strictly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobId(pub [u8; 16]);

impl JobId {
    pub fn parse(text: &str) -> Option<JobId> {
        let hex = text.strip_prefix("job_")?;
        Some(JobId(from_hex_exact::<16>(hex)?))
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "job_{}", crate::util::to_hex(&self.0))
    }
}

/// Closed job lifecycle vocabulary (mirrors the server core).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStateDto {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStateDto {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    /// Crate-facing strict parse (worker finish responses).
    pub(crate) fn parse_pub(s: &str) -> Option<Self> {
        Self::parse(s)
    }
}

/// One job's visible state. `result_asset`/`result_revision` are parsed
/// LENIENTLY out of the worker's result document when it follows the
/// publish convention (`{"asset_id": …, "revision": …}`); anything else is
/// simply `None` — the catalog event stream, not the job result, is the
/// authority for publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobStatusDto {
    pub job: JobId,
    pub namespace: String,
    pub kind: String,
    pub state: JobStateDto,
    pub created_ms: u64,
    /// `(permille, note)` from the worker heartbeat, when reported.
    pub progress: Option<(u16, String)>,
    /// Bounded, sanitized `result.outcome` string when a result document
    /// was recorded.
    pub outcome: Option<String>,
    pub result_asset: Option<AssetId>,
    pub result_revision: Option<AssetRevisionId>,
}

pub fn parse_job_status(v: &Value) -> ClientResult<JobStatusDto> {
    let job = JobId::parse(need_str(v, "job", 64, "job id")?)
        .ok_or(ClientError::Protocol { what: "job id" })?;
    let namespace = need_str(v, "namespace", MAX_NAMESPACE_BYTES, "job namespace")?.to_string();
    check_display(&namespace, "job namespace")?;
    let kind = need_str(v, "kind", 64, "job kind")?.to_string();
    check_display(&kind, "job kind")?;
    let state = JobStateDto::parse(need_str(v, "state", 16, "job state")?)
        .ok_or(ClientError::Protocol { what: "job state" })?;
    let created_ms = need_u64(v, "created_ms", "job created_ms")?;
    let progress = match v.get("progress") {
        None | Some(Value::Null) => None,
        Some(p) => {
            let permille = need_u64(p, "permille", "job progress permille")?;
            if permille > 1000 {
                return Err(ClientError::Protocol { what: "job progress permille" });
            }
            let note = match p.get("note") {
                None | Some(Value::Null) => String::new(),
                Some(n) => {
                    let text = n.as_str().ok_or(ClientError::Protocol { what: "job note" })?;
                    sanitize_text(text, crate::wire::MAX_PROGRESS_NOTE_BYTES)
                }
            };
            Some((permille as u16, note))
        }
    };
    let (outcome, result_asset, result_revision) = match v.get("result") {
        None | Some(Value::Null) => (None, None, None),
        Some(r) => {
            let outcome = r
                .get("outcome")
                .and_then(Value::as_str)
                .map(|s| sanitize_text(s, 32));
            let body = r.get("body");
            let asset = body
                .and_then(|b| b.get("asset_id"))
                .and_then(Value::as_str)
                .and_then(|s| AssetId::from_str(s).ok());
            let revision = body
                .and_then(|b| b.get("revision"))
                .and_then(Value::as_str)
                .and_then(|s| AssetRevisionId::from_str(s).ok());
            (outcome, asset, revision)
        }
    };
    Ok(JobStatusDto {
        job,
        namespace,
        kind,
        state,
        created_ms,
        progress,
        outcome,
        result_asset,
        result_revision,
    })
}

// ---- rich job detail + scoped job listing ------------------------------------

/// A server principal identity in its `prin_<32 lowercase hex>` display
/// spelling, parsed strictly (mirrors [`JobId`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PrincipalDto(pub [u8; 16]);

impl PrincipalDto {
    pub fn parse(text: &str) -> Option<PrincipalDto> {
        let hex = text.strip_prefix("prin_")?;
        Some(PrincipalDto(from_hex_exact::<16>(hex)?))
    }
}

impl std::fmt::Display for PrincipalDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "prin_{}", crate::util::to_hex(&self.0))
    }
}

/// Optional principal field: absent/null is "not reported" (older servers);
/// a present value must parse strictly or the response refuses.
fn opt_principal(
    v: &Value,
    key: &'static str,
    what: &'static str,
) -> ClientResult<Option<PrincipalDto>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(x) => {
            let s = x.as_str().ok_or(ClientError::Protocol { what })?;
            if s.len() > 64 {
                return Err(ClientError::Protocol { what });
            }
            Ok(Some(PrincipalDto::parse(s).ok_or(ClientError::Protocol { what })?))
        }
    }
}

/// One recorded execution attempt of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobAttemptDto {
    /// 1-based attempt number.
    pub attempt: u32,
    pub started_ms: u64,
    /// `None` while the attempt is still running (or was lost).
    pub ended_ms: Option<u64>,
}

/// Typed progress with its freshness timestamp — a UI can tell "42% just
/// now" from "42% three minutes ago and the worker may be gone".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobProgressDto {
    pub permille: u16,
    pub note: String,
    /// When the worker last reported, when the server includes it.
    pub updated_ms: Option<u64>,
}

/// The complete recorded terminal result: the closed `outcome` word, which
/// attempt produced it, when it was recorded, and the worker's bounded raw
/// result/error document exactly as the server stored it.
#[derive(Clone, Debug, PartialEq)]
pub struct JobResultDto {
    pub outcome: String,
    pub attempt: u32,
    pub recorded_ms: u64,
    /// Raw bounded document (`body` field); [`Value::Null`] when absent.
    pub body: Value,
}

/// Everything `GET /v1/jobs/<id>` reports. `status` is the exact legacy
/// projection ([`JobStatusDto`]) so existing pollers can be handed
/// `detail.status` unchanged; the surrounding fields preserve what that
/// projection drops (enqueuer, attempts, progress freshness, full result).
#[derive(Clone, Debug, PartialEq)]
pub struct JobDetailDto {
    pub status: JobStatusDto,
    pub enqueued_by: Option<PrincipalDto>,
    /// Recorded attempts in strictly increasing attempt order.
    pub attempts: Vec<JobAttemptDto>,
    pub progress: Option<JobProgressDto>,
    pub result: Option<JobResultDto>,
}

impl JobDetailDto {
    pub fn job(&self) -> JobId {
        self.status.job
    }

    pub fn latest_attempt(&self) -> Option<&JobAttemptDto> {
        self.attempts.last()
    }
}

pub fn parse_job_detail(v: &Value) -> ClientResult<JobDetailDto> {
    let status = parse_job_status(v)?;
    let enqueued_by = opt_principal(v, "enqueued_by", "job enqueued_by")?;
    let attempts = match v.get("attempts") {
        None | Some(Value::Null) => Vec::new(),
        Some(rows) => {
            let rows = rows.as_arr().ok_or(ClientError::Protocol { what: "job attempts" })?;
            if rows.len() > MAX_PAGE_ENTRIES {
                return Err(ClientError::Protocol { what: "job attempts too large" });
            }
            let mut out = Vec::with_capacity(rows.len());
            let mut last = 0u64;
            for r in rows {
                let attempt = need_u64(r, "attempt", "job attempt number")?;
                // The server orders by attempt; anything else cannot be
                // rendered as a truthful history.
                if attempt == 0 || attempt > u32::MAX as u64 || attempt <= last {
                    return Err(ClientError::Protocol { what: "job attempt number" });
                }
                last = attempt;
                let started_ms = need_u64(r, "started_ms", "job attempt started_ms")?;
                let ended_ms = opt_u64(r, "ended_ms", "job attempt ended_ms")?;
                out.push(JobAttemptDto { attempt: attempt as u32, started_ms, ended_ms });
            }
            out
        }
    };
    let progress = match v.get("progress") {
        None | Some(Value::Null) => None,
        Some(p) => {
            let permille = need_u64(p, "permille", "job progress permille")?;
            if permille > 1000 {
                return Err(ClientError::Protocol { what: "job progress permille" });
            }
            let note = match p.get("note") {
                None | Some(Value::Null) => String::new(),
                Some(n) => {
                    let text = n.as_str().ok_or(ClientError::Protocol { what: "job note" })?;
                    sanitize_text(text, crate::wire::MAX_PROGRESS_NOTE_BYTES)
                }
            };
            let updated_ms = opt_u64(p, "updated_ms", "job progress updated_ms")?;
            Some(JobProgressDto { permille: permille as u16, note, updated_ms })
        }
    };
    let result = match v.get("result") {
        None | Some(Value::Null) => None,
        Some(r) => {
            let outcome_s = need_str(r, "outcome", 64, "job result outcome")?;
            check_display(outcome_s, "job result outcome")?;
            let attempt = need_u64(r, "attempt", "job result attempt")?;
            if attempt == 0 || attempt > u32::MAX as u64 {
                return Err(ClientError::Protocol { what: "job result attempt" });
            }
            let recorded_ms = need_u64(r, "recorded_ms", "job result recorded_ms")?;
            let body = r.get("body").cloned().unwrap_or(Value::Null);
            Some(JobResultDto {
                outcome: outcome_s.to_string(),
                attempt: attempt as u32,
                recorded_ms,
                body,
            })
        }
    };
    Ok(JobDetailDto { status, enqueued_by, attempts, progress, result })
}

/// One row of the scoped job listing (`GET /v1/jobs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRowDto {
    pub job: JobId,
    pub namespace: String,
    pub kind: String,
    pub state: JobStateDto,
    pub enqueued_by: Option<PrincipalDto>,
    pub created_ms: u64,
}

pub fn parse_jobs_page(v: &Value) -> ClientResult<Vec<JobRowDto>> {
    let rows = need(v, "jobs", "jobs rows")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "jobs rows" })?;
    if rows.len() > MAX_PAGE_ENTRIES {
        return Err(ClientError::Protocol { what: "jobs page too large" });
    }
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let job = JobId::parse(need_str(r, "job", 64, "job row id")?)
            .ok_or(ClientError::Protocol { what: "job row id" })?;
        let namespace =
            need_str(r, "namespace", MAX_NAMESPACE_BYTES, "job row namespace")?.to_string();
        check_display(&namespace, "job row namespace")?;
        let kind = need_str(r, "kind", 64, "job row kind")?.to_string();
        check_display(&kind, "job row kind")?;
        let state = JobStateDto::parse(need_str(r, "state", 16, "job row state")?)
            .ok_or(ClientError::Protocol { what: "job row state" })?;
        let enqueued_by = opt_principal(r, "enqueued_by", "job row enqueued_by")?;
        let created_ms = need_u64(r, "created_ms", "job row created_ms")?;
        out.push(JobRowDto { job, namespace, kind, state, enqueued_by, created_ms });
    }
    Ok(out)
}

/// One claimed job from `POST /v1/worker/claim` — everything a fleet
/// dispatcher needs to run it. `body` is the enqueuer's job document
/// (bounded by the transport's JSON caps before it ever reaches here).
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimedJobDto {
    pub job: JobId,
    pub kind: String,
    pub namespace: String,
    pub attempt: u32,
    pub lease_expires_ms: u64,
    pub body: Value,
}

/// `null` job = nothing claimable right now.
pub fn parse_claimed_job(v: &Value) -> ClientResult<Option<ClaimedJobDto>> {
    match v.get("job") {
        None => Err(ClientError::Protocol { what: "claim job field" }),
        Some(Value::Null) => Ok(None),
        Some(j) => {
            let job = j
                .as_str()
                .and_then(JobId::parse)
                .ok_or(ClientError::Protocol { what: "claim job id" })?;
            let kind = need_str(v, "kind", 64, "claim kind")?.to_string();
            check_display(&kind, "claim kind")?;
            let namespace =
                need_str(v, "namespace", MAX_NAMESPACE_BYTES, "claim namespace")?.to_string();
            check_display(&namespace, "claim namespace")?;
            let attempt = need_u64(v, "attempt", "claim attempt")?;
            if attempt == 0 || attempt > u32::MAX as u64 {
                return Err(ClientError::Protocol { what: "claim attempt" });
            }
            let lease_expires_ms = need_u64(v, "lease_expires_ms", "claim lease")?;
            let body = need(v, "body", "claim body")?.clone();
            if !matches!(body, Value::Obj(_)) {
                return Err(ClientError::Protocol { what: "claim body" });
            }
            Ok(Some(ClaimedJobDto {
                job,
                kind,
                namespace,
                attempt: attempt as u32,
                lease_expires_ms,
                body,
            }))
        }
    }
}

/// One advertised generation capability from `GET /v1/job-profiles`.
#[derive(Clone, Debug, PartialEq)]
pub struct JobProfileDto {
    pub id: String,
    pub domain: String,
    pub label: String,
    pub kind: String,
    pub namespace: String,
    /// Default job-body fields; callers merge their prompt on top.
    pub defaults: Value,
}

pub fn parse_job_profiles(v: &Value) -> ClientResult<Vec<JobProfileDto>> {
    let rows = need(v, "profiles", "profiles rows")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "profiles rows" })?;
    if rows.len() > crate::wire::MAX_JOB_PROFILES {
        return Err(ClientError::Protocol { what: "profiles too many" });
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id = need_str(row, "id", 64, "profile id")?.to_string();
        if !crate::wire::query_value_ok(&id) {
            return Err(ClientError::Protocol { what: "profile id" });
        }
        let domain = need_str(row, "domain", 32, "profile domain")?.to_string();
        check_display(&domain, "profile domain")?;
        let label = need_str(row, "label", 128, "profile label")?.to_string();
        check_display(&label, "profile label")?;
        let kind = need_str(row, "kind", 64, "profile kind")?.to_string();
        check_display(&kind, "profile kind")?;
        let namespace =
            need_str(row, "namespace", MAX_NAMESPACE_BYTES, "profile namespace")?.to_string();
        check_display(&namespace, "profile namespace")?;
        let defaults = match row.get("defaults") {
            Some(d @ Value::Obj(_)) => d.clone(),
            _ => return Err(ClientError::Protocol { what: "profile defaults" }),
        };
        out.push(JobProfileDto { id, domain, label, kind, namespace, defaults });
    }
    Ok(out)
}

// ---- alias resolution ------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasDto {
    pub alias: AssetAlias,
    pub asset_id: AssetId,
    pub head_revision: AssetRevisionId,
}

pub fn parse_alias(v: &Value) -> ClientResult<AliasDto> {
    let alias_s = need_str(v, "alias", 128, "alias value")?;
    let alias =
        AssetAlias::from_str(alias_s).map_err(|_| ClientError::Protocol { what: "alias value" })?;
    let asset_id = parse_asset_id(need_str(v, "asset_id", 64, "alias asset_id")?)?;
    let head_revision = parse_revision(need_str(v, "head_revision", 80, "alias head_revision")?)?;
    Ok(AliasDto { alias, asset_id, head_revision })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameAliasDto {
    pub alias: GameAlias,
    pub game_id: GameId,
    pub head_revision: GameRevisionId,
}

pub fn parse_game_alias(v: &Value) -> ClientResult<GameAliasDto> {
    let alias_s = need_str(v, "alias", 128, "game alias value")?;
    let alias = GameAlias::from_str(alias_s)
        .map_err(|_| ClientError::Protocol { what: "game alias value" })?;
    let game_id = GameId::from_str(need_str(v, "game_id", 64, "game alias game_id")?)
        .map_err(|_| ClientError::Protocol {
            what: "game alias game_id",
        })?;
    let head_revision = GameRevisionId::from_str(need_str(
        v,
        "head_revision",
        80,
        "game alias head_revision",
    )?)
    .map_err(|_| ClientError::Protocol {
        what: "game alias head_revision",
    })?;
    Ok(GameAliasDto {
        alias,
        game_id,
        head_revision,
    })
}

// ---- typed asset operations ------------------------------------------------

/// `op_<32 lowercase hex>`: the operation id exactly as the server spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId(pub [u8; 16]);

impl OperationId {
    pub fn parse(text: &str) -> Option<OperationId> {
        let hex = text.strip_prefix("op_")?;
        Some(OperationId(crate::util::from_hex_exact::<16>(hex)?))
    }
}

impl std::fmt::Display for OperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "op_{}", crate::util::to_hex(&self.0))
    }
}

/// Live display state of one operation, joining the executor job truthfully.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationStateDto {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl OperationStateDto {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// One pinned input exactly as the server resolved it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationInputDto {
    pub slot: String,
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub role: String,
    pub tier: String,
    pub lod: u8,
    pub media: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationProgressDto {
    pub permille: u16,
    pub note: String,
    pub updated_ms: u64,
}

/// Complete owner-visible operation state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationStatusDto {
    pub operation: OperationId,
    pub namespace: String,
    pub kind: String,
    pub state: OperationStateDto,
    pub round: u32,
    pub job: JobId,
    pub idempotency_key: String,
    /// Canonical spec digest, 64 lowercase hex.
    pub spec_digest: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub inputs: Vec<OperationInputDto>,
    pub error: Option<String>,
    pub result: Option<(AssetId, AssetRevisionId)>,
    pub progress: Option<OperationProgressDto>,
    /// Only on create responses: whether the idempotent replay joined an
    /// existing operation.
    pub joined: Option<bool>,
}

pub fn parse_operation_status(v: &Value) -> ClientResult<OperationStatusDto> {
    let operation = OperationId::parse(need_str(v, "operation", 64, "operation id")?)
        .ok_or(ClientError::Protocol { what: "operation id" })?;
    let namespace = need_str(v, "namespace", MAX_NAMESPACE_BYTES, "operation namespace")?.to_string();
    check_display(&namespace, "operation namespace")?;
    let kind = need_str(v, "kind", 64, "operation kind")?.to_string();
    check_display(&kind, "operation kind")?;
    let state = OperationStateDto::parse(need_str(v, "state", 16, "operation state")?)
        .ok_or(ClientError::Protocol { what: "operation state" })?;
    let round = need_u64(v, "round", "operation round")?;
    if round > u32::MAX as u64 {
        return Err(ClientError::Protocol { what: "operation round" });
    }
    let job = JobId::parse(need_str(v, "job", 64, "operation job")?)
        .ok_or(ClientError::Protocol { what: "operation job" })?;
    let idempotency_key =
        need_str(v, "idempotency_key", 128, "operation idempotency key")?.to_string();
    check_display(&idempotency_key, "operation idempotency key")?;
    let spec_digest = need_str(v, "spec_digest", 64, "operation spec digest")?.to_string();
    if spec_digest.len() != 64 || !spec_digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ClientError::Protocol { what: "operation spec digest" });
    }
    let created_ms = need_u64(v, "created_ms", "operation created_ms")?;
    let updated_ms = need_u64(v, "updated_ms", "operation updated_ms")?;
    let inputs_v = need(v, "inputs", "operation inputs")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "operation inputs" })?;
    if inputs_v.len() > 64 {
        return Err(ClientError::Protocol { what: "operation inputs" });
    }
    let mut inputs = Vec::with_capacity(inputs_v.len());
    for entry in inputs_v {
        let slot = need_str(entry, "slot", 64, "operation input slot")?.to_string();
        check_display(&slot, "operation input slot")?;
        let asset = parse_asset_id(need_str(entry, "asset", 64, "operation input asset")?)?;
        let revision =
            parse_revision(need_str(entry, "revision", 80, "operation input revision")?)?;
        let role = need_str(entry, "role", 32, "operation input role")?.to_string();
        check_display(&role, "operation input role")?;
        let tier = need_str(entry, "tier", 16, "operation input tier")?.to_string();
        check_display(&tier, "operation input tier")?;
        let lod = need_u64(entry, "lod", "operation input lod")?;
        if lod > u8::MAX as u64 {
            return Err(ClientError::Protocol { what: "operation input lod" });
        }
        let media = need_str(entry, "media", 16, "operation input media")?.to_string();
        check_display(&media, "operation input media")?;
        inputs.push(OperationInputDto {
            slot,
            asset,
            revision,
            role,
            tier,
            lod: lod as u8,
            media,
        });
    }
    let error = match v.get("error") {
        None | Some(Value::Null) => None,
        Some(e) => {
            let text = e
                .as_str()
                .ok_or(ClientError::Protocol { what: "operation error" })?;
            Some(sanitize_text(text, 512))
        }
    };
    let result = match v.get("result") {
        None | Some(Value::Null) => None,
        Some(r) => Some((
            parse_asset_id(need_str(r, "asset", 64, "operation result asset")?)?,
            parse_revision(need_str(r, "revision", 80, "operation result revision")?)?,
        )),
    };
    let progress = match v.get("progress") {
        None | Some(Value::Null) => None,
        Some(p) => {
            let permille = need_u64(p, "permille", "operation progress")?;
            if permille > 1000 {
                return Err(ClientError::Protocol { what: "operation progress" });
            }
            let note = need_str(p, "note", 256, "operation progress note")?;
            Some(OperationProgressDto {
                permille: permille as u16,
                note: sanitize_text(note, 256),
                updated_ms: opt_u64(p, "updated_ms", "operation progress updated")?.unwrap_or(0),
            })
        }
    };
    let joined = match v.get("joined") {
        None | Some(Value::Null) => None,
        Some(j) => Some(
            j.as_bool()
                .ok_or(ClientError::Protocol { what: "operation joined" })?,
        ),
    };
    Ok(OperationStatusDto {
        operation,
        namespace,
        kind,
        state,
        round: round as u32,
        job,
        idempotency_key,
        spec_digest,
        created_ms,
        updated_ms,
        inputs,
        error,
        result,
        progress,
        joined,
    })
}

/// One registered operation type with truthful availability.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationTypeDto {
    pub kind: String,
    pub revision: u32,
    pub label: String,
    pub description: String,
    pub supports_seed: bool,
    pub available: bool,
    pub unavailable_reason: Option<String>,
    /// The raw slot/param/output contract objects, verbatim: consumers that
    /// render or validate deeply read these; the closed fields above are the
    /// stable typed surface.
    pub inputs: Value,
    pub params: Value,
    pub outputs: Value,
}

pub fn parse_operation_types(v: &Value) -> ClientResult<Vec<OperationTypeDto>> {
    let rows = need(v, "types", "operation types")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "operation types" })?;
    if rows.len() > 256 {
        return Err(ClientError::Protocol { what: "operation types count" });
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let kind = need_str(row, "kind", 64, "operation type kind")?.to_string();
        check_display(&kind, "operation type kind")?;
        let revision = need_u64(row, "revision", "operation type revision")?;
        if revision > u32::MAX as u64 {
            return Err(ClientError::Protocol { what: "operation type revision" });
        }
        let label = need_str(row, "label", 128, "operation type label")?.to_string();
        check_display(&label, "operation type label")?;
        let description =
            need_str(row, "description", 512, "operation type description")?.to_string();
        check_display(&description, "operation type description")?;
        let supports_seed = need_bool(row, "supports_seed", "operation type seed")?;
        let available = need_bool(row, "available", "operation type available")?;
        let unavailable_reason = match row.get("unavailable_reason") {
            None | Some(Value::Null) => None,
            Some(r) => {
                let text = r
                    .as_str()
                    .ok_or(ClientError::Protocol { what: "operation type reason" })?;
                Some(sanitize_text(text, 256))
            }
        };
        out.push(OperationTypeDto {
            kind,
            revision: revision as u32,
            label,
            description,
            supports_seed,
            available,
            unavailable_reason,
            inputs: row.get("inputs").cloned().unwrap_or(Value::Arr(Vec::new())),
            params: row.get("params").cloned().unwrap_or(Value::Arr(Vec::new())),
            outputs: row.get("outputs").cloned().unwrap_or(Value::Arr(Vec::new())),
        });
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationEventDto {
    pub seq: u64,
    pub kind: String,
    pub detail: String,
    pub created_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationEventsPageDto {
    pub events: Vec<OperationEventDto>,
    /// Highest consumed sequence number; pass back as `after`.
    pub cursor: u64,
}

pub fn parse_operation_events(v: &Value) -> ClientResult<OperationEventsPageDto> {
    let rows = need(v, "events", "operation events")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "operation events" })?;
    if rows.len() > MAX_PAGE_ENTRIES {
        return Err(ClientError::Protocol { what: "operation events count" });
    }
    let mut events = Vec::with_capacity(rows.len());
    let mut last_seq = 0u64;
    for row in rows {
        let seq = need_u64(row, "seq", "operation event seq")?;
        if seq <= last_seq {
            return Err(ClientError::Protocol { what: "operation event order" });
        }
        last_seq = seq;
        let kind = need_str(row, "kind", 32, "operation event kind")?.to_string();
        check_display(&kind, "operation event kind")?;
        let detail = need_str(row, "detail", 256, "operation event detail")?;
        events.push(OperationEventDto {
            seq,
            kind,
            detail: sanitize_text(detail, 256),
            created_ms: need_u64(row, "created_ms", "operation event ts")?,
        });
    }
    let cursor = need_u64(v, "cursor", "operation events cursor")?;
    if let Some(last) = events.last() {
        if last.seq > cursor {
            return Err(ClientError::Protocol { what: "operation events cursor" });
        }
    }
    Ok(OperationEventsPageDto { events, cursor })
}

// ---- chat broker -----------------------------------------------------------

/// `chat_<16 lowercase hex>`: one broker-owned conversation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChatSessionId([u8; 8]);

impl ChatSessionId {
    pub fn parse(text: &str) -> Option<ChatSessionId> {
        let hex = text.strip_prefix("chat_")?;
        Some(ChatSessionId(from_hex_exact::<8>(hex)?))
    }
}

impl std::fmt::Display for ChatSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "chat_{}", crate::util::to_hex(&self.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatProviderKind {
    FleetQwen,
    OpenAi,
    Grok,
}

impl ChatProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FleetQwen => "fleet-qwen",
            Self::OpenAi => "openai",
            Self::Grok => "grok",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fleet-qwen" => Some(Self::FleetQwen),
            "openai" => Some(Self::OpenAi),
            "grok" => Some(Self::Grok),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatProviderStateDto {
    Available { model: String },
    Unavailable { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatProviderDto {
    pub kind: ChatProviderKind,
    pub state: ChatProviderStateDto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatSessionStateDto {
    Idle,
    Streaming,
    Sealed,
}

impl ChatSessionStateDto {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Streaming => "streaming",
            Self::Sealed => "sealed",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "idle" => Some(Self::Idle),
            "streaming" => Some(Self::Streaming),
            "sealed" => Some(Self::Sealed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatSessionDto {
    pub session: ChatSessionId,
    pub namespace: String,
    pub provider: ChatProviderKind,
    pub owner: String,
    pub state: ChatSessionStateDto,
    pub turn: u64,
    pub idle: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChatToolOutcomeDto {
    Ok { value: Value },
    Unavailable { reason: String },
    Denied { what: String },
    Refused { what: String },
    Failed { message: String },
}

/// PRESENTATION-ONLY facts a chat service may attach to a `delta` (see the
/// chat wire's `ServingFacts`). Optional and additive: a service that
/// predates it sends nothing and every field here stays `None`.
///
/// `gen_tokens` is cumulative WITHIN the current provider round and
/// restarts at 0 each round, so a consumer computing a rate must read a
/// decrease as a restart. The lane pair is the serving box's advertised
/// decode contention at probe time — stale by construction, and absent
/// entirely when the box advertises no lanes (which means one lane).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChatServingDto {
    pub gen_tokens: u32,
    pub lanes_active: Option<u32>,
    pub slots_total: Option<u32>,
    /// Tokens the prefix cache let this turn skip (absent on old services).
    pub prefix_ingested: Option<u32>,
    pub prefix_resumed: Option<bool>,
    /// Hidden reasoning tokens so far. `visible_tokens` stays absent while
    /// the think block is still open — that absence IS the "thinking" flag.
    pub think_tokens: Option<u32>,
    pub visible_tokens: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChatEventBodyDto {
    Delta { text: String, serving: Option<ChatServingDto> },
    ToolCall { id: String, name: String, args: Value },
    ToolProgress { id: String, permille: u16, note: String },
    ToolResult { id: String, outcome: ChatToolOutcomeDto },
    Done,
    Cancelled,
    Error { code: String, message: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatEventDto {
    pub seq: u64,
    pub body: ChatEventBodyDto,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatEventsPageDto {
    pub events: Vec<ChatEventDto>,
    pub cursor: u64,
}

const MAX_CHAT_MODEL: usize = 128;
const MAX_CHAT_REASON: usize = 256;
const MAX_CHAT_NOTE: usize = 200;
const MAX_CHAT_TOOL_ID: usize = 64;
const MAX_CHAT_TOOL_NAME: usize = 32;
const MAX_CHAT_TOOL_JSON: usize = 16 * 1024;
const MAX_CHAT_DELTA: usize = 4 * 1024;

pub fn parse_chat_providers(v: &Value) -> ClientResult<Vec<ChatProviderDto>> {
    let rows = need(v, "providers", "chat providers")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "chat providers" })?;
    if rows.len() > 8 {
        return Err(ClientError::Protocol { what: "chat providers count" });
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let kind = ChatProviderKind::parse(need_str(row, "kind", 32, "chat provider kind")?)
            .ok_or(ClientError::Protocol { what: "chat provider kind" })?;
        let state = match need_str(row, "state", 16, "chat provider state")? {
            "available" => {
                if row.get("detail").is_some() {
                    return Err(ClientError::Protocol { what: "chat provider detail" });
                }
                let model = need_str(row, "model", MAX_CHAT_MODEL, "chat provider model")?;
                check_display(model, "chat provider model")?;
                if looks_like_url(model) {
                    return Err(ClientError::Protocol { what: "chat provider model" });
                }
                ChatProviderStateDto::Available { model: model.to_string() }
            }
            "unavailable" => {
                let reason = need_str(row, "reason", MAX_CHAT_REASON, "chat provider reason")?;
                check_display(reason, "chat provider reason")?;
                if looks_like_url(reason) || looks_like_secret(reason) {
                    return Err(ClientError::Protocol { what: "chat provider reason" });
                }
                ChatProviderStateDto::Unavailable { reason: reason.to_string() }
            }
            _ => return Err(ClientError::Protocol { what: "chat provider state" }),
        };
        out.push(ChatProviderDto { kind, state });
    }
    Ok(out)
}

pub fn parse_chat_session(v: &Value) -> ClientResult<ChatSessionDto> {
    let session = ChatSessionId::parse(need_str(v, "session", 32, "chat session id")?)
        .ok_or(ClientError::Protocol { what: "chat session id" })?;
    let namespace = need_str(v, "namespace", MAX_NAMESPACE_BYTES, "chat namespace")?.to_string();
    check_display(&namespace, "chat namespace")?;
    let provider = ChatProviderKind::parse(need_str(v, "provider", 32, "chat provider")?)
        .ok_or(ClientError::Protocol { what: "chat provider" })?;
    let owner = need_str(v, "owner", 80, "chat owner")?.to_string();
    check_display(&owner, "chat owner")?;
    let state = ChatSessionStateDto::parse(need_str(v, "state", 16, "chat session state")?)
        .ok_or(ClientError::Protocol { what: "chat session state" })?;
    let turn = need_u64(v, "turn", "chat turn")?;
    let idle = need_bool(v, "idle", "chat idle")?;
    Ok(ChatSessionDto { session, namespace, provider, owner, state, turn, idle })
}

pub fn parse_chat_send(v: &Value) -> ClientResult<u64> {
    need_u64(v, "turn", "chat send turn")
}

pub fn parse_chat_retired(v: &Value) -> ClientResult<bool> {
    need_bool(v, "retired", "chat retired")
}

pub fn parse_chat_events(v: &Value) -> ClientResult<ChatEventsPageDto> {
    let rows = need(v, "events", "chat events")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "chat events" })?;
    if rows.len() > MAX_PAGE_ENTRIES {
        return Err(ClientError::Protocol { what: "chat events count" });
    }
    let mut events = Vec::with_capacity(rows.len());
    let mut prev: Option<u64> = None;
    for row in rows {
        let seq = need_u64(row, "seq", "chat event seq")?;
        if prev.is_some_and(|p| seq <= p) {
            return Err(ClientError::Protocol { what: "chat event order" });
        }
        prev = Some(seq);
        events.push(ChatEventDto { seq, body: parse_chat_event_body(row)? });
    }
    let cursor = need_u64(v, "cursor", "chat events cursor")?;
    if let Some(last) = events.last() {
        if last.seq > cursor {
            return Err(ClientError::Protocol { what: "chat events cursor" });
        }
    }
    Ok(ChatEventsPageDto { events, cursor })
}

/// Same ceilings the chat wire clamps to; implausible values are pinned,
/// never refused (see [`ChatServingDto`]).
fn parse_chat_serving(v: &Value) -> Option<ChatServingDto> {
    if !matches!(v, Value::Obj(_)) {
        return None;
    }
    let lane = |key: &str| v.get(key).and_then(Value::as_u64).map(|n| n.min(1024) as u32);
    Some(ChatServingDto {
        gen_tokens: v.get("gen_tokens").and_then(Value::as_u64)?.min(10_000_000) as u32,
        lanes_active: lane("lanes_active"),
        slots_total: lane("slots_total"),
        prefix_ingested: v.get("prefix_ingested").and_then(Value::as_u64).map(|n| n.min(10_000_000) as u32),
        prefix_resumed: v.get("prefix_resumed").and_then(Value::as_bool),
        think_tokens: v.get("think_tokens").and_then(Value::as_u64).map(|n| n.min(10_000_000) as u32),
        visible_tokens: v.get("visible_tokens").and_then(Value::as_u64).map(|n| n.min(10_000_000) as u32),
    })
}

fn parse_chat_event_body(v: &Value) -> ClientResult<ChatEventBodyDto> {
    match need_str(v, "type", 32, "chat event type")? {
        "delta" => {
            let text = need_str(v, "text", MAX_CHAT_DELTA, "chat delta")?.to_string();
            // Lenient on purpose: this block is a readout, and a garbled
            // counter must never take down a live turn that is otherwise
            // perfectly readable.
            let serving = v.get("serving").and_then(parse_chat_serving);
            Ok(ChatEventBodyDto::Delta { text, serving })
        }
        "tool_call" => {
            let id = need_str(v, "id", MAX_CHAT_TOOL_ID, "chat tool id")?.to_string();
            let name = need_str(v, "name", MAX_CHAT_TOOL_NAME, "chat tool name")?.to_string();
            let args = v.get("args").cloned().ok_or(ClientError::Protocol { what: "chat tool args" })?;
            if !matches!(args, Value::Obj(_)) || args.to_json().len() > MAX_CHAT_TOOL_JSON {
                return Err(ClientError::Protocol { what: "chat tool args" });
            }
            Ok(ChatEventBodyDto::ToolCall { id, name, args })
        }
        "tool_progress" => {
            let permille = need_u64(v, "permille", "chat permille")?;
            if permille > 1000 {
                return Err(ClientError::Protocol { what: "chat permille" });
            }
            let note = need_str(v, "note", MAX_CHAT_NOTE, "chat progress note")?.to_string();
            Ok(ChatEventBodyDto::ToolProgress {
                id: need_str(v, "id", MAX_CHAT_TOOL_ID, "chat tool id")?.to_string(),
                permille: permille as u16,
                note,
            })
        }
        "tool_result" => {
            let id = need_str(v, "id", MAX_CHAT_TOOL_ID, "chat tool id")?.to_string();
            let result = need(v, "result", "chat tool result")?;
            Ok(ChatEventBodyDto::ToolResult { id, outcome: parse_chat_outcome(result)? })
        }
        "done" => Ok(ChatEventBodyDto::Done),
        "cancelled" => Ok(ChatEventBodyDto::Cancelled),
        "error" => Ok(ChatEventBodyDto::Error {
            code: need_str(v, "code", 64, "chat error code")?.to_string(),
            message: need_str(v, "message", MAX_CHAT_REASON, "chat error message")?.to_string(),
        }),
        _ => Err(ClientError::Protocol { what: "chat event type" }),
    }
}

fn parse_chat_outcome(v: &Value) -> ClientResult<ChatToolOutcomeDto> {
    if v.to_json().len() > MAX_CHAT_TOOL_JSON {
        return Err(ClientError::Protocol { what: "chat tool result" });
    }
    match need_str(v, "outcome", 16, "chat tool outcome")? {
        "ok" => {
            let value = need(v, "value", "chat tool value")?.clone();
            if !matches!(value, Value::Obj(_)) {
                return Err(ClientError::Protocol { what: "chat tool value" });
            }
            Ok(ChatToolOutcomeDto::Ok { value })
        }
        "unavailable" => Ok(ChatToolOutcomeDto::Unavailable {
            reason: need_str(v, "reason", 512, "chat tool reason")?.to_string(),
        }),
        "denied" => Ok(ChatToolOutcomeDto::Denied {
            what: need_str(v, "what", 512, "chat tool denied")?.to_string(),
        }),
        "refused" => Ok(ChatToolOutcomeDto::Refused {
            what: need_str(v, "what", 512, "chat tool refused")?.to_string(),
        }),
        "failed" => Ok(ChatToolOutcomeDto::Failed {
            message: need_str(v, "message", 512, "chat tool failed")?.to_string(),
        }),
        _ => Err(ClientError::Protocol { what: "chat tool outcome" }),
    }
}

fn looks_like_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("http://") || lower.contains("https://") || lower.contains("://")
}

fn looks_like_secret(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("bearer ")
        || lower.contains("authorization")
        || lower.contains("sk-")
        || lower.contains("xai-")
        || lower.contains("mpat_")
}

// ---- import / derived-variant projections ----------------------------------

/// Longest digest-ID display spelling (`prefix_` + 64 hex).
const MAX_DIGEST_ID_BYTES: usize = 80;

/// One row of `GET /v1/import-sources`. This is a browse projection, not the
/// canonical [`makepad_asset_data::SourceCollection`] document — there is
/// no get-by-id route for those bytes. One explicit page is capped at
/// [`crate::wire::MAX_SOURCE_PAGE_LIMIT`]; an aggregated list fail-closes
/// above [`crate::wire::MAX_PAGE_ENTRIES`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCollectionRowDto {
    pub source_id: String,
    pub title: String,
    pub license: String,
    pub credits: String,
    pub digest: SourceCollectionId,
}

/// One explicit page: `{sources, cursor}` where `cursor` is the last
/// `source_id` iff more rows exist, else null.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCollectionsPageDto {
    pub sources: Vec<SourceCollectionRowDto>,
    pub cursor: Option<String>,
}

/// One imported pack entry as the import routes project it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportEntryDto {
    pub key: PackEntryKey,
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
    pub alias: Option<AssetAlias>,
}

/// `POST /v1/imports` report. `created` is false on an exact-byte replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportReportDto {
    pub import_revision: ImportRevisionId,
    pub created: bool,
    pub entries: Vec<ImportEntryDto>,
}

/// `GET /v1/imports/{irev}` projection. The server does not serve the
/// canonical import-manifest bytes on this route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportStatusDto {
    pub import_revision: ImportRevisionId,
    pub source_id: String,
    pub pack_name: String,
    pub pack_version: String,
    pub license: String,
    pub credits: String,
    pub entries: Vec<ImportEntryDto>,
}

/// `POST /v1/variant-resolutions` envelope before the digest is checked
/// against the reconstructed canonical [`makepad_asset_data::ResolvedVariantMap`].
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedVariantMapDto {
    pub digest: ResolvedMapDigest,
    pub set: VariantSetId,
    pub profile: ClientProfileDigest,
    pub entries: Vec<ResolvedEntry>,
}

fn parse_digest_id<T: FromStr>(s: &str, what: &'static str) -> ClientResult<T> {
    T::from_str(s).map_err(|_| ClientError::Protocol { what })
}

fn check_source_id(s: &str, what: &'static str) -> ClientResult<()> {
    if s.is_empty() || s.len() > makepad_asset_data::limits::MAX_ALIAS_SEGMENT_BYTES {
        return Err(ClientError::Protocol { what });
    }
    let bytes = s.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err(ClientError::Protocol { what });
    }
    for &c in bytes {
        match c {
            b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => {}
            _ => return Err(ClientError::Protocol { what }),
        }
    }
    Ok(())
}

fn check_pack_version(s: &str, what: &'static str) -> ClientResult<()> {
    if s.is_empty() || s.len() > makepad_asset_data::limits::MAX_PACK_VERSION_BYTES {
        return Err(ClientError::Protocol { what });
    }
    let bytes = s.as_bytes();
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        return Err(ClientError::Protocol { what });
    }
    for &c in bytes {
        match c {
            b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' => {}
            _ => return Err(ClientError::Protocol { what }),
        }
    }
    Ok(())
}

fn parse_import_entry(v: &Value) -> ClientResult<ImportEntryDto> {
    let key_s = need_str(v, "key", 128, "import entry key")?;
    let key = PackEntryKey::from_str(key_s)
        .map_err(|_| ClientError::Protocol { what: "import entry key" })?;
    let asset_id = parse_asset_id(need_str(v, "asset_id", 64, "import entry asset_id")?)?;
    let revision = parse_revision(need_str(v, "revision", MAX_DIGEST_ID_BYTES, "import entry revision")?)?;
    let alias = match v.get("alias") {
        None | Some(Value::Null) => None,
        Some(a) => {
            let text = a
                .as_str()
                .ok_or(ClientError::Protocol { what: "import entry alias" })?;
            if text.len() > 128 {
                return Err(ClientError::Protocol { what: "import entry alias" });
            }
            Some(
                AssetAlias::from_str(text)
                    .map_err(|_| ClientError::Protocol { what: "import entry alias" })?,
            )
        }
    };
    Ok(ImportEntryDto { key, asset_id, revision, alias })
}

fn parse_import_entries(v: &Value) -> ClientResult<Vec<ImportEntryDto>> {
    let rows = need(v, "entries", "import entries")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "import entries" })?;
    if rows.len() > makepad_asset_data::limits::MAX_IMPORT_ASSETS {
        return Err(ClientError::Protocol { what: "import entries too large" });
    }
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        entries.push(parse_import_entry(row)?);
    }
    Ok(entries)
}

fn parse_source_collection_row(row: &Value) -> ClientResult<SourceCollectionRowDto> {
    let source_id = need_str(
        row,
        "source_id",
        makepad_asset_data::limits::MAX_ALIAS_SEGMENT_BYTES,
        "source_id",
    )?
    .to_string();
    check_source_id(&source_id, "source_id")?;
    let title = need_str(
        row,
        "title",
        makepad_asset_data::limits::MAX_NAME_BYTES * 2,
        "source title",
    )?
    .to_string();
    if title.is_empty() {
        return Err(ClientError::Protocol { what: "source title" });
    }
    check_display(&title, "source title")?;
    let license = need_str(
        row,
        "license",
        makepad_asset_data::limits::MAX_LICENSE_BYTES,
        "source license",
    )?
    .to_string();
    check_display(&license, "source license")?;
    if license.is_empty() {
        return Err(ClientError::Protocol { what: "source license" });
    }
    let credits = need_str(
        row,
        "credits",
        makepad_asset_data::limits::MAX_STRING_BYTES,
        "source credits",
    )?
    .to_string();
    check_display(&credits, "source credits")?;
    let digest = parse_digest_id::<SourceCollectionId>(
        need_str(row, "digest", MAX_DIGEST_ID_BYTES, "source digest")?,
        "source digest",
    )?;
    Ok(SourceCollectionRowDto {
        source_id,
        title,
        license,
        credits,
        digest,
    })
}

fn parse_source_collection_rows(
    v: &Value,
    max: usize,
) -> ClientResult<Vec<SourceCollectionRowDto>> {
    let rows = need(v, "sources", "source collections")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "source collections" })?;
    if rows.len() > max {
        return Err(ClientError::Protocol { what: "source collections too large" });
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let parsed = parse_source_collection_row(row)?;
        if out
            .last()
            .is_some_and(|prev: &SourceCollectionRowDto| parsed.source_id <= prev.source_id)
        {
            return Err(ClientError::Protocol { what: "source collection order" });
        }
        out.push(parsed);
    }
    Ok(out)
}

pub fn parse_source_collections(v: &Value) -> ClientResult<Vec<SourceCollectionRowDto>> {
    parse_source_collection_rows(v, MAX_PAGE_ENTRIES)
}

pub fn parse_source_collections_page(v: &Value) -> ClientResult<SourceCollectionsPageDto> {
    let sources = parse_source_collection_rows(v, MAX_SOURCE_PAGE_LIMIT as usize)?;
    let cursor = match v.get("cursor") {
        None => return Err(ClientError::Protocol { what: "source page cursor" }),
        Some(Value::Null) => None,
        Some(c) => {
            let s = c
                .as_str()
                .ok_or(ClientError::Protocol { what: "source page cursor" })?;
            if !crate::wire::source_cursor_ok(s) {
                return Err(ClientError::Protocol { what: "source page cursor" });
            }
            Some(s.to_string())
        }
    };
    match (&cursor, sources.last()) {
        (None, _) => {}
        (Some(_), None) => {
            return Err(ClientError::Protocol {
                what: "source page cursor on empty page",
            })
        }
        (Some(cur), Some(last)) if cur == &last.source_id => {}
        (Some(_), Some(_)) => {
            return Err(ClientError::Protocol {
                what: "source page cursor mismatch",
            })
        }
    }
    Ok(SourceCollectionsPageDto { sources, cursor })
}

pub fn parse_import_report(v: &Value) -> ClientResult<ImportReportDto> {
    let import_revision = parse_digest_id::<ImportRevisionId>(
        need_str(v, "import_revision", MAX_DIGEST_ID_BYTES, "import_revision")?,
        "import_revision",
    )?;
    let created = need_bool(v, "created", "import created")?;
    let entries = parse_import_entries(v)?;
    Ok(ImportReportDto { import_revision, created, entries })
}

pub fn parse_import_status(v: &Value) -> ClientResult<ImportStatusDto> {
    let import_revision = parse_digest_id::<ImportRevisionId>(
        need_str(v, "import_revision", MAX_DIGEST_ID_BYTES, "import_revision")?,
        "import_revision",
    )?;
    let source_id = need_str(
        v,
        "source_id",
        makepad_asset_data::limits::MAX_ALIAS_SEGMENT_BYTES,
        "import source_id",
    )?
    .to_string();
    check_source_id(&source_id, "import source_id")?;
    let pack_name = need_str(
        v,
        "pack_name",
        makepad_asset_data::limits::MAX_ALIAS_SEGMENT_BYTES,
        "import pack_name",
    )?
    .to_string();
    check_source_id(&pack_name, "import pack_name")?;
    let pack_version = need_str(
        v,
        "pack_version",
        makepad_asset_data::limits::MAX_PACK_VERSION_BYTES,
        "import pack_version",
    )?
    .to_string();
    check_pack_version(&pack_version, "import pack_version")?;
    let license = need_str(
        v,
        "license",
        makepad_asset_data::limits::MAX_LICENSE_BYTES,
        "import license",
    )?
    .to_string();
    check_display(&license, "import license")?;
    if license.is_empty() {
        return Err(ClientError::Protocol { what: "import license" });
    }
    let credits = need_str(
        v,
        "credits",
        makepad_asset_data::limits::MAX_STRING_BYTES,
        "import credits",
    )?
    .to_string();
    check_display(&credits, "import credits")?;
    let entries = parse_import_entries(v)?;
    Ok(ImportStatusDto {
        import_revision,
        source_id,
        pack_name,
        pack_version,
        license,
        credits,
        entries,
    })
}

pub fn parse_resolved_variant_map(v: &Value) -> ClientResult<ResolvedVariantMapDto> {
    let digest = parse_digest_id::<ResolvedMapDigest>(
        need_str(v, "digest", MAX_DIGEST_ID_BYTES, "resolution digest")?,
        "resolution digest",
    )?;
    let set = parse_digest_id::<VariantSetId>(
        need_str(v, "variant_set", MAX_DIGEST_ID_BYTES, "resolution variant_set")?,
        "resolution variant_set",
    )?;
    let profile = parse_digest_id::<ClientProfileDigest>(
        need_str(v, "profile", MAX_DIGEST_ID_BYTES, "resolution profile")?,
        "resolution profile",
    )?;
    let rows = need(v, "entries", "resolution entries")?
        .as_arr()
        .ok_or(ClientError::Protocol { what: "resolution entries" })?;
    if rows.len() > makepad_asset_data::limits::MAX_RESOLVED_ENTRIES {
        return Err(ClientError::Protocol { what: "resolution entries too large" });
    }
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let role_s = need_str(row, "role", 32, "resolution role")?;
        let role = variant_role_parse(role_s)
            .ok_or(ClientError::Protocol { what: "resolution role" })?;
        let variant = parse_digest_id::<DerivedVariantId>(
            need_str(row, "variant", MAX_DIGEST_ID_BYTES, "resolution variant")?,
            "resolution variant",
        )?;
        let blobs_v = need(row, "blobs", "resolution blobs")?
            .as_arr()
            .ok_or(ClientError::Protocol { what: "resolution blobs" })?;
        if blobs_v.is_empty()
            || blobs_v.len() > makepad_asset_data::limits::MAX_DERIVED_OUTPUTS + 1
        {
            return Err(ClientError::Protocol { what: "resolution blobs" });
        }
        let mut blobs = Vec::with_capacity(blobs_v.len());
        for b in blobs_v {
            let text = b
                .as_str()
                .ok_or(ClientError::Protocol { what: "resolution blob" })?;
            blobs.push(
                BlobId::from_str(text)
                    .map_err(|_| ClientError::Protocol { what: "resolution blob" })?,
            );
        }
        entries.push(ResolvedEntry { role, variant, blobs });
    }
    Ok(ResolvedVariantMapDto { digest, set, profile, entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    fn asset_id_str() -> String {
        AssetId::from_bytes([7u8; 16]).to_string()
    }

    fn rev_str() -> String {
        AssetRevisionId::from_bytes([9u8; 32]).to_string()
    }

    #[test]
    fn chat_providers_and_session_parse_fail_closed() {
        let providers = parse_chat_providers(
            &json::parse(
                br#"{"providers":[
                    {"kind":"fleet-qwen","state":"available","model":"qwen-scripted"},
                    {"kind":"openai","state":"unavailable","reason":"OPENAI_API_KEY is not set"}
                ]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].kind, ChatProviderKind::FleetQwen);
        assert!(parse_chat_providers(
            &json::parse(br#"{"providers":[{"kind":"claude","state":"available","model":"x"}]}"#)
                .unwrap(),
        )
        .is_err());
        assert!(parse_chat_providers(
            &json::parse(
                br#"{"providers":[{"kind":"openai","state":"available","model":"x","detail":"https://api.openai.com"}]}"#
            )
            .unwrap(),
        )
        .is_err());
        let sid = "chat_0123456789abcdef";
        let session = parse_chat_session(
            &json::parse(
                format!(
                    r#"{{"session":"{sid}","namespace":"gen","provider":"openai","owner":"prin_aa","state":"idle","turn":1,"idle":true}}"#
                )
                .as_bytes(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(session.session.to_string(), sid);
        assert_eq!(session.provider, ChatProviderKind::OpenAi);
        assert!(parse_chat_session(
            &json::parse(
                br#"{"session":"op_0123456789abcdef0123456789abcdef","namespace":"gen","provider":"openai","owner":"p","state":"idle","turn":0,"idle":true}"#
            )
            .unwrap(),
        )
        .is_err());
        let page = parse_chat_events(
            &json::parse(
                br#"{"events":[{"seq":0,"type":"delta","text":"hi"},{"seq":1,"type":"done"}],"cursor":1}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page.events.len(), 2);
        // A delta from a service that predates the serving block parses
        // exactly as before, with nothing invented.
        assert_eq!(
            page.events[0].body,
            ChatEventBodyDto::Delta { text: "hi".into(), serving: None }
        );
        // With the block, the counters come through; a broken block costs
        // the readout, never the delta.
        let page = parse_chat_events(
            &json::parse(
                br#"{"events":[
                    {"seq":0,"type":"delta","text":"a","serving":{"gen_tokens":40,"lanes_active":2,"slots_total":4}},
                    {"seq":1,"type":"delta","text":"b","serving":{"lanes_active":2}},
                    {"seq":2,"type":"delta","text":"c","serving":7}
                ],"cursor":2}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            page.events[0].body,
            ChatEventBodyDto::Delta {
                text: "a".into(),
                serving: Some(ChatServingDto {
                    gen_tokens: 40,
                    lanes_active: Some(2),
                    slots_total: Some(4),
                }),
            }
        );
        assert_eq!(
            page.events[1].body,
            ChatEventBodyDto::Delta { text: "b".into(), serving: None },
            "a block with no token count is no count at all"
        );
        assert_eq!(
            page.events[2].body,
            ChatEventBodyDto::Delta { text: "c".into(), serving: None }
        );
        assert!(parse_chat_events(
            &json::parse(br#"{"events":[{"seq":0,"type":"explode"}],"cursor":0}"#).unwrap(),
        )
        .is_err());
    }

    #[test]
    fn health_roundtrip_and_refusals() {
        let good = format!(r#"{{"server_id":"{}","protocol_version":1}}"#, "ab".repeat(16));
        let h = parse_health(&json::parse(good.as_bytes()).unwrap()).unwrap();
        assert_eq!(h.server_id, [0xab; 16]);
        assert_eq!(h.protocol_version, 1);
        for bad in [
            r#"{"protocol_version":1}"#.to_string(),
            format!(r#"{{"server_id":"{}","protocol_version":0}}"#, "ab".repeat(16)),
            format!(r#"{{"server_id":"{}","protocol_version":65536}}"#, "ab".repeat(16)),
            format!(r#"{{"server_id":"{}","protocol_version":1}}"#, "AB".repeat(16)),
            format!(r#"{{"server_id":"{}","protocol_version":1}}"#, "ab".repeat(15)),
        ] {
            assert!(parse_health(&json::parse(bad.as_bytes()).unwrap()).is_err(), "{bad}");
        }
    }

    #[test]
    fn catalog_page_parses_and_bounds() {
        let body = format!(
            r#"{{"hits":[{{"asset_id":"{}","namespace":"stock","kind":"mesh","title":"Rocket","snippet":"a rocket","score":90,"live":true}}],"total":1,"cursor":null}}"#,
            asset_id_str()
        );
        let page = parse_catalog_page(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.total, 1);
        assert!(page.cursor.is_none());
        assert_eq!(page.hits[0].kind, Some(AssetKind::Mesh));

        // Unknown kind refused; missing fields refused; control chars refused.
        let bad_kind = body.replace("\"mesh\"", "\"blob\"");
        assert!(parse_catalog_page(&json::parse(bad_kind.as_bytes()).unwrap()).is_err());
        let bad_title = body.replace("Rocket", "Ro\\u0007cket");
        assert!(parse_catalog_page(&json::parse(bad_title.as_bytes()).unwrap()).is_err());
        let no_total = body.replace(",\"total\":1", "");
        assert!(parse_catalog_page(&json::parse(no_total.as_bytes()).unwrap()).is_err());
    }

    /// Facets are optional on the wire — a server that does not count them
    /// (or a query that did not ask) leaves the field out, and that is not
    /// an error. What IS present is typed strictly.
    #[test]
    fn facet_shapes() {
        let with = |f: &str| format!(r#"{{"hits":[],"total":0,"cursor":null,"facets":{f}}}"#);
        let none = r#"{"hits":[],"total":0,"cursor":null}"#;
        assert!(parse_catalog_page(&json::parse(none.as_bytes()).unwrap())
            .unwrap()
            .facets
            .is_empty());
        assert!(parse_catalog_page(&json::parse(with("null").as_bytes()).unwrap())
            .unwrap()
            .facets
            .is_empty());
        let good = r#"[{"kind":"category","label":"doom","count":12},
                       {"kind":"tag","label":"prop","count":3}]"#;
        let page =
            parse_catalog_page(&json::parse(with(good).as_bytes()).unwrap()).unwrap();
        assert_eq!(
            page.facets,
            vec![
                CatalogFacet { kind: FacetKind::Category, label: "doom".into(), count: 12 },
                CatalogFacet { kind: FacetKind::Tag, label: "prop".into(), count: 3 },
            ]
        );
        for bad in [
            r#"[{"kind":"colour","label":"doom","count":1}]"#,
            r#"[{"label":"doom","count":1}]"#,
            r#"[{"kind":"tag","count":1}]"#,
            r#"[{"kind":"tag","label":"doom"}]"#,
            r#"7"#,
        ] {
            assert!(
                parse_catalog_page(&json::parse(with(bad).as_bytes()).unwrap()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn cursor_shapes() {
        let with = |c: &str| {
            format!(r#"{{"hits":[],"total":0,"cursor":{c}}}"#)
        };
        assert!(parse_catalog_page(&json::parse(with("null").as_bytes()).unwrap())
            .unwrap()
            .cursor
            .is_none());
        assert_eq!(
            parse_catalog_page(&json::parse(with("\"ab12\"").as_bytes()).unwrap())
                .unwrap()
                .cursor
                .as_deref(),
            Some("ab12")
        );
        for bad in ["\"\"", "\"a b\"", "\"a&b\"", "7", "\"a/b\""] {
            assert!(
                parse_catalog_page(&json::parse(with(bad).as_bytes()).unwrap()).is_err(),
                "{bad}"
            );
        }
        let huge = format!("\"{}\"", "a".repeat(MAX_CURSOR_BYTES + 1));
        assert!(parse_catalog_page(&json::parse(with(&huge).as_bytes()).unwrap()).is_err());
    }

    #[test]
    fn asset_detail_states() {
        let body = format!(
            r#"{{"asset_id":"{}","namespace":"stock","candidates":[
                {{"revision":"{}","state":"published","staged_ms":5,"published_ms":9,"quarantined_ms":null}}
            ]}}"#,
            asset_id_str(),
            rev_str()
        );
        let d = parse_asset_detail(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(d.candidates.len(), 1);
        assert!(d.latest_published().is_some());

        let unknown_state = body.replace("published\"", "haunted\"");
        assert!(parse_asset_detail(&json::parse(unknown_state.as_bytes()).unwrap()).is_err());
        // Published without a timestamp refused.
        let no_ts = body.replace(",\"published_ms\":9", ",\"published_ms\":null");
        assert!(parse_asset_detail(&json::parse(no_ts.as_bytes()).unwrap()).is_err());
    }

    #[test]
    fn alias_parses_strictly() {
        let body = format!(
            r#"{{"alias":"stock/rocket","asset_id":"{}","head_revision":"{}"}}"#,
            asset_id_str(),
            rev_str()
        );
        let a = parse_alias(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(a.alias.as_str(), "stock/rocket");
        // Single-segment alias violates the contract's MIN_ALIAS_SEGMENTS.
        let bad = body.replace("stock/rocket", "rocket");
        assert!(parse_alias(&json::parse(bad.as_bytes()).unwrap()).is_err());
        // Uppercase revision spelling refused by the strict ID parser.
        let bad_rev = body.replace("arev_", "AREV_");
        assert!(parse_alias(&json::parse(bad_rev.as_bytes()).unwrap()).is_err());
    }

    #[test]
    fn game_alias_parses_strictly() {
        let game = GameId::from_bytes([0x33; 16]);
        let revision = GameRevisionId::from_bytes([0x44; 32]);
        let body = format!(
            r#"{{"alias":"stock/games/arena","game_id":"{game}","head_revision":"{revision}"}}"#
        );
        let alias = parse_game_alias(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(alias.alias.as_str(), "stock/games/arena");
        assert_eq!(alias.game_id, game);
        assert_eq!(alias.head_revision, revision);
        assert!(parse_game_alias(
            &json::parse(body.replace("stock/games/arena", "arena").as_bytes()).unwrap()
        )
        .is_err());
        assert!(parse_game_alias(
            &json::parse(body.replace("grev_", "GREV_").as_bytes()).unwrap()
        )
        .is_err());
    }

    #[test]
    fn event_page_is_monotonic_bounded_and_cursor_covered() {
        let body = format!(
            r#"{{"events":[{{"seq":7,"kind":"asset_published","ns":"stock","asset_id":"{}","revision":"{}","game_id":null,"game_revision":null,"alias":"stock/clip","content_kind":"video","ts_ms":99}}],"cursor":"0123456789abcdef-7","gap":false}}"#,
            asset_id_str(),
            rev_str()
        );
        let page = parse_events_page(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].kind, CatalogEventKind::AssetPublished);
        assert_eq!(page.events[0].content_kind, Some(AssetKind::Video));

        // Build an explicit duplicate because whitespace changes should not
        // be part of the parser contract.
        let row = format!(
            r#"{{"seq":7,"kind":"asset_published","ns":"stock","asset_id":"{}","revision":"{}","game_id":null,"game_revision":null,"alias":null,"content_kind":"video","ts_ms":99}}"#,
            asset_id_str(),
            rev_str()
        );
        let duplicate = format!(
            r#"{{"events":[{row},{row}],"cursor":"0123456789abcdef-7","gap":false}}"#
        );
        assert!(parse_events_page(&json::parse(duplicate.as_bytes()).unwrap()).is_err());
        let behind = body.replace("abcdef-7", "abcdef-6");
        assert!(parse_events_page(&json::parse(behind.as_bytes()).unwrap()).is_err());
        let malformed = body.replace("0123456789abcdef-7", "safe-but-not-an-event-cursor");
        assert!(parse_events_page(&json::parse(malformed.as_bytes()).unwrap()).is_err());
    }

    #[test]
    fn job_id_and_status_parse_strictly_result_leniently() {
        let id = format!("job_{}", "ab".repeat(16));
        assert!(JobId::parse(&id).is_some());
        for bad in ["job_short", "JOB_", "prin_", &id.to_uppercase()] {
            assert!(JobId::parse(bad).is_none(), "{bad}");
        }
        let body = format!(
            r#"{{"job":"{id}","namespace":"gen","kind":"video.generate","state":"running",
                "created_ms":9,"progress":{{"permille":420,"note":"denoising"}}}}"#
        );
        let s = parse_job_status(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(s.state, JobStateDto::Running);
        assert!(!s.state.is_terminal());
        assert_eq!(s.progress, Some((420, "denoising".to_string())));
        assert!(s.result_asset.is_none());

        // Unknown state refused; over-range permille refused.
        let bad_state = body.replace("running", "haunted");
        assert!(parse_job_status(&json::parse(bad_state.as_bytes()).unwrap()).is_err());
        let bad_permille = body.replace("420", "1401");
        assert!(parse_job_status(&json::parse(bad_permille.as_bytes()).unwrap()).is_err());

        // Result asset ids parse leniently: valid ids surface, junk is None.
        let done = format!(
            r#"{{"job":"{id}","namespace":"gen","kind":"video.generate","state":"succeeded",
                "created_ms":9,"result":{{"outcome":"succeeded","attempt":1,"recorded_ms":10,
                "body":{{"asset_id":"{}","revision":"junk"}}}}}}"#,
            asset_id_str()
        );
        let s = parse_job_status(&json::parse(done.as_bytes()).unwrap()).unwrap();
        assert!(s.state.is_terminal());
        assert_eq!(s.outcome.as_deref(), Some("succeeded"));
        assert!(s.result_asset.is_some());
        assert!(s.result_revision.is_none());
    }

    #[test]
    fn job_detail_preserves_everything_status_drops() {
        let id = format!("job_{}", "ab".repeat(16));
        let prin = format!("prin_{}", "cd".repeat(16));
        let body = format!(
            r#"{{"job":"{id}","namespace":"gen","kind":"mesh.derive","state":"succeeded",
                "enqueued_by":"{prin}","created_ms":5,
                "attempts":[
                    {{"attempt":1,"started_ms":10,"ended_ms":20}},
                    {{"attempt":2,"started_ms":30,"ended_ms":null}}],
                "progress":{{"permille":900,"note":"publish:staging","updated_ms":31}},
                "result":{{"outcome":"succeeded","attempt":2,"recorded_ms":40,
                    "body":{{"asset_id":"{}","revision":"{}"}}}}}}"#,
            asset_id_str(),
            rev_str()
        );
        let d = parse_job_detail(&json::parse(body.as_bytes()).unwrap()).unwrap();
        // The embedded legacy projection matches parse_job_status exactly.
        let s = parse_job_status(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(d.status, s);
        assert_eq!(d.job(), s.job);
        let by = d.enqueued_by.expect("enqueuer preserved");
        assert_eq!(by.to_string(), prin);
        assert_eq!(d.attempts.len(), 2);
        assert_eq!(d.attempts[0], JobAttemptDto { attempt: 1, started_ms: 10, ended_ms: Some(20) });
        assert_eq!(d.latest_attempt().unwrap().ended_ms, None);
        let p = d.progress.as_ref().expect("typed progress");
        assert_eq!((p.permille, p.updated_ms), (900, Some(31)));
        assert_eq!(p.note, "publish:staging");
        let r = d.result.as_ref().expect("typed result");
        assert_eq!((r.outcome.as_str(), r.attempt, r.recorded_ms), ("succeeded", 2, 40));
        assert_eq!(
            r.body.get("asset_id").and_then(Value::as_str),
            Some(asset_id_str().as_str()),
            "raw result body preserved verbatim"
        );
        // The lenient legacy conveniences still surface from the same parse.
        assert!(d.status.result_asset.is_some());
        assert!(d.status.result_revision.is_some());

        // Absent extension fields stay tolerant (older server shape).
        let bare = format!(
            r#"{{"job":"{id}","namespace":"gen","kind":"mesh.derive","state":"pending","created_ms":5}}"#
        );
        let d = parse_job_detail(&json::parse(bare.as_bytes()).unwrap()).unwrap();
        assert!(d.enqueued_by.is_none());
        assert!(d.attempts.is_empty());
        assert!(d.progress.is_none() && d.result.is_none());

        // Out-of-shape extensions refuse the whole response.
        for (bad, sub) in [
            (body.replace(&prin, "prin_short"), "principal"),
            (body.replace(r#""attempt":2,"started_ms":30"#, r#""attempt":1,"started_ms":30"#), "attempt order"),
            (body.replace(r#""attempt":1,"started_ms":10"#, r#""attempt":0,"started_ms":10"#), "attempt zero"),
            (body.replace(r#""recorded_ms":40"#, r#""recorded_ms":-1"#), "recorded_ms"),
            (body.replace("\"outcome\":\"succeeded\"", "\"outcome\":\"a\\u0007b\""), "outcome control chars"),
        ] {
            assert!(parse_job_detail(&json::parse(bad.as_bytes()).unwrap()).is_err(), "{sub}");
        }
    }

    #[test]
    fn jobs_page_parses_and_refuses_out_of_shape() {
        let id = format!("job_{}", "ab".repeat(16));
        let prin = format!("prin_{}", "cd".repeat(16));
        let body = format!(
            r#"{{"jobs":[{{"job":"{id}","namespace":"gen","kind":"video.generate",
                "state":"running","enqueued_by":"{prin}","created_ms":7}}]}}"#
        );
        let rows = parse_jobs_page(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, JobStateDto::Running);
        assert_eq!(rows[0].enqueued_by.unwrap().to_string(), prin);
        assert_eq!(rows[0].kind, "video.generate");

        // Null enqueuer tolerated; unknown state refused; missing rows refused.
        let anon = body.replace(&format!("\"{prin}\""), "null");
        assert!(parse_jobs_page(&json::parse(anon.as_bytes()).unwrap()).unwrap()[0]
            .enqueued_by
            .is_none());
        let bad_state = body.replace("running", "haunted");
        assert!(parse_jobs_page(&json::parse(bad_state.as_bytes()).unwrap()).is_err());
        assert!(parse_jobs_page(&json::parse(br#"{"count":1}"#).unwrap()).is_err());
    }

    #[test]
    fn job_profiles_parse_and_refuse_out_of_shape() {
        let body = r#"{"profiles":[{"id":"h3-standard","domain":"video",
            "label":"MiniMax H3","kind":"video.generate","namespace":"gen",
            "defaults":{"model":"h3","width":640}}]}"#;
        let profiles = parse_job_profiles(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, "h3-standard");
        assert_eq!(
            profiles[0].defaults.get("width").and_then(Value::as_u64),
            Some(640)
        );
        // Charset-hostile id refused; non-object defaults refused.
        let bad_id = body.replace("h3-standard", "h3 standard");
        assert!(parse_job_profiles(&json::parse(bad_id.as_bytes()).unwrap()).is_err());
        let bad_defaults = body.replace(r#"{"model":"h3","width":640}"#, "7");
        assert!(parse_job_profiles(&json::parse(bad_defaults.as_bytes()).unwrap()).is_err());
    }

    #[test]
    fn error_detail_is_bounded_and_sanitized() {
        assert_eq!(
            parse_error_detail(br#"{"error":"not found"}"#).as_deref(),
            Some("not found")
        );
        assert_eq!(parse_error_detail(b"garbage"), None);
        assert_eq!(parse_error_detail(br#"{"error":""}"#), None);
        // The server sends the category AND the reason; a client that
        // reports only the category says nothing at all.
        assert_eq!(
            parse_error_detail(
                br#"{"error":"content contract violation","detail":"unsupported schema version 3"}"#
            )
            .as_deref(),
            Some("content contract violation: unsupported schema version 3")
        );
        // A detail that merely repeats the category is not printed twice.
        assert_eq!(
            parse_error_detail(br#"{"error":"not found","detail":"not found"}"#).as_deref(),
            Some("not found")
        );
        assert_eq!(
            parse_error_detail(br#"{"error":"not found","detail":""}"#).as_deref(),
            Some("not found")
        );
        let long = format!(r#"{{"error":"{}"}}"#, "x".repeat(4096));
        assert_eq!(
            parse_error_detail(long.as_bytes()).unwrap().len(),
            MAX_ERROR_DETAIL_BYTES
        );
        let sneaky = parse_error_detail(b"{\"error\":\"a\\u0007b\"}").unwrap();
        assert_eq!(sneaky, "ab");
    }

    #[test]
    fn source_collection_rows_parse_strictly() {
        let digest = SourceCollectionId::from_bytes([0x11; 32]);
        let body = format!(
            r#"{{"sources":[{{"source_id":"kenney","title":"Kenney","license":"CC0-1.0","credits":"Kenney","digest":"{digest}"}}]}}"#
        );
        let rows = parse_source_collections(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source_id, "kenney");
        assert_eq!(rows[0].digest, digest);
        for bad in [
            body.replace("kenney", "Kenney"),
            body.replace("CC0-1.0", ""),
            body.replace("scol_", "SCOL_"),
            body.replace("Kenney", "Ke\\u0007nney"),
        ] {
            assert!(
                parse_source_collections(&json::parse(bad.as_bytes()).unwrap()).is_err(),
                "{bad}"
            );
        }
        assert!(parse_source_collections(&json::parse(br#"{"count":1}"#).unwrap()).is_err());
        let page_ok = format!(
            r#"{{"sources":[{{"source_id":"alpha","title":"A","license":"CC0-1.0","credits":"c","digest":"{digest}"}},{{"source_id":"beta","title":"B","license":"CC0-1.0","credits":"c","digest":"{digest}"}}],"cursor":"beta"}}"#
        );
        let page = parse_source_collections_page(&json::parse(page_ok.as_bytes()).unwrap()).unwrap();
        assert_eq!(page.cursor.as_deref(), Some("beta"));
        let terminal = page_ok.replace("\"cursor\":\"beta\"", "\"cursor\":null");
        assert!(parse_source_collections_page(&json::parse(terminal.as_bytes()).unwrap())
            .unwrap()
            .cursor
            .is_none());
        let wrong_cursor = page_ok.replace("\"cursor\":\"beta\"", "\"cursor\":\"alpha\"");
        assert!(parse_source_collections_page(&json::parse(wrong_cursor.as_bytes()).unwrap()).is_err());
        let unordered = page_ok.replace("alpha", "zeta");
        assert!(parse_source_collections_page(&json::parse(unordered.as_bytes()).unwrap()).is_err());
        let missing_cursor = format!(
            r#"{{"sources":[{{"source_id":"kenney","title":"Kenney","license":"CC0-1.0","credits":"Kenney","digest":"{digest}"}}]}}"#
        );
        assert!(
            parse_source_collections_page(&json::parse(missing_cursor.as_bytes()).unwrap()).is_err()
        );
        let empty_with_cursor = format!(
            r#"{{"sources":[],"cursor":"kenney"}}"#
        );
        assert!(
            parse_source_collections_page(&json::parse(empty_with_cursor.as_bytes()).unwrap())
                .is_err()
        );
        let empty_terminal = br#"{"sources":[],"cursor":null}"#;
        assert!(parse_source_collections_page(&json::parse(empty_terminal).unwrap())
            .unwrap()
            .sources
            .is_empty());
    }

    #[test]
    fn source_and_import_status_use_canonical_limit_boundaries() {
        let digest = SourceCollectionId::from_bytes([0x11; 32]);
        let slug48 = "a".repeat(makepad_asset_data::limits::MAX_ALIAS_SEGMENT_BYTES);
        let slug49 = "a".repeat(makepad_asset_data::limits::MAX_ALIAS_SEGMENT_BYTES + 1);
        let title128 = "t".repeat(makepad_asset_data::limits::MAX_NAME_BYTES * 2);
        let title129 = "t".repeat(makepad_asset_data::limits::MAX_NAME_BYTES * 2 + 1);
        let row = |id: &str, title: &str| {
            format!(
                r#"{{"sources":[{{"source_id":"{id}","title":"{title}","license":"CC0-1.0","credits":"c","digest":"{digest}"}}],"cursor":null}}"#
            )
        };
        assert!(parse_source_collections_page(&json::parse(row(&slug48, "Kenney").as_bytes()).unwrap()).is_ok());
        assert!(parse_source_collections_page(&json::parse(row(&slug49, "Kenney").as_bytes()).unwrap()).is_err());
        assert!(parse_source_collections_page(&json::parse(row("kenney", &title128).as_bytes()).unwrap()).is_ok());
        assert!(parse_source_collections_page(&json::parse(row("kenney", &title129).as_bytes()).unwrap()).is_err());
        assert!(parse_source_collections_page(&json::parse(row("kenney", "").as_bytes()).unwrap()).is_err());

        let irev = ImportRevisionId::from_bytes([0x22; 32]);
        let rev = AssetRevisionId::from_bytes([0x33; 32]);
        let ver32 = "v".repeat(makepad_asset_data::limits::MAX_PACK_VERSION_BYTES);
        let ver33 = "v".repeat(makepad_asset_data::limits::MAX_PACK_VERSION_BYTES + 1);
        let status = |ver: &str| {
            format!(
                r#"{{"import_revision":"{irev}","source_id":"kenney","pack_name":"space-kit","pack_version":"{ver}","license":"CC0-1.0","credits":"Kenney","entries":[{{"key":"models/watchtower","asset_id":"{}","revision":"{rev}","alias":null}}]}}"#,
                asset_id_str()
            )
        };
        assert!(parse_import_status(&json::parse(status(&ver32).as_bytes()).unwrap()).is_ok());
        assert!(parse_import_status(&json::parse(status(&ver33).as_bytes()).unwrap()).is_err());
        let pack49 = status("1.0").replace("space-kit", &slug49);
        assert!(parse_import_status(&json::parse(pack49.as_bytes()).unwrap()).is_err());
    }

    #[test]
    fn source_page_parses_a_legal_max_shaped_response_over_ordinary_cap() {
        let digest = SourceCollectionId::from_bytes([0x11; 32]);
        let n = 80usize;
        let credits = "c".repeat(3_200);
        let mut rows = Vec::with_capacity(n);
        let mut last = String::new();
        for i in 0..n {
            let id = format!("s{i:03}");
            last = id.clone();
            rows.push(json::obj(vec![
                ("source_id", json::s(id)),
                ("title", json::s("Kenney")),
                ("license", json::s("CC0-1.0")),
                ("credits", json::s(credits.clone())),
                ("digest", json::s(digest.to_string())),
            ]));
        }
        let value = json::obj(vec![
            ("sources", Value::Arr(rows)),
            ("cursor", json::s(last.clone())),
        ]);
        let bytes = value.to_json().into_bytes();
        assert!(
            bytes.len() as u64 > crate::wire::MAX_JSON_RESPONSE_BYTES,
            "max-shaped source page must exceed the ordinary 256 KiB JSON cap"
        );
        assert!(
            bytes.len() as u64 <= crate::wire::MAX_SOURCE_PAGE_JSON_RESPONSE_BYTES,
            "max-shaped source page must fit the derived source-page ceiling"
        );
        let page = parse_source_collections_page(&json::parse(&bytes).unwrap()).unwrap();
        assert_eq!(page.sources.len(), n);
        assert_eq!(page.cursor.as_deref(), Some(last.as_str()));
    }

    #[test]
    fn import_report_and_status_parse_strictly() {
        let irev = ImportRevisionId::from_bytes([0x22; 32]);
        let rev = AssetRevisionId::from_bytes([0x33; 32]);
        let body = format!(
            r#"{{"import_revision":"{irev}","created":true,"entries":[{{"key":"models/watchtower","asset_id":"{}","revision":"{rev}","alias":"kenney/space-kit/models/watchtower"}}]}}"#,
            asset_id_str()
        );
        let report = parse_import_report(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert!(report.created);
        assert_eq!(report.entries[0].key.as_str(), "models/watchtower");
        assert_eq!(
            report.entries[0].alias.as_ref().unwrap().as_str(),
            "kenney/space-kit/models/watchtower"
        );
        let status_body = format!(
            r#"{{"import_revision":"{irev}","source_id":"kenney","pack_name":"space-kit","pack_version":"1.0","license":"CC0-1.0","credits":"Kenney","entries":[{{"key":"models/watchtower","asset_id":"{}","revision":"{rev}","alias":null}}]}}"#,
            asset_id_str()
        );
        let status = parse_import_status(&json::parse(status_body.as_bytes()).unwrap()).unwrap();
        assert_eq!(status.pack_version, "1.0");
        assert!(status.entries[0].alias.is_none());
        assert!(parse_import_report(
            &json::parse(body.replace("irev_", "IREV_").as_bytes()).unwrap()
        )
        .is_err());
        assert!(parse_import_status(
            &json::parse(status_body.replace("1.0", "1.0/x").as_bytes()).unwrap()
        )
        .is_err());
        assert!(parse_import_report(
            &json::parse(body.replace("models/watchtower", "Models/Watchtower").as_bytes())
                .unwrap()
        )
        .is_err());
    }

    #[test]
    fn resolved_variant_map_parses_closed_roles() {
        let digest = ResolvedMapDigest::from_bytes([0x44; 32]);
        let set = VariantSetId::from_bytes([0x55; 32]);
        let profile = ClientProfileDigest::from_bytes([0x66; 32]);
        let variant = DerivedVariantId::from_bytes([0x77; 32]);
        let blob = BlobId::from_bytes([0x88; 32]);
        let body = format!(
            r#"{{"digest":"{digest}","variant_set":"{set}","profile":"{profile}","entries":[{{"role":"thumbnail","variant":"{variant}","blobs":["{blob}"]}}]}}"#
        );
        let map = parse_resolved_variant_map(&json::parse(body.as_bytes()).unwrap()).unwrap();
        assert_eq!(map.entries[0].role, VariantRole::Thumbnail);
        assert_eq!(map.entries[0].blobs[0], blob);
        let file_role = body.replace("thumbnail", "lod1_glb");
        assert_eq!(
            parse_resolved_variant_map(&json::parse(file_role.as_bytes()).unwrap())
                .unwrap()
                .entries[0]
                .role,
            VariantRole::File(FileRole::Lod1Glb)
        );
        assert!(parse_resolved_variant_map(
            &json::parse(body.replace("thumbnail", "haunted").as_bytes()).unwrap()
        )
        .is_err());
        assert!(parse_resolved_variant_map(
            &json::parse(body.replace(&format!("[\"{blob}\"]"), "[]").as_bytes()).unwrap()
        )
        .is_err());
    }

    #[test]
    fn import_report_parses_a_committed_max_entry_legal_report() {
        let irev = ImportRevisionId::from_bytes([0x22; 32]);
        let n = makepad_asset_data::limits::MAX_IMPORT_ASSETS;
        let mut entries = Vec::with_capacity(n);
        for i in 0..n as u16 {
            let key = format!(
                "aaaaaaaaaaaaaaa/bbbbbbbbbbbbbbb/ccccccccccccccc/ddddddddddddddd/eeeeeeeeeeeeeee/{i:015}"
            );
            let mut asset_bytes = [0u8; 16];
            asset_bytes[14..].copy_from_slice(&i.to_be_bytes());
            let mut rev_bytes = [0u8; 32];
            rev_bytes[30..].copy_from_slice(&i.to_be_bytes());
            let asset = AssetId::from_bytes(asset_bytes);
            let rev = AssetRevisionId::from_bytes(rev_bytes);
            let alias = format!("s/p/{key}");
            entries.push(json::obj(vec![
                ("key", json::s(key)),
                ("asset_id", json::s(asset.to_string())),
                ("revision", json::s(rev.to_string())),
                ("alias", json::s(alias)),
            ]));
        }
        let value = json::obj(vec![
            ("import_revision", json::s(irev.to_string())),
            ("created", Value::Bool(true)),
            ("entries", Value::Arr(entries)),
        ]);
        let bytes = value.to_json().into_bytes();
        assert!(
            bytes.len() as u64 > crate::wire::MAX_JSON_RESPONSE_BYTES,
            "max legal report must exceed the ordinary 256 KiB JSON cap"
        );
        assert!(
            bytes.len() as u64 <= crate::wire::MAX_IMPORT_JSON_RESPONSE_BYTES,
            "max legal report must fit the derived import ceiling"
        );
        let report = parse_import_report(&json::parse(&bytes).unwrap()).unwrap();
        assert_eq!(report.entries.len(), n);
        assert_eq!(report.import_revision, irev);
        assert!(report.created);
    }
}
