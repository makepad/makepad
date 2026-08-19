//! Typed calls against the two HTTP planes of one Asset Server.
//!
//! This layer owns status mapping (401 → `Unauthenticated`, 403 → `Denied`,
//! 404 → `NotFound`, everything else a bounded [`ClientError::Server`]),
//! response-body budgets per route, DTO parsing, and digest verification of
//! canonical manifest bytes. It does NOT own caching, resume loops, or
//! server-identity policy — that is [`crate::client::AssetClient`].
//!
//! Control plane: health, catalog/search, listings, asset detail, aliases,
//! canonical manifest bytes. Data plane: blob HEAD/GET (Range/resume).

use crate::dto::{
    self, AliasDto, AssetDetailDto, AssetsPageDto, CatalogPageDto, ClaimedJobDto, EventsPageDto,
    GameAliasDto, HealthDto, ImportReportDto, ImportStatusDto, JobDetailDto, JobId, JobProfileDto,
    JobRowDto, JobStatusDto, SourceCollectionRowDto, SourceCollectionsPageDto,
};
use crate::error::{ClientError, ClientResult};
use crate::http::{self, HttpLimits, Request, Response};
use crate::json::{self, Value};
use crate::wire;
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, AssetRevisionId, AssetRevisionRef, BlobId, ClientProfile,
    DerivedVariantId, DeviceTier, FileRole, GameAlias, GameId, GameRevisionId, ImportManifest,
    ImportRevisionId, MediaType, ResolvedVariantMap, Sha256, SourceCollection, SourceCollectionId,
    VariantSetId, VariantSetManifest, RESOLUTION_POLICY_V1,
};
use std::net::SocketAddr;

/// Longest refusal body this client will read for its `error` detail; larger
/// refusal bodies are dropped unread.
const MAX_REFUSAL_BODY_BYTES: u64 = 16 * 1024;
/// Server-side search page cap; requesting more is a local input error.
pub const MAX_SEARCH_LIMIT: u32 = 100;
/// Listing page cap.
pub const MAX_LIST_LIMIT: u64 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApiEndpoints {
    pub control: SocketAddr,
    pub data: SocketAddr,
}

/// One blob-garbage-collection request. Every field is optional policy; the
/// server owns the batch sizes, so a client can never ask for an unbounded
/// unit of work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcRequest {
    /// Count what would be reclaimed; delete (and retire) nothing.
    pub dry_run: bool,
    /// Protect blobs recorded within this window of the run's start.
    /// `None` = the server's configured default.
    pub grace_ms: Option<u64>,
    /// Also retire every revision of each asset except the newest `n` and
    /// any revision an alias head still points at. `None` = keep all
    /// revisions (the default: GC then only reclaims what is already
    /// unreferenced).
    pub retain_per_asset: Option<u32>,
    /// How many bounded steps this ONE call may perform. `None` = the
    /// server's per-request maximum.
    pub max_steps: Option<u32>,
}

impl GcRequest {
    /// Preview: no deletion, no retirement, exact byte accounting.
    pub fn dry_run() -> Self {
        Self { dry_run: true, ..Self::default() }
    }

    /// Collect what is already unreferenced.
    pub fn collect() -> Self {
        Self::default()
    }
}

/// A validated catalog search. `text` empty = browse mode (filters only).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogQuery {
    pub text: String,
    pub namespace: Option<String>,
    pub kind: Option<AssetKind>,
    pub category: Option<String>,
    pub tag: Option<String>,
    /// Server-side exclusion: hits carrying this tag are dropped by the
    /// server, so a page is never short of `page_size` for local reasons.
    pub exclude_tag: Option<String>,
    pub creator: Option<String>,
    /// Only assets currently referenced by an alias head.
    pub live_only: bool,
    /// 1..=[`MAX_SEARCH_LIMIT`].
    pub page_size: u32,
}

impl CatalogQuery {
    pub fn browse(page_size: u32) -> Self {
        Self { page_size, ..Self::default() }
    }

    pub fn text(text: impl Into<String>, page_size: u32) -> Self {
        Self { text: text.into(), page_size, ..Self::default() }
    }

    fn validate(&self) -> ClientResult<()> {
        if self.page_size == 0 || self.page_size > MAX_SEARCH_LIMIT {
            return Err(ClientError::InvalidInput { what: "search page_size" });
        }
        if self.text.len() > wire::MAX_QUERY_TEXT_BYTES {
            return Err(ClientError::InvalidInput { what: "search text too long" });
        }
        if self.text.chars().any(char::is_control) {
            return Err(ClientError::InvalidInput { what: "search text control chars" });
        }
        for v in [&self.namespace, &self.category, &self.tag, &self.exclude_tag, &self.creator]
            .into_iter()
            .flatten()
        {
            if v.is_empty()
                || v.len() > wire::MAX_FILTER_VALUE_BYTES
                || v.chars().any(char::is_control)
            {
                return Err(ClientError::InvalidInput { what: "search filter value" });
            }
        }
        Ok(())
    }

    /// The POST body. `cursor` continues a previous page of this same query.
    fn body(&self, cursor: Option<&str>) -> Value {
        let mut pairs: Vec<(&str, Value)> = vec![("q", json::s(self.text.clone()))];
        if let Some(ns) = &self.namespace {
            pairs.push(("ns", json::s(ns.clone())));
        }
        if let Some(kind) = self.kind {
            pairs.push(("kind", json::s(dto::kind_name(kind))));
        }
        if let Some(c) = &self.category {
            pairs.push(("category", json::s(c.clone())));
        }
        if let Some(t) = &self.tag {
            pairs.push(("tag", json::s(t.clone())));
        }
        if let Some(t) = &self.exclude_tag {
            pairs.push(("exclude_tag", json::s(t.clone())));
        }
        if let Some(c) = &self.creator {
            pairs.push(("creator", json::s(c.clone())));
        }
        if self.live_only {
            pairs.push(("live", Value::Bool(true)));
        }
        pairs.push(("limit", Value::Int(self.page_size as i64)));
        if let Some(c) = cursor {
            pairs.push(("cursor", json::s(c)));
        }
        json::obj(pairs)
    }
}

/// What a blob HEAD reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobHead {
    pub size: u64,
    /// The strong ETag equalled the blob's canonical `sha256:<hex>` spelling.
    pub etag_matches: bool,
}

/// The searchable annotation written alongside a published artifact.
/// `prompt`/`provenance` are owner-only fields server-side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnnotationUpload {
    pub title: String,
    pub description: String,
    pub kind: Option<AssetKind>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub creator: String,
    pub generator: String,
    pub backend: String,
    pub model: String,
    pub prompt: String,
    pub provenance: String,
    pub private: bool,
}

impl AnnotationUpload {
    fn validate(&self) -> ClientResult<()> {
        if self.title.is_empty() || self.title.len() > wire::MAX_TITLE_BYTES {
            return Err(ClientError::InvalidInput { what: "annotation title" });
        }
        for text in [&self.title, &self.description, &self.prompt, &self.provenance] {
            if text.chars().any(char::is_control) {
                return Err(ClientError::InvalidInput { what: "annotation control chars" });
            }
        }
        for label in self.categories.iter().chain(&self.tags) {
            if label.is_empty()
                || label.len() > wire::MAX_FILTER_VALUE_BYTES
                || label.chars().any(char::is_control)
            {
                return Err(ClientError::InvalidInput { what: "annotation label" });
            }
        }
        Ok(())
    }
}

/// One exact operation input selector: asset + revision + role, optionally
/// narrowed by tier/lod, optionally guarded by the expected media type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationInputRef {
    pub slot: String,
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub role: FileRole,
    pub tier: Option<DeviceTier>,
    pub lod: Option<u8>,
    pub expected_media: Option<MediaType>,
}

/// Alias expectation of a `publish_and_alias` publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationAliasExpect {
    Any,
    Absent,
    Head(AssetRevisionId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationPublicationRef {
    Publish,
    PublishAndAlias { alias: AssetAlias, expect: OperationAliasExpect },
}

/// A typed operation-creation request. `params` is the (already validated
/// server-side) bounded parameter object; unknown parameters refuse.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationCreateRequest {
    pub namespace: String,
    pub kind: String,
    pub idempotency_key: String,
    pub inputs: Vec<OperationInputRef>,
    pub params: Value,
    pub publication: OperationPublicationRef,
}

impl OperationCreateRequest {
    pub fn new(
        namespace: impl Into<String>,
        kind: impl Into<String>,
        idempotency_key: impl Into<String>,
        inputs: Vec<OperationInputRef>,
    ) -> OperationCreateRequest {
        OperationCreateRequest {
            namespace: namespace.into(),
            kind: kind.into(),
            idempotency_key: idempotency_key.into(),
            inputs,
            params: Value::Obj(Vec::new()),
            publication: OperationPublicationRef::Publish,
        }
    }

    fn body(&self) -> ClientResult<Value> {
        if self.namespace.is_empty() || self.namespace.len() > wire::MAX_NAMESPACE_BYTES {
            return Err(ClientError::InvalidInput { what: "operation namespace" });
        }
        if self.kind.is_empty() || self.kind.len() > 64 {
            return Err(ClientError::InvalidInput { what: "operation kind" });
        }
        if self.idempotency_key.is_empty()
            || self.idempotency_key.len() > 128
            || !self.idempotency_key.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(ClientError::InvalidInput { what: "operation idempotency key" });
        }
        if self.inputs.is_empty() || self.inputs.len() > 64 {
            return Err(ClientError::InvalidInput { what: "operation inputs" });
        }
        if !matches!(self.params, Value::Obj(_)) {
            return Err(ClientError::InvalidInput { what: "operation params" });
        }
        let inputs: Vec<Value> = self
            .inputs
            .iter()
            .map(|input| {
                let mut pairs = vec![
                    ("slot", json::s(input.slot.clone())),
                    ("asset", json::s(input.asset.to_string())),
                    ("revision", json::s(input.revision.to_string())),
                    ("role", json::s(dto::role_name(input.role))),
                ];
                if let Some(t) = input.tier {
                    pairs.push(("tier", json::s(dto::tier_name(t))));
                }
                if let Some(l) = input.lod {
                    pairs.push(("lod", Value::Int(l as i64)));
                }
                if let Some(m) = input.expected_media {
                    pairs.push(("media", json::s(dto::media_name(m))));
                }
                json::obj(pairs)
            })
            .collect();
        let mut pairs = vec![
            ("api_version", Value::Int(1)),
            ("namespace", json::s(self.namespace.clone())),
            ("kind", json::s(self.kind.clone())),
            ("idempotency_key", json::s(self.idempotency_key.clone())),
            ("inputs", Value::Arr(inputs)),
            ("params", self.params.clone()),
        ];
        match &self.publication {
            OperationPublicationRef::Publish => {}
            OperationPublicationRef::PublishAndAlias { alias, expect } => {
                let mut pub_pairs = vec![
                    ("mode", json::s("publish_and_alias")),
                    ("alias", json::s(alias.to_string())),
                ];
                match expect {
                    OperationAliasExpect::Any => pub_pairs.push(("expect", json::s("any"))),
                    OperationAliasExpect::Absent => {
                        pub_pairs.push(("expect", json::s("absent")))
                    }
                    OperationAliasExpect::Head(rev) => {
                        pub_pairs.push(("expect", json::s("head")));
                        pub_pairs.push(("expect_head", json::s(rev.to_string())));
                    }
                }
                pairs.push(("publication", json::obj(pub_pairs)));
            }
        }
        Ok(json::obj(pairs))
    }
}

/// One reported output file of an operation finalize.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationOutputFile {
    pub role: FileRole,
    pub tier: DeviceTier,
    pub lod: u8,
    pub media: MediaType,
    pub blob: BlobId,
    pub byte_len: u64,
    pub dims: Option<(u32, u32)>,
}

/// The worker's typed completion facts for the single named output of the
/// first-slice operations, plus the ACTUAL model facts of the run.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationFinalizeRequest {
    pub job: JobId,
    pub suffix: Option<String>,
    pub output_name: String,
    pub files: Vec<OperationOutputFile>,
    pub thumbnail: Option<(BlobId, &'static str, u32, u32, u64)>,
    /// (total_bytes, triangles, vertices, joints, clips, max_texture_dim,
    /// media_millis) — measured, never fabricated.
    pub metrics: (u64, u32, u32, u16, u16, u32, u32),
    pub bounds: Option<([f32; 3], [f32; 3])>,
    pub generator: String,
    pub model: String,
    pub version: String,
    pub seed: u64,
}

impl OperationFinalizeRequest {
    fn body(&self) -> ClientResult<Value> {
        if self.output_name.is_empty() || self.output_name.len() > 64 {
            return Err(ClientError::InvalidInput { what: "finalize output name" });
        }
        if self.files.is_empty() || self.files.len() > 64 {
            return Err(ClientError::InvalidInput { what: "finalize files" });
        }
        for text in [&self.generator, &self.model, &self.version] {
            if text.is_empty() || text.len() > 64 || text.chars().any(char::is_control) {
                return Err(ClientError::InvalidInput { what: "finalize model facts" });
            }
        }
        let files: Vec<Value> = self
            .files
            .iter()
            .map(|f| {
                let mut pairs = vec![
                    ("role", json::s(dto::role_name(f.role))),
                    ("tier", json::s(dto::tier_name(f.tier))),
                    ("lod", Value::Int(f.lod as i64)),
                    ("media", json::s(dto::media_name(f.media))),
                    ("blob", json::s(f.blob.to_string())),
                    ("byte_len", Value::Int(f.byte_len.min(i64::MAX as u64) as i64)),
                ];
                if let Some((w, h)) = f.dims {
                    pairs.push((
                        "dims",
                        json::obj(vec![
                            ("width", Value::Int(w as i64)),
                            ("height", Value::Int(h as i64)),
                        ]),
                    ));
                }
                json::obj(pairs)
            })
            .collect();
        let mut output = vec![("files", Value::Arr(files))];
        if let Some((blob, media, w, h, len)) = &self.thumbnail {
            output.push((
                "thumbnail",
                json::obj(vec![
                    ("blob", json::s(blob.to_string())),
                    ("media", json::s(*media)),
                    ("width", Value::Int(*w as i64)),
                    ("height", Value::Int(*h as i64)),
                    ("byte_len", Value::Int((*len).min(i64::MAX as u64) as i64)),
                ]),
            ));
        }
        let (total, tris, verts, joints, clips, maxdim, millis) = self.metrics;
        output.push((
            "metrics",
            json::obj(vec![
                ("total_bytes", Value::Int(total.min(i64::MAX as u64) as i64)),
                ("triangles", Value::Int(tris as i64)),
                ("vertices", Value::Int(verts as i64)),
                ("joints", Value::Int(joints as i64)),
                ("clips", Value::Int(clips as i64)),
                ("max_texture_dim", Value::Int(maxdim as i64)),
                ("media_millis", Value::Int(millis as i64)),
            ]),
        ));
        if let Some((min, max)) = &self.bounds {
            let arr = |v: &[f32; 3]| {
                Value::Arr(v.iter().map(|c| Value::F64(*c as f64)).collect())
            };
            output.push((
                "bounds",
                json::obj(vec![("min", arr(min)), ("max", arr(max))]),
            ));
        }
        let mut pairs = vec![("job", json::s(self.job.to_string()))];
        if let Some(sfx) = &self.suffix {
            pairs.push(("suffix", json::s(sfx.clone())));
        }
        pairs.push((
            "outputs",
            Value::Obj(vec![(self.output_name.clone(), json::obj(output))]),
        ));
        pairs.push((
            "model",
            json::obj(vec![
                ("generator", json::s(self.generator.clone())),
                ("model", json::s(self.model.clone())),
                ("version", json::s(self.version.clone())),
                ("seed", Value::Int(self.seed.min(i64::MAX as u64) as i64)),
            ]),
        ));
        Ok(json::obj(pairs))
    }
}

/// Open a broker-owned chat session bound to one explicit provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatCreateRequest {
    pub namespace: String,
    pub provider: crate::dto::ChatProviderKind,
}

impl ChatCreateRequest {
    pub fn new(namespace: impl Into<String>, provider: crate::dto::ChatProviderKind) -> Self {
        ChatCreateRequest { namespace: namespace.into(), provider }
    }

    fn body(&self) -> ClientResult<Value> {
        if self.namespace.is_empty() || self.namespace.len() > wire::MAX_NAMESPACE_BYTES {
            return Err(ClientError::InvalidInput { what: "chat namespace" });
        }
        Ok(json::obj(vec![
            ("api_version", Value::Int(1)),
            ("namespace", json::s(self.namespace.clone())),
            ("provider", json::s(self.provider.as_str())),
        ]))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatAttachment {
    pub revision: AssetRevisionId,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatSendRequest {
    pub text: String,
    pub attachments: Vec<ChatAttachment>,
}

impl ChatSendRequest {
    pub fn text(text: impl Into<String>) -> Self {
        ChatSendRequest { text: text.into(), attachments: Vec::new() }
    }

    fn body(&self) -> ClientResult<Value> {
        if self.text.is_empty() || self.text.len() > wire::MAX_CHAT_MESSAGE_BYTES {
            return Err(ClientError::InvalidInput { what: "chat message" });
        }
        if self.attachments.len() > wire::MAX_CHAT_ATTACHMENTS {
            return Err(ClientError::InvalidInput { what: "chat attachments" });
        }
        let mut pairs = vec![("text", json::s(self.text.clone()))];
        if !self.attachments.is_empty() {
            let mut atts = Vec::with_capacity(self.attachments.len());
            for a in &self.attachments {
                if a.role.is_empty()
                    || a.role.len() > 32
                    || !a.role.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                    })
                {
                    return Err(ClientError::InvalidInput { what: "chat attachment role" });
                }
                atts.push(json::obj(vec![
                    ("revision", json::s(a.revision.to_string())),
                    ("role", json::s(a.role.clone())),
                ]));
            }
            pairs.push(("attachments", Value::Arr(atts)));
        }
        Ok(json::obj(pairs))
    }
}

/// `PUT /v1/import-sources` identity echo. `digest` is the SHA-256 of the
/// canonical bytes that were sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCollectionRegistered {
    pub source_id: String,
    pub digest: SourceCollectionId,
}

#[derive(Clone)]
pub struct Api {
    pub endpoints: ApiEndpoints,
    pub limits: HttpLimits,
    /// Full validated bearer token (`mpat_…`), attached to every request.
    token: Option<String>,
}

impl Api {
    pub fn new(endpoints: ApiEndpoints, limits: HttpLimits, token: Option<String>) -> ClientResult<Api> {
        limits.validate()?;
        if let Some(t) = &token {
            if !wire::token_shape_ok(t) {
                return Err(ClientError::InvalidInput { what: "bearer token shape" });
            }
        }
        Ok(Api { endpoints, limits, token })
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    fn bearer(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Issue a request whose success body is JSON, mapping refusals.
    fn call_json(&self, addr: SocketAddr, req: Request<'_>) -> ClientResult<Value> {
        self.call_json_accept(addr, req, &[200])
    }

    fn call_json_accept(
        &self,
        addr: SocketAddr,
        req: Request<'_>,
        allowed: &[u16],
    ) -> ClientResult<Value> {
        Ok(self.call_json_with_status(addr, req, allowed)?.1)
    }

    fn call_json_with_status(
        &self,
        addr: SocketAddr,
        req: Request<'_>,
        allowed: &[u16],
    ) -> ClientResult<(u16, Value)> {
        self.call_json_limited(addr, req, allowed, wire::MAX_JSON_RESPONSE_BYTES)
    }

    fn call_json_limited(
        &self,
        addr: SocketAddr,
        req: Request<'_>,
        allowed: &[u16],
        max_body: u64,
    ) -> ClientResult<(u16, Value)> {
        let resp = http::http_call(addr, &req, &self.limits)?;
        let resp = self.accept(resp, allowed)?;
        let status = resp.head().status;
        if resp.head().content_length > max_body {
            return Err(ClientError::OverBudget {
                what: "json response body",
                limit: max_body,
                found: resp.head().content_length,
            });
        }
        let body = resp.read_full(max_body)?;
        let value = json::parse(&body)
            .map_err(|_| ClientError::Protocol { what: "malformed json body" })?;
        Ok((status, value))
    }

    /// Enforce an allowed status set; anything else becomes a typed refusal
    /// (with a bounded, sanitized detail when the body offers one).
    fn accept(&self, resp: Response, allowed: &[u16]) -> ClientResult<Response> {
        let status = resp.head().status;
        if allowed.contains(&status) {
            return Ok(resp);
        }
        Err(self.refusal(resp))
    }

    fn refusal(&self, resp: Response) -> ClientError {
        let status = resp.head().status;
        let detail = if resp.head().content_length <= MAX_REFUSAL_BODY_BYTES {
            resp.read_full(MAX_REFUSAL_BODY_BYTES)
                .ok()
                .and_then(|b| dto::parse_error_detail(&b))
        } else {
            None
        };
        match status {
            401 => ClientError::Unauthenticated,
            403 => ClientError::Denied,
            404 => ClientError::NotFound { what: "server object" },
            _ => ClientError::Server { status, detail },
        }
    }

    // ---- control plane -----------------------------------------------------

    pub fn health(&self) -> ClientResult<HealthDto> {
        let path = wire::path_health();
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_health(&v)
    }

    /// One page of catalog search. The cursor must come from a previous page
    /// of the same query on the same server (the caller enforces the server
    /// half via [`crate::client::PageCursor`]; the server enforces the query
    /// shape).
    pub fn catalog_search(
        &self,
        query: &CatalogQuery,
        cursor: Option<&str>,
    ) -> ClientResult<CatalogPageDto> {
        query.validate()?;
        if let Some(c) = cursor {
            check_cursor_out(c)?;
        }
        let body = query.body(cursor).to_json().into_bytes();
        let path = wire::path_catalog_search();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_catalog_page(&v)
    }

    /// One page of the keyset asset listing.
    pub fn assets_page(
        &self,
        namespace: Option<&str>,
        cursor: Option<&str>,
        limit: u64,
    ) -> ClientResult<AssetsPageDto> {
        if limit == 0 || limit > MAX_LIST_LIMIT {
            return Err(ClientError::InvalidInput { what: "listing limit" });
        }
        if let Some(ns) = namespace {
            if ns.len() > wire::MAX_NAMESPACE_BYTES || !wire::query_value_ok(ns) {
                return Err(ClientError::InvalidInput { what: "listing namespace" });
            }
        }
        if let Some(c) = cursor {
            check_cursor_out(c)?;
        }
        let path = wire::path_assets(namespace, cursor, limit);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_assets_page(&v)
    }

    pub fn asset_detail(&self, id: &AssetId) -> ClientResult<AssetDetailDto> {
        let path = wire::path_asset(id);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let detail = dto::parse_asset_detail(&v)?;
        // The response must describe the asset that was asked for.
        if &detail.asset_id != id {
            return Err(ClientError::Protocol { what: "asset detail id mismatch" });
        }
        Ok(detail)
    }

    pub fn resolve_alias(&self, alias: &AssetAlias) -> ClientResult<AliasDto> {
        let path = wire::path_alias(alias);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let dto = dto::parse_alias(&v)?;
        if &dto.alias != alias {
            return Err(ClientError::Protocol { what: "alias response mismatch" });
        }
        Ok(dto)
    }

    pub fn resolve_game_alias(&self, alias: &GameAlias) -> ClientResult<GameAliasDto> {
        let path = wire::path_game_alias(alias);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let dto = dto::parse_game_alias(&v)?;
        if &dto.alias != alias {
            return Err(ClientError::Protocol {
                what: "game alias response mismatch",
            });
        }
        Ok(dto)
    }

    /// One page of the committed-catalog event feed. `wait_ms` may be zero
    /// (pure poll); the HTTP head deadline stretches to cover the server-side
    /// wait plus margin, because a long-poll legitimately answers late.
    pub fn events_page(
        &self,
        cursor: Option<&str>,
        wait_ms: u64,
        limit: u32,
        kind: Option<makepad_asset_data::AssetKind>,
    ) -> ClientResult<EventsPageDto> {
        if wait_ms > wire::MAX_EVENT_WAIT_MS {
            return Err(ClientError::InvalidInput { what: "event wait too long" });
        }
        if limit == 0 || limit > wire::MAX_EVENT_BATCH {
            return Err(ClientError::InvalidInput { what: "event limit" });
        }
        if let Some(c) = cursor {
            if c.len() > wire::MAX_EVENT_CURSOR_BYTES || wire::event_cursor_seq(c).is_none() {
                return Err(ClientError::InvalidInput { what: "event cursor shape" });
            }
        }
        let path = wire::path_events(cursor, wait_ms, limit, kind.map(dto::kind_name));
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        req.head_deadline_ms = Some(wait_ms + self.limits.head_deadline_ms);
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_events_page(&v)
    }

    /// Canonical asset-manifest bytes, digest-verified against `rev` before
    /// they are returned. The caller decodes via the content contract.
    pub fn fetch_revision_bytes(&self, rev: &AssetRevisionId) -> ClientResult<Vec<u8>> {
        let path = wire::path_revision(rev);
        self.fetch_canonical(self.endpoints.control, &path, rev.as_bytes(), "asset revision bytes")
    }

    /// Canonical game-revision bytes, digest-verified against `rev`.
    pub fn fetch_game_revision_bytes(&self, rev: &GameRevisionId) -> ClientResult<Vec<u8>> {
        let path = wire::path_game_revision(rev);
        self.fetch_canonical(self.endpoints.control, &path, rev.as_bytes(), "game revision bytes")
    }

    fn fetch_canonical(
        &self,
        addr: SocketAddr,
        path: &str,
        expected: &[u8; 32],
        what: &'static str,
    ) -> ClientResult<Vec<u8>> {
        let mut req = Request::get(path);
        req.bearer = self.bearer();
        let resp = http::http_call(addr, &req, &self.limits)?;
        let resp = self.accept(resp, &[200])?;
        if resp.head().content_length > wire::MAX_MANIFEST_RESPONSE_BYTES {
            return Err(ClientError::OverBudget {
                what,
                limit: wire::MAX_MANIFEST_RESPONSE_BYTES,
                found: resp.head().content_length,
            });
        }
        let bytes = resp.read_full(wire::MAX_MANIFEST_RESPONSE_BYTES)?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let found = hasher.finalize();
        if &found != expected {
            return Err(ClientError::DigestMismatch { what, expected: *expected, found });
        }
        Ok(bytes)
    }

    // ---- data plane --------------------------------------------------------

    /// Size probe for a blob. Also reports whether the strong ETag equals the
    /// blob's canonical spelling — a server that cannot echo the identity it
    /// serves is not trusted with a later `If-Range` resume.
    pub fn blob_head(&self, blob: &BlobId) -> ClientResult<BlobHead> {
        let path = wire::path_blob(blob);
        let mut req = Request::head(&path);
        req.bearer = self.bearer();
        let resp = http::http_call(self.endpoints.data, &req, &self.limits)?;
        let resp = self.accept(resp, &[200])?;
        let head = resp.head();
        let etag_matches = head.etag.as_deref() == Some(&blob.to_string());
        Ok(BlobHead { size: head.content_length, etag_matches })
    }

    // ---- jobs (generation scheduling) --------------------------------------

    /// Generation capabilities this server advertises. The server stays the
    /// scheduler/credential authority: clients only ever enqueue against a
    /// profile's kind/namespace, never talk to compute nodes.
    pub fn job_profiles(&self, domain: Option<&str>) -> ClientResult<Vec<JobProfileDto>> {
        if let Some(d) = domain {
            if d.len() > 32 || !wire::query_value_ok(d) {
                return Err(ClientError::InvalidInput { what: "profile domain" });
            }
        }
        let path = wire::path_job_profiles(domain);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_job_profiles(&v)
    }

    /// Enqueue one job; the server picks the compute slot.
    pub fn enqueue_job(&self, ns: &str, kind: &str, body: &Value) -> ClientResult<JobId> {
        if ns.is_empty() || ns.len() > wire::MAX_NAMESPACE_BYTES || !wire::query_value_ok(ns) {
            return Err(ClientError::InvalidInput { what: "job namespace" });
        }
        if kind.is_empty() || kind.len() > 64 || kind.chars().any(char::is_control) {
            return Err(ClientError::InvalidInput { what: "job kind" });
        }
        if !matches!(body, Value::Obj(_)) {
            return Err(ClientError::InvalidInput { what: "job body must be an object" });
        }
        let payload = json::obj(vec![
            ("namespace", json::s(ns)),
            ("kind", json::s(kind)),
            ("body", body.clone()),
        ])
        .to_json()
        .into_bytes();
        if payload.len() as u64 > wire::MAX_JSON_RESPONSE_BYTES {
            return Err(ClientError::OverBudget {
                what: "job body",
                limit: wire::MAX_JSON_RESPONSE_BYTES,
                found: payload.len() as u64,
            });
        }
        let path = wire::path_jobs();
        let mut req = Request::post(&path, &payload);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        v.get("job")
            .and_then(Value::as_str)
            .and_then(JobId::parse)
            .ok_or(ClientError::Protocol { what: "enqueue job id" })
    }

    /// One job's visible state; the response must describe the job asked for.
    pub fn job_status(&self, job: &JobId) -> ClientResult<JobStatusDto> {
        let path = wire::path_job(&job.to_string());
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let status = dto::parse_job_status(&v)?;
        if status.job != *job {
            return Err(ClientError::Protocol { what: "job status id mismatch" });
        }
        Ok(status)
    }

    /// Complete visible state of one job: the legacy status projection plus
    /// enqueuer, attempt history, progress freshness, and the full recorded
    /// terminal result document. Same route as [`Self::job_status`]; nothing
    /// the server reports is dropped.
    pub fn job_detail(&self, job: &JobId) -> ClientResult<JobDetailDto> {
        let path = wire::path_job(&job.to_string());
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let detail = dto::parse_job_detail(&v)?;
        if detail.status.job != *job {
            return Err(ClientError::Protocol { what: "job detail id mismatch" });
        }
        Ok(detail)
    }

    /// One page of the scoped job listing. `namespace` requires a job
    /// capability on that namespace server-side; `None` lists the caller's
    /// own jobs. The server returns newest first, capped at
    /// [`MAX_LIST_LIMIT`].
    pub fn list_jobs(
        &self,
        namespace: Option<&str>,
        limit: u64,
    ) -> ClientResult<Vec<JobRowDto>> {
        if limit == 0 || limit > MAX_LIST_LIMIT {
            return Err(ClientError::InvalidInput { what: "job list limit" });
        }
        if let Some(ns) = namespace {
            if ns.len() > wire::MAX_NAMESPACE_BYTES || !wire::query_value_ok(ns) {
                return Err(ClientError::InvalidInput { what: "job list namespace" });
            }
        }
        let path = wire::path_jobs_list(namespace, limit);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_jobs_page(&v)
    }

    /// Cancel a job (with its pending descendants); returns how many the
    /// server cancelled (0 = already terminal).
    pub fn cancel_job(&self, job: &JobId) -> ClientResult<u64> {
        let path = wire::path_job_cancel(&job.to_string());
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        v.get("cancelled")
            .and_then(Value::as_u64)
            .ok_or(ClientError::Protocol { what: "cancel count" })
    }

    // ---- worker protocol (fleet dispatchers) --------------------------------

    /// Claim one queued job under a lease. `suffix` distinguishes several
    /// worker threads of one principal. `Ok(None)` = nothing claimable.
    pub fn worker_claim(
        &self,
        lease_ms: u64,
        suffix: Option<&str>,
    ) -> ClientResult<Option<ClaimedJobDto>> {
        self.worker_claim_inner(lease_ms, suffix, None)
    }

    /// Claim one job restricted to this non-empty, duplicate-free set of
    /// kinds. Specialized workers must use this instead of claiming the
    /// shared queue indiscriminately.
    pub fn worker_claim_kinds(
        &self,
        lease_ms: u64,
        suffix: Option<&str>,
        kinds: &[&str],
    ) -> ClientResult<Option<ClaimedJobDto>> {
        check_worker_kinds(kinds)?;
        self.worker_claim_inner(lease_ms, suffix, Some(kinds))
    }

    fn worker_claim_inner(
        &self,
        lease_ms: u64,
        suffix: Option<&str>,
        kinds: Option<&[&str]>,
    ) -> ClientResult<Option<ClaimedJobDto>> {
        check_worker_suffix(suffix)?;
        if lease_ms == 0 {
            return Err(ClientError::InvalidInput { what: "claim lease_ms" });
        }
        let mut pairs: Vec<(&str, Value)> =
            vec![("lease_ms", Value::Int(lease_ms.min(i64::MAX as u64) as i64))];
        if let Some(s) = suffix {
            pairs.push(("suffix", json::s(s)));
        }
        if let Some(kinds) = kinds {
            pairs.push((
                "kinds",
                Value::Arr(kinds.iter().map(|kind| json::s(*kind)).collect()),
            ));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let mut req = Request::post("/v1/worker/claim", &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_claimed_job(&v)
    }

    /// Extend the lease; optionally report `(permille, note)` progress.
    /// Returns the new lease expiry.
    pub fn worker_heartbeat(
        &self,
        job: &JobId,
        extend_ms: u64,
        suffix: Option<&str>,
        progress: Option<(u16, &str)>,
    ) -> ClientResult<u64> {
        check_worker_suffix(suffix)?;
        if extend_ms == 0 {
            return Err(ClientError::InvalidInput { what: "heartbeat extend_ms" });
        }
        let mut pairs: Vec<(&str, Value)> = vec![
            ("job", json::s(job.to_string())),
            ("extend_ms", Value::Int(extend_ms.min(i64::MAX as u64) as i64)),
        ];
        if let Some(s) = suffix {
            pairs.push(("suffix", json::s(s)));
        }
        if let Some((permille, note)) = progress {
            if permille > 1000 || note.len() > 200 || note.chars().any(char::is_control) {
                return Err(ClientError::InvalidInput { what: "heartbeat progress" });
            }
            pairs.push((
                "progress",
                json::obj(vec![
                    ("permille", Value::Int(permille as i64)),
                    ("note", json::s(note)),
                ]),
            ));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let mut req = Request::post("/v1/worker/heartbeat", &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        v.get("lease_expires_ms")
            .and_then(Value::as_u64)
            .ok_or(ClientError::Protocol { what: "heartbeat lease" })
    }

    /// Report success, optionally with a bounded result document. By the
    /// publish convention the result carries the produced identities:
    /// `{"asset_id": "ast_…", "revision": "arev_…"}` (see
    /// [`dto::JobStatusDto::result_asset`]). Returns the job's new state.
    pub fn worker_succeed(
        &self,
        job: &JobId,
        suffix: Option<&str>,
        result: Option<&Value>,
    ) -> ClientResult<crate::dto::JobStateDto> {
        self.worker_finish("/v1/worker/succeed", job, suffix, "result", result, None)
    }

    /// Report failure; `retry_delay_ms` defers the next attempt. Returns the
    /// job's new state — `Pending` means a retry was scheduled, `Failed`
    /// means terminal (and the error document, if any, was recorded).
    pub fn worker_fail(
        &self,
        job: &JobId,
        suffix: Option<&str>,
        retry_delay_ms: u64,
        error: Option<&Value>,
    ) -> ClientResult<crate::dto::JobStateDto> {
        self.worker_finish(
            "/v1/worker/fail",
            job,
            suffix,
            "error",
            error,
            Some(retry_delay_ms),
        )
    }

    fn worker_finish(
        &self,
        path: &str,
        job: &JobId,
        suffix: Option<&str>,
        doc_key: &'static str,
        doc: Option<&Value>,
        retry_delay_ms: Option<u64>,
    ) -> ClientResult<crate::dto::JobStateDto> {
        check_worker_suffix(suffix)?;
        let mut pairs: Vec<(&str, Value)> = vec![("job", json::s(job.to_string()))];
        if let Some(s) = suffix {
            pairs.push(("suffix", json::s(s)));
        }
        if let Some(delay) = retry_delay_ms {
            pairs.push(("retry_delay_ms", Value::Int(delay.min(i64::MAX as u64) as i64)));
        }
        if let Some(doc) = doc {
            if !matches!(doc, Value::Obj(_)) {
                return Err(ClientError::InvalidInput { what: "worker result must be object" });
            }
            let bytes = doc.to_json().into_bytes();
            if bytes.len() > 16 * 1024 {
                return Err(ClientError::OverBudget {
                    what: "worker result document",
                    limit: 16 * 1024,
                    found: bytes.len() as u64,
                });
            }
            pairs.push((doc_key, doc.clone()));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let mut req = Request::post(path, &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        v.get("state")
            .and_then(Value::as_str)
            .and_then(crate::dto::JobStateDto::parse_pub)
            .ok_or(ClientError::Protocol { what: "worker finish state" })
    }

    // ---- write plane (artifact publication) --------------------------------

    /// Content-addressed upload to the data plane. The response identity
    /// must equal the locally computed digest — a server that answers with
    /// another identity is refused.
    pub fn upload_blob(&self, ns: &str, bytes: &[u8]) -> ClientResult<BlobId> {
        if ns.is_empty() || ns.len() > wire::MAX_NAMESPACE_BYTES || !wire::query_value_ok(ns) {
            return Err(ClientError::InvalidInput { what: "upload namespace" });
        }
        if bytes.is_empty() {
            return Err(ClientError::InvalidInput { what: "upload empty blob" });
        }
        let local = BlobId::hash_of(bytes);
        let path = wire::path_blob_upload(ns);
        let mut req = Request::post(&path, bytes);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.data, req, &[200, 201])?;
        let got = v
            .get("blob_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<BlobId>().ok())
            .ok_or(ClientError::Protocol { what: "upload blob_id" })?;
        if got != local {
            return Err(ClientError::DigestMismatch {
                what: "uploaded blob identity",
                expected: *local.as_bytes(),
                found: *got.as_bytes(),
            });
        }
        Ok(local)
    }

    /// Register an asset id (server-minted when `id` is None). Registering
    /// an id that already exists surfaces the server's 409 as
    /// [`ClientError::Server`]; callers that re-publish an existing asset
    /// treat that as "already registered".
    pub fn register_asset(&self, ns: &str, id: Option<&AssetId>) -> ClientResult<AssetId> {
        let mut pairs: Vec<(&str, Value)> = vec![("namespace", json::s(ns))];
        if let Some(id) = id {
            pairs.push(("asset_id", json::s(id.to_string())));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let path = wire::path_asset_register();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let got = v
            .get("asset_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<AssetId>().ok())
            .ok_or(ClientError::Protocol { what: "register asset_id" })?;
        if let Some(id) = id {
            if got != *id {
                return Err(ClientError::Protocol { what: "register id mismatch" });
            }
        }
        Ok(got)
    }

    /// Stage canonical manifest bytes as a new revision. The server's echoed
    /// revision must equal the digest of the bytes that were sent.
    pub fn stage_asset_revision(
        &self,
        asset: &AssetId,
        canonical: &[u8],
    ) -> ClientResult<AssetRevisionId> {
        let mut hasher = Sha256::new();
        hasher.update(canonical);
        let local = AssetRevisionId::from_bytes(hasher.finalize());
        let path = wire::path_asset_stage(asset);
        let mut req = Request::post(&path, canonical);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let got = v
            .get("revision")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<AssetRevisionId>().ok())
            .ok_or(ClientError::Protocol { what: "stage revision" })?;
        if got != local {
            return Err(ClientError::DigestMismatch {
                what: "staged revision identity",
                expected: *local.as_bytes(),
                found: *got.as_bytes(),
            });
        }
        Ok(local)
    }

    /// Delete an asset from the store: every revision is retired, every
    /// alias head pointing at it drops, its search rows are removed, and its
    /// bytes become collectable by [`Self::gc_blobs`]. Idempotent — a repeat
    /// answers with `already_retired`.
    ///
    /// Requires the moderation capability (`asset_quarantine`) on the
    /// asset's namespace, exactly like pulling content.
    pub fn retire_asset(&self, asset: &AssetId) -> ClientResult<crate::dto::RetireDto> {
        let path = wire::path_asset_retire(asset);
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        let dto = crate::dto::parse_retire(&v)?;
        if &dto.asset_id != asset {
            return Err(ClientError::Protocol { what: "retire response mismatch" });
        }
        Ok(dto)
    }

    /// Delete ONE revision (typically a superseded one); the asset stays
    /// live. Idempotent.
    pub fn retire_revision(
        &self,
        asset: &AssetId,
        rev: &AssetRevisionId,
    ) -> ClientResult<crate::dto::RetireDto> {
        let path = wire::path_revision_retire(asset, rev);
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        let dto = crate::dto::parse_retire(&v)?;
        if &dto.asset_id != asset || dto.revision.as_ref() != Some(rev) {
            return Err(ClientError::Protocol { what: "retire response mismatch" });
        }
        Ok(dto)
    }

    /// Advance blob garbage collection, starting a run if none is active,
    /// and return the run's durable progress. ONE call does a bounded amount
    /// of work — poll until `done` (the server also finishes runs on its own
    /// janitor, so a caller that stops polling does not strand one).
    ///
    /// `dry_run` counts what would be reclaimed and deletes nothing.
    /// Whole-store admin operation: the bootstrap admin token.
    pub fn gc_blobs(&self, req: &GcRequest) -> ClientResult<crate::dto::GcStatusDto> {
        let mut pairs: Vec<(&str, Value)> = vec![("dry_run", Value::Bool(req.dry_run))];
        if let Some(grace) = req.grace_ms {
            pairs.push(("grace_ms", Value::Int(grace as i64)));
        }
        if let Some(keep) = req.retain_per_asset {
            if keep == 0 {
                return Err(ClientError::InvalidInput { what: "retain_per_asset zero" });
            }
            pairs.push(("retain_per_asset", Value::Int(keep as i64)));
        }
        if let Some(steps) = req.max_steps {
            if steps == 0 {
                return Err(ClientError::InvalidInput { what: "gc max_steps zero" });
            }
            pairs.push(("max_steps", Value::Int(steps as i64)));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let path = wire::path_gc();
        let mut http = Request::post(&path, &body);
        http.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, http, &[200])?;
        crate::dto::parse_gc_status(&v)
    }

    /// The newest GC run's progress, without starting or advancing one.
    pub fn gc_status(&self) -> ClientResult<crate::dto::GcStatusDto> {
        let path = wire::path_gc();
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        crate::dto::parse_gc_status(&v)
    }

    /// Abandon the active run. Returns whether one was stopped; anything
    /// already collected stays collected.
    pub fn gc_cancel(&self) -> ClientResult<bool> {
        let path = wire::path_gc_cancel();
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        v.get("cancelled")
            .and_then(Value::as_bool)
            .ok_or(ClientError::Protocol { what: "gc cancel flag" })
    }

    pub fn publish_asset_revision(
        &self,
        asset: &AssetId,
        rev: &AssetRevisionId,
    ) -> ClientResult<()> {
        let path = wire::path_asset_publish(asset, rev);
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        self.call_json_accept(self.endpoints.control, req, &[200])?;
        Ok(())
    }

    /// Register a game identity in one authorization namespace. Supplying an
    /// id makes retries deterministic; omitting it asks the server to mint
    /// one.
    pub fn register_game(&self, ns: &str, id: Option<&GameId>) -> ClientResult<GameId> {
        let mut pairs: Vec<(&str, Value)> = vec![("namespace", json::s(ns))];
        if let Some(id) = id {
            pairs.push(("game_id", json::s(id.to_string())));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let path = wire::path_game_register();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let got = v
            .get("game_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<GameId>().ok())
            .ok_or(ClientError::Protocol {
                what: "register game_id",
            })?;
        if let Some(id) = id {
            if got != *id {
                return Err(ClientError::Protocol {
                    what: "register game id mismatch",
                });
            }
        }
        Ok(got)
    }

    /// Stage one canonical game manifest together with the exact canonical
    /// ContentLock bytes it names. The transport framing is deliberately
    /// private to this typed method so callers cannot accidentally drift from
    /// the server's two-document contract.
    pub fn stage_game_revision(
        &self,
        game: &GameId,
        canonical_manifest: &[u8],
        canonical_lock: &[u8],
    ) -> ClientResult<GameRevisionId> {
        for (what, bytes) in [
            ("game manifest bytes", canonical_manifest),
            ("game lock bytes", canonical_lock),
        ] {
            if bytes.is_empty() {
                return Err(ClientError::InvalidInput { what });
            }
            if bytes.len() as u64 > wire::MAX_MANIFEST_RESPONSE_BYTES {
                return Err(ClientError::OverBudget {
                    what,
                    limit: wire::MAX_MANIFEST_RESPONSE_BYTES,
                    found: bytes.len() as u64,
                });
            }
        }
        let capacity = 16usize
            .checked_add(canonical_manifest.len())
            .and_then(|n| n.checked_add(canonical_lock.len()))
            .ok_or(ClientError::OverBudget {
                what: "game revision frame bytes",
                limit: wire::MAX_MANIFEST_RESPONSE_BYTES.saturating_mul(2) + 16,
                found: u64::MAX,
            })?;
        let mut body = Vec::with_capacity(capacity);
        body.extend_from_slice(&(canonical_manifest.len() as u64).to_be_bytes());
        body.extend_from_slice(canonical_manifest);
        body.extend_from_slice(&(canonical_lock.len() as u64).to_be_bytes());
        body.extend_from_slice(canonical_lock);

        let local = GameRevisionId::hash_of(canonical_manifest);
        let path = wire::path_game_stage(game);
        let mut req = Request::post(&path, &body);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let got = v
            .get("revision")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<GameRevisionId>().ok())
            .ok_or(ClientError::Protocol {
                what: "stage game revision",
            })?;
        if got != local {
            return Err(ClientError::DigestMismatch {
                what: "staged game revision identity",
                expected: *local.as_bytes(),
                found: *got.as_bytes(),
            });
        }
        Ok(got)
    }

    pub fn publish_game_revision(
        &self,
        game: &GameId,
        rev: &GameRevisionId,
    ) -> ClientResult<()> {
        let path = wire::path_game_publish(game, rev);
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        self.call_json_accept(self.endpoints.control, req, &[200])?;
        Ok(())
    }

    pub fn put_game_alias(
        &self,
        alias: &GameAlias,
        game: &GameId,
        rev: &GameRevisionId,
    ) -> ClientResult<()> {
        let body = json::obj(vec![
            ("game_id", json::s(game.to_string())),
            ("revision", json::s(rev.to_string())),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_game_alias(alias);
        let mut req = Request::put(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        let got_game = v
            .get("game_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<GameId>().ok())
            .ok_or(ClientError::Protocol {
                what: "game alias game_id",
            })?;
        let got_rev = v
            .get("head_revision")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<GameRevisionId>().ok())
            .ok_or(ClientError::Protocol {
                what: "game alias head_revision",
            })?;
        if got_game != *game || got_rev != *rev {
            return Err(ClientError::Protocol {
                what: "game alias target mismatch",
            });
        }
        Ok(())
    }

    /// Point an alias head at an exact `{asset, revision}` pair.
    pub fn put_alias(
        &self,
        alias: &AssetAlias,
        asset: &AssetId,
        rev: &AssetRevisionId,
    ) -> ClientResult<()> {
        let body = json::obj(vec![
            ("asset_id", json::s(asset.to_string())),
            ("revision", json::s(rev.to_string())),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_alias(alias);
        let mut req = Request::put(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        let echoed = v
            .get("head_revision")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<AssetRevisionId>().ok())
            .ok_or(ClientError::Protocol { what: "alias head_revision" })?;
        if echoed != *rev {
            return Err(ClientError::Protocol { what: "alias head mismatch" });
        }
        Ok(())
    }

    /// Write the search annotation (title/kind/categories/tags/prompt…).
    pub fn put_annotation(&self, asset: &AssetId, ann: &AnnotationUpload) -> ClientResult<()> {
        ann.validate()?;
        let labels =
            |v: &[String]| Value::Arr(v.iter().map(|s| json::s(s.clone())).collect());
        let mut pairs: Vec<(&str, Value)> = vec![
            ("title", json::s(ann.title.clone())),
            ("description", json::s(ann.description.clone())),
            ("categories", labels(&ann.categories)),
            ("tags", labels(&ann.tags)),
            ("creator", json::s(ann.creator.clone())),
            ("generator", json::s(ann.generator.clone())),
            ("backend", json::s(ann.backend.clone())),
            ("model", json::s(ann.model.clone())),
            ("prompt", json::s(ann.prompt.clone())),
            ("provenance", json::s(ann.provenance.clone())),
            (
                "visibility",
                json::s(if ann.private { "private" } else { "public" }),
            ),
        ];
        if let Some(kind) = ann.kind {
            pairs.push(("kind", json::s(dto::kind_name(kind))));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let path = wire::path_annotation(asset);
        let mut req = Request::put(&path, &body);
        req.bearer = self.bearer();
        req.allow_no_content = true;
        let resp = http::http_call(self.endpoints.control, &req, &self.limits)?;
        self.accept(resp, &[200, 204])?;
        Ok(())
    }

    /// Open a blob body stream, optionally resuming from `range_start`.
    /// Returns the raw response (status 200, 206 or 416); the caller owns the
    /// resume math and digest accounting.
    pub fn blob_get(
        &self,
        blob: &BlobId,
        range_start: Option<u64>,
        body_deadline_ms: u64,
    ) -> ClientResult<Response> {
        let path = wire::path_blob(blob);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        req.body_deadline_ms = Some(body_deadline_ms);
        let etag;
        if let Some(start) = range_start {
            if start == 0 {
                return Err(ClientError::InvalidInput { what: "range start zero" });
            }
            req.range_start = Some(start);
            etag = blob.to_string();
            req.if_range = Some(&etag);
        }
        let resp = http::http_call(self.endpoints.data, &req, &self.limits)?;
        self.accept(resp, &[200, 206, 416])
    }

    // ---- typed asset operations --------------------------------------------

    /// The versioned operation registry with truthful availability.
    pub fn operation_types(&self) -> ClientResult<Vec<crate::dto::OperationTypeDto>> {
        let path = wire::path_operation_types();
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_operation_types(&v)
    }

    /// Create (or idempotently join) a typed operation. 201 = created,
    /// 200 = joined; both return the full status with `joined` set.
    pub fn operation_create(
        &self,
        request: &OperationCreateRequest,
    ) -> ClientResult<crate::dto::OperationStatusDto> {
        let body = request.body()?.to_json().into_bytes();
        let path = wire::path_operations();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200, 201])?;
        let status = dto::parse_operation_status(&v)?;
        if status.kind != request.kind || status.namespace != request.namespace {
            return Err(ClientError::Protocol { what: "operation create echo" });
        }
        Ok(status)
    }

    /// One owner-scoped status snapshot; the response must describe the
    /// operation that was asked for.
    pub fn operation_get(
        &self,
        op: &crate::dto::OperationId,
    ) -> ClientResult<crate::dto::OperationStatusDto> {
        let path = wire::path_operation(&op.to_string());
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let status = dto::parse_operation_status(&v)?;
        if status.operation != *op {
            return Err(ClientError::Protocol { what: "operation status id mismatch" });
        }
        Ok(status)
    }

    /// One page of the durable event log, `seq > after`, optionally long-
    /// polling up to `wait_ms` for new events.
    pub fn operation_events(
        &self,
        op: &crate::dto::OperationId,
        after: u64,
        wait_ms: u64,
        limit: u32,
    ) -> ClientResult<crate::dto::OperationEventsPageDto> {
        if wait_ms > wire::MAX_OPERATION_WAIT_MS {
            return Err(ClientError::InvalidInput { what: "operation wait too long" });
        }
        if limit == 0 || limit > 256 {
            return Err(ClientError::InvalidInput { what: "operation event limit" });
        }
        let path = wire::path_operation_events(&op.to_string(), after, wait_ms, limit);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        req.head_deadline_ms = Some(wait_ms + self.limits.head_deadline_ms);
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_operation_events(&v)
    }

    /// Cancel an operation (idempotent: false = already terminal).
    pub fn operation_cancel(&self, op: &crate::dto::OperationId) -> ClientResult<bool> {
        let path = wire::path_operation_cancel(&op.to_string());
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        v.get("cancelled")
            .and_then(Value::as_bool)
            .ok_or(ClientError::Protocol { what: "operation cancel" })
    }

    /// Retry a terminally failed/cancelled operation: arms the next round.
    pub fn operation_retry(
        &self,
        op: &crate::dto::OperationId,
    ) -> ClientResult<crate::dto::OperationStatusDto> {
        let path = wire::path_operation_retry(&op.to_string());
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let status = dto::parse_operation_status(&v)?;
        if status.operation != *op {
            return Err(ClientError::Protocol { what: "operation retry id mismatch" });
        }
        Ok(status)
    }

    /// Worker-side: upload typed completion facts; the server validates and
    /// finalizes atomically, returning the published identities.
    pub fn operation_finalize(
        &self,
        op: &crate::dto::OperationId,
        request: &OperationFinalizeRequest,
    ) -> ClientResult<(AssetId, AssetRevisionId)> {
        let body = request.body()?.to_json().into_bytes();
        let path = wire::path_operation_finalize(&op.to_string());
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let asset = v
            .get("asset")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<AssetId>().ok())
            .ok_or(ClientError::Protocol { what: "finalize asset" })?;
        let revision = v
            .get("revision")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<AssetRevisionId>().ok())
            .ok_or(ClientError::Protocol { what: "finalize revision" })?;
        Ok((asset, revision))
    }

    // ---- chat broker -------------------------------------------------------

    /// Honest provider availability. The response must not name keys or URLs.
    pub fn chat_providers(&self) -> ClientResult<Vec<crate::dto::ChatProviderDto>> {
        let path = wire::path_chat_providers();
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_chat_providers(&v)
    }

    pub fn chat_create(
        &self,
        request: &ChatCreateRequest,
    ) -> ClientResult<crate::dto::ChatSessionDto> {
        let body = request.body()?.to_json().into_bytes();
        let path = wire::path_chat_sessions();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let session = dto::parse_chat_session(&v)?;
        if session.namespace != request.namespace || session.provider != request.provider {
            return Err(ClientError::Protocol { what: "chat create echo" });
        }
        Ok(session)
    }

    pub fn chat_get(
        &self,
        id: &crate::dto::ChatSessionId,
    ) -> ClientResult<crate::dto::ChatSessionDto> {
        let path = wire::path_chat_session(&id.to_string());
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let session = dto::parse_chat_session(&v)?;
        if session.session != *id {
            return Err(ClientError::Protocol { what: "chat session id mismatch" });
        }
        Ok(session)
    }

    pub fn chat_send(
        &self,
        id: &crate::dto::ChatSessionId,
        request: &ChatSendRequest,
    ) -> ClientResult<u64> {
        let body = request.body()?.to_json().into_bytes();
        let path = wire::path_chat_send(&id.to_string());
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_chat_send(&v)
    }

    pub fn chat_events(
        &self,
        id: &crate::dto::ChatSessionId,
        after: u64,
        wait_ms: u64,
        limit: u32,
    ) -> ClientResult<crate::dto::ChatEventsPageDto> {
        if wait_ms > wire::MAX_CHAT_WAIT_MS {
            return Err(ClientError::InvalidInput { what: "chat wait too long" });
        }
        if limit == 0 || limit > 256 {
            return Err(ClientError::InvalidInput { what: "chat event limit" });
        }
        let path = wire::path_chat_events(&id.to_string(), after, wait_ms, limit);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        req.head_deadline_ms = Some(wait_ms + self.limits.head_deadline_ms);
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_chat_events(&v)
    }

    pub fn chat_cancel(
        &self,
        id: &crate::dto::ChatSessionId,
    ) -> ClientResult<crate::dto::ChatSessionDto> {
        let path = wire::path_chat_cancel(&id.to_string());
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let session = dto::parse_chat_session(&v)?;
        if session.session != *id {
            return Err(ClientError::Protocol { what: "chat cancel id mismatch" });
        }
        Ok(session)
    }

    pub fn chat_retire(&self, id: &crate::dto::ChatSessionId) -> ClientResult<bool> {
        let path = wire::path_chat_session(&id.to_string());
        let mut req = Request::delete(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_chat_retired(&v)
    }

    // ---- import + immutable derived variants -------------------------------
    //
    // There is no source get-by-id, no import validate, and no variant-set
    // finalize route. Import report/status use the derived import JSON
    // ceiling; every other JSON call stays at `MAX_JSON_RESPONSE_BYTES`.

    /// Register an approved source collection from its canonical bytes.
    /// The server's echoed `digest` must equal the SHA-256 of those bytes
    /// and `source_id` must equal the decoded collection id. Same-digest
    /// retries are idempotent and still answer 201.
    pub fn register_source_collection(
        &self,
        bytes: &[u8],
    ) -> ClientResult<SourceCollectionRegistered> {
        if bytes.is_empty() {
            return Err(ClientError::InvalidInput { what: "source collection bytes" });
        }
        if bytes.len() as u64 > wire::MAX_MANIFEST_RESPONSE_BYTES {
            return Err(ClientError::OverBudget {
                what: "source collection bytes",
                limit: wire::MAX_MANIFEST_RESPONSE_BYTES,
                found: bytes.len() as u64,
            });
        }
        let collection = SourceCollection::from_canonical_bytes(bytes)?;
        let expected = SourceCollectionId::hash_of(bytes);
        let path = wire::path_import_sources();
        let mut req = Request::put(&path, bytes);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let source_id = v
            .get("source_id")
            .and_then(Value::as_str)
            .ok_or(ClientError::Protocol { what: "source collection source_id" })?
            .to_string();
        if source_id != collection.id {
            return Err(ClientError::Protocol {
                what: "source collection id mismatch",
            });
        }
        let digest = v
            .get("digest")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<SourceCollectionId>().ok())
            .ok_or(ClientError::Protocol { what: "source collection digest" })?;
        if digest != expected {
            return Err(ClientError::DigestMismatch {
                what: "source collection digest",
                expected: *expected.as_bytes(),
                found: *digest.as_bytes(),
            });
        }
        Ok(SourceCollectionRegistered { source_id, digest })
    }

    /// One explicit source-collection page (`limit` always sent so the
    /// request never uses the legacy no-query listing). `cursor` is the last
    /// `source_id` of the previous page. Limit is `1..=MAX_SOURCE_PAGE_LIMIT`.
    pub fn source_collections_page(
        &self,
        cursor: Option<&str>,
        limit: u64,
    ) -> ClientResult<SourceCollectionsPageDto> {
        if limit == 0 || limit > wire::MAX_SOURCE_PAGE_LIMIT {
            return Err(ClientError::InvalidInput { what: "source page limit" });
        }
        if let Some(c) = cursor {
            if !wire::source_cursor_ok(c) {
                return Err(ClientError::InvalidInput { what: "source page cursor" });
            }
        }
        let path = wire::path_import_sources_page(cursor, limit);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self
            .call_json_limited(
                self.endpoints.control,
                req,
                &[200],
                wire::MAX_SOURCE_PAGE_JSON_RESPONSE_BYTES,
            )?
            .1;
        let page = dto::parse_source_collections_page(&v)?;
        if page.sources.len() as u64 > limit {
            return Err(ClientError::Protocol { what: "source page longer than limit" });
        }
        if page.cursor.is_some() && page.sources.len() as u64 != limit {
            return Err(ClientError::Protocol { what: "source page cursor on short page" });
        }
        if let Some(prev) = cursor {
            if page
                .sources
                .first()
                .is_some_and(|row| row.source_id.as_str() <= prev)
            {
                return Err(ClientError::Protocol { what: "source page does not follow cursor" });
            }
        }
        Ok(page)
    }

    /// Aggregate source collections by walking explicit pages. Fails closed
    /// above [`wire::MAX_PAGE_ENTRIES`] rather than returning a prefix.
    /// There is no get-by-id route for the canonical document.
    pub fn list_source_collections(&self) -> ClientResult<Vec<SourceCollectionRowDto>> {
        let mut acc: Vec<SourceCollectionRowDto> = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.source_collections_page(cursor.as_deref(), wire::MAX_SOURCE_PAGE_LIMIT)?;
            let found = acc.len() as u64 + page.sources.len() as u64;
            if found > wire::MAX_PAGE_ENTRIES as u64 {
                return Err(ClientError::OverBudget {
                    what: "source collections",
                    limit: wire::MAX_PAGE_ENTRIES as u64,
                    found,
                });
            }
            if let (Some(prev), Some(next)) = (acc.last(), page.sources.first()) {
                if next.source_id <= prev.source_id {
                    return Err(ClientError::Protocol { what: "source collection order" });
                }
            }
            let more = page.cursor.is_some();
            acc.extend(page.sources);
            if !more {
                return Ok(acc);
            }
            if acc.len() == wire::MAX_PAGE_ENTRIES {
                return Err(ClientError::OverBudget {
                    what: "source collections",
                    limit: wire::MAX_PAGE_ENTRIES as u64,
                    found: wire::MAX_PAGE_ENTRIES as u64 + 1,
                });
            }
            cursor = page.cursor;
        }
    }

    /// Run (or idempotently replay) one pack import from canonical manifest
    /// bytes. The echoed `import_revision` must be the digest of those
    /// bytes; every entry identity must match the content-contract mapping.
    /// 201 = first commit, 200 = exact-byte replay.
    pub fn run_import(&self, bytes: &[u8]) -> ClientResult<ImportReportDto> {
        if bytes.is_empty() {
            return Err(ClientError::InvalidInput { what: "import manifest bytes" });
        }
        if bytes.len() as u64 > wire::MAX_MANIFEST_RESPONSE_BYTES {
            return Err(ClientError::OverBudget {
                what: "import manifest bytes",
                limit: wire::MAX_MANIFEST_RESPONSE_BYTES,
                found: bytes.len() as u64,
            });
        }
        let manifest = ImportManifest::from_canonical_bytes(bytes)?;
        let expected = ImportRevisionId::hash_of(bytes);
        let path = wire::path_imports();
        let mut req = Request::post(&path, bytes);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let (status, v) = self.call_json_limited(
            self.endpoints.control,
            req,
            &[200, 201],
            wire::MAX_IMPORT_JSON_RESPONSE_BYTES,
        )?;
        let report = dto::parse_import_report(&v)?;
        if report.import_revision != expected {
            return Err(ClientError::DigestMismatch {
                what: "import revision",
                expected: *expected.as_bytes(),
                found: *report.import_revision.as_bytes(),
            });
        }
        if (status == 201) != report.created {
            return Err(ClientError::Protocol { what: "import created vs status" });
        }
        verify_import_entries(&manifest, &report.import_revision, &report.entries)?;
        Ok(report)
    }

    /// Import status projection for `revision`. The echoed id must equal
    /// the requested one. The route does not return canonical manifest bytes.
    pub fn import_status(&self, revision: &ImportRevisionId) -> ClientResult<ImportStatusDto> {
        let path = wire::path_import(revision);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self
            .call_json_limited(
                self.endpoints.control,
                req,
                &[200],
                wire::MAX_IMPORT_JSON_RESPONSE_BYTES,
            )?
            .1;
        let status = dto::parse_import_status(&v)?;
        if &status.import_revision != revision {
            return Err(ClientError::Protocol { what: "import status id mismatch" });
        }
        Ok(status)
    }

    /// Canonical derived-variant bytes, digest-verified against `id`.
    pub fn fetch_derived_variant_bytes(&self, id: &DerivedVariantId) -> ClientResult<Vec<u8>> {
        let path = wire::path_derived_variant(id);
        self.fetch_canonical_etag(
            &path,
            id.as_bytes(),
            &id.to_string(),
            "derived variant bytes",
        )
    }

    /// Freeze an immutable variant set. The echoed id must equal the
    /// digest of the canonical [`VariantSetManifest`] the request names.
    pub fn freeze_variant_set(
        &self,
        base: &AssetRevisionRef,
        variants: &[DerivedVariantId],
    ) -> ClientResult<VariantSetId> {
        let expected = expected_variant_set_id(base, variants)?;
        let body = json::obj(vec![
            ("base_asset", json::s(base.asset_id.to_string())),
            ("base_revision", json::s(base.revision.to_string())),
            (
                "variants",
                Value::Arr(variants.iter().map(|v| json::s(v.to_string())).collect()),
            ),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_variant_sets();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let got = v
            .get("variant_set")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<VariantSetId>().ok())
            .ok_or(ClientError::Protocol { what: "variant set id" })?;
        if got != expected {
            return Err(ClientError::DigestMismatch {
                what: "variant set identity",
                expected: *expected.as_bytes(),
                found: *got.as_bytes(),
            });
        }
        Ok(expected)
    }

    /// Canonical variant-set bytes, digest-verified against `id`.
    pub fn fetch_variant_set_bytes(&self, id: &VariantSetId) -> ClientResult<Vec<u8>> {
        let path = wire::path_variant_set(id);
        self.fetch_canonical_etag(
            &path,
            id.as_bytes(),
            &id.to_string(),
            "variant set bytes",
        )
    }

    /// Deterministic server-side resolution of one frozen set. The response
    /// is reconstructed as a canonical [`ResolvedVariantMap`] and refused
    /// unless its digest, set id, and profile digest match.
    pub fn resolve_variant_set(
        &self,
        set: &VariantSetId,
        profile: &ClientProfile,
    ) -> ClientResult<ResolvedVariantMap> {
        let expected_profile = profile.digest()?;
        let body = json::obj(vec![
            ("variant_set", json::s(set.to_string())),
            ("profile", profile_json(profile)?),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_variant_resolutions();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let dto = dto::parse_resolved_variant_map(&v)?;
        if &dto.set != set {
            return Err(ClientError::Protocol { what: "resolution variant_set mismatch" });
        }
        if dto.profile != expected_profile {
            return Err(ClientError::DigestMismatch {
                what: "resolution profile digest",
                expected: *expected_profile.as_bytes(),
                found: *dto.profile.as_bytes(),
            });
        }
        let map = ResolvedVariantMap {
            set: dto.set,
            profile: dto.profile,
            entries: dto.entries,
        };
        map.validate()?;
        let digest = map.digest()?;
        if digest != dto.digest {
            return Err(ClientError::DigestMismatch {
                what: "resolution map digest",
                expected: *digest.as_bytes(),
                found: *dto.digest.as_bytes(),
            });
        }
        Ok(map)
    }

    fn fetch_canonical_etag(
        &self,
        path: &str,
        expected: &[u8; 32],
        etag: &str,
        what: &'static str,
    ) -> ClientResult<Vec<u8>> {
        let mut req = Request::get(path);
        req.bearer = self.bearer();
        let resp = http::http_call(self.endpoints.control, &req, &self.limits)?;
        let resp = self.accept(resp, &[200])?;
        if resp.head().etag.as_deref() != Some(etag) {
            return Err(ClientError::Protocol { what: "canonical etag mismatch" });
        }
        if resp.head().content_length > wire::MAX_MANIFEST_RESPONSE_BYTES {
            return Err(ClientError::OverBudget {
                what,
                limit: wire::MAX_MANIFEST_RESPONSE_BYTES,
                found: resp.head().content_length,
            });
        }
        let bytes = resp.read_full(wire::MAX_MANIFEST_RESPONSE_BYTES)?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let found = hasher.finalize();
        if &found != expected {
            return Err(ClientError::DigestMismatch { what, expected: *expected, found });
        }
        Ok(bytes)
    }
}

fn verify_import_entries(
    manifest: &ImportManifest,
    irev: &ImportRevisionId,
    entries: &[crate::dto::ImportEntryDto],
) -> ClientResult<()> {
    if entries.len() != manifest.assets.len() {
        return Err(ClientError::Protocol { what: "import entry count" });
    }
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        if !seen.insert(&entry.key) {
            return Err(ClientError::Protocol { what: "import entry key duplicate" });
        }
        let asset = manifest
            .assets
            .iter()
            .find(|a| a.key == entry.key)
            .ok_or(ClientError::Protocol { what: "import entry key" })?;
        let expected_asset = manifest.asset_id_for(&asset.key);
        if entry.asset_id != expected_asset {
            return Err(ClientError::Protocol { what: "import entry asset_id mismatch" });
        }
        let produced = manifest.asset_manifest_for(asset, irev)?;
        let expected_rev = produced.revision()?;
        if entry.revision != expected_rev {
            return Err(ClientError::DigestMismatch {
                what: "import entry revision",
                expected: *expected_rev.as_bytes(),
                found: *entry.revision.as_bytes(),
            });
        }
        let expected_alias = manifest.alias_for(&asset.key)?;
        match &entry.alias {
            Some(alias) if alias == &expected_alias => {}
            _ => return Err(ClientError::Protocol { what: "import entry alias mismatch" }),
        }
    }
    Ok(())
}

fn expected_variant_set_id(
    base: &AssetRevisionRef,
    variants: &[DerivedVariantId],
) -> ClientResult<VariantSetId> {
    if variants.is_empty() {
        return Err(ClientError::InvalidInput { what: "variant set variants" });
    }
    if variants.len() > makepad_asset_data::limits::MAX_VARIANTS_PER_SET {
        return Err(ClientError::InvalidInput { what: "variant set variants" });
    }
    let mut set = VariantSetManifest {
        base: *base,
        variants: variants.to_vec(),
        policy_version: RESOLUTION_POLICY_V1,
    };
    set.canonicalize();
    Ok(set.id()?)
}

fn profile_json(profile: &ClientProfile) -> ClientResult<Value> {
    profile.validate()?;
    if profile.max_variant_bytes > i64::MAX as u64 {
        return Err(ClientError::InvalidInput {
            what: "profile max_variant_bytes",
        });
    }
    let mut accept = Vec::new();
    if profile.accept_png {
        accept.push(json::s("png"));
    }
    if profile.accept_jpeg {
        accept.push(json::s("jpeg"));
    }
    if profile.accept_glb {
        accept.push(json::s("glb"));
    }
    if profile.accept_bin {
        accept.push(json::s("bin"));
    }
    Ok(json::obj(vec![
        ("policy_version", Value::Int(profile.policy_version as i64)),
        ("tier", json::s(dto::tier_name(profile.tier))),
        ("max_texture_dim", Value::Int(profile.max_texture_dim as i64)),
        ("max_triangles", Value::Int(profile.max_triangles as i64)),
        (
            "max_variant_bytes",
            Value::Int(profile.max_variant_bytes as i64),
        ),
        ("accept", Value::Arr(accept)),
    ]))
}

/// Worker suffixes are transport-shaped: 1..=32 of `[a-z0-9_-]`.
fn check_worker_suffix(suffix: Option<&str>) -> ClientResult<()> {
    let Some(s) = suffix else { return Ok(()) };
    let ok = !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(ClientError::InvalidInput { what: "worker suffix" })
    }
}

/// Worker kind filters use the job contract's `[a-z0-9_.-]`, 1..=64-byte
/// vocabulary. The list itself is deliberately small and duplicate-free so
/// transport and server-side SQL stay bounded and unambiguous.
fn check_worker_kinds(kinds: &[&str]) -> ClientResult<()> {
    if kinds.is_empty() || kinds.len() > 32 {
        return Err(ClientError::InvalidInput { what: "worker kinds count" });
    }
    for (index, kind) in kinds.iter().enumerate() {
        let valid = !kind.is_empty()
            && kind.len() <= 64
            && kind.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b)
            });
        if !valid {
            return Err(ClientError::InvalidInput { what: "worker kind" });
        }
        if kinds[..index].contains(kind) {
            return Err(ClientError::InvalidInput { what: "worker kind duplicate" });
        }
    }
    Ok(())
}

/// A cursor about to be SENT must still be shaped like the cursors this
/// client accepts from responses (defense in depth around callers that
/// persist cursors).
fn check_cursor_out(c: &str) -> ClientResult<()> {
    if c.is_empty() || c.len() > wire::MAX_CURSOR_BYTES || !wire::query_value_ok(c) {
        return Err(ClientError::InvalidInput { what: "cursor shape" });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_validation() {
        assert!(CatalogQuery::browse(0).validate().is_err());
        assert!(CatalogQuery::browse(MAX_SEARCH_LIMIT + 1).validate().is_err());
        assert!(CatalogQuery::text("rocket", 10).validate().is_ok());
        assert!(CatalogQuery::text("bad\u{7}", 10).validate().is_err());
        let long = "x".repeat(wire::MAX_QUERY_TEXT_BYTES + 1);
        assert!(CatalogQuery::text(long, 10).validate().is_err());
        let mut q = CatalogQuery::browse(10);
        q.namespace = Some(String::new());
        assert!(q.validate().is_err());
        // `exclude_tag` is bounded exactly like the other filter values.
        let mut q = CatalogQuery::browse(10);
        q.exclude_tag = Some(String::new());
        assert!(q.validate().is_err());
        q.exclude_tag = Some("bad\u{7}".into());
        assert!(q.validate().is_err());
        q.exclude_tag = Some("x".repeat(wire::MAX_FILTER_VALUE_BYTES + 1));
        assert!(q.validate().is_err());
        q.exclude_tag = Some("intermediate".into());
        assert!(q.validate().is_ok());
    }

    #[test]
    fn query_body_shape() {
        let mut q = CatalogQuery::text("fancy rocket", 25);
        q.kind = Some(AssetKind::Mesh);
        q.live_only = true;
        let body = q.body(Some("ab12"));
        assert_eq!(body.get("q").unwrap().as_str(), Some("fancy rocket"));
        assert_eq!(body.get("kind").unwrap().as_str(), Some("mesh"));
        assert_eq!(body.get("live").unwrap().as_bool(), Some(true));
        assert_eq!(body.get("limit").unwrap().as_i64(), Some(25));
        assert_eq!(body.get("cursor").unwrap().as_str(), Some("ab12"));
        assert!(q.body(None).get("cursor").is_none());
        // Absent filters emit no key at all; set ones travel verbatim.
        assert!(body.get("tag").is_none());
        assert!(body.get("exclude_tag").is_none());
        q.tag = Some("keep".into());
        q.exclude_tag = Some("intermediate".into());
        let body = q.body(None);
        assert_eq!(body.get("tag").unwrap().as_str(), Some("keep"));
        assert_eq!(body.get("exclude_tag").unwrap().as_str(), Some("intermediate"));
    }

    #[test]
    fn token_shape_enforced_at_construction() {
        let endpoints = ApiEndpoints {
            control: "127.0.0.1:1".parse().unwrap(),
            data: "127.0.0.1:2".parse().unwrap(),
        };
        assert!(Api::new(endpoints, HttpLimits::default_v1(), Some("garbage".into())).is_err());
        let good = format!("mpat_{}", "ab".repeat(32));
        assert!(Api::new(endpoints, HttpLimits::default_v1(), Some(good)).is_ok());
        assert!(Api::new(endpoints, HttpLimits::default_v1(), None).is_ok());
    }

    #[test]
    fn outgoing_cursor_shape() {
        assert!(check_cursor_out("abc123").is_ok());
        assert!(check_cursor_out("").is_err());
        assert!(check_cursor_out("a b").is_err());
        let long = "a".repeat(wire::MAX_CURSOR_BYTES + 1);
        assert!(check_cursor_out(&long).is_err());
    }

    #[test]
    fn worker_kind_filters_are_strict_and_bounded() {
        assert!(check_worker_kinds(&["video.generate"]).is_ok());
        assert!(check_worker_kinds(&[]).is_err());
        assert!(check_worker_kinds(&["video.generate", "video.generate"]).is_err());
        assert!(check_worker_kinds(&["Music Generate"]).is_err());
        let many = vec!["video.generate"; 33];
        assert!(check_worker_kinds(&many).is_err());
    }

    #[test]
    fn source_page_limit_is_local() {
        let endpoints = ApiEndpoints {
            control: "127.0.0.1:1".parse().unwrap(),
            data: "127.0.0.1:2".parse().unwrap(),
        };
        let api = Api::new(endpoints, HttpLimits::default_v1(), None).unwrap();
        let start = std::time::Instant::now();
        match api.source_collections_page(None, 501) {
            Err(ClientError::InvalidInput { what }) => assert_eq!(what, "source page limit"),
            other => panic!("501 must refuse locally, got {other:?}"),
        }
        match api.source_collections_page(None, 0) {
            Err(ClientError::InvalidInput { what }) => assert_eq!(what, "source page limit"),
            other => panic!("0 must refuse locally, got {other:?}"),
        }
        match api.source_collections_page(Some("Kenney"), 10) {
            Err(ClientError::InvalidInput { what }) => assert_eq!(what, "source page cursor"),
            other => panic!("bad cursor must refuse locally, got {other:?}"),
        }
        assert!(
            start.elapsed() < std::time::Duration::from_millis(50),
            "must not touch the network"
        );
    }

    #[test]
    fn profile_json_refuses_unrepresentable_max_variant_bytes_without_io() {
        let ok = ClientProfile {
            policy_version: RESOLUTION_POLICY_V1,
            tier: DeviceTier::High,
            max_texture_dim: 2048,
            max_triangles: 1_000_000,
            max_variant_bytes: i64::MAX as u64,
            accept_png: true,
            accept_jpeg: false,
            accept_glb: true,
            accept_bin: false,
        };
        assert!(profile_json(&ok).is_ok());
        let mut too_big = ok;
        too_big.max_variant_bytes = (i64::MAX as u64) + 1;
        match profile_json(&too_big) {
            Err(ClientError::InvalidInput { what }) => {
                assert_eq!(what, "profile max_variant_bytes")
            }
            other => panic!("expected local InvalidInput, got {other:?}"),
        }
        let endpoints = ApiEndpoints {
            control: "127.0.0.1:1".parse().unwrap(),
            data: "127.0.0.1:2".parse().unwrap(),
        };
        let api = Api::new(endpoints, HttpLimits::default_v1(), None).unwrap();
        let start = std::time::Instant::now();
        match api.resolve_variant_set(&VariantSetId::from_bytes([1; 32]), &too_big) {
            Err(ClientError::InvalidInput { what }) => {
                assert_eq!(what, "profile max_variant_bytes")
            }
            other => panic!("must refuse locally, got {other:?}"),
        }
        assert!(
            start.elapsed() < std::time::Duration::from_millis(50),
            "must not touch the network"
        );
    }
}
