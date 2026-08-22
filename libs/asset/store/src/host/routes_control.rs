//! Control-plane routes: health, auth administration, catalog (assets,
//! revisions, aliases, games), search + annotations, and the job/worker
//! protocol. Byte-heavy routes (blobs, thumbnails) live on the data plane.
//!
//! Capability map (one core capability per route, nothing implied):
//!   asset register/stage + annotation write . `asset_register` on the ns
//!   asset publish ........................... `asset_publish`
//!   asset AND game quarantine ............... `asset_quarantine` (the
//!       moderation capability; the core defines no game-specific one)
//!   asset/revision RETIREMENT (deletion) .... `asset_quarantine` on the ns
//!       (deletion is the strongest pull there is, so it rides the pull
//!       capability — publishing rights alone must not delete history)
//!   blob garbage collection ................. bootstrap root admin only
//!       (whole-store operation; no namespace can scope it)
//!   alias + game-alias writes ............... `alias_write`
//!   game register/stage ..................... `game_register`
//!   game publish ............................ `game_publish`
//!   job enqueue / status / list ............. `job_enqueue` (status also
//!       visible to the enqueuer and to `job_worker`/`job_cancel` holders)
//!   worker claim/heartbeat/succeed/fail ..... `job_worker`
//!   job cancel .............................. `job_cancel`
//!   principal/token minting, `*`-scope grants: bootstrap root admin only;
//!       namespace-scope grants also allowed to `auth_admin` on that ns
//!   reads (catalog, manifests, search) ...... any authenticated principal;
//!       private annotation fields stay owner-only (search dual weights +
//!       the gated annotation read below)
//!
//! The worker claim gate: the core's claim is namespace-blind and a claim
//! cannot be undone without burning an attempt, so claiming requires
//! `job_worker` on EVERY namespace jobs have been routed under (a bounded
//! set, enforced at enqueue). That is the strongest scoping the current core
//! contract supports.

use super::api::{
    body_str, body_u64, parse_capability, parse_job, parse_json_body, parse_limit,
    parse_principal, principal_str, Fail, RouteResult, TOKEN_PREFIX,
};
use super::events::{self, CatalogEvent, EventBody, EventCursor};
use super::http::{Conn, Head, Method, Resp};
use super::json::{obj, s, Value};
use super::routes::{
    call_state, is_read, method_not_allowed, not_found, read_body, require_cap, secret_of,
    Outcome, RouteCtx,
};
use super::state::{envelope_build, envelope_parse, StateCtx, MAX_JOB_NAMESPACES};
use super::util::{from_hex_bounded, from_hex_exact, now_ms, rand16, rand32, to_hex};
use crate::search::{kind_name, kind_parse};
use crate::{
    token_hash, AssetAnnotation, Capability, CandidateState, JobId, JobState, NewJob, PrincipalId,
    Scope, SearchFilters, SearchQuery, SearchViewer, ServerError, ViewerScope, Visibility,
    SERVER_SCHEMA_VERSION,
};
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, AssetManifest, AssetRevisionId, AssetRevisionRef, GameAlias,
    GameId, GameRevisionId, GameRevisionManifest,
};

/// Largest worker-supplied result/error document the transport records.
const MAX_RESULT_BYTES: usize = 16 * 1024;
const MAX_PROGRESS_NOTE_BYTES: usize = 200;
const CACHE_IMMUTABLE: &str = "private, max-age=31536000, immutable";

pub fn dispatch(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let segs = head.segs.clone();
    let seg: Vec<&str> = segs.iter().map(String::as_str).collect();
    let m = head.method;
    if let Some(r) = super::routes_chat::dispatch(conn, head, rc, &seg) {
        return r;
    }
    if let Some(r) = super::routes_rooms::dispatch(conn, head, rc, &seg) {
        return r;
    }
    match seg.as_slice() {
        ["v1", "health"] => {
            if is_read(m) {
                health(rc)
            } else {
                method_not_allowed()
            }
        }

        // ---- auth ----------------------------------------------------------
        ["v1", "auth", "whoami"] if is_read(m) => whoami(head, rc),
        ["v1", "auth", "principals"] if m == Method::Post => principal_create(conn, head, rc),
        ["v1", "auth", "principals", p, "disable"] if m == Method::Post => {
            let target = parse_principal(p).ok_or(Fail::Http(400, "malformed principal"))?;
            principal_disable(head, rc, target)
        }
        ["v1", "auth", "tokens"] if m == Method::Post => token_create(conn, head, rc),
        ["v1", "auth", "tokens", "revoke"] if m == Method::Post => token_revoke(conn, head, rc),
        ["v1", "auth", "grants"] if m == Method::Post => grant_edit(conn, head, rc, true),
        ["v1", "auth", "grants", "revoke"] if m == Method::Post => grant_edit(conn, head, rc, false),

        // ---- assets --------------------------------------------------------
        ["v1", "assets"] if m == Method::Post => asset_register(conn, head, rc),
        ["v1", "assets"] if is_read(m) => assets_list(head, rc),
        ["v1", "assets", a] if is_read(m) => asset_get(head, rc, ast_of(a)?),
        ["v1", "assets", a, "revisions"] if m == Method::Post => {
            asset_stage(conn, head, rc, ast_of(a)?)
        }
        ["v1", "assets", a, "revisions", r] if is_read(m) => {
            asset_candidate(head, rc, ast_of(a)?, arev_of(r)?)
        }
        ["v1", "assets", a, "revisions", r, "publish"] if m == Method::Post => {
            asset_lifecycle(head, rc, ast_of(a)?, arev_of(r)?, true)
        }
        ["v1", "assets", a, "revisions", r, "quarantine"] if m == Method::Post => {
            asset_lifecycle(head, rc, ast_of(a)?, arev_of(r)?, false)
        }
        // Deletion. `retire` is quarantine plus the intent to reclaim the
        // bytes; DELETE on the asset is the same operation under the verb a
        // browser-shaped client expects.
        ["v1", "assets", a, "retire"] if m == Method::Post => asset_retire(head, rc, ast_of(a)?),
        ["v1", "assets", a] if m == Method::Delete => asset_retire(head, rc, ast_of(a)?),
        ["v1", "assets", a, "revisions", r, "retire"] if m == Method::Post => {
            revision_retire(head, rc, ast_of(a)?, arev_of(r)?)
        }
        ["v1", "assets", a, "annotation"] => match m {
            Method::Put => annotation_put(conn, head, rc, ast_of(a)?),
            Method::Get | Method::Head => annotation_get(head, rc, ast_of(a)?),
            Method::Delete => annotation_delete(head, rc, ast_of(a)?),
            _ => method_not_allowed(),
        },

        // ---- asset revision manifests --------------------------------------
        // Canonical bytes at /v1/revisions/{arev} (the path the asset client
        // wire contract pins); browsing projection at .../json.
        ["v1", "revisions", r] if is_read(m) => manifest_get(head, rc, arev_of(r)?, false),
        ["v1", "revisions", r, "json"] if is_read(m) => manifest_get(head, rc, arev_of(r)?, true),
        ["v1", "game-revisions", r] if is_read(m) => {
            game_manifest_get(head, rc, grev_of(r)?)
        }

        // ---- aliases -------------------------------------------------------
        ["v1", "aliases", rest @ ..] if !rest.is_empty() => {
            let alias = asset_alias_of(rest)?;
            match m {
                Method::Get | Method::Head => alias_get(head, rc, alias),
                Method::Put => alias_put(conn, head, rc, alias),
                Method::Delete => alias_delete(head, rc, alias),
                _ => method_not_allowed(),
            }
        }
        ["v1", "game-aliases", rest @ ..] if !rest.is_empty() => {
            let alias = game_alias_of(rest)?;
            match m {
                Method::Get | Method::Head => game_alias_get(head, rc, alias),
                Method::Put => game_alias_put(conn, head, rc, alias),
                Method::Delete => game_alias_delete(head, rc, alias),
                _ => method_not_allowed(),
            }
        }

        // ---- games ---------------------------------------------------------
        ["v1", "games"] if m == Method::Post => game_register(conn, head, rc),
        ["v1", "games", g] if is_read(m) => game_get(head, rc, gam_of(g)?),
        ["v1", "games", g, "revisions"] if m == Method::Post => {
            game_stage(conn, head, rc, gam_of(g)?)
        }
        ["v1", "games", g, "revisions", r] if is_read(m) => {
            game_candidate(head, rc, gam_of(g)?, grev_of(r)?)
        }
        ["v1", "games", g, "revisions", r, "publish"] if m == Method::Post => {
            game_lifecycle(head, rc, gam_of(g)?, grev_of(r)?, true)
        }
        ["v1", "games", g, "revisions", r, "quarantine"] if m == Method::Post => {
            game_lifecycle(head, rc, gam_of(g)?, grev_of(r)?, false)
        }
        ["v1", "games", g, "revisions", r, "refs"] if is_read(m) => {
            game_refs(head, rc, gam_of(g)?, grev_of(r)?)
        }

        // ---- blob garbage collection ---------------------------------------
        ["v1", "gc"] if m == Method::Post => gc_run(conn, head, rc),
        ["v1", "gc"] if is_read(m) => gc_get(head, rc),
        ["v1", "gc", "cancel"] if m == Method::Post => gc_cancel(head, rc),

        // ---- reference blobs (catalogued, not copied) -----------------------
        ["v1", "blob-refs"] if is_read(m) => blob_refs_rescan(head, rc),

        // ---- catalog event feed --------------------------------------------
        ["v1", "events"] => {
            if is_read(m) {
                events_route(head, rc)
            } else {
                method_not_allowed()
            }
        }

        // ---- search --------------------------------------------------------
        ["v1", "search"] if is_read(m) => {
            let params = search_params_from_query(head, rc)?;
            run_search(head, rc, params)
        }
        // Free search text cannot travel in the percent-free query charset;
        // POST /v1/catalog (the asset client's route) carries it as JSON.
        ["v1", "catalog"] if m == Method::Post => {
            let bytes = match read_body(conn, head, rc.cfg.max_json_body_bytes, rc.cfg.control_body_deadline_ms) {
                Ok(b) => b,
                Err(o) => return Ok(o),
            };
            let body = parse_json_body(&bytes)?;
            let params = search_params_from_body(&body, rc)?;
            run_search(head, rc, params)
        }

        // ---- job profiles (advertised generation capabilities) -------------
        ["v1", "job-profiles"] => {
            if is_read(m) {
                job_profiles(head, rc)
            } else if m == Method::Put {
                job_profiles_announce(conn, head, rc)
            } else {
                method_not_allowed()
            }
        }

        // ---- jobs ----------------------------------------------------------
        ["v1", "jobs"] if m == Method::Post => job_enqueue(conn, head, rc),
        ["v1", "jobs"] if is_read(m) => job_list(head, rc),
        ["v1", "jobs", j] if is_read(m) => {
            let job = parse_job(j).ok_or(Fail::Http(400, "malformed job id"))?;
            job_get(head, rc, job)
        }
        ["v1", "jobs", j, "cancel"] if m == Method::Post => {
            let job = parse_job(j).ok_or(Fail::Http(400, "malformed job id"))?;
            job_cancel(head, rc, job)
        }

        // ---- worker protocol -----------------------------------------------
        ["v1", "worker", "claim"] if m == Method::Post => worker_claim(conn, head, rc),
        ["v1", "worker", "heartbeat"] if m == Method::Post => worker_heartbeat(conn, head, rc),
        ["v1", "worker", "succeed"] if m == Method::Post => worker_succeed(conn, head, rc),
        ["v1", "worker", "fail"] if m == Method::Post => worker_fail(conn, head, rc),

        // ---- external-pack import ------------------------------------------
        ["v1", "import-sources"] => match m {
            Method::Put => super::routes_import::source_put(conn, head, rc),
            Method::Get | Method::Head => super::routes_import::sources_list(head, rc),
            _ => method_not_allowed(),
        },
        ["v1", "imports"] if m == Method::Post => super::routes_import::import_run(conn, head, rc),
        ["v1", "imports", r] if is_read(m) => {
            super::routes_import::import_get(head, rc, super::routes_import::irev_of(r)?)
        }

        // ---- derived variants ----------------------------------------------
        ["v1", "derive-recipes"] if is_read(m) => super::routes_import::derive_recipes_list(head, rc),
        ["v1", "derived-variant-lookups"] if m == Method::Post => {
            super::routes_import::derived_variant_lookup(conn, head, rc)
        }
        ["v1", "derivations"] if m == Method::Post => {
            super::routes_import::derivation_request(conn, head, rc)
        }
        ["v1", "derivations", k] if is_read(m) => {
            super::routes_import::derivation_get(head, rc, super::routes_import::dkey_of(k)?)
        }
        ["v1", "derivations", k, "complete"] if m == Method::Post => {
            super::routes_import::derivation_complete(
                conn,
                head,
                rc,
                super::routes_import::dkey_of(k)?,
            )
        }
        ["v1", "derived-variants", v] if is_read(m) => {
            super::routes_import::derived_variant_get(head, rc, super::routes_import::dvar_of(v)?)
        }
        ["v1", "variant-sets"] if m == Method::Post => {
            super::routes_import::variant_set_freeze(conn, head, rc)
        }
        ["v1", "variant-sets", v] if is_read(m) => {
            super::routes_import::variant_set_get(head, rc, super::routes_import::vset_of(v)?)
        }
        ["v1", "variant-resolutions"] if m == Method::Post => {
            super::routes_import::variant_resolve(conn, head, rc)
        }

        // ---- typed asset operations ----------------------------------------
        ["v1", "operation-types"] if is_read(m) => {
            super::routes_operations::operation_types(head, rc)
        }
        ["v1", "operations"] if m == Method::Post => {
            super::routes_operations::operation_create(conn, head, rc)
        }
        ["v1", "operations", o] if is_read(m) => {
            super::routes_operations::operation_get(head, rc, super::routes_operations::oid_of(o)?)
        }
        ["v1", "operations", o, "events"] if is_read(m) => {
            super::routes_operations::operation_events(
                head,
                rc,
                super::routes_operations::oid_of(o)?,
            )
        }
        ["v1", "operations", o, "cancel"] if m == Method::Post => {
            super::routes_operations::operation_cancel(
                conn,
                head,
                rc,
                super::routes_operations::oid_of(o)?,
            )
        }
        ["v1", "operations", o, "retry"] if m == Method::Post => {
            super::routes_operations::operation_retry(
                conn,
                head,
                rc,
                super::routes_operations::oid_of(o)?,
            )
        }
        ["v1", "operations", o, "finalize"] if m == Method::Post => {
            super::routes_operations::operation_finalize(
                conn,
                head,
                rc,
                super::routes_operations::oid_of(o)?,
            )
        }

        _ => not_found(),
    }
}

// ---------------------------------------------------------------------------
// path/id parsing
// ---------------------------------------------------------------------------

fn ast_of(t: &str) -> RouteResult<AssetId> {
    t.parse().map_err(|_| Fail::Http(400, "malformed asset id"))
}

fn arev_of(t: &str) -> RouteResult<AssetRevisionId> {
    t.parse().map_err(|_| Fail::Http(400, "malformed asset revision"))
}

fn gam_of(t: &str) -> RouteResult<GameId> {
    t.parse().map_err(|_| Fail::Http(400, "malformed game id"))
}

fn grev_of(t: &str) -> RouteResult<GameRevisionId> {
    t.parse().map_err(|_| Fail::Http(400, "malformed game revision"))
}

fn asset_alias_of(rest: &[&str]) -> RouteResult<AssetAlias> {
    AssetAlias::new(rest.join("/")).map_err(|_| Fail::Http(400, "malformed alias"))
}

fn game_alias_of(rest: &[&str]) -> RouteResult<GameAlias> {
    GameAlias::new(rest.join("/")).map_err(|_| Fail::Http(400, "malformed alias"))
}

pub(crate) fn read_json_body(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> Result<RouteResult<Value>, Outcome> {
    let bytes = match read_body(conn, head, rc.cfg.max_json_body_bytes, rc.cfg.control_body_deadline_ms) {
        Ok(b) => b,
        Err(o) => return Err(o),
    };
    Ok(parse_json_body(&bytes))
}

/// `let body = json_body!(conn, head, rc);` — bail with the framing outcome
/// or the parse refusal, otherwise bind the parsed object.
macro_rules! json_body {
    ($conn:expr, $head:expr, $rc:expr) => {
        match read_json_body($conn, $head, $rc) {
            Err(o) => return Ok(o),
            Ok(r) => r?,
        }
    };
}

// ---------------------------------------------------------------------------
// health
// ---------------------------------------------------------------------------

pub(crate) fn health(rc: &RouteCtx) -> RouteResult<Outcome> {
    // Liveness probe: a poisoned or dead state thread means the service
    // cannot answer anything else truthfully.
    if rc.state.call(|_| ()).is_none() {
        return Ok(Outcome::Resp(Resp::error(503, "state unavailable")));
    }
    let b = &rc.cfg.budgets;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("status", s("ok")),
            // The identity handshake clients pin against discovery beacons:
            // the same server_id and protocol version the beacon carries.
            ("server_id", s(to_hex(&rc.server_id))),
            (
                "protocol_version",
                Value::Int(super::discovery::PROTOCOL_VERSION as i64),
            ),
            ("schema_version", Value::Int(SERVER_SCHEMA_VERSION)),
            (
                "transport_schema_version",
                Value::Int(super::state::TRANSPORT_SCHEMA_VERSION as i64),
            ),
            (
                "limits",
                obj(vec![
                    ("max_blob_bytes", Value::Int(b.max_blob_bytes as i64)),
                    ("max_manifest_bytes", Value::Int(b.max_manifest_bytes as i64)),
                    ("max_job_payload_bytes", Value::Int(b.max_job_payload_bytes as i64)),
                    ("max_search_results", Value::Int(b.max_search_results as i64)),
                    ("max_json_body_bytes", Value::Int(rc.cfg.max_json_body_bytes as i64)),
                ]),
            ),
        ]),
    )))
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

fn whoami(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let p = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("principal", s(principal_str(&p)))]),
    )))
}

fn require_root(ctx: &StateCtx, p: &PrincipalId) -> Result<(), ServerError> {
    if ctx.is_root(p)? {
        Ok(())
    } else {
        Err(ServerError::Denied { capability: "auth_admin" })
    }
}

fn principal_create(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let name = body_str(&body, "name")?.to_string();
    let id = PrincipalId(rand16()?);
    let now = now_ms();
    let created = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_root(ctx, &p)?;
        ctx.core.auth().create_principal(&id, &name, now)?;
        Ok(id)
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![("principal", s(principal_str(&created)))]),
    )))
}

fn principal_disable(head: &Head, rc: &RouteCtx, target: PrincipalId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_root(ctx, &p)?;
        // Disabling the bootstrap admin would leave the deployment with no
        // principal able to administer auth at all; refuse.
        if ctx.root_admin_get()?.as_ref() == Some(&target) {
            return Err(ServerError::Conflict { what: "bootstrap admin principal" });
        }
        ctx.core.auth().disable_principal(&target)
    })?;
    Ok(Outcome::Resp(Resp::empty(204)))
}

fn token_create(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let target = parse_principal(body_str(&body, "principal")?)
        .ok_or(Fail::Http(400, "malformed principal"))?;
    let ttl = body_u64(&body, "ttl_ms").unwrap_or(30 * 24 * 60 * 60 * 1000);
    if ttl == 0 || ttl > rc.cfg.max_token_ttl_ms {
        return Err(Fail::Http(400, "token ttl out of range"));
    }
    // The secret is minted here, shown exactly once in this response, and
    // only its hash ever reaches storage.
    let new_secret = format!("{}{}", TOKEN_PREFIX, to_hex(&rand32()?));
    let hash = token_hash(new_secret.as_bytes());
    let now = now_ms();
    let expires = now
        .checked_add(ttl)
        .ok_or(Fail::Http(400, "token ttl out of range"))?;
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_root(ctx, &p)?;
        ctx.core.auth().register_token(&target, &hash, expires, now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![
            ("token", s(new_secret)),
            ("principal", s(principal_str(&target))),
            ("expires_ms", Value::Int(expires as i64)),
        ]),
    )))
}

fn token_revoke(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let presented = body_str(&body, "token")?;
    let hex = presented
        .strip_prefix(TOKEN_PREFIX)
        .and_then(from_hex_exact::<32>)
        .ok_or(Fail::Http(400, "malformed token"))?;
    let hash = token_hash(format!("{}{}", TOKEN_PREFIX, to_hex(&hex)).as_bytes());
    let now = now_ms();
    call_state(&rc.state, move |ctx| {
        // Any authenticated caller may revoke a secret it can present:
        // possession already grants everything the token can do.
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core.auth().revoke_token(&hash)
    })?;
    Ok(Outcome::Resp(Resp::empty(204)))
}

fn grant_edit(conn: &mut Conn, head: &mut Head, rc: &RouteCtx, add: bool) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let target = parse_principal(body_str(&body, "principal")?)
        .ok_or(Fail::Http(400, "malformed principal"))?;
    let cap = parse_capability(body_str(&body, "capability")?)
        .ok_or(Fail::Http(400, "unknown capability"))?;
    let scope = body_str(&body, "scope")?.to_string();
    let now = now_ms();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let scope_ref = if scope == "*" {
            // Wildcard grants reach every namespace: bootstrap admin only.
            require_root(ctx, &p)?;
            Scope::All
        } else {
            // Namespace-scope grants: root, or an auth_admin delegated for
            // exactly that namespace.
            if !ctx.is_root(&p)? {
                require_cap(ctx, &p, Capability::AuthAdmin, &scope)?;
            }
            Scope::Namespace(&scope)
        };
        if add {
            ctx.core.auth().grant(&target, cap, scope_ref, now)
        } else {
            ctx.core.auth().revoke_grant(&target, cap, scope_ref)
        }
    })?;
    Ok(Outcome::Resp(Resp::empty(204)))
}

// ---------------------------------------------------------------------------
// assets
// ---------------------------------------------------------------------------

fn asset_register(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let ns = body_str(&body, "namespace")?.to_string();
    let id = match body.get("asset_id") {
        None => AssetId::from_bytes(rand16()?),
        Some(v) => v
            .as_str()
            .and_then(|t| t.parse().ok())
            .ok_or(Fail::Http(400, "malformed asset id"))?,
    };
    let now = now_ms();
    let ns_resp = ns.clone();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::AssetRegister, &ns)?;
        ctx.core.catalog().register_asset(&id, &ns, now)?;
        // Mirror for the browse listing (single-writer law: every register
        // flows through this process).
        ctx.asset_index_insert(id.as_bytes(), &ns, now)?;
        Ok(())
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![
            ("asset_id", s(id.to_string())),
            ("namespace", s(ns_resp)),
        ]),
    )))
}

/// Keyset asset listing over the transport registry mirror. The cursor is
/// the last asset id's display spelling; order equals byte order equals
/// display order (fixed-width base32).
fn assets_list(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let ns = head.query_get("ns").map(str::to_string);
    let limit = parse_limit(head.query_get("limit"), 100, 500)?;
    let after = match head.query_get("cursor") {
        None => None,
        Some(t) => Some(*ast_of(t)?.as_bytes()),
    };
    let now = now_ms();
    let rows = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.asset_index_page(ns.as_deref(), after, limit + 1)
    })?;
    let more = rows.len() as u64 > limit;
    let page = &rows[..rows.len().min(limit as usize)];
    let assets: Vec<Value> = page
        .iter()
        .map(|r| {
            obj(vec![
                ("asset_id", s(AssetId::from_bytes(r.asset_id).to_string())),
                ("namespace", s(r.ns.clone())),
                ("created_ms", Value::Int(r.created_ms as i64)),
            ])
        })
        .collect();
    let cursor = match (more, page.last()) {
        (true, Some(last)) => s(AssetId::from_bytes(last.asset_id).to_string()),
        _ => Value::Null,
    };
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("assets", Value::Arr(assets)), ("cursor", cursor)]),
    )))
}

/// Full asset detail: namespace from the core plus the candidate lifecycle
/// list from the registry mirror.
fn asset_get(head: &Head, rc: &RouteCtx, ast: AssetId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let (ns, revs, retired_ms) = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ns = ctx
            .core
            .catalog()
            .asset_namespace(&ast)?
            .ok_or(ServerError::NotFound { what: "asset" })?;
        // Lifecycle comes from the CORE, not the transport mirror: the GC
        // retention rule retires revisions without passing through a route,
        // so a mirrored state could be stale here.
        let revs = ctx.core.catalog().asset_candidates(&ast, 512)?;
        let retired_ms = ctx.core.catalog().asset_retired_ms(&ast)?;
        Ok((ns, revs, retired_ms))
    })?;
    let candidates: Vec<Value> = revs
        .iter()
        .map(|r| {
            let opt = |v: Option<u64>| match v {
                Some(t) => Value::Int(t as i64),
                None => Value::Null,
            };
            // A retired revision reports `retired`, not the `quarantined`
            // its lifecycle row carries: the state machine is shared, the
            // meaning to a client is not (retired bytes are collectable).
            let state = if r.retired_ms.is_some() || retired_ms.is_some() {
                "retired"
            } else {
                r.state.as_str()
            };
            obj(vec![
                ("revision", s(r.revision.to_string())),
                ("state", s(state)),
                ("staged_ms", Value::Int(r.staged_ms as i64)),
                ("published_ms", opt(r.published_ms)),
                ("quarantined_ms", opt(r.quarantined_ms)),
                ("retired_ms", opt(r.retired_ms.or(retired_ms))),
            ])
        })
        .collect();
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("asset_id", s(ast.to_string())),
            ("namespace", s(ns)),
            ("retired", Value::Bool(retired_ms.is_some())),
            ("retired_ms", match retired_ms {
                Some(t) => Value::Int(t as i64),
                None => Value::Null,
            }),
            ("candidates", Value::Arr(candidates)),
        ]),
    )))
}

fn asset_stage(conn: &mut Conn, head: &mut Head, rc: &RouteCtx, ast: AssetId) -> RouteResult<Outcome> {
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
    let now = now_ms();
    let rev = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ns = ctx
            .core
            .catalog()
            .asset_namespace(&ast)?
            .ok_or(ServerError::NotFound { what: "asset" })?;
        require_cap(ctx, &p, Capability::AssetRegister, &ns)?;
        let manifest = AssetManifest::from_canonical_bytes(&bytes)?;
        if manifest.asset_id != ast {
            return Err(ServerError::Conflict { what: "manifest asset_id" });
        }
        let rev = ctx.core.catalog().stage_asset_revision(&bytes, now)?;
        Ok(rev)
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![
            ("asset_id", s(ast.to_string())),
            ("revision", s(rev.to_string())),
            ("state", s("staged")),
        ]),
    )))
}

fn asset_candidate(head: &Head, rc: &RouteCtx, ast: AssetId, rev: AssetRevisionId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let state = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core
            .catalog()
            .asset_candidate_state(&ast, &rev)?
            .ok_or(ServerError::NotFound { what: "candidate" })
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("asset_id", s(ast.to_string())),
            ("revision", s(rev.to_string())),
            ("state", s(state.as_str())),
        ]),
    )))
}

fn asset_lifecycle(
    head: &Head,
    rc: &RouteCtx,
    ast: AssetId,
    rev: AssetRevisionId,
    publish: bool,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let hub = rc.events.clone();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ns = ctx
            .core
            .catalog()
            .asset_namespace(&ast)?
            .ok_or(ServerError::NotFound { what: "asset" })?;
        if publish {
            require_cap(ctx, &p, Capability::AssetPublish, &ns)?;
            ctx.core.catalog().publish_asset(&ast, &rev, now)?;
        } else {
            require_cap(ctx, &p, Capability::AssetQuarantine, &ns)?;
            ctx.core.catalog().quarantine_asset(&ast, &rev, now)?;
        }
        // Emitted only after the core call above committed, still on the
        // state thread, so journal order equals commit order.
        let kind = if publish {
            events::KIND_ASSET_PUBLISHED
        } else {
            events::KIND_ASSET_QUARANTINED
        };
        hub.publish(
            EventBody::asset(kind, &ns, ast.to_string(), now)
                .with_revision(rev.to_string())
                .with_content_kind(annotation_kind(ctx, &ast)),
        );
        Ok(())
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("asset_id", s(ast.to_string())),
            ("revision", s(rev.to_string())),
            ("state", s(if publish { "published" } else { "quarantined" })),
        ]),
    )))
}

/// Delete an asset from the store: every revision retired, every alias head
/// dropped, its whole search footprint removed, and its bytes handed to blob
/// GC. Idempotent — retiring an already retired asset answers 200 with
/// `already_retired: true`.
///
/// Capability: `asset_quarantine` on the asset's namespace, the existing
/// moderation capability for pulling content (retirement is the strongest
/// pull there is). Publishing rights alone must not delete history.
fn asset_retire(head: &Head, rc: &RouteCtx, ast: AssetId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let hub = rc.events.clone();
    let report = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ns = ctx
            .core
            .catalog()
            .asset_namespace(&ast)?
            .ok_or(ServerError::NotFound { what: "asset" })?;
        require_cap(ctx, &p, Capability::AssetQuarantine, &ns)?;
        // The annotation carries the content kind subscribers filter on, and
        // retirement deletes it: read it BEFORE the mutation.
        let content_kind = annotation_kind(ctx, &ast);
        let report = ctx.core.catalog().retire_asset(&ast, now)?;
        ctx.asset_mark_retired(ast.as_bytes(), now)?;
        if !report.already_retired {
            hub.publish(
                EventBody::asset(events::KIND_ASSET_RETIRED, &ns, ast.to_string(), now)
                    .with_content_kind(content_kind),
            );
        }
        Ok(report)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("asset_id", s(ast.to_string())),
            ("state", s("retired")),
            ("already_retired", Value::Bool(report.already_retired)),
            ("revisions_retired", Value::Int(report.revisions_retired as i64)),
            ("aliases_dropped", Value::Int(report.aliases_dropped as i64)),
            ("annotation_cleared", Value::Bool(report.annotation_cleared)),
        ]),
    )))
}

/// Delete ONE revision (a superseded one, typically). The asset itself
/// stays live; alias heads pointing at this revision drop exactly as
/// quarantine drops them.
fn revision_retire(
    head: &Head,
    rc: &RouteCtx,
    ast: AssetId,
    rev: AssetRevisionId,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let hub = rc.events.clone();
    let changed = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ns = ctx
            .core
            .catalog()
            .asset_namespace(&ast)?
            .ok_or(ServerError::NotFound { what: "asset" })?;
        require_cap(ctx, &p, Capability::AssetQuarantine, &ns)?;
        let content_kind = annotation_kind(ctx, &ast);
        let changed = ctx.core.catalog().retire_revision(&ast, &rev, now)?;
        if changed {
            hub.publish(
                EventBody::asset(events::KIND_REVISION_RETIRED, &ns, ast.to_string(), now)
                    .with_revision(rev.to_string())
                    .with_content_kind(content_kind),
            );
        }
        Ok(changed)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("asset_id", s(ast.to_string())),
            ("revision", s(rev.to_string())),
            ("state", s("retired")),
            ("already_retired", Value::Bool(!changed)),
        ]),
    )))
}

// ---------------------------------------------------------------------------
// blob garbage collection
// ---------------------------------------------------------------------------

fn gc_status_value(status: &crate::GcStatus) -> Value {
    obj(vec![
        ("run_id", Value::Int(status.run_id as i64)),
        ("phase", s(status.phase.as_str())),
        ("done", Value::Bool(status.finished())),
        ("dry_run", Value::Bool(status.dry_run)),
        ("started_ms", Value::Int(status.started_ms as i64)),
        ("updated_ms", Value::Int(status.updated_ms as i64)),
        ("horizon_ms", Value::Int(status.horizon_ms as i64)),
        ("retain_keep", match status.retain_keep {
            Some(k) => Value::Int(k as i64),
            None => Value::Null,
        }),
        ("retired_revisions", Value::Int(status.retired_revisions as i64)),
        ("scanned_revisions", Value::Int(status.scanned_revisions as i64)),
        ("marked_blobs", Value::Int(status.marked_blobs as i64)),
        ("examined_blobs", Value::Int(status.examined_blobs as i64)),
        ("unreferenced_blobs", Value::Int(status.unreferenced_blobs as i64)),
        ("unreferenced_bytes", Value::Int(status.unreferenced_bytes as i64)),
        ("deleted_blobs", Value::Int(status.deleted_blobs as i64)),
        ("deleted_bytes", Value::Int(status.deleted_bytes as i64)),
    ])
}

/// Start (or resume) a garbage collection run and do a BOUNDED amount of its
/// work before answering. A whole-store sweep is never performed inside one
/// request: the response carries the durable progress, `done` says whether
/// the run finished, and the caller polls this route (or lets the janitor
/// finish it). Whole-store admin operation: bootstrap admin only.
fn gc_run(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let dry_run = match body.get("dry_run") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(Fail::Http(400, "malformed dry_run")),
    };
    let grace_ms = body_u64(&body, "grace_ms").unwrap_or(rc.cfg.gc_grace_ms);
    let retain_keep = match body.get("retain_per_asset") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let n = v.as_u64().ok_or(Fail::Http(400, "malformed retain_per_asset"))?;
            if n == 0 || n > 10_000 {
                return Err(Fail::Http(400, "retain_per_asset out of range"));
            }
            Some(n as u32)
        }
    };
    let max_steps = match body_u64(&body, "max_steps") {
        None => rc.cfg.gc_max_steps_per_request,
        Some(n) => {
            if n == 0 || n > rc.cfg.gc_max_steps_per_request as u64 {
                return Err(Fail::Http(400, "max_steps out of range"));
            }
            n as u32
        }
    };
    let cfg = crate::GcConfig {
        dry_run,
        grace_ms,
        mark_batch: rc.cfg.gc_mark_batch,
        sweep_batch: rc.cfg.gc_sweep_batch,
        retain_keep,
        retain_batch: rc.cfg.gc_retain_batch,
    };
    let now = now_ms();
    let status = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_root(ctx, &p)?;
        // An active run is RESUMED rather than restarted: two runs would
        // each hold half the truth about what is referenced. Resuming with
        // different policy would silently answer a different question than
        // the caller asked (a "collect" advancing someone's dry run), so
        // that refuses instead — cancel first.
        match ctx.core.gc_status()? {
            Some(existing) if !existing.finished() => {
                if existing.dry_run != cfg.dry_run || existing.retain_keep != cfg.retain_keep {
                    return Err(ServerError::Conflict { what: "gc run already active" });
                }
            }
            _ => {
                ctx.core.gc_begin(cfg, now)?;
            }
        }
        ctx.core
            .gc_advance(max_steps, now)?
            .ok_or(ServerError::NotFound { what: "gc run" })
    })?;
    Ok(Outcome::Resp(Resp::json(200, &gc_status_value(&status))))
}

/// `GET /v1/blob-refs?after=<blob_id>&limit=<n>` — RE-SCAN one bounded page
/// of the reference blobs, reporting what each one's file looks like right
/// now: `present`, `missing`, `size_changed`, `content_changed`, or
/// `unreadable`.
///
/// This is the honest counterpart to not copying: the store cannot promise
/// bytes it does not own, so it offers a way to LOOK. Verifying re-hashes
/// each file, which is why the page is bounded and the caller drives it —
/// a UI can walk a library over many frames, or check one asset after a
/// serve refused.
///
/// Paths are the operator's own filesystem layout, so this is admin-only.
fn blob_refs_rescan(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let after: Option<makepad_asset_data::BlobId> = match head.query_get("after") {
        None => None,
        Some(text) => Some(
            text.parse()
                .map_err(|_| Fail::Http(400, "malformed after blob id"))?,
        ),
    };
    let limit: u32 = match head.query_get("limit") {
        None => 32,
        Some(text) => text
            .parse()
            .ok()
            .filter(|n| (1..=256).contains(n))
            .ok_or(Fail::Http(400, "limit out of range"))?,
    };
    let now = now_ms();
    let (page, total) = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_root(ctx, &p)?;
        let total = ctx.core.blob_refs().count()?;
        Ok((ctx.core.rescan_blob_refs(after.as_ref(), limit)?, total))
    })?;
    let rows: Vec<Value> = page
        .entries
        .iter()
        .map(|(entry, state)| {
            obj(vec![
                ("blob_id", s(entry.blob_id.to_string())),
                ("path", s(entry.path.display().to_string())),
                ("size", Value::Int(entry.size as i64)),
                ("recorded_ms", Value::Int(entry.recorded_ms as i64)),
                ("state", s(state.tag())),
                ("ok", Value::Bool(state.is_present())),
            ])
        })
        .collect();
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("total", Value::Int(total as i64)),
            ("refs", Value::Arr(rows)),
            (
                "next",
                match page.next {
                    Some(id) => s(id.to_string()),
                    None => Value::Null,
                },
            ),
        ]),
    )))
}

fn gc_get(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let status = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_root(ctx, &p)?;
        ctx.core.gc_status()
    })?;
    Ok(Outcome::Resp(match status {
        Some(st) => Resp::json(200, &gc_status_value(&st)),
        None => Resp::json(200, &obj(vec![("run_id", Value::Null), ("done", Value::Bool(true))])),
    }))
}

fn gc_cancel(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let stopped = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_root(ctx, &p)?;
        ctx.core.gc_cancel(now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("cancelled", Value::Bool(stopped))]),
    )))
}

// ---------------------------------------------------------------------------
// manifests
// ---------------------------------------------------------------------------

fn manifest_get(head: &Head, rc: &RouteCtx, rev: AssetRevisionId, as_json: bool) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let bytes = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let bytes = ctx
            .core
            .catalog()
            .asset_revision_manifest(&rev)?
            .ok_or(ServerError::NotFound { what: "manifest" })?;
        // Pulled content stops being served: quarantined revisions present
        // exactly like absent ones.
        let manifest = AssetManifest::from_canonical_bytes(&bytes)?;
        match ctx.core.catalog().asset_candidate_state(&manifest.asset_id, &rev)? {
            Some(CandidateState::Quarantined) | None => {
                Err(ServerError::NotFound { what: "manifest" })
            }
            Some(_) => Ok(bytes),
        }
    })?;
    // Revisions are immutable and content-addressed: the revision id is a
    // perfect strong validator.
    let etag = if as_json {
        format!("\"{rev}.json\"")
    } else {
        format!("\"{rev}\"")
    };
    if let Some(inm) = &head.if_none_match {
        if super::http::etag_matches(inm, &etag) {
            return Ok(Outcome::Resp(
                Resp::empty(304).with_header("ETag", etag),
            ));
        }
    }
    let resp = if as_json {
        let manifest =
            AssetManifest::from_canonical_bytes(&bytes).map_err(ServerError::from)?;
        let value = super::api::asset_manifest_value(&manifest);
        Resp::bytes(200, "application/json", value.to_json().into_bytes())
    } else {
        Resp::bytes(200, "application/octet-stream", bytes)
    };
    Ok(Outcome::Resp(
        resp.with_header("ETag", etag)
            .with_header("Cache-Control", CACHE_IMMUTABLE.to_string()),
    ))
}

fn game_manifest_get(
    head: &Head,
    rc: &RouteCtx,
    rev: GameRevisionId,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let bytes = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let bytes = ctx
            .core
            .catalog()
            .game_revision_manifest(&rev)?
            .ok_or(ServerError::NotFound {
                what: "game revision manifest",
            })?;
        let manifest = GameRevisionManifest::from_canonical_bytes(&bytes)?;
        match ctx
            .core
            .catalog()
            .game_candidate_state(&manifest.game_id, &rev)?
        {
            Some(CandidateState::Quarantined) | None => Err(ServerError::NotFound {
                what: "game revision manifest",
            }),
            Some(_) => Ok(bytes),
        }
    })?;
    let etag = format!("\"{rev}\"");
    if let Some(inm) = &head.if_none_match {
        if super::http::etag_matches(inm, &etag) {
            return Ok(Outcome::Resp(
                Resp::empty(304).with_header("ETag", etag),
            ));
        }
    }
    Ok(Outcome::Resp(
        Resp::bytes(200, "application/octet-stream", bytes)
            .with_header("ETag", etag)
            .with_header("Cache-Control", CACHE_IMMUTABLE.to_string()),
    ))
}

// ---------------------------------------------------------------------------
// asset aliases
// ---------------------------------------------------------------------------

fn alias_get(head: &Head, rc: &RouteCtx, alias: AssetAlias) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let alias_str = alias.as_str().to_string();
    let target = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core
            .catalog()
            .resolve_asset_alias(&alias)?
            .ok_or(ServerError::NotFound { what: "alias" })
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("alias", s(alias_str)),
            ("asset_id", s(target.asset_id.to_string())),
            ("head_revision", s(target.revision.to_string())),
        ]),
    )))
}

fn alias_put(conn: &mut Conn, head: &mut Head, rc: &RouteCtx, alias: AssetAlias) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let target = AssetRevisionRef {
        asset_id: ast_of(body_str(&body, "asset_id")?)?,
        revision: arev_of(body_str(&body, "revision")?)?,
    };
    let now = now_ms();
    let alias_str = alias.as_str().to_string();
    let hub = rc.events.clone();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::AliasWrite, alias.namespace())?;
        ctx.core.catalog().set_asset_alias(&alias, &target, now)?;
        hub.publish(
            EventBody::asset(
                events::KIND_ALIAS_SET,
                alias.namespace(),
                target.asset_id.to_string(),
                now,
            )
            .with_revision(target.revision.to_string())
            .with_alias(alias.as_str().to_string())
            .with_content_kind(annotation_kind(ctx, &target.asset_id)),
        );
        Ok(())
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("alias", s(alias_str)),
            ("asset_id", s(target.asset_id.to_string())),
            ("head_revision", s(target.revision.to_string())),
        ]),
    )))
}

fn alias_delete(head: &Head, rc: &RouteCtx, alias: AssetAlias) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let hub = rc.events.clone();
    let existed = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::AliasWrite, alias.namespace())?;
        // The target is unreadable after the clear; capture it for the event.
        let old = ctx.core.catalog().resolve_asset_alias(&alias)?;
        let existed = ctx.core.catalog().clear_asset_alias(&alias)?;
        if existed {
            let mut body = EventBody {
                kind: events::KIND_ALIAS_CLEARED,
                namespace: alias.namespace().to_string(),
                asset_id: None,
                revision: None,
                game_id: None,
                game_revision: None,
                alias: Some(alias.as_str().to_string()),
                content_kind: None,
                ts_ms: now,
            };
            if let Some(target) = old {
                body.asset_id = Some(target.asset_id.to_string());
                body.revision = Some(target.revision.to_string());
                body.content_kind = annotation_kind(ctx, &target.asset_id);
            }
            hub.publish(body);
        }
        Ok(existed)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("existed", Value::Bool(existed))]),
    )))
}

// ---------------------------------------------------------------------------
// games
// ---------------------------------------------------------------------------

fn game_register(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let ns = body_str(&body, "namespace")?.to_string();
    let id = match body.get("game_id") {
        None => GameId::from_bytes(rand16()?),
        Some(v) => v
            .as_str()
            .and_then(|t| t.parse().ok())
            .ok_or(Fail::Http(400, "malformed game id"))?,
    };
    let now = now_ms();
    let ns_resp = ns.clone();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::GameRegister, &ns)?;
        ctx.core.catalog().register_game(&id, &ns, now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![
            ("game_id", s(id.to_string())),
            ("namespace", s(ns_resp)),
        ]),
    )))
}

fn game_get(head: &Head, rc: &RouteCtx, gam: GameId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let ns = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core
            .catalog()
            .game_namespace(&gam)?
            .ok_or(ServerError::NotFound { what: "game" })
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("game_id", s(gam.to_string())), ("namespace", s(ns))]),
    )))
}

/// A game revision body is two length-prefixed canonical documents:
/// `u64be(manifest len) manifest u64be(lock len) lock`, exact-consume.
fn frames_two(body: &[u8], max_each: u64) -> RouteResult<(Vec<u8>, Vec<u8>)> {
    let malformed = Fail::Http(400, "malformed frame");
    let total = body.len() as u64;
    if total < 16 {
        return Err(malformed);
    }
    let len1 = u64::from_be_bytes(body[0..8].try_into().expect("8 bytes"));
    if len1 > max_each {
        return Err(Fail::Http(413, "frame too large"));
    }
    let lock_len_at = 8u64.checked_add(len1).ok_or(malformed)?;
    if lock_len_at.checked_add(8).map_or(true, |v| v > total) {
        return Err(Fail::Http(400, "malformed frame"));
    }
    let a = lock_len_at as usize;
    let len2 = u64::from_be_bytes(body[a..a + 8].try_into().expect("8 bytes"));
    if len2 > max_each {
        return Err(Fail::Http(413, "frame too large"));
    }
    let end = lock_len_at
        .checked_add(8)
        .and_then(|v| v.checked_add(len2))
        .ok_or(Fail::Http(400, "malformed frame"))?;
    if end != total {
        return Err(Fail::Http(400, "malformed frame"));
    }
    Ok((
        body[8..8 + len1 as usize].to_vec(),
        body[a + 8..(a + 8) + len2 as usize].to_vec(),
    ))
}

fn game_stage(conn: &mut Conn, head: &mut Head, rc: &RouteCtx, gam: GameId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let bytes = match read_body(
        conn,
        head,
        rc.cfg.control_max_body_bytes,
        rc.cfg.control_body_deadline_ms,
    ) {
        Ok(b) => b,
        Err(o) => return Ok(o),
    };
    let (manifest_bytes, lock_bytes) = frames_two(&bytes, rc.cfg.budgets.max_manifest_bytes)?;
    let now = now_ms();
    let rev = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ns = ctx
            .core
            .catalog()
            .game_namespace(&gam)?
            .ok_or(ServerError::NotFound { what: "game" })?;
        require_cap(ctx, &p, Capability::GameRegister, &ns)?;
        let manifest = GameRevisionManifest::from_canonical_bytes(&manifest_bytes)?;
        if manifest.game_id != gam {
            return Err(ServerError::Conflict { what: "manifest game_id" });
        }
        ctx.core.catalog().stage_game_revision(&manifest_bytes, &lock_bytes, now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![
            ("game_id", s(gam.to_string())),
            ("revision", s(rev.to_string())),
            ("state", s("staged")),
        ]),
    )))
}

fn game_candidate(head: &Head, rc: &RouteCtx, gam: GameId, rev: GameRevisionId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let state = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core
            .catalog()
            .game_candidate_state(&gam, &rev)?
            .ok_or(ServerError::NotFound { what: "candidate" })
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("game_id", s(gam.to_string())),
            ("revision", s(rev.to_string())),
            ("state", s(state.as_str())),
        ]),
    )))
}

fn game_lifecycle(
    head: &Head,
    rc: &RouteCtx,
    gam: GameId,
    rev: GameRevisionId,
    publish: bool,
) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let hub = rc.events.clone();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ns = ctx
            .core
            .catalog()
            .game_namespace(&gam)?
            .ok_or(ServerError::NotFound { what: "game" })?;
        if publish {
            require_cap(ctx, &p, Capability::GamePublish, &ns)?;
            ctx.core.catalog().publish_game(&gam, &rev, now)?;
        } else {
            // The core defines one quarantine capability; it is the
            // moderation power for both catalog kinds.
            require_cap(ctx, &p, Capability::AssetQuarantine, &ns)?;
            ctx.core.catalog().quarantine_game(&gam, &rev, now)?;
        }
        let kind = if publish {
            events::KIND_GAME_PUBLISHED
        } else {
            events::KIND_GAME_QUARANTINED
        };
        hub.publish(
            EventBody::game(kind, &ns, gam.to_string(), now)
                .with_game_revision(rev.to_string()),
        );
        Ok(())
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("game_id", s(gam.to_string())),
            ("revision", s(rev.to_string())),
            ("state", s(if publish { "published" } else { "quarantined" })),
        ]),
    )))
}

fn game_refs(head: &Head, rc: &RouteCtx, gam: GameId, rev: GameRevisionId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let refs = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        match ctx.core.catalog().game_candidate_state(&gam, &rev)? {
            None | Some(CandidateState::Quarantined) => {
                Err(ServerError::NotFound { what: "game revision" })
            }
            Some(_) => ctx.core.catalog().game_revision_refs(&rev),
        }
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![(
            "refs",
            Value::Arr(
                refs.iter()
                    .map(|r| {
                        obj(vec![
                            ("asset_id", s(r.asset_id.to_string())),
                            ("revision", s(r.revision.to_string())),
                        ])
                    })
                    .collect(),
            ),
        )]),
    )))
}

// ---------------------------------------------------------------------------
// game aliases
// ---------------------------------------------------------------------------

fn game_alias_get(head: &Head, rc: &RouteCtx, alias: GameAlias) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let alias_str = alias.as_str().to_string();
    let (gam, rev) = call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        ctx.core
            .catalog()
            .resolve_game_alias(&alias)?
            .ok_or(ServerError::NotFound { what: "alias" })
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("alias", s(alias_str)),
            ("game_id", s(gam.to_string())),
            ("head_revision", s(rev.to_string())),
        ]),
    )))
}

fn game_alias_put(conn: &mut Conn, head: &mut Head, rc: &RouteCtx, alias: GameAlias) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let gam = gam_of(body_str(&body, "game_id")?)?;
    let rev = grev_of(body_str(&body, "revision")?)?;
    let now = now_ms();
    let alias_str = alias.as_str().to_string();
    let hub = rc.events.clone();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::AliasWrite, alias.namespace())?;
        ctx.core.catalog().set_game_alias(&alias, &gam, &rev, now)?;
        hub.publish(
            EventBody::game(
                events::KIND_GAME_ALIAS_SET,
                alias.namespace(),
                gam.to_string(),
                now,
            )
            .with_game_revision(rev.to_string())
            .with_alias(alias.as_str().to_string()),
        );
        Ok(())
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("alias", s(alias_str)),
            ("game_id", s(gam.to_string())),
            ("head_revision", s(rev.to_string())),
        ]),
    )))
}

fn game_alias_delete(head: &Head, rc: &RouteCtx, alias: GameAlias) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let hub = rc.events.clone();
    let existed = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::AliasWrite, alias.namespace())?;
        let old = ctx.core.catalog().resolve_game_alias(&alias)?;
        let existed = ctx.core.catalog().clear_game_alias(&alias)?;
        if existed {
            let mut body = EventBody {
                kind: events::KIND_GAME_ALIAS_CLEARED,
                namespace: alias.namespace().to_string(),
                asset_id: None,
                revision: None,
                game_id: None,
                game_revision: None,
                alias: Some(alias.as_str().to_string()),
                content_kind: None,
                ts_ms: now,
            };
            if let Some((gam, rev)) = old {
                body.game_id = Some(gam.to_string());
                body.game_revision = Some(rev.to_string());
            }
            hub.publish(body);
        }
        Ok(existed)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("existed", Value::Bool(existed))]),
    )))
}

// ---------------------------------------------------------------------------
// catalog event feed
// ---------------------------------------------------------------------------

/// The asset's declared content kind (from its annotation) at emit time, as
/// the lowercase wire word. Never fails the surrounding mutation — the
/// mutation is already committed; a failed lookup just means `None`.
fn annotation_kind(ctx: &StateCtx, ast: &AssetId) -> Option<&'static str> {
    ctx.core
        .search()
        .annotation(ast)
        .ok()
        .flatten()
        .and_then(|a| a.kind)
        .map(kind_name)
}

fn events_resp(
    events: &[CatalogEvent],
    cursor: &EventCursor,
    gap: bool,
    vocabulary: u32,
) -> Resp {
    let arr = events
        .iter()
        .map(|e| {
            let mut pairs = vec![
                ("seq", Value::Int(e.seq as i64)),
                // Kinds added after v1 are rendered in the vocabulary the
                // subscriber asked for, so an older client never sees a
                // kind it would refuse.
                ("kind", s(events::downgrade_kind(e.kind, vocabulary))),
                ("ns", s(e.namespace.clone())),
            ];
            if let Some(v) = &e.asset_id {
                pairs.push(("asset_id", s(v.clone())));
            }
            if let Some(v) = &e.revision {
                pairs.push(("revision", s(v.clone())));
            }
            if let Some(v) = &e.game_id {
                pairs.push(("game_id", s(v.clone())));
            }
            if let Some(v) = &e.game_revision {
                pairs.push(("game_revision", s(v.clone())));
            }
            if let Some(v) = &e.alias {
                pairs.push(("alias", s(v.clone())));
            }
            if let Some(k) = e.content_kind {
                pairs.push(("content_kind", s(k)));
            }
            pairs.push(("ts_ms", Value::Int(e.ts_ms as i64)));
            obj(pairs)
        })
        .collect();
    Resp::json(
        200,
        &obj(vec![
            ("events", Value::Arr(arr)),
            ("cursor", s(cursor.render())),
            ("gap", Value::Bool(gap)),
        ]),
    )
}

/// `GET /v1/events?cursor=&wait=&limit=&kind=` — resumable long-poll over
/// the committed-catalog journal. Without a cursor it returns the tail
/// resume point immediately (the subscriber then loads its catalog view
/// once and follows from there). Waiting happens on the connection thread
/// against the event hub — never on the state thread, never inside a core
/// transaction.
fn events_route(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    // Authenticate before any wait: unauthenticated pollers never park a
    // thread, and the refusal is the uniform 401.
    call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        Ok(())
    })?;

    // `wait` may legitimately be zero (pure poll); values clamp to the
    // configured ceiling.
    let wait_ms = match head.query_get("wait") {
        None => 0,
        Some(t) => {
            if t.is_empty() || t.len() > 6 || !t.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Fail::Http(400, "malformed wait"));
            }
            let n: u64 = t.parse().map_err(|_| Fail::Http(400, "malformed wait"))?;
            n.min(rc.cfg.event_max_wait_ms)
        }
    };
    let limit = parse_limit(
        head.query_get("limit"),
        rc.cfg.event_max_batch as u64,
        rc.cfg.event_max_batch as u64,
    )? as usize;
    let kind = head
        .query_get("kind")
        .map(parse_kind)
        .transpose()?
        .map(kind_name);
    // Event vocabulary the subscriber speaks. Absent = 1 (the kinds that
    // shipped before retirement existed).
    let vocabulary = match head.query_get("ev") {
        None => 1,
        Some(t) => {
            if t.len() > 2 || !t.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Fail::Http(400, "malformed event vocabulary"));
            }
            let v: u32 = t.parse().map_err(|_| Fail::Http(400, "malformed event vocabulary"))?;
            if v == 0 || v > events::EVENT_VOCABULARY {
                return Err(Fail::Http(400, "unsupported event vocabulary"));
            }
            v
        }
    };

    let Some(cursor_text) = head.query_get("cursor") else {
        let tail = rc.events.tail_cursor();
        return Ok(Outcome::Resp(events_resp(&[], &tail, false, vocabulary)));
    };
    let mut cursor =
        EventCursor::parse(cursor_text).ok_or(Fail::Http(400, "malformed cursor"))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    loop {
        let poll = rc.events.poll_after(cursor, kind, limit);
        if poll.gap || !poll.events.is_empty() {
            return Ok(Outcome::Resp(events_resp(
                &poll.events,
                &poll.cursor,
                poll.gap,
                vocabulary,
            )));
        }
        // Empty scan still advances past filtered-out events; wait (and
        // later polls) resume from the advanced cursor.
        cursor = poll.cursor;
        if wait_ms == 0
            || std::time::Instant::now() >= deadline
            || !rc.events.wait_beyond(cursor.seq, deadline)
        {
            return Ok(Outcome::Resp(events_resp(&[], &cursor, false, vocabulary)));
        }
    }
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

struct SearchParams {
    text: String,
    namespace: Option<String>,
    kind: Option<AssetKind>,
    category: Option<String>,
    tag: Option<String>,
    /// Negative tag filter, validated by the core exactly like `tag`.
    exclude_tag: Option<String>,
    creator: Option<String>,
    generator: Option<String>,
    backend: Option<String>,
    model: Option<String>,
    owner_me: bool,
    live_only: bool,
    page_size: u32,
    cursor: Option<Vec<u8>>,
    /// Facet rows to return with the page; 0 (the default) asks for none.
    facets: u32,
}

fn parse_kind(t: &str) -> RouteResult<AssetKind> {
    kind_parse(t).ok_or(Fail::Http(400, "unknown asset kind"))
}

fn parse_flag(t: &str) -> RouteResult<bool> {
    match t {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(Fail::Http(400, "malformed flag")),
    }
}

fn parse_cursor(t: &str) -> RouteResult<Vec<u8>> {
    from_hex_bounded(t, crate::search::MAX_SEARCH_CURSOR_BYTES)
        .ok_or(Fail::Http(400, "malformed cursor"))
}

fn search_params_from_query(head: &Head, rc: &RouteCtx) -> RouteResult<SearchParams> {
    let q = |k: &str| head.query_get(k).map(str::to_string);
    let page_size = parse_limit(
        head.query_get("limit"),
        20,
        rc.cfg.budgets.max_search_results as u64,
    )? as u32;
    Ok(SearchParams {
        // The request-target charset has no spaces; multi-term queries join
        // terms with any non-alphanumeric byte the charset allows (`-`, `.`,
        // `_`) — the tokenizer splits on all of them. POST /v1/search takes
        // free text.
        text: q("q").unwrap_or_default(),
        namespace: q("ns"),
        kind: head.query_get("kind").map(parse_kind).transpose()?,
        category: q("category"),
        tag: q("tag"),
        exclude_tag: q("exclude_tag"),
        creator: q("creator"),
        generator: q("generator"),
        backend: q("backend"),
        model: q("model"),
        owner_me: match head.query_get("owner") {
            None => false,
            Some("me") => true,
            Some(_) => return Err(Fail::Http(400, "owner filter must be me")),
        },
        live_only: head.query_get("live").map(parse_flag).transpose()?.unwrap_or(false),
        page_size,
        cursor: head.query_get("cursor").map(parse_cursor).transpose()?,
        facets: match head.query_get("facets") {
            None => 0,
            Some(_) => parse_limit(
                head.query_get("facets"),
                0,
                rc.cfg.budgets.max_search_facets as u64,
            )? as u32,
        },
    })
}

fn search_params_from_body(body: &Value, rc: &RouteCtx) -> RouteResult<SearchParams> {
    let field = |k: &str| -> RouteResult<Option<String>> {
        match body.get(k) {
            None => Ok(None),
            Some(v) => Ok(Some(
                v.as_str().ok_or(Fail::Http(400, "malformed search field"))?.to_string(),
            )),
        }
    };
    let page_size = match body.get("limit") {
        None => 20,
        Some(v) => {
            let n = v.as_u64().ok_or(Fail::Http(400, "malformed limit"))?;
            if n == 0 {
                return Err(Fail::Http(400, "malformed limit"));
            }
            n.min(rc.cfg.budgets.max_search_results as u64) as u32
        }
    };
    Ok(SearchParams {
        text: field("q")?.unwrap_or_default(),
        namespace: field("ns")?,
        kind: field("kind")?.as_deref().map(parse_kind).transpose()?,
        category: field("category")?,
        tag: field("tag")?,
        exclude_tag: field("exclude_tag")?,
        creator: field("creator")?,
        generator: field("generator")?,
        backend: field("backend")?,
        model: field("model")?,
        owner_me: match field("owner")?.as_deref() {
            None => false,
            Some("me") => true,
            Some(_) => return Err(Fail::Http(400, "owner filter must be me")),
        },
        live_only: match body.get("live") {
            None => false,
            Some(v) => v.as_bool().ok_or(Fail::Http(400, "malformed flag"))?,
        },
        page_size,
        cursor: field("cursor")?.as_deref().map(parse_cursor).transpose()?,
        facets: match body.get("facets") {
            None => 0,
            Some(v) => {
                let n = v.as_u64().ok_or(Fail::Http(400, "malformed facets"))?;
                n.min(rc.cfg.budgets.max_search_facets as u64) as u32
            }
        },
    })
}

fn run_search(head: &Head, rc: &RouteCtx, params: SearchParams) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let page = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let filters = SearchFilters {
            namespace: params.namespace.as_deref(),
            kind: params.kind,
            category: params.category.as_deref(),
            tag: params.tag.as_deref(),
            exclude_tag: params.exclude_tag.as_deref(),
            creator: params.creator.as_deref(),
            generator: params.generator.as_deref(),
            backend: params.backend.as_deref(),
            model: params.model.as_deref(),
            owner: if params.owner_me { Some(p) } else { None },
            live_only: params.live_only,
        };
        let query = SearchQuery {
            text: &params.text,
            filters,
            page_size: params.page_size,
            facets: params.facets,
        };
        // Read policy: every authenticated principal browses the whole
        // catalog; private annotation fields are still owner-only via the
        // core's dual-weight index and snippet rules.
        let viewer = SearchViewer { principal: Some(p), scope: ViewerScope::All };
        ctx.core.search().search(&query, &viewer, params.cursor.as_deref())
    })?;
    let hits = page
        .hits
        .iter()
        .map(|h| {
            obj(vec![
                ("asset_id", s(h.asset_id.to_string())),
                ("namespace", s(h.namespace.clone())),
                ("kind", match h.kind {
                    Some(k) => s(kind_name(k)),
                    None => Value::Null,
                }),
                ("title", s(h.title.clone())),
                ("snippet", s(h.snippet.clone())),
                ("score", Value::Int(h.score as i64)),
                ("live", Value::Bool(h.live)),
                ("alias", match &h.alias {
                    Some(a) => s(a.clone()),
                    None => Value::Null,
                }),
                ("updated_ms", Value::Int(h.updated_ms as i64)),
            ])
        })
        .collect();
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![
            ("hits", Value::Arr(hits)),
            ("total", Value::Int(page.total as i64)),
            ("cursor", match &page.cursor {
                Some(c) => s(to_hex(c)),
                None => Value::Null,
            }),
            // Absent unless asked for: an older client parses the page it
            // knows and a newer one reads the labels it asked to count.
            ("facets", Value::Arr(
                page.facets
                    .iter()
                    .map(|facet| {
                        obj(vec![
                            ("kind", s(facet.kind.as_str())),
                            ("label", s(facet.label.clone())),
                            ("count", Value::Int(facet.count as i64)),
                        ])
                    })
                    .collect(),
            )),
        ]),
    )))
}

// ---------------------------------------------------------------------------
// annotations
// ---------------------------------------------------------------------------

fn body_labels(body: &Value, key: &'static str) -> RouteResult<Vec<String>> {
    match body.get(key) {
        None => Ok(Vec::new()),
        Some(v) => {
            let arr = v.as_arr().ok_or(Fail::Http(400, "labels must be an array"))?;
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(
                    item.as_str()
                        .ok_or(Fail::Http(400, "labels must be strings"))?
                        .to_string(),
                );
            }
            Ok(out)
        }
    }
}

fn opt_str(body: &Value, key: &'static str) -> RouteResult<String> {
    match body.get(key) {
        None => Ok(String::new()),
        Some(v) => Ok(v
            .as_str()
            .ok_or(Fail::Http(400, "malformed annotation field"))?
            .to_string()),
    }
}

fn annotation_put(conn: &mut Conn, head: &mut Head, rc: &RouteCtx, ast: AssetId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let title = body_str(&body, "title")?.to_string();
    let description = opt_str(&body, "description")?;
    let kind = match body.get("kind") {
        None => None,
        Some(v) => Some(parse_kind(
            v.as_str().ok_or(Fail::Http(400, "malformed annotation field"))?,
        )?),
    };
    let categories = body_labels(&body, "categories")?;
    let tags = body_labels(&body, "tags")?;
    let creator = opt_str(&body, "creator")?;
    let generator = opt_str(&body, "generator")?;
    let backend = opt_str(&body, "backend")?;
    let model = opt_str(&body, "model")?;
    let prompt = opt_str(&body, "prompt")?;
    let provenance = opt_str(&body, "provenance")?;
    let visibility = match body.get("visibility") {
        None => Visibility::Public,
        Some(v) => match v.as_str() {
            Some("public") => Visibility::Public,
            Some("private") => Visibility::Private,
            _ => return Err(Fail::Http(400, "malformed visibility")),
        },
    };
    let now = now_ms();
    let hub = rc.events.clone();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ns = ctx
            .core
            .catalog()
            .asset_namespace(&ast)?
            .ok_or(ServerError::NotFound { what: "asset" })?;
        require_cap(ctx, &p, Capability::AssetRegister, &ns)?;
        // An existing owned annotation may only be replaced by its owner (or
        // root): overwriting is destruction of another principal's private
        // metadata, so it is refused, never silent.
        if let Some(prev) = ctx.core.search().annotation(&ast)? {
            if let Some(owner) = prev.owner {
                if owner != p && !ctx.is_root(&p)? {
                    return Err(ServerError::Denied { capability: "annotation_owner" });
                }
            }
        }
        let ann = AssetAnnotation {
            title,
            description,
            kind,
            categories,
            tags,
            creator,
            owner: Some(p),
            generator,
            backend,
            model,
            prompt,
            provenance,
            visibility,
        };
        ctx.core.search().set_annotation(&ast, &ann, now)?;
        hub.publish(
            EventBody::asset(events::KIND_ANNOTATION_SET, &ns, ast.to_string(), now)
                .with_content_kind(ann.kind.map(kind_name)),
        );
        Ok(())
    })?;
    Ok(Outcome::Resp(Resp::empty(204)))
}

fn annotation_get(head: &Head, rc: &RouteCtx, ast: AssetId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let (ann, privileged) = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let ann = ctx
            .core
            .search()
            .annotation(&ast)?
            .ok_or(ServerError::NotFound { what: "annotation" })?;
        let privileged = ann.owner == Some(p) || ctx.is_root(&p)?;
        // A private annotation is indistinguishable from an absent one for
        // everyone but its owner: same status, same bytes, no oracle.
        if !privileged && ann.visibility == Visibility::Private {
            return Err(ServerError::NotFound { what: "annotation" });
        }
        Ok((ann, privileged))
    })?;
    let labels = |v: &[String]| Value::Arr(v.iter().map(|l| s(l.clone())).collect());
    let mut pairs = vec![
        ("asset_id", s(ast.to_string())),
        ("title", s(ann.title)),
        ("description", s(ann.description)),
        ("kind", match ann.kind {
            Some(k) => s(kind_name(k)),
            None => Value::Null,
        }),
        ("categories", labels(&ann.categories)),
        ("tags", labels(&ann.tags)),
        ("creator", s(ann.creator)),
        ("generator", s(ann.generator)),
        ("backend", s(ann.backend)),
        ("model", s(ann.model)),
        ("visibility", s(match ann.visibility {
            Visibility::Public => "public",
            Visibility::Private => "private",
        })),
    ];
    if privileged {
        // Prompt, provenance and ownership are owner-only fields, exactly as
        // in the search index weights.
        pairs.push(("prompt", s(ann.prompt)));
        pairs.push(("provenance", s(ann.provenance)));
        if let Some(owner) = &ann.owner {
            pairs.push(("owner", s(principal_str(owner))));
        }
    }
    Ok(Outcome::Resp(Resp::json(200, &obj(pairs))))
}

fn annotation_delete(head: &Head, rc: &RouteCtx, ast: AssetId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let hub = rc.events.clone();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        match ctx.core.search().annotation(&ast)? {
            // Clearing what does not exist is a successful no-op.
            None => Ok(()),
            Some(ann) => {
                let allowed = ann.owner == Some(p) || ctx.is_root(&p)?;
                if !allowed {
                    return Err(ServerError::Denied { capability: "annotation_owner" });
                }
                let ns = ctx.core.catalog().asset_namespace(&ast)?.unwrap_or_default();
                ctx.core.search().clear_annotation(&ast)?;
                hub.publish(
                    EventBody::asset(events::KIND_ANNOTATION_CLEARED, &ns, ast.to_string(), now)
                        .with_content_kind(ann.kind.map(kind_name)),
                );
                Ok(())
            }
        }
    })?;
    Ok(Outcome::Resp(Resp::empty(204)))
}

// ---------------------------------------------------------------------------
// job profiles
// ---------------------------------------------------------------------------

/// `GET /v1/job-profiles[?domain=…]` — the generation capabilities this
/// deployment advertises. Authenticated like every other read; clients
/// enqueue against the returned kind/namespace and never learn a compute
/// node address.
///
/// The answer is the deployment's config advertisement MERGED with what
/// live workers announced they can execute right now (see
/// `super::profiles`), so a tier whose weights left the fleet stops being
/// offered instead of accepting jobs that would sit pending forever.
fn job_profiles(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    call_state(&rc.state, move |ctx| {
        ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        Ok(())
    })?;
    let domain = head.query_get("domain");
    let profiles: Vec<Value> = rc
        .profiles
        .advertised(&rc.cfg.job_profiles, now)
        .iter()
        .filter(|p| domain.is_none_or(|d| p.domain == d))
        .map(|p| {
            obj(vec![
                ("id", s(p.id.clone())),
                ("domain", s(p.domain.clone())),
                ("label", s(p.label.clone())),
                ("kind", s(p.kind.clone())),
                ("namespace", s(p.namespace.clone())),
                ("defaults", p.defaults.clone()),
            ])
        })
        .collect();
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("profiles", Value::Arr(profiles))]),
    )))
}

/// `PUT /v1/job-profiles` — a worker announces what it can execute.
///
/// Body: `{"worker":"<id>", "namespace":"<ns>", "ttl_ms":<lease>,
/// "domains":[…], "profiles":[…]}`. An empty `profiles` list with a
/// non-empty `domains` list is the meaningful "I cover these domains and
/// can execute nothing in them" statement; `retract: true` withdraws the
/// announcement outright (clean worker shutdown).
///
/// Authorization: `job_worker` on the worker's own namespace AND on every
/// namespace an announced profile routes jobs into — the same grant that
/// lets the announcer claim those jobs. A principal that cannot work a
/// namespace cannot advertise into it.
fn job_profiles_announce(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    use super::profiles::{parse_announced_profiles, MAX_DOMAINS, MAX_PROFILES};
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let worker = body_str(&body, "worker")?.to_string();
    if worker.is_empty() || worker.len() > 64 || worker.chars().any(char::is_control) {
        return Err(Fail::Http(400, "worker id out of bounds"));
    }
    let ns = body_str(&body, "namespace")?.to_string();
    let retract = body.get("retract").and_then(Value::as_bool).unwrap_or(false);
    let now = now_ms();
    if retract {
        // A retraction proves nothing about namespaces (its profile list is
        // gone), so it only needs an authenticated caller that holds
        // job_worker somewhere — the same principal that announced.
        call_state(&rc.state, move |ctx| {
            ctx.core.auth().authenticate(secret.as_bytes(), now)?;
            Ok(())
        })?;
        rc.profiles.retract(&worker, now);
        return Ok(Outcome::Resp(Resp::empty(204)));
    }
    let ttl_ms = body
        .get("ttl_ms")
        .and_then(Value::as_u64)
        .ok_or(Fail::Http(400, "ttl_ms required"))?;
    let domains: Vec<String> = match body.get("domains") {
        None => Vec::new(),
        Some(v) => {
            let arr = v.as_arr().ok_or(Fail::Http(400, "domains must be an array"))?;
            if arr.len() > MAX_DOMAINS {
                return Err(Fail::Http(400, "too many domains"));
            }
            let mut out = Vec::with_capacity(arr.len());
            for value in arr {
                let d = value.as_str().ok_or(Fail::Http(400, "domains must be strings"))?;
                if d.is_empty() || d.len() > 32 || d.chars().any(char::is_control) {
                    return Err(Fail::Http(400, "domain out of bounds"));
                }
                out.push(d.to_string());
            }
            out
        }
    };
    let rows: Vec<Value> = match body.get("profiles") {
        None => Vec::new(),
        Some(v) => {
            let arr = v.as_arr().ok_or(Fail::Http(400, "profiles must be an array"))?;
            if arr.len() > MAX_PROFILES {
                return Err(Fail::Http(400, "too many profiles"));
            }
            arr.to_vec()
        }
    };
    let profiles = parse_announced_profiles(&rows).map_err(|what| Fail::Http(400, what))?;
    // Every namespace the announcement touches, checked once: the worker's
    // own namespace (a domains-only announcement HIDES advertisements there,
    // which is a privileged effect on its own) plus each profile's.
    let mut namespaces: Vec<String> = vec![ns.clone()];
    for profile in &profiles {
        if !namespaces.contains(&profile.namespace) {
            namespaces.push(profile.namespace.clone());
        }
    }
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        for ns in &namespaces {
            require_cap(ctx, &p, Capability::JobWorker, ns)?;
        }
        Ok(())
    })?;
    rc.profiles
        .announce(&worker, ttl_ms, domains, profiles, now)
        .map_err(|e| Fail::Http(400, e.as_str()))?;
    Ok(Outcome::Resp(Resp::empty(204)))
}

// ---------------------------------------------------------------------------
// jobs
// ---------------------------------------------------------------------------

const JOB_VIEW_CAPS: [Capability; 3] =
    [Capability::JobEnqueue, Capability::JobWorker, Capability::JobCancel];

/// True when the principal holds any of `caps` on `ns`. Only `Denied` is
/// swallowed; store failures propagate.
fn holds_any(ctx: &StateCtx, p: &PrincipalId, ns: &str, caps: &[Capability]) -> Result<bool, ServerError> {
    for cap in caps {
        match require_cap(ctx, p, *cap, ns) {
            Ok(()) => return Ok(true),
            Err(ServerError::Denied { .. }) => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

fn job_enqueue(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let ns = body_str(&body, "namespace")?.to_string();
    let kind = body_str(&body, "kind")?.to_string();
    let job_body = match body.get("body") {
        None => Value::Obj(Vec::new()),
        Some(v @ Value::Obj(_)) => v.clone(),
        Some(_) => return Err(Fail::Http(400, "job body must be an object")),
    };
    let priority = match body.get("priority") {
        None => 0,
        Some(v) => v.as_i64().ok_or(Fail::Http(400, "malformed priority"))?,
    };
    let max_attempts = match body.get("max_attempts") {
        None => 1u32,
        Some(v) => u32::try_from(v.as_u64().ok_or(Fail::Http(400, "malformed max_attempts"))?)
            .map_err(|_| Fail::Http(400, "malformed max_attempts"))?,
    };
    let not_before_ms = body_u64(&body, "not_before_ms").unwrap_or(0);
    let parent = match body.get("parent") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .and_then(parse_job)
                .ok_or(Fail::Http(400, "malformed parent job id"))?,
        ),
    };
    let deps: Vec<JobId> = match body.get("deps") {
        None => Vec::new(),
        Some(v) => {
            let arr = v.as_arr().ok_or(Fail::Http(400, "deps must be an array"))?;
            if arr.len() as u64 > rc.cfg.budgets.max_job_deps as u64 {
                return Err(Fail::Http(413, "too many deps"));
            }
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(
                    item.as_str()
                        .and_then(parse_job)
                        .ok_or(Fail::Http(400, "malformed dep job id"))?,
                );
            }
            out
        }
    };
    let job_id = JobId(rand16()?);
    let now = now_ms();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        require_cap(ctx, &p, Capability::JobEnqueue, &ns)?;
        // Bound the distinct-namespace set the worker claim gate iterates.
        let known = ctx.meta_distinct_ns()?;
        if !known.iter().any(|k| k == &ns) && known.len() as u64 >= MAX_JOB_NAMESPACES {
            return Err(ServerError::OverBudget {
                what: "job namespaces",
                limit: MAX_JOB_NAMESPACES,
                found: known.len() as u64 + 1,
            });
        }
        // Parent and dependencies must live in the same namespace: cancel
        // cascades through the parent tree and scheduling follows deps, so a
        // cross-namespace edge would let one tenant's job steer another's.
        if let Some(parent) = &parent {
            let m = ctx
                .meta_get(parent)?
                .ok_or(ServerError::NotFound { what: "parent job" })?;
            if m.ns != ns {
                return Err(ServerError::Conflict { what: "parent job namespace" });
            }
        }
        for dep in &deps {
            let m = ctx
                .meta_get(dep)?
                .ok_or(ServerError::NotFound { what: "job dependency" })?;
            if m.ns != ns {
                return Err(ServerError::Conflict { what: "job dependency namespace" });
            }
        }
        let payload = envelope_build(&ns, &p, &job_body);
        // Routing metadata first: the claim gate reads namespaces from it,
        // so a crash between the two writes leaves a gated orphan row (jobs
        // never run un-gated), which status reads treat as absent.
        ctx.meta_insert(&job_id, &ns, &kind, &p, now)?;
        let new_job = NewJob {
            job_id,
            parent,
            kind: &kind,
            payload: &payload,
            priority,
            max_attempts,
            not_before_ms,
            deps: &deps,
        };
        if let Err(e) = ctx.core.jobs().enqueue(&new_job, now) {
            let _ = ctx.meta_delete(&job_id);
            return Err(e);
        }
        Ok(())
    })?;
    Ok(Outcome::Resp(Resp::json(
        201,
        &obj(vec![("job", s(super::api::job_str(&job_id)))]),
    )))
}

fn attempts_value(attempts: &[crate::AttemptRow]) -> Value {
    Value::Arr(
        attempts
            .iter()
            .map(|a| {
                obj(vec![
                    ("attempt", Value::Int(a.attempt as i64)),
                    ("started_ms", Value::Int(a.started_ms as i64)),
                    ("ended_ms", match a.ended_ms {
                        Some(t) => Value::Int(t as i64),
                        None => Value::Null,
                    }),
                ])
            })
            .collect(),
    )
}

fn job_get(head: &Head, rc: &RouteCtx, job: JobId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let (meta, state, attempts, progress, result) = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let meta = ctx
            .meta_get(&job)?
            .ok_or(ServerError::NotFound { what: "job" })?;
        // Visible to the enqueuer and to principals holding any job
        // capability on the namespace; everyone else sees the same 404 an
        // unknown id gets — job ids don't leak across tenants.
        if meta.enqueued_by != p && !holds_any(ctx, &p, &meta.ns, &JOB_VIEW_CAPS)? {
            return Err(ServerError::NotFound { what: "job" });
        }
        let state = ctx
            .core
            .jobs()
            .state(&job)?
            .ok_or(ServerError::NotFound { what: "job" })?;
        let attempts = ctx.core.jobs().attempts(&job)?;
        let progress = ctx.progress_get(&job)?;
        let result = ctx.result_get(&job)?;
        Ok((meta, state, attempts, progress, result))
    })?;
    let mut pairs = vec![
        ("job", s(super::api::job_str(&job))),
        ("namespace", s(meta.ns)),
        ("kind", s(meta.kind)),
        ("state", s(state.as_str())),
        ("enqueued_by", s(principal_str(&meta.enqueued_by))),
        ("created_ms", Value::Int(meta.created_ms as i64)),
        ("attempts", attempts_value(&attempts)),
    ];
    if let Some(pr) = progress {
        pairs.push(("progress", obj(vec![
            ("permille", Value::Int(pr.permille as i64)),
            ("note", s(pr.note)),
            ("updated_ms", Value::Int(pr.updated_ms as i64)),
        ])));
    }
    if let Some(r) = result {
        let body = super::json::parse(&r.body).map_err(|_| {
            Fail::Srv(ServerError::Io { op: "job result body", kind: std::io::ErrorKind::InvalidData })
        })?;
        pairs.push(("result", obj(vec![
            ("outcome", s(r.outcome)),
            ("attempt", Value::Int(r.attempt as i64)),
            ("recorded_ms", Value::Int(r.recorded_ms as i64)),
            ("body", body),
        ])));
    }
    Ok(Outcome::Resp(Resp::json(200, &obj(pairs))))
}

fn job_list(head: &Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let ns = head.query_get("ns").map(str::to_string);
    let limit = parse_limit(head.query_get("limit"), 50, 500)?;
    let now = now_ms();
    let rows = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let metas = match &ns {
            Some(ns) => {
                if !holds_any(ctx, &p, ns, &JOB_VIEW_CAPS)? {
                    return Err(ServerError::Denied { capability: "job_enqueue" });
                }
                ctx.meta_list_ns(ns, limit)?
            }
            // No namespace filter: the caller's own jobs.
            None => ctx.meta_list_by(&p, limit)?,
        };
        let mut rows = Vec::with_capacity(metas.len());
        for meta in metas {
            // Skip crash-window orphans (meta row without a core job).
            if let Some(state) = ctx.core.jobs().state(&meta.job_id)? {
                rows.push((meta, state));
            }
        }
        Ok(rows)
    })?;
    let jobs = rows
        .iter()
        .map(|(meta, state)| {
            obj(vec![
                ("job", s(super::api::job_str(&meta.job_id))),
                ("namespace", s(meta.ns.clone())),
                ("kind", s(meta.kind.clone())),
                ("state", s(state.as_str())),
                ("enqueued_by", s(principal_str(&meta.enqueued_by))),
                ("created_ms", Value::Int(meta.created_ms as i64)),
            ])
        })
        .collect();
    Ok(Outcome::Resp(Resp::json(200, &obj(vec![("jobs", Value::Arr(jobs))]))))
}

fn job_cancel(head: &Head, rc: &RouteCtx, job: JobId) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let now = now_ms();
    let cancelled = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let meta = ctx
            .meta_get(&job)?
            .ok_or(ServerError::NotFound { what: "job" })?;
        require_cap(ctx, &p, Capability::JobCancel, &meta.ns)?;
        ctx.core.jobs().cancel(&job, now)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("cancelled", Value::Int(cancelled as i64))]),
    )))
}

// ---------------------------------------------------------------------------
// worker protocol
// ---------------------------------------------------------------------------

/// Optional worker suffix, letting one principal run several worker threads
/// with distinct lease identities.
pub(crate) fn worker_suffix(body: &Value) -> RouteResult<Option<String>> {
    match body.get("suffix") {
        None => Ok(None),
        Some(v) => {
            let t = v.as_str().ok_or(Fail::Http(400, "malformed worker suffix"))?;
            if t.is_empty()
                || t.len() > 32
                || !t.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
                })
            {
                return Err(Fail::Http(400, "malformed worker suffix"));
            }
            Ok(Some(t.to_string()))
        }
    }
}

/// Worker identity is always derived from the authenticated principal, so
/// one principal can never report on another's lease.
pub(crate) fn worker_name(p: &PrincipalId, suffix: &Option<String>) -> String {
    match suffix {
        None => principal_str(p),
        Some(sfx) => format!("{}/{}", principal_str(p), sfx),
    }
}

/// Bounded worker-supplied JSON document (result or error) as stored bytes.
fn result_bytes(body: &Value, key: &'static str) -> RouteResult<Option<Vec<u8>>> {
    match body.get(key) {
        None => Ok(None),
        Some(v @ Value::Obj(_)) => {
            let bytes = v.to_json().into_bytes();
            if bytes.len() > MAX_RESULT_BYTES {
                return Err(Fail::Http(413, "result too large"));
            }
            Ok(Some(bytes))
        }
        Some(_) => Err(Fail::Http(400, "result must be an object")),
    }
}

fn job_of_body(body: &Value) -> RouteResult<JobId> {
    parse_job(body_str(body, "job")?).ok_or(Fail::Http(400, "malformed job id"))
}

fn worker_allowed_kinds(body: &Value) -> RouteResult<Option<Vec<String>>> {
    let Some(value) = body.get("kinds") else {
        return Ok(None);
    };
    let kinds = value.as_arr().ok_or(Fail::Http(400, "worker kinds must be an array"))?;
    if kinds.is_empty() || kinds.len() > 32 {
        return Err(Fail::Http(400, "worker kinds count out of bounds"));
    }
    let mut out = Vec::with_capacity(kinds.len());
    for value in kinds {
        let kind = value.as_str().ok_or(Fail::Http(400, "worker kinds must be strings"))?;
        if kind.is_empty()
            || kind.len() > 64
            || !kind
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
        {
            return Err(Fail::Http(400, "malformed worker kind"));
        }
        if out.iter().any(|existing| existing == kind) {
            return Err(Fail::Http(400, "duplicate worker kind"));
        }
        out.push(kind.to_string());
    }
    Ok(Some(out))
}

fn worker_claim(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let lease_ms = body_u64(&body, "lease_ms").ok_or(Fail::Http(400, "missing lease_ms"))?;
    let suffix = worker_suffix(&body)?;
    let allowed_kinds = worker_allowed_kinds(&body)?;
    let now = now_ms();
    let claimed = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        // The pre-claim gate: job_worker on every namespace this transport
        // has routed jobs under. Checked before the claim mutates anything,
        // because the core cannot un-claim without burning an attempt.
        for ns in ctx.meta_distinct_ns()? {
            require_cap(ctx, &p, Capability::JobWorker, &ns)?;
        }
        // Claim polling doubles as operation-executor liveness: record the
        // offered kinds BEFORE claiming, so an empty queue still proves a
        // live worker for `operation-types` availability. Only a principal
        // that could actually execute jobs somewhere (a job_worker grant,
        // or root) counts — otherwise any authenticated caller could forge
        // availability on a fresh server and strand operations queued.
        if let Some(kinds) = &allowed_kinds {
            let can_work = ctx.is_root(&p)?
                || ctx.core.auth().holds_anywhere(&p, Capability::JobWorker)?;
            if can_work {
                let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
                ctx.core.operations().note_worker_kinds(&kinds, now)?;
            }
        }
        let worker = worker_name(&p, &suffix);
        let claim = match &allowed_kinds {
            Some(kinds) => {
                let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
                ctx.core.jobs().claim_allowed(&worker, now, lease_ms, &kinds)?
            }
            None => ctx.core.jobs().claim(&worker, now, lease_ms)?,
        };
        match claim {
            None => Ok(None),
            Some(cj) => {
                let (ns, by, job_body) = envelope_parse(&cj.payload).ok_or(ServerError::Io {
                    op: "job envelope",
                    kind: std::io::ErrorKind::InvalidData,
                })?;
                Ok(Some((cj.job_id, cj.kind, cj.attempt, cj.lease_expires_ms, ns, by, job_body)))
            }
        }
    })?;
    let resp = match claimed {
        None => obj(vec![("job", Value::Null)]),
        Some((job_id, kind, attempt, lease_expires_ms, ns, by, job_body)) => obj(vec![
            ("job", s(super::api::job_str(&job_id))),
            ("kind", s(kind)),
            ("namespace", s(ns)),
            ("attempt", Value::Int(attempt as i64)),
            ("lease_expires_ms", Value::Int(lease_expires_ms as i64)),
            ("enqueued_by", s(principal_str(&by))),
            ("body", job_body),
        ]),
    };
    Ok(Outcome::Resp(Resp::json(200, &resp)))
}

fn worker_heartbeat(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let job = job_of_body(&body)?;
    let extend_ms = body_u64(&body, "extend_ms").ok_or(Fail::Http(400, "missing extend_ms"))?;
    let suffix = worker_suffix(&body)?;
    let progress = match body.get("progress") {
        None => None,
        Some(v) => {
            let permille = v
                .get("permille")
                .and_then(Value::as_u64)
                .ok_or(Fail::Http(400, "malformed progress"))?;
            if permille > 1000 {
                return Err(Fail::Http(400, "malformed progress"));
            }
            let note = match v.get("note") {
                None => String::new(),
                Some(n) => n
                    .as_str()
                    .ok_or(Fail::Http(400, "malformed progress"))?
                    .to_string(),
            };
            if note.len() > MAX_PROGRESS_NOTE_BYTES || note.chars().any(char::is_control) {
                return Err(Fail::Http(400, "malformed progress"));
            }
            Some((permille, note))
        }
    };
    let now = now_ms();
    let expires = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let meta = ctx
            .meta_get(&job)?
            .ok_or(ServerError::NotFound { what: "job" })?;
        require_cap(ctx, &p, Capability::JobWorker, &meta.ns)?;
        let worker = worker_name(&p, &suffix);
        let expires = ctx.core.jobs().heartbeat(&job, &worker, now, extend_ms)?;
        // Progress only records under a live lease the line above proved.
        if let Some((permille, note)) = &progress {
            ctx.progress_set(&job, *permille, note, now)?;
        }
        Ok(expires)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("lease_expires_ms", Value::Int(expires as i64))]),
    )))
}

fn worker_succeed(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let job = job_of_body(&body)?;
    let suffix = worker_suffix(&body)?;
    let result = result_bytes(&body, "result")?;
    let now = now_ms();
    call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let meta = ctx
            .meta_get(&job)?
            .ok_or(ServerError::NotFound { what: "job" })?;
        require_cap(ctx, &p, Capability::JobWorker, &meta.ns)?;
        // Operation executor jobs succeed ONLY through the finalize route,
        // where the worker's facts validate and publication is atomic; a
        // bare succeed would strand the operation without an output.
        if ctx.core.operations().operation_of_job(&job)?.is_some() {
            return Err(ServerError::InvalidState {
                what: "operation job",
                state: "requires finalize",
            });
        }
        let worker = worker_name(&p, &suffix);
        ctx.core.jobs().succeed(&job, &worker, now)?;
        if let Some(bytes) = &result {
            let attempt = ctx
                .core
                .jobs()
                .attempts(&job)?
                .last()
                .map(|a| a.attempt as u64)
                .unwrap_or(0);
            ctx.result_set(&job, "succeeded", attempt, bytes, now)?;
        }
        Ok(())
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("state", s(JobState::Succeeded.as_str()))]),
    )))
}

fn worker_fail(conn: &mut Conn, head: &mut Head, rc: &RouteCtx) -> RouteResult<Outcome> {
    let secret = secret_of(head)?;
    let body = json_body!(conn, head, rc);
    let job = job_of_body(&body)?;
    let suffix = worker_suffix(&body)?;
    let retry_delay_ms = body_u64(&body, "retry_delay_ms").unwrap_or(0);
    let error = result_bytes(&body, "error")?;
    let now = now_ms();
    let state = call_state(&rc.state, move |ctx| {
        let p = ctx.core.auth().authenticate(secret.as_bytes(), now)?;
        let meta = ctx
            .meta_get(&job)?
            .ok_or(ServerError::NotFound { what: "job" })?;
        require_cap(ctx, &p, Capability::JobWorker, &meta.ns)?;
        let worker = worker_name(&p, &suffix);
        let state = ctx.core.jobs().fail(&job, &worker, now, retry_delay_ms)?;
        // The error document is only the job's outcome once the failure is
        // terminal; a retrying job records nothing.
        if state == JobState::Failed {
            if let Some(bytes) = &error {
                let attempt = ctx
                    .core
                    .jobs()
                    .attempts(&job)?
                    .last()
                    .map(|a| a.attempt as u64)
                    .unwrap_or(0);
                ctx.result_set(&job, "failed", attempt, bytes, now)?;
            }
        }
        Ok(state)
    })?;
    Ok(Outcome::Resp(Resp::json(
        200,
        &obj(vec![("state", s(state.as_str()))]),
    )))
}
