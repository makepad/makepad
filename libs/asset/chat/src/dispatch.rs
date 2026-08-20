//! Server-side execution of the content tool allowlist against a real
//! Asset Server, through the hardened `makepad-asset-client` API.
//!
//! This is where every requirement that must NOT live in a UI client lands:
//! the bearer token (never on the chat wire), honest capability filtering
//! (the server's registered operation types with live worker availability),
//! input pinning (operation inputs must be session-known revisions), typed
//! operation creation (the server resolves and pins the exact file,
//! validates rights, and owns publication), and cancellation.
//!
//! Immutability and privilege are structural: the tool surface has no raw
//! job enqueue, no publish, and no alias mutation — outputs appear only
//! through the server's atomic operation finalizer, which records exact
//! parent lineage and inherited rights inside the immutable manifest.

use crate::session::{CancelFlag, ExecCtx, SessionId, ToolExecutor};
use crate::tools::{
    canonicalize_json, encode_args, AliasExpectArg, ContentToolCall, InspectTarget,
    OperationInputArg, PublicationArg,
};
use crate::wire::{sanitize_public_error, ToolOutcome};
use makepad_asset_client::error::{ClientError, ClientResult};
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::{
    Api, ApiEndpoints, CatalogQuery, HttpLimits, OperationAliasExpect, OperationCreateRequest,
    OperationId, OperationInputRef, OperationPublicationRef, OperationStatusDto, OperationTypeDto,
};
use makepad_asset_data::{
    sha256, AssetKind, AssetManifest, AssetRevisionId, DeviceTier, FileRole, MediaType,
};
use std::collections::{HashMap, HashSet, VecDeque};

fn call_prompt(call: &ContentToolCall) -> Option<&str> {
    match call {
        ContentToolCall::ImageGenerate { prompt, .. }
        | ContentToolCall::VideoGenerate { prompt, .. }
        | ContentToolCall::AudioGenerate { prompt, .. }
        | ContentToolCall::SpeechGenerate { prompt, .. }
        | ContentToolCall::MusicGenerate { prompt, .. }
        | ContentToolCall::MeshGenerate { prompt, .. }
        | ContentToolCall::WorldGenerate { prompt, .. }
        | ContentToolCall::CharacterGenerate { prompt, .. } => Some(prompt.as_str()),
        _ => None,
    }
}

/// Poll cadence of `operation.wait`.
const AWAIT_POLL_MS: u64 = 200;
/// Distinct chat sessions retained by one dispatcher.
const MAX_KNOWN_SESSIONS: usize = 64;
/// Operations remembered per session before oldest-first eviction.
const MAX_KNOWN_OPS_PER_SESSION: usize = 32;

/// Bounded, fail-closed session → operation index. Evicted or retired
/// entries are forgotten: later get/wait/cancel/retry is Denied.
struct SessionOpIndex {
    order: VecDeque<SessionId>,
    by_session: HashMap<SessionId, SessionOps>,
}

struct SessionOps {
    order: VecDeque<OperationId>,
    set: HashSet<OperationId>,
}

impl SessionOpIndex {
    fn new() -> SessionOpIndex {
        SessionOpIndex { order: VecDeque::new(), by_session: HashMap::new() }
    }

    fn remember(&mut self, session: &SessionId, operation: OperationId) {
        if let Some(ops) = self.by_session.get_mut(session) {
            if ops.set.contains(&operation) {
                return;
            }
            if ops.set.len() >= MAX_KNOWN_OPS_PER_SESSION {
                if let Some(old) = ops.order.pop_front() {
                    ops.set.remove(&old);
                }
            }
            ops.order.push_back(operation);
            ops.set.insert(operation);
            return;
        }
        if self.order.len() >= MAX_KNOWN_SESSIONS {
            if let Some(old) = self.order.pop_front() {
                self.by_session.remove(&old);
            }
        }
        let mut ops = SessionOps { order: VecDeque::new(), set: HashSet::new() };
        ops.order.push_back(operation);
        ops.set.insert(operation);
        self.by_session.insert(session.clone(), ops);
        self.order.push_back(session.clone());
    }

    fn contains(&self, session: &SessionId, operation: &OperationId) -> bool {
        self.by_session.get(session).is_some_and(|ops| ops.set.contains(operation))
    }

    fn retire(&mut self, session: &SessionId) -> bool {
        let removed = self.by_session.remove(session).is_some();
        self.order.retain(|s| s != session);
        removed
    }
}

pub struct AssetServerTools {
    api: Api,
    /// Namespace this gateway creates operations in (its working project).
    namespace: String,
    types: Vec<OperationTypeDto>,
    types_err: Option<String>,
    /// Operations this dispatcher created or joined, keyed by chat session.
    known_ops: SessionOpIndex,
}

impl AssetServerTools {
    /// Connect with the SERVER-side bearer token (broker or service
    /// deployment) and the namespace operations are created in. UI clients
    /// never construct this.
    pub fn connect(
        endpoints: ApiEndpoints,
        token: Option<String>,
        namespace: impl Into<String>,
    ) -> ClientResult<AssetServerTools> {
        let api = Api::new(endpoints, HttpLimits::default_v1(), token)?;
        let mut tools = AssetServerTools {
            api,
            namespace: namespace.into(),
            types: Vec::new(),
            types_err: None,
            known_ops: SessionOpIndex::new(),
        };
        tools.refresh_types();
        Ok(tools)
    }

    /// Forget every operation bound to `session`. Subsequent get, wait,
    /// cancel, and retry for those ops is Denied.
    pub fn retire_session(&mut self, session: &SessionId) -> bool {
        self.known_ops.retire(session)
    }

    /// Re-probe the registered operation types (availability is live).
    pub fn refresh_types(&mut self) {
        match self.api.operation_types() {
            Ok(t) => {
                self.types = t;
                self.types_err = None;
            }
            Err(e) => self.types_err = Some(public_error(&e.to_string())),
        }
    }

    pub fn operation_types(&self) -> &[OperationTypeDto] {
        &self.types
    }

    fn remember_operation(&mut self, session: &SessionId, operation: OperationId) {
        self.known_ops.remember(session, operation);
    }

    fn session_may_control(&self, session: &SessionId, operation: &OperationId) -> bool {
        self.known_ops.contains(session, operation)
    }

    fn foreign_operation() -> ToolOutcome {
        ToolOutcome::Denied { what: "operation is not bound to this session".to_string() }
    }

    // ------------------------------------------------------------ executors

    fn asset_search(&self, query: &str, limit: u32) -> ToolOutcome {
        let mut q = CatalogQuery::browse(limit);
        q.text = query.to_string();
        match self.api.catalog_search(&q, None) {
            Err(e) => err_outcome(e),
            Ok(page) => ToolOutcome::Ok {
                value: json::obj(vec![(
                    "hits",
                    Value::Arr(
                        page.hits
                            .iter()
                            .map(|h| {
                                let mut pairs = vec![
                                    ("asset", json::s(h.asset_id.to_string())),
                                    ("title", json::s(h.title.clone())),
                                    ("namespace", json::s(h.namespace.clone())),
                                ];
                                if let Some(k) = h.kind {
                                    pairs.push(("kind", json::s(kind_label(k))));
                                }
                                if let Some(a) = &h.alias {
                                    pairs.push(("alias", json::s(a.to_string())));
                                }
                                json::obj(pairs)
                            })
                            .collect(),
                    ),
                )]),
            },
        }
    }

    fn inspect(&self, target: &InspectTarget) -> ToolOutcome {
        match target {
            InspectTarget::Revision(rev) => self.revision_summary(rev),
            InspectTarget::Alias(alias) => match self.api.resolve_alias(alias) {
                Err(e) => err_outcome(e),
                Ok(dto) => ToolOutcome::Ok {
                    value: json::obj(vec![
                        ("alias", json::s(dto.alias.to_string())),
                        ("asset", json::s(dto.asset_id.to_string())),
                        ("revision", json::s(dto.head_revision.to_string())),
                    ]),
                },
            },
            InspectTarget::Asset(id) => match self.api.asset_detail(id) {
                Err(e) => err_outcome(e),
                Ok(detail) => ToolOutcome::Ok {
                    value: json::obj(vec![
                        ("asset", json::s(detail.asset_id.to_string())),
                        ("namespace", json::s(detail.namespace.clone())),
                        (
                            "candidates",
                            Value::Arr(
                                detail
                                    .candidates
                                    .iter()
                                    .map(|c| {
                                        json::obj(vec![
                                            ("revision", json::s(c.revision.to_string())),
                                            ("state", json::s(c.state.as_str())),
                                        ])
                                    })
                                    .collect(),
                            ),
                        ),
                    ]),
                },
            },
        }
    }

    fn revision_summary(&self, rev: &AssetRevisionId) -> ToolOutcome {
        let bytes = match self.api.fetch_revision_bytes(rev) {
            Ok(b) => b,
            Err(e) => return err_outcome(e),
        };
        let manifest = match AssetManifest::from_canonical_bytes(&bytes) {
            Ok(m) => m,
            Err(e) => {
                return ToolOutcome::Failed { message: public_error(&format!("manifest decode: {e}")) }
            }
        };
        let mut pairs = vec![
            ("revision", json::s(rev.to_string())),
            ("asset", json::s(manifest.asset_id.to_string())),
            ("kind", json::s(kind_label(manifest.kind))),
            ("files", Value::Int(manifest.files.len() as i64)),
        ];
        if let Some(p) = &manifest.provenance {
            pairs.push((
                "parents",
                Value::Arr(p.parents.iter().map(|r| json::s(r.to_string())).collect()),
            ));
        }
        ToolOutcome::Ok { value: json::obj(pairs) }
    }

    fn capabilities_value(&mut self) -> ToolOutcome {
        self.refresh_types();
        if let Some(e) = &self.types_err {
            return ToolOutcome::Failed { message: public_error(&format!("operation types unavailable: {e}")) };
        }
        ToolOutcome::Ok {
            value: json::obj(vec![(
                "operations",
                Value::Arr(
                    self.types
                        .iter()
                        .map(|t| {
                            let mut pairs = vec![
                                ("kind", json::s(t.kind.clone())),
                                ("label", json::s(t.label.clone())),
                                ("description", json::s(t.description.clone())),
                                ("available", Value::Bool(t.available)),
                                ("inputs", t.inputs.clone()),
                                ("params", t.params.clone()),
                                ("outputs", t.outputs.clone()),
                            ];
                            if let Some(reason) = &t.unavailable_reason {
                                pairs.push(("unavailable_reason", json::s(public_error(reason))));
                            }
                            json::obj(pairs)
                        })
                        .collect(),
                ),
            )]),
        }
    }

    fn create(
        &mut self,
        kind: &str,
        inputs: &[OperationInputArg],
        params: &Value,
        publication: &PublicationArg,
        idempotency_key: &Option<String>,
        ctx: &ExecCtx,
    ) -> ToolOutcome {
        // Honest availability BEFORE any state changes: an unregistered or
        // worker-less operation is a structured Unavailable, never an enqueue.
        self.refresh_types();
        let Some(op_type) = self.types.iter().find(|t| t.kind == kind) else {
            let known: Vec<&str> = self.types.iter().map(|t| t.kind.as_str()).collect();
            let detail = match &self.types_err {
                Some(e) => format!("operation types unavailable: {e}"),
                None => format!("registered operations: {known:?}"),
            };
            return ToolOutcome::Unavailable {
                reason: public_error(&format!(
                    "operation '{kind}' is not registered on this server ({detail})"
                )),
            };
        };
        if !op_type.available {
            return ToolOutcome::Unavailable {
                reason: public_error(&format!(
                    "operation '{kind}' is currently unavailable: {}",
                    op_type.unavailable_reason.as_deref().unwrap_or("no live worker")
                )),
            };
        }
        // Input pinning: the model may only name revisions this session has
        // legitimately seen. Refused BEFORE any server call.
        for input in inputs {
            if !ctx.known.contains(&input.revision) {
                return ToolOutcome::Refused {
                    what: format!(
                        "input {} is not bound to this session (attach it or obtain it from a tool result)",
                        input.revision
                    ),
                };
            }
        }
        let mut typed_inputs = Vec::with_capacity(inputs.len());
        for input in inputs {
            let role = match role_parse(&input.role) {
                Some(r) => r,
                None => {
                    return ToolOutcome::Refused {
                        what: format!("unknown input role '{}'", input.role),
                    }
                }
            };
            let tier = match &input.tier {
                None => None,
                Some(t) => match tier_parse(t) {
                    Some(t) => Some(t),
                    None => {
                        return ToolOutcome::Refused { what: format!("unknown input tier '{t}'") }
                    }
                },
            };
            let media = match &input.media {
                None => None,
                Some(m) => match media_parse(m) {
                    Some(m) => Some(m),
                    None => {
                        return ToolOutcome::Refused { what: format!("unknown input media '{m}'") }
                    }
                },
            };
            typed_inputs.push(OperationInputRef {
                slot: input.slot.clone(),
                asset: input.asset,
                revision: input.revision,
                role,
                tier,
                lod: input.lod,
                expected_media: media,
            });
        }
        // SessionId scopes idempotency hashing and in-process ownership
        // only. Origin.principal is not forwarded: OperationCreateRequest
        // has no audit/session field in this foundation.
        let session = &ctx.origin.session;
        let key = match idempotency_key {
            Some(k) => session_bound_key(session, k.as_bytes()),
            None => {
                let call = ContentToolCall::OperationCreate {
                    kind: kind.to_string(),
                    inputs: inputs.to_vec(),
                    params: params.clone(),
                    publication: publication.clone(),
                    idempotency_key: None,
                };
                let material = canonicalize_json(&encode_args(&call)).to_json();
                session_bound_key(session, material.as_bytes())
            }
        };
        let mut request =
            OperationCreateRequest::new(self.namespace.clone(), kind, key, typed_inputs);
        request.params = params.clone();
        request.publication = match publication {
            PublicationArg::Publish => OperationPublicationRef::Publish,
            PublicationArg::PublishAndAlias { alias, expect } => {
                OperationPublicationRef::PublishAndAlias {
                    alias: alias.clone(),
                    expect: match expect {
                        AliasExpectArg::Any => OperationAliasExpect::Any,
                        AliasExpectArg::Absent => OperationAliasExpect::Absent,
                        AliasExpectArg::Head(rev) => OperationAliasExpect::Head(*rev),
                    },
                }
            }
        };
        match self.api.operation_create(&request) {
            Err(e) => err_outcome(e),
            Ok(status) => {
                self.remember_operation(session, status.operation);
                ToolOutcome::Ok { value: status_value(&status) }
            }
        }
    }

    fn wait(
        &self,
        operation: &OperationId,
        after: u64,
        timeout_ms: u64,
        progress: &mut dyn FnMut(u16, &str),
        cancel: &CancelFlag,
    ) -> ToolOutcome {
        let start = std::time::Instant::now();
        let mut cancelled = false;
        loop {
            if cancel.is_cancelled() && !cancelled {
                // Propagate the user's cancel to the server-side operation,
                // then keep polling so the returned state is the truth.
                let _ = self.api.operation_cancel(operation);
                cancelled = true;
            }
            let status = match self.api.operation_get(operation) {
                Ok(s) => s,
                Err(e) => return err_outcome(e),
            };
            if let Some(p) = &status.progress {
                let note = public_error(&p.note);
                progress(p.permille, &note);
            }
            if status.state.is_terminal() {
                let events = self
                    .api
                    .operation_events(operation, after, 0, 64)
                    .map(|page| {
                        Value::Arr(
                            page.events
                                .iter()
                                .map(|e| {
                                    json::obj(vec![
                                        ("seq", Value::Int(e.seq.min(i64::MAX as u64) as i64)),
                                        ("kind", json::s(e.kind.clone())),
                                        ("detail", json::s(public_error(&e.detail))),
                                    ])
                                })
                                .collect(),
                        )
                    })
                    .unwrap_or(Value::Arr(Vec::new()));
                let mut value = status_value(&status);
                if let Value::Obj(pairs) = &mut value {
                    pairs.push(("events".to_string(), events));
                }
                return ToolOutcome::Ok { value };
            }
            if start.elapsed().as_millis() as u64 >= timeout_ms {
                return ToolOutcome::Failed {
                    message: format!(
                        "operation still '{}' after {timeout_ms} ms",
                        status.state.as_str()
                    ),
                };
            }
            std::thread::sleep(std::time::Duration::from_millis(AWAIT_POLL_MS));
        }
    }
}

impl ToolExecutor for AssetServerTools {
    fn capability_doc(&mut self) -> String {
        self.refresh_types();
        if let Some(e) = &self.types_err {
            return public_error(&format!(
                "Operation types are currently unavailable: {e}. Catalog inspection tools still work."
            ));
        }
        if self.types.is_empty() {
            return "This server registers no operation types.".to_string();
        }
        let mut out = String::from("Registered operations (for operation.create):\n");
        for t in &self.types {
            out.push_str("- ");
            out.push_str(&t.kind);
            if t.available {
                out.push_str(" [available]");
            } else {
                out.push_str(" [UNAVAILABLE: ");
                out.push_str(&public_error(
                    t.unavailable_reason.as_deref().unwrap_or("no live worker"),
                ));
                out.push(']');
            }
            out.push_str(": ");
            out.push_str(&t.label);
            out.push('\n');
        }
        out.push_str(&format!(
            "Operations are created in namespace '{}'. Inputs must be exact revisions \
             bound to this session.\n",
            self.namespace
        ));
        out
    }

    fn execute(
        &mut self,
        call: &ContentToolCall,
        ctx: &ExecCtx,
        progress: &mut dyn FnMut(u16, &str),
        cancel: &CancelFlag,
    ) -> ToolOutcome {
        match call {
            ContentToolCall::ImageGenerate { .. }
            | ContentToolCall::VideoGenerate { .. }
            | ContentToolCall::AudioGenerate { .. }
            | ContentToolCall::SpeechGenerate { .. }
            | ContentToolCall::MusicGenerate { .. }
            | ContentToolCall::MeshGenerate { .. }
            | ContentToolCall::WorldGenerate { .. }
            | ContentToolCall::CharacterGenerate { .. } => ToolOutcome::Ok {
                value: json::obj(vec![
                    ("queued", Value::Bool(true)),
                    ("tool", json::s(call.name())),
                    ("prompt", json::s(call_prompt(call).unwrap_or("").to_string())),
                    (
                        "note",
                        json::s(
                            "fleet generate tools are executed by the AI Content app; \
                             the Asset Server dispatcher only acknowledges the request",
                        ),
                    ),
                ]),
            },
            ContentToolCall::DefaultsGet
            | ContentToolCall::DefaultsSet { .. }
            | ContentToolCall::FleetIntrospect { .. } => ToolOutcome::Unavailable {
                reason: "defaults and fleet introspection are owned by the AI Content chat session"
                    .into(),
            },
            ContentToolCall::AssetSearch { query, limit } => self.asset_search(query, *limit),
            ContentToolCall::AssetInspect { target } => self.inspect(target),
            ContentToolCall::OperationCapabilities => self.capabilities_value(),
            ContentToolCall::OperationCreate {
                kind,
                inputs,
                params,
                publication,
                idempotency_key,
            } => self.create(kind, inputs, params, publication, idempotency_key, ctx),
            ContentToolCall::OperationGet { operation } => {
                if !self.session_may_control(&ctx.origin.session, operation) {
                    return Self::foreign_operation();
                }
                match self.api.operation_get(operation) {
                    Ok(status) => ToolOutcome::Ok { value: status_value(&status) },
                    Err(e) => err_outcome(e),
                }
            }
            ContentToolCall::OperationWait { operation, after, timeout_ms } => {
                if !self.session_may_control(&ctx.origin.session, operation) {
                    return Self::foreign_operation();
                }
                self.wait(operation, *after, *timeout_ms, progress, cancel)
            }
            ContentToolCall::OperationCancel { operation } => {
                if !self.session_may_control(&ctx.origin.session, operation) {
                    return Self::foreign_operation();
                }
                match self.api.operation_cancel(operation) {
                    Ok(changed) => ToolOutcome::Ok {
                        value: json::obj(vec![
                            ("operation", json::s(operation.to_string())),
                            ("cancelled", Value::Bool(changed)),
                        ]),
                    },
                    Err(e) => err_outcome(e),
                }
            }
            ContentToolCall::OperationRetry { operation } => {
                if !self.session_may_control(&ctx.origin.session, operation) {
                    return Self::foreign_operation();
                }
                match self.api.operation_retry(operation) {
                    Ok(status) => ToolOutcome::Ok { value: status_value(&status) },
                    Err(e) => err_outcome(e),
                }
            }
            ContentToolCall::LlmConsult { .. } => ToolOutcome::Unavailable {
                reason: "llm.consult is executed by the chat broker".to_string(),
            },
            // Game-client tools: the broker's dispatcher never advertises
            // them (see `ToolExecutor::tool_definitions`); a model that
            // calls one anyway gets the honest answer, not an execution.
            ContentToolCall::AssetsQuery { .. }
            | ContentToolCall::AssetsSchema
            | ContentToolCall::WorldPlace { .. }
            | ContentToolCall::WorldRemove { .. }
            | ContentToolCall::WorldMove { .. }
            | ContentToolCall::WorldList
            | ContentToolCall::WorldGetSource
            | ContentToolCall::WorldSetSource { .. } => ToolOutcome::Unavailable {
                reason: "catalog SQL and world tools run in a game chat session".to_string(),
            },
        }
    }
}

fn session_bound_key(session: &SessionId, material: &[u8]) -> String {
    let mut buf = Vec::with_capacity(session.as_str().len() + 1 + material.len());
    buf.extend_from_slice(session.as_str().as_bytes());
    buf.push(0);
    buf.extend_from_slice(material);
    let digest = sha256(&buf);
    let mut hex = String::with_capacity(32);
    for b in &digest[..16] {
        hex.push_str(&format!("{b:02x}"));
    }
    format!("chat-{hex}")
}

/// Compact status projection: identifiers and summaries only, never bytes.
fn status_value(status: &OperationStatusDto) -> Value {
    let mut pairs = vec![
        ("operation", json::s(status.operation.to_string())),
        ("kind", json::s(status.kind.clone())),
        ("state", json::s(status.state.as_str())),
        ("round", Value::Int(status.round as i64)),
    ];
    if let Some(joined) = status.joined {
        pairs.push(("joined", Value::Bool(joined)));
    }
    if let Some(p) = &status.progress {
        pairs.push((
            "progress",
            json::obj(vec![
                ("permille", Value::Int(p.permille as i64)),
                ("note", json::s(public_error(&p.note))),
            ]),
        ));
    }
    if let Some(e) = &status.error {
        pairs.push(("error", json::s(public_error(e))));
    }
    if let Some((asset, revision)) = &status.result {
        pairs.push((
            "result",
            json::obj(vec![
                ("asset", json::s(asset.to_string())),
                ("revision", json::s(revision.to_string())),
            ]),
        ));
    }
    json::obj(pairs)
}

/// Typed error mapping: authorization failures are `Denied` (the ACL
/// answer), everything else is an operational failure with a bounded
/// description. Never a panic, never a silent Ok.
fn err_outcome(e: ClientError) -> ToolOutcome {
    match e {
        ClientError::Unauthenticated => {
            ToolOutcome::Denied { what: "not authenticated to the asset server".to_string() }
        }
        ClientError::Denied => {
            ToolOutcome::Denied { what: "the asset server denied this operation".to_string() }
        }
        ClientError::NotFound { what } => {
            ToolOutcome::Failed { message: public_error(&format!("not found: {what}")) }
        }
        ClientError::Server { status: 409, detail } => ToolOutcome::Refused {
            what: public_error(&format!(
                "the asset server refused this state transition: {}",
                detail.as_deref().unwrap_or("conflict")
            )),
        },
        other => ToolOutcome::Failed { message: public_error(&other.to_string()) },
    }
}

fn public_error(message: &str) -> String {
    sanitize_public_error(message)
}

pub(crate) fn kind_label(kind: AssetKind) -> &'static str {
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
    }
}

fn role_parse(s: &str) -> Option<FileRole> {
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
        "splat" => R::Splat,
        "ao_texture" => R::AoTexture,
        _ => return None,
    })
}

fn tier_parse(s: &str) -> Option<DeviceTier> {
    Some(match s {
        "any" => DeviceTier::Any,
        "low" => DeviceTier::Low,
        "medium" => DeviceTier::Medium,
        "high" => DeviceTier::High,
        _ => return None,
    })
}

fn media_parse(s: &str) -> Option<MediaType> {
    use MediaType as M;
    Some(match s {
        "png" => M::Png,
        "jpeg" => M::Jpeg,
        "glb" => M::Glb,
        "wav" => M::Wav,
        "ogg" => M::Ogg,
        "mp4" => M::Mp4,
        "bin" => M::Bin,
        "text" => M::Text,
        "ply" => M::Ply,
        "mp3" => M::Mp3,
        _ => return None,
    })
}

#[cfg(test)]
mod session_idempotency_tests {
    use super::*;
    use makepad_asset_client::json::{self, Value};
    use makepad_asset_client::{JobId, OperationProgressDto, OperationStateDto};

    #[test]
    fn derived_keys_canonicalize_and_bind_session() {
        let session = SessionId::parse("chat_0123456789abcdef").unwrap();
        let other = SessionId::parse("chat_fedcba9876543210").unwrap();
        let a = json::obj(vec![
            ("z", Value::Int(1)),
            ("nested", json::obj(vec![("b", Value::Int(2)), ("a", Value::Int(3))])),
        ]);
        let b = json::obj(vec![
            ("nested", json::obj(vec![("a", Value::Int(3)), ("b", Value::Int(2))])),
            ("z", Value::Int(1)),
        ]);
        let ka = session_bound_key(&session, canonicalize_json(&a).to_json().as_bytes());
        let kb = session_bound_key(&session, canonicalize_json(&b).to_json().as_bytes());
        assert_eq!(ka, kb);
        let ko = session_bound_key(&other, canonicalize_json(&a).to_json().as_bytes());
        assert_ne!(ka, ko);
        let supplied_a = session_bound_key(&session, b"retry-1");
        let supplied_b = session_bound_key(&other, b"retry-1");
        assert_ne!(supplied_a, supplied_b);
    }

    #[test]
    fn principal_is_not_part_of_session_bound_identity() {
        use crate::session::Origin;
        let session = SessionId::parse("chat_0123456789abcdef").unwrap();
        let a = Origin { principal: "prin_aaa".into(), session: session.clone() };
        let b = Origin { principal: "prin_bbb".into(), session };
        assert_ne!(a.principal, b.principal);
        assert_eq!(
            session_bound_key(&a.session, b"same-material"),
            session_bound_key(&b.session, b"same-material"),
        );
    }

    fn session_n(n: usize) -> SessionId {
        SessionId::parse(&format!("chat_{n:016x}")).unwrap()
    }

    #[test]
    fn known_ops_are_bounded_and_fail_closed_after_eviction() {
        let mut idx = SessionOpIndex::new();
        let session = session_n(1);
        let first = OperationId([0; 16]);
        idx.remember(&session, first);
        for i in 1..=MAX_KNOWN_OPS_PER_SESSION {
            idx.remember(&session, OperationId([i as u8; 16]));
        }
        assert!(
            !idx.contains(&session, &first),
            "oldest op must be evicted once the per-session cap is exceeded"
        );
        assert!(idx.contains(&session, &OperationId([MAX_KNOWN_OPS_PER_SESSION as u8; 16])));
        assert_eq!(idx.by_session.get(&session).map(|o| o.set.len()), Some(MAX_KNOWN_OPS_PER_SESSION));

        let mut idx = SessionOpIndex::new();
        let first_session = session_n(0);
        let op = OperationId([1; 16]);
        idx.remember(&first_session, op);
        for i in 1..=MAX_KNOWN_SESSIONS {
            idx.remember(&session_n(i), op);
        }
        assert!(
            !idx.contains(&first_session, &op),
            "oldest session must be evicted once the dispatcher cap is exceeded"
        );
        assert!(idx.contains(&session_n(MAX_KNOWN_SESSIONS), &op));
        assert_eq!(idx.order.len(), MAX_KNOWN_SESSIONS);
        assert_eq!(idx.by_session.len(), MAX_KNOWN_SESSIONS);
    }

    #[test]
    fn retire_session_forgets_ops_and_is_fail_closed() {
        let mut idx = SessionOpIndex::new();
        let session = session_n(2);
        let other = session_n(3);
        let op = OperationId([9; 16]);
        let kept = OperationId([8; 16]);
        idx.remember(&session, op);
        idx.remember(&other, kept);
        assert!(idx.retire(&session));
        assert!(!idx.contains(&session, &op));
        assert!(idx.contains(&other, &kept));
        assert!(!idx.retire(&session));
        idx.remember(&session, op);
        assert!(idx.contains(&session, &op));
    }

    fn leaky_status() -> OperationStatusDto {
        OperationStatusDto {
            operation: OperationId([1; 16]),
            namespace: "gen".into(),
            kind: "mesh.from_image.v1".into(),
            state: OperationStateDto::Failed,
            round: 0,
            job: JobId([2; 16]),
            idempotency_key: "k".into(),
            spec_digest: "00".repeat(32),
            created_ms: 0,
            updated_ms: 0,
            inputs: Vec::new(),
            error: Some("worker died Authorization: Bearer mpat_LEAK123".into()),
            result: None,
            progress: Some(OperationProgressDto {
                permille: 100,
                note: "Authorization: Bearer mpat_NOTE".into(),
                updated_ms: 0,
            }),
            joined: None,
        }
    }

    fn assert_no_credential(s: &str) {
        let lower = s.to_ascii_lowercase();
        assert!(!lower.contains("bearer"), "{s}");
        assert!(!lower.contains("mpat_"), "{s}");
        assert!(!s.contains("LEAK123"), "{s}");
        assert!(!s.contains("NOTE"), "{s}");
    }

    #[test]
    fn status_and_tool_errors_redact_bearer_mpat() {
        let v = status_value(&leaky_status());
        let err = v.get("error").and_then(Value::as_str).expect("error");
        assert_eq!(err, "provider error");
        assert_no_credential(err);
        let note = v
            .get("progress")
            .and_then(|p| p.get("note"))
            .and_then(Value::as_str)
            .expect("note");
        assert_eq!(note, "provider error");
        assert_no_credential(note);

        match err_outcome(ClientError::Server {
            status: 500,
            detail: Some("Authorization: Bearer mpat_LEAK123".into()),
        }) {
            ToolOutcome::Failed { message } => {
                assert_eq!(message, "provider error");
                assert_no_credential(&message);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        match err_outcome(ClientError::Server {
            status: 409,
            detail: Some("Authorization: Bearer mpat_LEAK123".into()),
        }) {
            ToolOutcome::Refused { what } => assert_no_credential(&what),
            other => panic!("expected Refused, got {other:?}"),
        }
        match err_outcome(ClientError::NotFound { what: "Bearer mpat_LEAK123" }) {
            ToolOutcome::Failed { message } => assert_no_credential(&message),
            other => panic!("expected Failed, got {other:?}"),
        }
        match err_outcome(ClientError::Protocol { what: "Bearer mpat_LEAK123" }) {
            ToolOutcome::Failed { message } => assert_no_credential(&message),
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
