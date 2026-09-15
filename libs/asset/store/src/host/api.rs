//! Shared route vocabulary: refusal mapping, transport token shapes, ID
//! codecs, enum string maps, and read-only JSON projections of canonical
//! manifests. Canonical bytes remain the source of truth; projections exist
//! for browsing only.

use super::http::Resp;
use super::json::{self, obj, s, Value};
use super::util::{from_hex_exact, to_hex};
use crate::variants::JobId;
use crate::{Capability, PrincipalId, ServerError};
use makepad_asset_data::{
    AssetFile, AssetKind, AssetManifest, DeviceTier, FileRole, GameRevisionManifest,
};

/// Uniform route refusal. `Srv` maps core errors; `Http` is a transport-level
/// refusal; `StateDown` is the poisoned-state-thread 503.
pub enum Fail {
    Http(u16, &'static str),
    Srv(ServerError),
    StateDown,
}

impl From<ServerError> for Fail {
    fn from(e: ServerError) -> Fail {
        Fail::Srv(e)
    }
}

pub type RouteResult<T> = Result<T, Fail>;

/// Map a refusal to its response. Io/Db/schema failures surface as an opaque
/// 500 (detail goes to stderr at the call site); everything else carries the
/// static-string detail, which by construction never contains caller bytes.
pub fn fail_resp(f: &Fail) -> Resp {
    match f {
        Fail::Http(status, msg) => Resp::error(*status, msg),
        Fail::StateDown => Resp::error(503, "state unavailable"),
        Fail::Srv(e) => match e {
            ServerError::Content(_) => {
                Resp::json(422, &obj(vec![
                    ("error", s("content contract violation")),
                    ("detail", s(format!("{e}"))),
                ]))
            }
            ServerError::DigestMismatch { .. } | ServerError::SizeMismatch { .. } => {
                Resp::json(422, &obj(vec![
                    ("error", s("integrity mismatch")),
                    ("detail", s(format!("{e}"))),
                ]))
            }
            ServerError::OverBudget { .. } => Resp::json(413, &obj(vec![
                ("error", s("over budget")),
                ("detail", s(format!("{e}"))),
            ])),
            ServerError::NotFound { .. } => Resp::error(404, "not found"),
            ServerError::Conflict { .. }
            | ServerError::InvalidState { .. }
            | ServerError::LeaseLost { .. } => Resp::json(409, &obj(vec![
                ("error", s("conflict")),
                ("detail", s(format!("{e}"))),
            ])),
            ServerError::InvalidInput { .. } => Resp::json(400, &obj(vec![
                ("error", s("invalid input")),
                ("detail", s(format!("{e}"))),
            ])),
            ServerError::Unauthenticated => unauthenticated_resp(),
            ServerError::Denied { capability } => Resp::json(403, &obj(vec![
                ("error", s("denied")),
                ("capability", s(*capability)),
            ])),
            ServerError::Io { .. }
            | ServerError::Db { .. }
            | ServerError::UnsupportedSchema { .. }
            | ServerError::UnsupportedContentSchema { .. } => {
                Resp::error(500, "internal")
            }
        },
    }
}

/// The single 401 shape. Identical bytes for missing, malformed, unknown,
/// expired, revoked and disabled credentials: no oracle.
pub fn unauthenticated_resp() -> Resp {
    Resp::error(401, "unauthenticated")
        .with_header("WWW-Authenticate", "Bearer".to_string())
}

/// True when a response status/refusal should also close the connection.
pub fn fail_closes(f: &Fail) -> bool {
    matches!(f, Fail::Http(408 | 413 | 431 | 500 | 501 | 505, _))
}

// ---- transport token + principal shapes -----------------------------------

pub const TOKEN_PREFIX: &str = "mpat_";
pub const PRINCIPAL_PREFIX: &str = "prin_";
pub const JOB_PREFIX: &str = "job_";
pub const PIPELINE_PREFIX: &str = "pipe_";
pub const OPERATION_PREFIX: &str = "op_";

/// Extract the opaque bearer secret from an Authorization header. Only the
/// exact `Bearer mpat_<64 lowercase hex>` shape ever reaches the auth store;
/// everything else is (uniformly) unauthenticated.
pub fn bearer_secret(authorization: Option<&str>) -> Option<String> {
    let h = authorization?;
    let token = h.strip_prefix("Bearer ")?;
    let hex = token.strip_prefix(TOKEN_PREFIX)?;
    from_hex_exact::<32>(hex)?;
    Some(token.to_string())
}

pub fn principal_str(p: &PrincipalId) -> String {
    format!("{}{}", PRINCIPAL_PREFIX, to_hex(&p.0))
}

pub fn parse_principal(s: &str) -> Option<PrincipalId> {
    let hex = s.strip_prefix(PRINCIPAL_PREFIX)?;
    Some(PrincipalId(from_hex_exact::<16>(hex)?))
}

pub fn job_str(j: &JobId) -> String {
    format!("{}{}", JOB_PREFIX, to_hex(&j.0))
}

pub fn parse_job(s: &str) -> Option<JobId> {
    let hex = s.strip_prefix(JOB_PREFIX)?;
    Some(JobId(from_hex_exact::<16>(hex)?))
}

// ---- capability names ------------------------------------------------------

pub const ALL_CAPS: [Capability; 16] = [
    Capability::BlobWrite,
    Capability::AssetRegister,
    Capability::AssetPublish,
    Capability::AssetQuarantine,
    Capability::AliasWrite,
    Capability::GameRegister,
    Capability::GamePublish,
    Capability::JobEnqueue,
    Capability::JobWorker,
    Capability::JobCancel,
    Capability::AuthAdmin,
    Capability::ImportSource,
    Capability::ImportRun,
    Capability::DeriveRequest,
    Capability::OperationRun,
    Capability::Chat,
];

pub fn parse_capability(name: &str) -> Option<Capability> {
    ALL_CAPS.iter().copied().find(|c| c.as_str() == name)
}

// ---- content enum string maps ----------------------------------------------

pub fn kind_str(k: AssetKind) -> &'static str {
    match k {
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

pub fn role_str(r: FileRole) -> &'static str {
    match r {
        FileRole::RenderGlb => "render_glb",
        FileRole::Lod1Glb => "lod1_glb",
        FileRole::Lod2Glb => "lod2_glb",
        FileRole::Collider => "collider",
        FileRole::AoMesh => "ao_mesh",
        FileRole::ShadowSdf => "shadow_sdf",
        FileRole::Albedo => "albedo",
        FileRole::Normal => "normal",
        FileRole::Orm => "orm",
        FileRole::Texture => "texture",
        FileRole::PreviewFront => "preview_front",
        FileRole::PreviewSide => "preview_side",
        FileRole::Turntable => "turntable",
        FileRole::Audio => "audio",
        FileRole::Video => "video",
        FileRole::Source => "source",
        FileRole::Depth => "depth",
        FileRole::Splat => "splat",
        FileRole::AoTexture => "ao_texture",
        FileRole::StemDrums => "stem_drums",
        FileRole::StemBass => "stem_bass",
        FileRole::StemVocals => "stem_vocals",
        FileRole::StemOther => "stem_other",
        FileRole::Lyrics => "lyrics",
        FileRole::DjAnalysis => "dj_analysis",
        FileRole::DjLoopSplat => "dj_loop_splat",
    }
}

pub const ALL_ROLES: [FileRole; 26] = [
    FileRole::RenderGlb,
    FileRole::Lod1Glb,
    FileRole::Lod2Glb,
    FileRole::Collider,
    FileRole::AoMesh,
    FileRole::ShadowSdf,
    FileRole::Albedo,
    FileRole::Normal,
    FileRole::Orm,
    FileRole::Texture,
    FileRole::PreviewFront,
    FileRole::PreviewSide,
    FileRole::Turntable,
    FileRole::Audio,
    FileRole::Video,
    FileRole::Source,
    FileRole::Depth,
    FileRole::Splat,
    FileRole::AoTexture,
    FileRole::StemDrums,
    FileRole::StemBass,
    FileRole::StemVocals,
    FileRole::StemOther,
    FileRole::Lyrics,
    FileRole::DjAnalysis,
    FileRole::DjLoopSplat,
];

pub fn parse_role(name: &str) -> Option<FileRole> {
    ALL_ROLES.iter().copied().find(|r| role_str(*r) == name)
}

pub fn tier_str(t: DeviceTier) -> &'static str {
    match t {
        DeviceTier::Any => "any",
        DeviceTier::Low => "low",
        DeviceTier::Medium => "medium",
        DeviceTier::High => "high",
    }
}

pub fn parse_tier(name: &str) -> Option<DeviceTier> {
    match name {
        "any" => Some(DeviceTier::Any),
        "low" => Some(DeviceTier::Low),
        "medium" => Some(DeviceTier::Medium),
        "high" => Some(DeviceTier::High),
        _ => None,
    }
}

pub fn media_str(m: makepad_asset_data::MediaType) -> &'static str {
    use makepad_asset_data::MediaType as M;
    match m {
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

pub fn parse_media(name: &str) -> Option<makepad_asset_data::MediaType> {
    use makepad_asset_data::MediaType as M;
    [
        M::Png,
        M::Jpeg,
        M::Glb,
        M::Wav,
        M::Ogg,
        M::Mp4,
        M::Bin,
        M::Text,
        M::Ply,
        M::Mp3,
        M::Json,
    ]
    .into_iter()
    .find(|m| media_str(*m) == name)
}

/// HTTP Content-Type for a media type. `Text` is validated UTF-8 at
/// admission, everything binary is exact-typed; nosniff rides on every
/// response regardless.
pub fn media_content_type(m: makepad_asset_data::MediaType) -> &'static str {
    use makepad_asset_data::MediaType as M;
    match m {
        M::Png => "image/png",
        M::Jpeg => "image/jpeg",
        M::Glb => "model/gltf-binary",
        M::Wav => "audio/wav",
        M::Ogg => "audio/ogg",
        M::Mp4 => "video/mp4",
        M::Bin => "application/octet-stream",
        M::Text => "text/plain; charset=utf-8",
        M::Ply => "application/x-ply",
        M::Mp3 => "audio/mpeg",
        M::Json => "application/json",
    }
}

pub fn thumb_content_type(m: makepad_asset_data::ThumbnailMedia) -> &'static str {
    match m {
        makepad_asset_data::ThumbnailMedia::Png => "image/png",
        makepad_asset_data::ThumbnailMedia::Jpeg => "image/jpeg",
    }
}

// ---- read-only projections -------------------------------------------------

fn file_value(f: &AssetFile) -> Value {
    let mut pairs = vec![
        ("role", s(role_str(f.role))),
        ("tier", s(tier_str(f.tier))),
        ("lod", Value::Int(f.lod as i64)),
        ("media", s(media_str(f.media))),
        ("blob", s(f.blob.to_string())),
        ("byte_len", Value::Int(f.byte_len as i64)),
    ];
    if let Some(d) = &f.dims {
        pairs.push(("dims", obj(vec![
            ("width", Value::Int(d.width as i64)),
            ("height", Value::Int(d.height as i64)),
        ])));
    }
    obj(pairs)
}

/// Browsing projection of an asset manifest. Geometry detail
/// (coordinate system, spawn recipes, anchor transforms) stays canonical-only;
/// consumers that need it fetch the manifest bytes.
pub fn asset_manifest_value(m: &AssetManifest) -> Value {
    let mut pairs = vec![
        ("asset_id", s(m.asset_id.to_string())),
        ("kind", s(kind_str(m.kind))),
        ("files", Value::Arr(m.files.iter().map(file_value).collect())),
        (
            "dependencies",
            Value::Arr(
                m.dependencies
                    .iter()
                    .map(|d| {
                        obj(vec![
                            ("asset_id", s(d.asset_id.to_string())),
                            ("revision", s(d.revision.to_string())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("metrics", obj(vec![
            ("total_bytes", Value::Int(m.metrics.total_bytes as i64)),
            ("triangles", Value::Int(m.metrics.triangles as i64)),
            ("vertices", Value::Int(m.metrics.vertices as i64)),
            ("joints", Value::Int(m.metrics.joints as i64)),
            ("clips", Value::Int(m.metrics.clips as i64)),
            ("max_texture_dim", Value::Int(m.metrics.max_texture_dim as i64)),
            ("media_millis", Value::Int(m.metrics.media_millis as i64)),
        ])),
        ("capabilities", obj(vec![
            ("rigged", Value::Bool(m.capabilities.rigged)),
            ("animated", Value::Bool(m.capabilities.animated)),
            ("collidable", Value::Bool(m.capabilities.collidable)),
            ("loopable", Value::Bool(m.capabilities.loopable)),
            ("spawnable", Value::Bool(m.capabilities.spawnable)),
        ])),
        ("bounds", obj(vec![
            ("min", vec3_value(m.bounds.min)),
            ("max", vec3_value(m.bounds.max)),
        ])),
        (
            "anchors",
            Value::Arr(m.anchors.iter().map(|a| s(a.name.clone())).collect()),
        ),
        ("rights", rights_value(&m.rights)),
    ];
    if let Some(t) = &m.thumbnail {
        pairs.push(("thumbnail", obj(vec![
            ("blob", s(t.blob.to_string())),
            ("media", s(match t.media {
                makepad_asset_data::ThumbnailMedia::Png => "png",
                makepad_asset_data::ThumbnailMedia::Jpeg => "jpeg",
            })),
            ("width", Value::Int(t.width as i64)),
            ("height", Value::Int(t.height as i64)),
            ("byte_len", Value::Int(t.byte_len as i64)),
        ])));
    }
    if let Some(p) = &m.provenance {
        pairs.push(("provenance", obj(vec![
            ("generator", s(p.generator.clone())),
            ("model", s(p.model.clone())),
            ("version", s(p.version.clone())),
            // u64 seed as a decimal string: the strict parser is i64-only.
            ("seed", s(format!("{}", p.seed))),
            (
                "parents",
                Value::Arr(p.parents.iter().map(|r| s(r.to_string())).collect()),
            ),
            ("params_digest", match &p.params_digest {
                Some(d) => s(to_hex(d)),
                None => Value::Null,
            }),
        ])));
    }
    obj(pairs)
}

/// Complete rights-record projection: exact license identity, pinned terms,
/// attribution, upstream provenance, and the source-determined policies.
pub fn rights_value(r: &makepad_asset_data::Rights) -> Value {
    use makepad_asset_data::{DerivativePolicy, Redistribution};
    obj(vec![
        ("license", s(r.license.clone())),
        ("license_revision", s(r.license_revision.clone())),
        ("terms_digest", match &r.terms_digest {
            Some(d) => s(to_hex(d)),
            None => Value::Null,
        }),
        ("terms_url", s(r.terms_url.clone())),
        ("credits", s(r.credits.clone())),
        ("source", s(r.source.clone())),
        ("source_archive", match &r.source_archive {
            Some(d) => s(to_hex(d)),
            None => Value::Null,
        }),
        ("redistribution", s(match r.redistribution {
            Redistribution::Allowed => "allowed",
            Redistribution::AttributionRequired => "attribution_required",
            Redistribution::Forbidden => "forbidden",
            Redistribution::LanLocal => "lan_local",
        })),
        ("derivatives", s(match r.derivatives {
            DerivativePolicy::Allowed => "allowed",
            DerivativePolicy::AttributionRequired => "attribution_required",
            DerivativePolicy::Forbidden => "forbidden",
            DerivativePolicy::LocalPreview => "local_preview",
        })),
    ])
}

fn vec3_value(v: makepad_asset_data::Vec3) -> Value {
    Value::Arr(vec![Value::F64(v.x as f64), Value::F64(v.y as f64), Value::F64(v.z as f64)])
}

pub fn game_manifest_value(m: &GameRevisionManifest) -> Value {
    obj(vec![
        ("game_id", s(m.game_id.to_string())),
        ("name", s(m.name.clone())),
        ("description", s(m.description.clone())),
        ("author", s(m.author.clone())),
        ("splash_blob", s(m.splash_blob.to_string())),
        ("manifest_blob", s(m.manifest_blob.to_string())),
        ("lock_blob", s(m.lock_blob.to_string())),
        ("thumbnail", obj(vec![
            ("blob", s(m.thumbnail.blob.to_string())),
            ("media", s(match m.thumbnail.media {
                makepad_asset_data::ThumbnailMedia::Png => "png",
                makepad_asset_data::ThumbnailMedia::Jpeg => "jpeg",
            })),
            ("width", Value::Int(m.thumbnail.width as i64)),
            ("height", Value::Int(m.thumbnail.height as i64)),
            ("byte_len", Value::Int(m.thumbnail.byte_len as i64)),
        ])),
        ("catalog_snapshot", match &m.catalog_snapshot {
            Some(b) => s(b.to_string()),
            None => Value::Null,
        }),
        ("search_algorithm_version", Value::Int(m.search_algorithm_version as i64)),
        ("engine_version", Value::Int(m.engine_version as i64)),
        ("protocol_version", Value::Int(m.protocol_version as i64)),
        ("splash_byte_len", Value::Int(m.splash_byte_len as i64)),
    ])
}

// ---- request body helpers --------------------------------------------------

/// Parse a JSON body object, mapping parse failures to a 400.
pub fn parse_json_body(bytes: &[u8]) -> RouteResult<Value> {
    let v = json::parse(bytes).map_err(|m| Fail::Http(400, m))?;
    if !matches!(v, Value::Obj(_)) {
        return Err(Fail::Http(400, "json object required"));
    }
    Ok(v)
}

pub fn body_str<'a>(v: &'a Value, key: &'static str) -> RouteResult<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or(Fail::Http(400, "missing string field"))
}

pub fn body_u64(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

/// Bounded `limit` query param: default when absent, refuse garbage, clamp.
pub fn parse_limit(raw: Option<&str>, default: u64, max: u64) -> RouteResult<u64> {
    match raw {
        None => Ok(default),
        Some(t) => {
            if t.is_empty() || t.len() > 6 || !t.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Fail::Http(400, "malformed limit"));
            }
            let n: u64 = t.parse().map_err(|_| Fail::Http(400, "malformed limit"))?;
            if n == 0 {
                return Err(Fail::Http(400, "malformed limit"));
            }
            Ok(n.min(max))
        }
    }
}
