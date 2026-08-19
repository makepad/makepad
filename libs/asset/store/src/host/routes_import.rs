//! Import and derived-variant routes: approved source collections, the
//! atomic pack import, single-flight derivation requests, worker completion,
//! frozen variant sets, and deterministic profile resolution.
//!
//! Capability map (extending the control-plane map):
//!   source collection registration .... `import_source` on the collection id
//!   pack import ....................... `import_run` on the source id
//!   derivation request ................ `derive_request` on the base's ns
//!   derivation completion ............. `job_worker` on the job's ns
//!   variant-set freeze ................ `asset_publish` on the base's ns
//!   reads (sources, imports, variants, sets, resolution): any authenticated
//!       principal — everything served is immutable canonical bytes.
//!
//! The server never executes processing kernels: a derivation request arms a
//! typed job; completion validates worker-claimed facts against the recipe
//! contract and the blob store before anything publishes.

use super::api::{
    body_str, body_u64, media_str, parse_job, parse_limit, parse_role, parse_tier, role_str, Fail,
    RouteResult,
};
use super::events::{self, EventBody};
use super::http::{Conn, Head, Resp};
use super::json::{obj, s, Value};
use super::routes::{call_state, read_body, require_cap, secret_of, Outcome, RouteCtx};
use super::routes_control::{read_json_body, worker_name, worker_suffix};
use super::util::{from_hex_bounded, now_ms};
use crate::{
    imports::MAX_SOURCE_PAGE_ROWS, Capability, DerivationOutcome, DerivationStatus, DerivedResult,
    ImportEntryRow, NewJob, ServerError,
};
use makepad_asset_data::{
    derivation_key, AssetFile, AssetId, AssetManifest, AssetRevisionId, AssetRevisionRef, BlobId,
    ClientProfile, DerivationKey, DerivedInput, DerivedVariantId, DeviceTier, ImageDims,
    ImportManifest, ImportRevisionId, MediaType, Metrics, ProcessingRecipe, RecipeDigest,
    SourceCollection, ThumbnailMedia, ThumbnailMeta, VariantSetId, RESOLUTION_POLICY_V1,
};

/// Largest canonical recipe document accepted as hex in a JSON body. Recipes
/// are tiny typed documents; this is generous headroom, not a manifest bound.
const MAX_RECIPE_HEX_BYTES: usize = 4096;

/// Source collections are a browse surface, not a bulk export. Explicitly
/// paged callers use the same 100/500 policy as the other administrative
/// listings. A legacy no-query call may still receive every row up to the
/// existing client's 512-row ceiling; above it the server refuses rather than
/// silently truncating an older client that does not understand cursors.
const DEFAULT_SOURCE_PAGE_LIMIT: u64 = 100;
const MAX_SOURCE_PAGE_LIMIT: u64 = 500;
const LEGACY_SOURCE_LIST_LIMIT: usize = 512;

const CACHE_IMMUTABLE: &str = "private, max-age=31536000, immutable";

// ---------------------------------------------------------------------------
// path/id parsing
// ---------------------------------------------------------------------------

pub fn irev_of(t: &str) -> RouteResult<ImportRevisionId> {
    t.parse().map_err(|_| Fail::Http(400, "malformed import revision"))
}

pub fn dkey_of(t: &str) -> RouteResult<DerivationKey> {
    t.parse().map_err(|_| Fail::Http(400, "malformed derivation key"))
}

pub fn dvar_of(t: &str) -> RouteResult<DerivedVariantId> {
    t.parse().map_err(|_| Fail::Http(400, "malformed derived variant"))
}

pub fn vset_of(t: &str) -> RouteResult<VariantSetId> {
    t.parse().map_err(|_| Fail::Http(400, "malformed variant set"))
}

fn base_of_body(body: &Value) -> RouteResult<AssetRevisionRef> {
    let asset_id: AssetId = body_str(body, "base_asset")?
        .parse()
        .map_err(|_| Fail::Http(400, "malformed asset id"))?;
    let revision: AssetRevisionId = body_str(body, "base_revision")?
        .parse()
        .map_err(|_| Fail::Http(400, "malformed asset revision"))?;
    Ok(AssetRevisionRef { asset_id, revision })
}

// ---------------------------------------------------------------------------
// approved source collections
// ---------------------------------------------------------------------------

fn source_cursor(raw: Option<&str>) -> RouteResult<Option<String>> {
    let Some(cursor) = raw else { return Ok(None) };
    // This is exactly SourceCollection::id's canonical slug grammar. Keep the
    // check here because the content crate intentionally does not expose its
    // internal validator, and constructing a synthetic content document just
    // to validate a browse cursor would invent unrelated fields.
    if cursor.is_empty()
        || cursor.len() > makepad_asset_data::limits::MAX_ALIAS_SEGMENT_BYTES
    {
        return Err(Fail::Http(400, "malformed source cursor"));
    }
    let bytes = cursor.as_bytes();
    if (!bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit())
        || !cursor
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
    {
        return Err(Fail::Http(400, "malformed source cursor"));
    }
    Ok(Some(cursor.to_string()))
}

pub fn source_put(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let bytes = match read_body(
        conn,
        head,
        rc.cfg.budgets.max_manifest_bytes,
        rc.cfg.control_body_deadline_ms,
    ) {
        Ok(b) => b,
        Err(o) => return Ok(o),
    };
    // Decode before the state call so the capability scope (the collection's
    // own id) is known; the core re-validates the exact bytes.
    let collection =
        SourceCollection::from_canonical_bytes(&bytes).map_err(ServerError::from)?;
    let source_id = collection.id.clone();
    let now = now_ms();
    let ns = source_id.clone();
    let digest = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::ImportSource, &ns)?;
        ctx.core.imports().register_source(&bytes, now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![
            ("source_id", s(source_id)),
            ("digest", s(digest.to_string())),
        ]),
    )))
}

pub fn sources_list(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let explicit_page = head.query_get("limit").is_some() || head.query_get("cursor").is_some();
    let limit = if explicit_page {
        parse_limit(
            head.query_get("limit"),
            DEFAULT_SOURCE_PAGE_LIMIT,
            MAX_SOURCE_PAGE_LIMIT,
        )? as usize
    } else {
        LEGACY_SOURCE_LIST_LIMIT
    };
    let after = source_cursor(head.query_get("cursor"))?;
    let now = now_ms();
    let fetch_limit = limit
        .checked_add(1)
        .filter(|n| *n <= MAX_SOURCE_PAGE_ROWS as usize)
        .ok_or(Fail::Http(500, "source page limit invariant"))? as u32;
    let sources = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core.imports().sources_page(after.as_deref(), fetch_limit)
    })?;
    let more = sources.len() > limit;
    let mut rows = Vec::with_capacity(sources.len().min(limit));
    let mut last_source_id = None;
    for bytes in sources.into_iter().take(limit) {
        let c = SourceCollection::from_canonical_bytes(&bytes).map_err(ServerError::from)?;
        last_source_id = Some(c.id.clone());
        rows.push(obj(vec![
            ("source_id", s(c.id.clone())),
            ("title", s(c.title.clone())),
            ("license", s(c.terms.license.clone())),
            ("credits", s(c.terms.credits.clone())),
            ("digest", s(c.digest().map_err(ServerError::from)?.to_string())),
        ]));
    }
    if more && !explicit_page {
        return Err(Fail::Http(413, "source listing requires pagination"));
    }
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("sources", Value::Arr(rows)),
            (
                "cursor",
                match (more, last_source_id) {
                    (true, Some(cursor)) => s(cursor),
                    _ => Value::Null,
                },
            ),
        ]),
    )))
}

// ---------------------------------------------------------------------------
// pack import
// ---------------------------------------------------------------------------

fn entry_rows(manifest: &ImportManifest, entries: &[ImportEntryRow]) -> RouteResult<Value> {
    let mut rows = Vec::with_capacity(entries.len());
    for entry in entries {
        let alias = manifest
            .assets
            .iter()
            .find(|a| a.key.as_str() == entry.key)
            .map(|a| manifest.alias_for(&a.key))
            .transpose()
            .map_err(ServerError::from)?;
        rows.push(obj(vec![
            ("key", s(entry.key.clone())),
            ("asset_id", s(entry.asset_id.to_string())),
            ("revision", s(entry.revision.to_string())),
            ("alias", match alias {
                Some(a) => s(a.as_str().to_string()),
                None => Value::Null,
            }),
        ]));
    }
    Ok(Value::Arr(rows))
}

pub fn import_run(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let bytes = match read_body(
        conn,
        head,
        rc.cfg.budgets.max_manifest_bytes,
        rc.cfg.control_body_deadline_ms,
    ) {
        Ok(b) => b,
        Err(o) => return Ok(o),
    };
    let manifest = ImportManifest::from_canonical_bytes(&bytes).map_err(ServerError::from)?;
    let ns = manifest.source_id.clone();
    // Deterministic alias per entry key, computed from the manifest alone.
    let mut aliases = Vec::with_capacity(manifest.assets.len());
    for a in &manifest.assets {
        aliases.push((
            a.key.as_str().to_string(),
            manifest
                .alias_for(&a.key)
                .map_err(ServerError::from)?
                .as_str()
                .to_string(),
        ));
    }
    let now = now_ms();
    let hub = rc.events.clone();
    let report = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::ImportRun, &ns)?;
        let report = ctx.core.imports().run_import(&bytes, now)?;
        if report.created {
            // Mirror the registry rows and announce, entry by entry, only
            // after the atomic core transaction committed. Still on the
            // state thread, so journal order equals commit order.
            for entry in &report.entries {
                ctx.asset_index_insert(entry.asset_id.as_bytes(), &ns, now)?;
                hub.publish(
                    EventBody::asset(
                        events::KIND_ASSET_PUBLISHED,
                        &ns,
                        entry.asset_id.to_string(),
                        now,
                    )
                    .with_revision(entry.revision.to_string()),
                );
                if let Some((_, alias)) = aliases.iter().find(|(k, _)| k == &entry.key) {
                    hub.publish(
                        EventBody::asset(
                            events::KIND_ALIAS_SET,
                            &ns,
                            entry.asset_id.to_string(),
                            now,
                        )
                        .with_revision(entry.revision.to_string())
                        .with_alias(alias.clone()),
                    );
                }
            }
        }
        Ok(report)
    })?;
    let entries = entry_rows(&manifest, &report.entries)?;
    let status = if report.created { 201 } else { 200 };
    Ok(Outcome::Resp(Resp::json(
        status,
        &obj(vec![
            ("import_revision", s(report.import_revision.to_string())),
            ("created", Value::Bool(report.created)),
            ("entries", entries),
        ]),
    )))
}

pub fn import_get(head: &Head, rc: &RouteCtx, irev: ImportRevisionId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let (bytes, entries) = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let bytes = ctx
            .core
            .imports()
            .import_manifest_bytes(&irev)?
            .ok_or(ServerError::NotFound { what: "import" })?;
        let entries = ctx.core.imports().entries(&irev)?;
        Ok((bytes, entries))
    })?;
    let manifest = ImportManifest::from_canonical_bytes(&bytes).map_err(ServerError::from)?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("import_revision", s(irev.to_string())),
            ("source_id", s(manifest.source_id.clone())),
            ("pack_name", s(manifest.pack_name.clone())),
            ("pack_version", s(manifest.pack_version.clone())),
            ("license", s(manifest.rights.license.clone())),
            ("credits", s(manifest.rights.credits.clone())),
            ("entries", entry_rows(&manifest, &entries)?),
        ]),
    )))
}

// ---------------------------------------------------------------------------
// advertised stock recipes + fail-closed lookup
// ---------------------------------------------------------------------------

pub fn derive_recipes_list(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        Ok(())
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &super::derive_recipes::recipes_json(),
    )))
}

/// Resolve one exact advertised (or caller-supplied) recipe against a base
/// revision. Ready variants return their identity and blobs. Pending is 409.
/// Missing/failed is 404. The original revision files are never substituted.
pub fn derived_variant_lookup(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let base = base_of_body(&body)?;
    let recipe_bytes = from_hex_bounded(body_str(&body, "recipe")?, MAX_RECIPE_HEX_BYTES)
        .ok_or(Fail::Http(400, "malformed recipe hex"))?;
    let recipe = ProcessingRecipe::from_canonical_bytes(&recipe_bytes).map_err(ServerError::from)?;
    let recipe_digest = RecipeDigest::hash_of(&recipe_bytes);
    let now = now_ms();
    let found = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let manifest_bytes = ctx
            .core
            .catalog()
            .asset_revision_manifest(&base.revision)?
            .ok_or(ServerError::NotFound { what: "derivation base revision" })?;
        let manifest = AssetManifest::from_canonical_bytes(&manifest_bytes)?;
        if manifest.asset_id != base.asset_id {
            return Err(ServerError::Conflict { what: "derivation base asset" });
        }
        let role = recipe.settings.input_role();
        let inputs: Vec<DerivedInput> = manifest
            .files
            .iter()
            .filter(|f| f.role == role)
            .map(|f| DerivedInput {
                role: f.role,
                blob: f.blob,
            })
            .collect();
        if inputs.is_empty() {
            return Err(ServerError::NotFound { what: "recipe input role in base" });
        }
        let dkey = derivation_key(&base, &recipe_digest, &inputs)?;
        let status = ctx.core.variants().derivation_status(&dkey)?;
        Ok((dkey, status))
    })?;
    let (dkey, status) = found;
    let Some(status) = status else {
        return Ok(Outcome::Resp(Resp::json(
            404,
            &obj(vec![
                ("error", s("derived variant not ready")),
                ("dkey", s(dkey.to_string())),
                ("state", s("missing")),
            ]),
        )));
    };
    match status.state {
        "ready" => {
            let variant = status.variant.ok_or(Fail::Http(500, "ready without variant"))?;
            let bytes = call_state(&rc.state, move |ctx| {
                ctx.core
                    .variants()
                    .variant_manifest(&variant)?
                    .ok_or(ServerError::NotFound { what: "derived variant" })
            })?;
            let manifest = makepad_asset_data::DerivedVariantManifest::from_canonical_bytes(&bytes)
                .map_err(ServerError::from)?;
            let blobs: Vec<Value> = manifest
                .blob_closure()
                .into_iter()
                .map(|b| s(b.to_string()))
                .collect();
            Ok(Outcome::Resp(Resp::json(
                200,
                &obj(vec![
                    ("status", s("ready")),
                    ("dkey", s(dkey.to_string())),
                    ("variant", s(variant.to_string())),
                    ("blobs", Value::Arr(blobs)),
                ]),
            )))
        }
        "pending" => Ok(Outcome::Resp(Resp::json(
            409,
            &obj(vec![
                ("error", s("derived variant pending")),
                ("dkey", s(dkey.to_string())),
                ("state", s("pending")),
                ("job", s(super::api::job_str(&status.job_id))),
            ]),
        ))),
        _ => Ok(Outcome::Resp(Resp::json(
            404,
            &obj(vec![
                ("error", s("derived variant not ready")),
                ("dkey", s(dkey.to_string())),
                ("state", s(status.state)),
            ]),
        ))),
    }
}

// ---------------------------------------------------------------------------
// derivations
// ---------------------------------------------------------------------------

fn status_value(status: &DerivationStatus) -> Value {
    obj(vec![
        ("dkey", s(status.dkey.to_string())),
        ("state", s(status.state)),
        ("base_asset", s(status.base.asset_id.to_string())),
        ("base_revision", s(status.base.revision.to_string())),
        ("recipe_digest", s(status.recipe_digest.to_string())),
        ("round", Value::Int(status.round as i64)),
        ("job", s(super::api::job_str(&status.job_id))),
        ("job_state", match status.job_state {
            Some(js) => s(js.as_str()),
            None => Value::Null,
        }),
        ("variant", match &status.variant {
            Some(v) => s(v.to_string()),
            None => Value::Null,
        }),
    ])
}

pub fn derivation_request(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let base = base_of_body(&body)?;
    let recipe_bytes = from_hex_bounded(body_str(&body, "recipe")?, MAX_RECIPE_HEX_BYTES)
        .ok_or(Fail::Http(400, "malformed recipe hex"))?;
    let now = now_ms();
    let outcome = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ns = ctx
            .core
            .catalog()
            .asset_namespace(&base.asset_id)?
            .ok_or(ServerError::NotFound { what: "derivation base asset" })?;
        require_cap(ctx, &p, Capability::DeriveRequest, &ns)?;
        let outcome = ctx.core.variants().begin_derivation(&base, &recipe_bytes, now)?;
        if let DerivationOutcome::NeedsJob {
            dkey,
            job_id,
            kind,
            recipe_digest,
        } = &outcome
        {
            // Arm the typed worker job through the SAME invariants the job
            // route enforces: metadata row first (the claim gate and job
            // visibility read it), core enqueue second, meta rolled back on
            // enqueue refusal. The payload envelope carries everything a
            // worker needs to fetch its inputs.
            let job_body = obj(vec![
                ("dkey", s(dkey.to_string())),
                ("base_asset", s(base.asset_id.to_string())),
                ("base_revision", s(base.revision.to_string())),
                ("recipe_digest", s(recipe_digest.to_string())),
            ]);
            let payload = super::state::envelope_build(&ns, &p, &job_body);
            let distinct = ctx.meta_distinct_ns()?;
            if !distinct.iter().any(|n| n == &ns)
                && distinct.len() as u64 >= super::state::MAX_JOB_NAMESPACES
            {
                return Err(ServerError::OverBudget {
                    what: "job namespaces",
                    limit: super::state::MAX_JOB_NAMESPACES,
                    found: distinct.len() as u64 + 1,
                });
            }
            if ctx.meta_get(job_id)?.is_none() {
                ctx.meta_insert(job_id, &ns, kind, &p, now)?;
            }
            let enqueue = ctx.core.jobs().enqueue(
                &NewJob {
                    job_id: *job_id,
                    parent: None,
                    kind,
                    payload: &payload,
                    priority: 0,
                    max_attempts: 3,
                    not_before_ms: 0,
                    deps: &[],
                },
                now,
            );
            match enqueue {
                Ok(()) => {}
                // Crash-repair replay: the job already exists from a prior
                // arming of this same round — joining it is the point.
                Err(ServerError::Conflict { what: "job id" }) => {}
                Err(e) => {
                    ctx.meta_delete(job_id)?;
                    return Err(e);
                }
            }
        }
        Ok(outcome)
    })?;
    let (status, value) = match outcome {
        DerivationOutcome::Ready { dkey, variant } => (
            200,
            obj(vec![
                ("status", s("ready")),
                ("dkey", s(dkey.to_string())),
                ("variant", s(variant.to_string())),
            ]),
        ),
        DerivationOutcome::InFlight { dkey, job_id } => (
            202,
            obj(vec![
                ("status", s("pending")),
                ("dkey", s(dkey.to_string())),
                ("job", s(super::api::job_str(&job_id))),
                ("joined", Value::Bool(true)),
            ]),
        ),
        DerivationOutcome::NeedsJob { dkey, job_id, .. } => (
            202,
            obj(vec![
                ("status", s("pending")),
                ("dkey", s(dkey.to_string())),
                ("job", s(super::api::job_str(&job_id))),
                ("joined", Value::Bool(false)),
            ]),
        ),
    };
    Ok(Outcome::Resp(Resp::json(status, &value)))
}

pub fn derivation_get(head: &Head, rc: &RouteCtx, dkey: DerivationKey) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let status = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core
            .variants()
            .derivation_status(&dkey)?
            .ok_or(ServerError::NotFound { what: "derivation" })
    })?;
    Ok(Outcome::Resp(Resp::json(200, &status_value(&status))))
}

// ---------------------------------------------------------------------------
// worker completion
// ---------------------------------------------------------------------------

fn parse_media(name: &str) -> Option<MediaType> {
    [
        MediaType::Png,
        MediaType::Jpeg,
        MediaType::Glb,
        MediaType::Wav,
        MediaType::Ogg,
        MediaType::Mp4,
        MediaType::Bin,
        MediaType::Text,
        MediaType::Ply,
    ]
    .into_iter()
    .find(|m| media_str(*m) == name)
}

fn parse_thumb_media(name: &str) -> Option<ThumbnailMedia> {
    match name {
        "png" => Some(ThumbnailMedia::Png),
        "jpeg" => Some(ThumbnailMedia::Jpeg),
        _ => None,
    }
}

fn blob_of(v: &Value, key: &'static str) -> RouteResult<BlobId> {
    body_str(v, key)?
        .parse()
        .map_err(|_| Fail::Http(400, "malformed blob id"))
}

fn output_of(v: &Value) -> RouteResult<AssetFile> {
    let role = parse_role(body_str(v, "role")?).ok_or(Fail::Http(400, "malformed output role"))?;
    let tier = match v.get("tier") {
        None => DeviceTier::Any,
        Some(t) => parse_tier(t.as_str().ok_or(Fail::Http(400, "malformed output tier"))?)
            .ok_or(Fail::Http(400, "malformed output tier"))?,
    };
    let lod = body_u64(v, "lod").unwrap_or(0);
    if lod > u8::MAX as u64 {
        return Err(Fail::Http(400, "malformed output lod"));
    }
    let media =
        parse_media(body_str(v, "media")?).ok_or(Fail::Http(400, "malformed output media"))?;
    let blob = blob_of(v, "blob")?;
    let byte_len = body_u64(v, "byte_len").ok_or(Fail::Http(400, "missing output byte_len"))?;
    let dims = match v.get("dims") {
        None => None,
        Some(d) => Some(ImageDims {
            width: body_u64(d, "width").ok_or(Fail::Http(400, "malformed output dims"))? as u32,
            height: body_u64(d, "height").ok_or(Fail::Http(400, "malformed output dims"))? as u32,
        }),
    };
    Ok(AssetFile {
        role,
        tier,
        lod: lod as u8,
        media,
        blob,
        byte_len,
        dims,
    })
}

fn metrics_of(v: &Value) -> RouteResult<Metrics> {
    let m = v.get("metrics").ok_or(Fail::Http(400, "missing metrics"))?;
    let field = |key: &str| body_u64(m, key).unwrap_or(0);
    Ok(Metrics {
        total_bytes: body_u64(m, "total_bytes").ok_or(Fail::Http(400, "missing total_bytes"))?,
        triangles: field("triangles") as u32,
        vertices: field("vertices") as u32,
        joints: field("joints") as u16,
        clips: field("clips") as u16,
        max_texture_dim: field("max_texture_dim") as u32,
        media_millis: field("media_millis") as u32,
    })
}

fn derived_result_of(body: &Value) -> RouteResult<DerivedResult> {
    let outputs = match body.get("outputs") {
        None => Vec::new(),
        Some(v) => {
            let arr = v.as_arr().ok_or(Fail::Http(400, "outputs must be an array"))?;
            let mut out = Vec::with_capacity(arr.len());
            for entry in arr {
                out.push(output_of(entry)?);
            }
            out
        }
    };
    let thumbnail = match body.get("thumbnail") {
        None => None,
        Some(t) => Some(ThumbnailMeta {
            blob: blob_of(t, "blob")?,
            media: parse_thumb_media(body_str(t, "media")?)
                .ok_or(Fail::Http(400, "malformed thumbnail media"))?,
            width: body_u64(t, "width").ok_or(Fail::Http(400, "malformed thumbnail"))? as u32,
            height: body_u64(t, "height").ok_or(Fail::Http(400, "malformed thumbnail"))? as u32,
            byte_len: body_u64(t, "byte_len").ok_or(Fail::Http(400, "malformed thumbnail"))?,
        }),
    };
    let metrics = metrics_of(body)?;
    Ok(DerivedResult {
        outputs,
        thumbnail,
        metrics,
    })
}

pub fn derivation_complete(
    conn: &mut Conn,
    head: &mut Head,
    rc: &RouteCtx,
    dkey: DerivationKey,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let job = parse_job(body_str(&body, "job")?).ok_or(Fail::Http(400, "malformed job id"))?;
    let suffix = worker_suffix(&body)?;
    let result = derived_result_of(&body)?;
    let now = now_ms();
    let variant = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let meta = ctx
            .meta_get(&job)?
            .ok_or(ServerError::NotFound { what: "job" })?;
        require_cap(ctx, &p, Capability::JobWorker, &meta.ns)?;
        let worker = worker_name(&p, &suffix);
        let variant = ctx
            .core
            .variants()
            .complete_derivation(&dkey, &job, &worker, &result, now)?;
        // Mirror the outcome for job_get consumers.
        let attempt = ctx
            .core
            .jobs()
            .attempts(&job)?
            .last()
            .map(|a| a.attempt as u64)
            .unwrap_or(0);
        let doc = obj(vec![("variant", s(variant.to_string()))]).to_json().into_bytes();
        ctx.result_set(&job, "succeeded", attempt, &doc, now)?;
        Ok(variant)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("variant", s(variant.to_string()))]),
    )))
}

// ---------------------------------------------------------------------------
// derived variants and variant sets
// ---------------------------------------------------------------------------

/// Serve immutable canonical bytes with the standard strong-ETag treatment.
fn canonical_bytes_resp(head: &Head, etag_body: String, bytes: Vec<u8>) -> Outcome {
    let etag = format!("\"{etag_body}\"");
    if let Some(inm) = &head.if_none_match {
        if super::http::etag_matches(inm, &etag) {
            return Outcome::Resp(Resp::empty(304).with_header("ETag", etag));
        }
    }
    Outcome::Resp(
        Resp::bytes(200, "application/octet-stream", bytes)
            .with_header("ETag", etag)
            .with_header("Cache-Control", CACHE_IMMUTABLE.to_string()),
    )
}

pub fn derived_variant_get(
    head: &Head,
    rc: &RouteCtx,
    dvar: DerivedVariantId,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let bytes = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core
            .variants()
            .variant_manifest(&dvar)?
            .ok_or(ServerError::NotFound { what: "derived variant" })
    })?;
    Ok(canonical_bytes_resp(head, dvar.to_string(), bytes))
}

pub fn variant_set_freeze(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let base = base_of_body(&body)?;
    let variants_value = body
        .get("variants")
        .and_then(Value::as_arr)
        .ok_or(Fail::Http(400, "variants must be an array"))?;
    let mut variants = Vec::with_capacity(variants_value.len());
    for v in variants_value {
        variants.push(
            v.as_str()
                .and_then(|t| t.parse::<DerivedVariantId>().ok())
                .ok_or(Fail::Http(400, "malformed derived variant"))?,
        );
    }
    let now = now_ms();
    let set = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ns = ctx
            .core
            .catalog()
            .asset_namespace(&base.asset_id)?
            .ok_or(ServerError::NotFound { what: "variant set base asset" })?;
        require_cap(ctx, &p, Capability::AssetPublish, &ns)?;
        ctx.core.variants().freeze_variant_set(&base, &variants, now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![("variant_set", s(set.to_string()))]),
    )))
}

pub fn variant_set_get(head: &Head, rc: &RouteCtx, vset: VariantSetId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let bytes = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core
            .variants()
            .variant_set_manifest(&vset)?
            .ok_or(ServerError::NotFound { what: "variant set" })
    })?;
    Ok(canonical_bytes_resp(head, vset.to_string(), bytes))
}

// ---------------------------------------------------------------------------
// deterministic profile resolution
// ---------------------------------------------------------------------------

fn profile_of(body: &Value) -> RouteResult<ClientProfile> {
    let p = body.get("profile").ok_or(Fail::Http(400, "missing profile"))?;
    let tier = parse_tier(body_str(p, "tier")?).ok_or(Fail::Http(400, "malformed profile tier"))?;
    let accept = p
        .get("accept")
        .and_then(Value::as_arr)
        .ok_or(Fail::Http(400, "profile accept must be an array"))?;
    let mut profile = ClientProfile {
        policy_version: body_u64(p, "policy_version").unwrap_or(RESOLUTION_POLICY_V1 as u64)
            as u32,
        tier,
        max_texture_dim: body_u64(p, "max_texture_dim")
            .ok_or(Fail::Http(400, "missing max_texture_dim"))? as u32,
        max_triangles: body_u64(p, "max_triangles")
            .ok_or(Fail::Http(400, "missing max_triangles"))? as u32,
        max_variant_bytes: body_u64(p, "max_variant_bytes")
            .ok_or(Fail::Http(400, "missing max_variant_bytes"))?,
        accept_png: false,
        accept_jpeg: false,
        accept_glb: false,
        accept_bin: false,
    };
    for a in accept {
        match a.as_str() {
            Some("png") => profile.accept_png = true,
            Some("jpeg") => profile.accept_jpeg = true,
            Some("glb") => profile.accept_glb = true,
            Some("bin") => profile.accept_bin = true,
            _ => return Err(Fail::Http(400, "malformed profile accept entry")),
        }
    }
    Ok(profile)
}

pub fn variant_resolve(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = match read_json_body(conn, head, rc) {
        Err(o) => return Ok(o),
        Ok(r) => r?,
    };
    let set: VariantSetId = body_str(&body, "variant_set")?
        .parse()
        .map_err(|_| Fail::Http(400, "malformed variant set"))?;
    let profile = profile_of(&body)?;
    let now = now_ms();
    let map = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core.variants().resolve(&set, &profile)
    })?;
    let entries: Vec<Value> = map
        .entries
        .iter()
        .map(|e| {
            obj(vec![
                ("role", s(match e.role {
                    makepad_asset_data::VariantRole::Thumbnail => "thumbnail",
                    makepad_asset_data::VariantRole::File(r) => role_str(r),
                })),
                ("variant", s(e.variant.to_string())),
                (
                    "blobs",
                    Value::Arr(e.blobs.iter().map(|b| s(b.to_string())).collect()),
                ),
            ])
        })
        .collect();
    let digest = map.digest().map_err(ServerError::from)?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("digest", s(digest.to_string())),
            ("variant_set", s(map.set.to_string())),
            ("profile", s(map.profile.to_string())),
            ("entries", Value::Arr(entries)),
        ]),
    )))
}
