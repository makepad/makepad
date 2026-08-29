//! Typed asset operations: the durable, owner-scoped control plane that lets
//! an authenticated caller (a UI, or an LLM behind the content-chat gateway)
//! request one *registered* derivation — starting with `mesh.from_image.v1` —
//! without ever touching raw job enqueue, publish, or alias routes.
//!
//! Laws enforced here, all fail-closed:
//! - Operations are selected from the versioned in-code REGISTRY. An unknown
//!   kind (including an unknown version suffix) is a structured refusal,
//!   never a passthrough to the raw job queue.
//! - The authenticated transport principal is the operation OWNER. It arrives
//!   as a typed [`PrincipalId`], never as caller JSON; get/events/cancel/
//!   retry are owner-scoped and cross-owner access reads as NotFound.
//! - Inputs are exact `{asset, revision, role, tier?, lod?, media?}`
//!   selectors resolved and PINNED to one manifest file before any job is
//!   armed; the worker and model can never choose another file. Derivative
//!   rights are validated before enqueue.
//! - Creation is idempotent on `{owner, namespace, idempotency_key}` plus the
//!   canonical spec digest: an exact replay joins the existing operation and
//!   a mismatched replay conflicts.
//! - Execution reuses the existing durable [`Jobs`] system — deterministic
//!   per-round job ids (the variants pattern), so crash-repair re-offers the
//!   SAME job instead of minting strays. No second scheduler exists.
//! - Finalization is ONE transaction: validated worker facts, the immutable
//!   revision (server-built manifest with exact parent lineage, inherited
//!   rights, real model facts and the spec digest), publication, optional
//!   alias compare-and-set, the recorded outputs, the succeeded state, and
//!   the job success land together or not at all.
//! - Availability is truthful: a registered operation with no live worker
//!   for its executor kind reports (and refuses creation as) unavailable.

use crate::auth::PrincipalId;
use crate::budget::Budgets;
use crate::catalog::{fixed16, fixed32, validate_namespace, CandidateState, Catalog};
use crate::error::{ServerError, ServerResult};
use crate::jobs::{JobId, JobState, Jobs};
use crate::sqlite::Db;
use makepad_asset_data::limits::MAX_NAME_BYTES;
use makepad_asset_data::sha256::Sha256;
use makepad_asset_data::{
    Anchor, AssetAlias, AssetFile, AssetId, AssetKind, AssetManifest, AssetRevisionId, Axis,
    Bounds, Capabilities, CoordinateSystem, DerivativePolicy, DeviceTier, FileRole, MediaType,
    Metrics, Pivot, Provenance, ThumbnailMeta, Vec3,
};

pub const OPERATIONS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS operations(
    operation_id BLOB PRIMARY KEY,
    owner BLOB NOT NULL,
    namespace TEXT NOT NULL,
    kind TEXT NOT NULL,
    def_revision INTEGER NOT NULL,
    idempotency_key TEXT NOT NULL,
    spec_digest BLOB NOT NULL,
    spec BLOB NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','succeeded','failed','cancelled')),
    round INTEGER NOT NULL DEFAULT 0,
    job_id BLOB NOT NULL,
    error TEXT,
    result_asset BLOB,
    result_revision BLOB,
    created_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS operations_idem
    ON operations(owner, namespace, idempotency_key);
CREATE INDEX IF NOT EXISTS operations_by_job ON operations(job_id);
CREATE TABLE IF NOT EXISTS operation_events(
    operation_id BLOB NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    detail TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY(operation_id, seq)
);
CREATE TABLE IF NOT EXISTS operation_worker_seen(
    kind TEXT PRIMARY KEY,
    last_seen_ms INTEGER NOT NULL
);
";

// ---------------------------------------------------------------------------
// identity
// ---------------------------------------------------------------------------

/// Operation identity, minted by the transport (which owns randomness).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId(pub [u8; 16]);

/// Deterministic job identity for one operation round, so retries and
/// crash-repair re-arm the SAME job instead of minting strays.
fn operation_job_id(op: &OperationId, round: u32) -> JobId {
    let mut h = Sha256::new();
    h.update(b"mp-operation-job:v1\0");
    h.update(&op.0);
    h.update(&round.to_be_bytes());
    let digest = h.finalize();
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    JobId(id)
}

/// Deterministic output asset identity: replaying a finalize (or finishing a
/// later round) lands on the SAME asset, accreting revisions instead of
/// scattering new asset ids.
fn operation_asset_id(op: &OperationId, output: &str) -> AssetId {
    let mut h = Sha256::new();
    h.update(b"mp-operation-asset:v1\0");
    h.update(&op.0);
    h.update(output.as_bytes());
    let digest = h.finalize();
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    AssetId::from_bytes(id)
}

// ---------------------------------------------------------------------------
// enum vocabularies (server-side canonical names)
// ---------------------------------------------------------------------------

pub fn file_role_name(role: FileRole) -> &'static str {
    match role {
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
    }
}

pub fn file_role_parse(s: &str) -> Option<FileRole> {
    ALL_ROLES.iter().copied().find(|r| file_role_name(*r) == s)
}

const ALL_ROLES: &[FileRole] = &[
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
];

pub fn media_type_name(media: MediaType) -> &'static str {
    match media {
        MediaType::Png => "png",
        MediaType::Jpeg => "jpeg",
        MediaType::Glb => "glb",
        MediaType::Wav => "wav",
        MediaType::Ogg => "ogg",
        MediaType::Mp4 => "mp4",
        MediaType::Bin => "bin",
        MediaType::Text => "text",
        MediaType::Ply => "ply",
        MediaType::Mp3 => "mp3",
        MediaType::Json => "json",
    }
}

pub fn media_type_parse(s: &str) -> Option<MediaType> {
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
        MediaType::Mp3,
        MediaType::Json,
    ]
    .into_iter()
    .find(|m| media_type_name(*m) == s)
}

pub fn device_tier_name(tier: DeviceTier) -> &'static str {
    match tier {
        DeviceTier::Any => "any",
        DeviceTier::Low => "low",
        DeviceTier::Medium => "medium",
        DeviceTier::High => "high",
    }
}

pub fn device_tier_parse(s: &str) -> Option<DeviceTier> {
    [DeviceTier::Any, DeviceTier::Low, DeviceTier::Medium, DeviceTier::High]
        .into_iter()
        .find(|t| device_tier_name(*t) == s)
}

// ---------------------------------------------------------------------------
// the versioned operation registry
// ---------------------------------------------------------------------------

/// One typed parameter of an operation. Every parameter has an explicit
/// default so a canonical spec always carries the COMPLETE resolved set —
/// `{}` and `{"seed": 0}` digest identically.
#[derive(Clone, Copy, Debug)]
pub enum ParamSpecKind {
    Int { min: i64, max: i64, default: i64 },
    Text { max_bytes: usize, default: &'static str },
    Bool { default: bool },
}

#[derive(Clone, Copy, Debug)]
pub struct OperationParamSpec {
    pub name: &'static str,
    pub kind: ParamSpecKind,
}

/// One typed input slot: what asset kinds, file roles, and media it accepts.
#[derive(Clone, Copy, Debug)]
pub struct OperationInputSlot {
    pub name: &'static str,
    pub min_count: u32,
    pub max_count: u32,
    pub accepted_kinds: &'static [AssetKind],
    pub accepted_roles: &'static [FileRole],
    pub accepted_media: &'static [MediaType],
}

/// One typed output: the asset kind the server will publish and the file
/// roles the worker must (and may) report for it.
#[derive(Clone, Copy, Debug)]
pub struct OperationOutputSpec {
    pub name: &'static str,
    pub kind: AssetKind,
    pub required_roles: &'static [FileRole],
    pub allowed_roles: &'static [FileRole],
}

/// One immutable, versioned operation definition. The registry is code: only
/// operations with a real implementation and a tested finalizer are listed,
/// and the version is part of the kind string (`mesh.from_image.v1`).
#[derive(Clone, Copy, Debug)]
pub struct OperationDef {
    pub kind: &'static str,
    /// Definition revision, folded into every canonical spec digest.
    pub revision: u32,
    /// Exact worker job kind armed for this operation. Distinct from the
    /// operation kind so profile-based legacy workers can never claim it.
    pub executor_kind: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub inputs: &'static [OperationInputSlot],
    pub params: &'static [OperationParamSpec],
    pub outputs: &'static [OperationOutputSpec],
    pub supports_seed: bool,
}

pub const MESH_FROM_IMAGE_V1: &str = "mesh.from_image.v1";
pub const DEPTH_FROM_IMAGE_V1: &str = "depth.from_image.v1";

static REGISTRY: &[OperationDef] = &[
    OperationDef {
        kind: MESH_FROM_IMAGE_V1,
        revision: 1,
        executor_kind: "op.mesh.from_image.v1",
        label: "Image to 3D mesh",
        description: "Turn one exact selected image revision into a textured 3D mesh asset.",
        inputs: &[OperationInputSlot {
            name: "image",
            min_count: 1,
            max_count: 1,
            accepted_kinds: &[AssetKind::Texture],
            accepted_roles: &[FileRole::Texture, FileRole::Albedo],
            accepted_media: &[MediaType::Png, MediaType::Jpeg],
        }],
        params: &[
            OperationParamSpec {
                name: "seed",
                kind: ParamSpecKind::Int { min: 0, max: u32::MAX as i64, default: 0 },
            },
            OperationParamSpec {
                name: "title",
                kind: ParamSpecKind::Text { max_bytes: 120, default: "" },
            },
            // The physical size the expand step declared, metres as text
            // (params are flat typed values): the worker calibrates the
            // unitless generated mesh to it and publishes an
            // `asset-dimensions` sidecar (`FileRole::Source`). Empty =
            // undeclared, no sidecar.
            OperationParamSpec {
                name: "dim_class",
                kind: ParamSpecKind::Text { max_bytes: 16, default: "" },
            },
            OperationParamSpec {
                name: "dim_height",
                kind: ParamSpecKind::Text { max_bytes: 16, default: "" },
            },
            OperationParamSpec {
                name: "dim_length",
                kind: ParamSpecKind::Text { max_bytes: 16, default: "" },
            },
            OperationParamSpec {
                name: "dim_width",
                kind: ParamSpecKind::Text { max_bytes: 16, default: "" },
            },
            OperationParamSpec {
                name: "dim_preset",
                kind: ParamSpecKind::Text { max_bytes: 16, default: "" },
            },
        ],
        outputs: &[OperationOutputSpec {
            name: "mesh",
            kind: AssetKind::Mesh,
            required_roles: &[FileRole::RenderGlb],
            allowed_roles: &[
                FileRole::RenderGlb,
                FileRole::Albedo,
                FileRole::Normal,
                FileRole::Orm,
                FileRole::Collider,
                // The `asset-dimensions` text sidecar.
                FileRole::Source,
            ],
        }],
        supports_seed: true,
    },
    OperationDef {
        kind: DEPTH_FROM_IMAGE_V1,
        revision: 1,
        executor_kind: "op.depth.from_image.v1",
        label: "Image to metric depth",
        description: "Turn one exact selected image revision into a 16-bit millimeter depth map plus JSON sidecar.",
        inputs: &[OperationInputSlot {
            name: "image",
            min_count: 1,
            max_count: 1,
            accepted_kinds: &[AssetKind::Texture],
            accepted_roles: &[FileRole::Texture, FileRole::Albedo],
            // PNG only until a JPEG->PNG transcode step exists: every depth
            // executor hard-requires PNG, so advertising Jpeg armed jobs
            // that were guaranteed to fail at the worker.
            accepted_media: &[MediaType::Png],
        }],
        params: &[OperationParamSpec {
            name: "title",
            kind: ParamSpecKind::Text { max_bytes: 120, default: "" },
        }],
        outputs: &[OperationOutputSpec {
            name: "depth",
            kind: AssetKind::Texture,
            required_roles: &[FileRole::Depth],
            allowed_roles: &[FileRole::Depth, FileRole::Source],
        }],
        supports_seed: false,
    },
];

pub fn registry() -> &'static [OperationDef] {
    REGISTRY
}

pub fn operation_def(kind: &str) -> Option<&'static OperationDef> {
    REGISTRY.iter().find(|d| d.kind == kind)
}

// ---------------------------------------------------------------------------
// request / spec types
// ---------------------------------------------------------------------------

/// One typed parameter value as provided by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamValue {
    Int(i64),
    Text(String),
    Bool(bool),
}

/// The caller's exact compound input selector, BEFORE resolution. `tier` and
/// `lod` disambiguate when a role has several files; `expected_media` guards
/// against a retargeted expectation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationInputBinding {
    pub slot: String,
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
    pub role: FileRole,
    pub tier: Option<DeviceTier>,
    pub lod: Option<u8>,
    pub expected_media: Option<MediaType>,
}

/// A binding resolved and pinned to ONE exact manifest file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedInput {
    pub slot: String,
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
    pub role: FileRole,
    pub tier: DeviceTier,
    pub lod: u8,
    pub media: MediaType,
    pub blob: makepad_asset_data::BlobId,
    pub byte_len: u64,
}

/// Alias expectation of a `publish_and_alias` publication: the compare half
/// of the final compare-and-set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AliasExpect {
    /// Set unconditionally (the owner chose to move the alias regardless).
    Unconditional,
    /// The alias must not exist yet.
    Absent,
    /// The alias head must currently be exactly this revision.
    Head(AssetRevisionId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationPublication {
    /// Publish the output revision; no alias moves.
    Publish,
    /// Publish, then compare-and-set the alias in the same transaction.
    PublishAndAlias { alias: AssetAlias, expect: AliasExpect },
}

pub struct OperationCreateRequest<'a> {
    /// Fresh identity minted by the transport; ignored when the idempotent
    /// replay joins an existing operation.
    pub operation_id: OperationId,
    /// The authenticated transport principal. NEVER caller JSON.
    pub owner: PrincipalId,
    pub namespace: &'a str,
    pub kind: &'a str,
    pub idempotency_key: &'a str,
    pub inputs: &'a [OperationInputBinding],
    pub params: &'a [(String, ParamValue)],
    pub publication: OperationPublication,
}

/// A job the transport must now enqueue through the jobs system (with its
/// own metadata invariants). Deterministic id; re-offered after a crash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArmedJob {
    pub job_id: JobId,
    pub kind: &'static str,
    pub round: u32,
}

#[derive(Clone, Debug)]
pub enum OperationCreateOutcome {
    /// A fresh operation was recorded; enqueue exactly this job.
    Created { snapshot: OperationSnapshot, job: ArmedJob },
    /// The idempotent replay joined an existing operation. `rearm` is Some
    /// when the recorded job vanished before enqueue (crash window) — the
    /// transport must enqueue it again under the SAME id.
    Joined { snapshot: OperationSnapshot, rearm: Option<ArmedJob> },
}

/// Row states. `queued` covers everything before a terminal outcome; the
/// LIVE display state joins the job ("running" while the executor holds it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationState {
    Queued,
    Succeeded,
    Failed,
    Cancelled,
}

impl OperationState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Queued)
    }
}

/// Complete owner-visible state of one operation.
#[derive(Clone, Debug)]
pub struct OperationSnapshot {
    pub id: OperationId,
    pub owner: PrincipalId,
    pub namespace: String,
    pub kind: String,
    pub def_revision: u32,
    pub idempotency_key: String,
    pub spec_digest: [u8; 32],
    pub state: OperationState,
    /// Live display state: `queued`/`running`/`succeeded`/`failed`/
    /// `cancelled`, joining the executor job truthfully.
    pub display_state: &'static str,
    pub round: u32,
    pub job_id: JobId,
    pub job_state: Option<JobState>,
    pub error: Option<String>,
    pub result: Option<(AssetId, AssetRevisionId)>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub inputs: Vec<PinnedInput>,
    pub params: Vec<(String, ParamValue)>,
    pub publication: OperationPublication,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationEventRow {
    pub seq: u64,
    pub kind: String,
    pub detail: String,
    pub created_ms: u64,
}

/// Worker-reported completion facts for ONE named output. The server rebuilds
/// the canonical manifest itself; a worker can never author identity fields.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationOutputFacts {
    pub name: String,
    pub files: Vec<AssetFile>,
    pub thumbnail: Option<ThumbnailMeta>,
    pub metrics: Metrics,
    pub bounds: Option<Bounds>,
}

/// The complete finalize report: outputs plus the ACTUAL model facts of the
/// run. The seed must match the pinned spec; the parameter digest is
/// server-computed from the canonical spec, never worker-authored.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationResultFacts {
    pub outputs: Vec<OperationOutputFacts>,
    pub generator: String,
    pub model: String,
    pub version: String,
    pub seed: u64,
}

/// Truthful availability of one registered operation.
#[derive(Clone, Debug)]
pub struct OperationAvailability {
    pub def: &'static OperationDef,
    pub available: bool,
    pub reason: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// canonical spec encoding
// ---------------------------------------------------------------------------

const SPEC_MAGIC: &[u8] = b"mpopspec1\0";

struct SpecWriter {
    out: Vec<u8>,
}

impl SpecWriter {
    fn new() -> Self {
        Self { out: SPEC_MAGIC.to_vec() }
    }
    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.out.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
}

struct SpecReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

const SPEC_CORRUPT: ServerError =
    ServerError::InvalidState { what: "operation spec", state: "corrupt" };

impl<'a> SpecReader<'a> {
    fn new(buf: &'a [u8]) -> ServerResult<Self> {
        if !buf.starts_with(SPEC_MAGIC) {
            return Err(SPEC_CORRUPT);
        }
        Ok(Self { buf, pos: SPEC_MAGIC.len() })
    }
    fn take(&mut self, n: usize) -> ServerResult<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(SPEC_CORRUPT)?;
        if end > self.buf.len() {
            return Err(SPEC_CORRUPT);
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn u8(&mut self) -> ServerResult<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> ServerResult<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| SPEC_CORRUPT)?))
    }
    fn u64(&mut self) -> ServerResult<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| SPEC_CORRUPT)?))
    }
    fn i64(&mut self) -> ServerResult<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().map_err(|_| SPEC_CORRUPT)?))
    }
    fn raw16(&mut self) -> ServerResult<[u8; 16]> {
        self.take(16)?.try_into().map_err(|_| SPEC_CORRUPT)
    }
    fn raw32(&mut self) -> ServerResult<[u8; 32]> {
        self.take(32)?.try_into().map_err(|_| SPEC_CORRUPT)
    }
    fn str(&mut self) -> ServerResult<String> {
        let len = self.u32()? as usize;
        if len > 4096 {
            return Err(SPEC_CORRUPT);
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| SPEC_CORRUPT)
    }
    fn done(&self) -> ServerResult<()> {
        if self.pos == self.buf.len() {
            Ok(())
        } else {
            Err(SPEC_CORRUPT)
        }
    }
}

/// The canonical, deterministic byte encoding of one resolved operation
/// request. The idempotency digest is the SHA-256 of these bytes, so any
/// semantic difference — a different pinned file, parameter, or publication
/// policy — conflicts, while re-spellings that resolve identically join.
fn encode_spec(
    def: &OperationDef,
    namespace: &str,
    inputs: &[PinnedInput],
    params: &[(String, ParamValue)],
    publication: &OperationPublication,
) -> Vec<u8> {
    let mut w = SpecWriter::new();
    w.str(def.kind);
    w.u32(def.revision);
    w.str(namespace);
    w.u32(inputs.len() as u32);
    for p in inputs {
        w.str(&p.slot);
        w.out.extend_from_slice(p.asset_id.as_bytes());
        w.out.extend_from_slice(p.revision.as_bytes());
        w.str(file_role_name(p.role));
        w.str(device_tier_name(p.tier));
        w.u8(p.lod);
        w.str(media_type_name(p.media));
        w.out.extend_from_slice(p.blob.as_bytes());
        w.u64(p.byte_len);
    }
    w.u32(params.len() as u32);
    for (name, value) in params {
        w.str(name);
        match value {
            ParamValue::Int(v) => {
                w.u8(0);
                w.i64(*v);
            }
            ParamValue::Text(v) => {
                w.u8(1);
                w.str(v);
            }
            ParamValue::Bool(v) => {
                w.u8(2);
                w.u8(*v as u8);
            }
        }
    }
    match publication {
        OperationPublication::Publish => w.u8(0),
        OperationPublication::PublishAndAlias { alias, expect } => {
            w.u8(1);
            w.str(alias.as_str());
            match expect {
                AliasExpect::Unconditional => w.u8(0),
                AliasExpect::Absent => w.u8(1),
                AliasExpect::Head(rev) => {
                    w.u8(2);
                    w.out.extend_from_slice(rev.as_bytes());
                }
            }
        }
    }
    w.out
}

struct DecodedSpec {
    inputs: Vec<PinnedInput>,
    params: Vec<(String, ParamValue)>,
    publication: OperationPublication,
}

/// The blob digests one stored operation spec pins as inputs. Blob GC marks
/// these for QUEUED operations: the operation was armed against exactly
/// those bytes, and collecting them mid-flight would turn a scheduled job
/// into an unexplainable failure even though the store promised them.
pub(crate) fn spec_input_blobs(bytes: &[u8]) -> ServerResult<Vec<makepad_asset_data::BlobId>> {
    Ok(decode_spec(bytes)?.inputs.into_iter().map(|i| i.blob).collect())
}

fn decode_spec(bytes: &[u8]) -> ServerResult<DecodedSpec> {
    use std::str::FromStr;
    let mut r = SpecReader::new(bytes)?;
    let _kind = r.str()?;
    let _revision = r.u32()?;
    let _namespace = r.str()?;
    let input_count = r.u32()?;
    if input_count > 1024 {
        return Err(SPEC_CORRUPT);
    }
    let mut inputs = Vec::with_capacity(input_count as usize);
    for _ in 0..input_count {
        let slot = r.str()?;
        let asset_id = AssetId::from_bytes(r.raw16()?);
        let revision = AssetRevisionId::from_bytes(r.raw32()?);
        let role = file_role_parse(&r.str()?).ok_or(SPEC_CORRUPT)?;
        let tier = device_tier_parse(&r.str()?).ok_or(SPEC_CORRUPT)?;
        let lod = r.u8()?;
        let media = media_type_parse(&r.str()?).ok_or(SPEC_CORRUPT)?;
        let blob = makepad_asset_data::BlobId::from_bytes(r.raw32()?);
        let byte_len = r.u64()?;
        inputs.push(PinnedInput {
            slot,
            asset_id,
            revision,
            role,
            tier,
            lod,
            media,
            blob,
            byte_len,
        });
    }
    let param_count = r.u32()?;
    if param_count > 1024 {
        return Err(SPEC_CORRUPT);
    }
    let mut params = Vec::with_capacity(param_count as usize);
    for _ in 0..param_count {
        let name = r.str()?;
        let value = match r.u8()? {
            0 => ParamValue::Int(r.i64()?),
            1 => ParamValue::Text(r.str()?),
            2 => ParamValue::Bool(r.u8()? != 0),
            _ => return Err(SPEC_CORRUPT),
        };
        params.push((name, value));
    }
    let publication = match r.u8()? {
        0 => OperationPublication::Publish,
        1 => {
            let alias = AssetAlias::from_str(&r.str()?).map_err(|_| SPEC_CORRUPT)?;
            let expect = match r.u8()? {
                0 => AliasExpect::Unconditional,
                1 => AliasExpect::Absent,
                2 => AliasExpect::Head(AssetRevisionId::from_bytes(r.raw32()?)),
                _ => return Err(SPEC_CORRUPT),
            };
            OperationPublication::PublishAndAlias { alias, expect }
        }
        _ => return Err(SPEC_CORRUPT),
    };
    r.done()?;
    Ok(DecodedSpec { inputs, params, publication })
}

// ---------------------------------------------------------------------------
// the store
// ---------------------------------------------------------------------------

pub struct Operations<'a> {
    pub(crate) db: &'a Db,
    pub(crate) budgets: &'a Budgets,
}

struct OperationRow {
    id: OperationId,
    owner: PrincipalId,
    namespace: String,
    kind: String,
    def_revision: u32,
    idempotency_key: String,
    spec_digest: [u8; 32],
    spec: Vec<u8>,
    state: OperationState,
    round: u32,
    job_id: JobId,
    error: Option<String>,
    result: Option<(AssetId, AssetRevisionId)>,
    created_ms: u64,
    updated_ms: u64,
}

impl<'a> Operations<'a> {
    fn catalog(&self) -> Catalog<'a> {
        Catalog { db: self.db, budgets: self.budgets }
    }
    fn jobs(&self) -> Jobs<'a> {
        Jobs { db: self.db, budgets: self.budgets }
    }

    // ---- worker liveness ---------------------------------------------------

    /// Record that a live worker just offered to execute these job kinds.
    /// Only registered executor kinds are recorded; the transport calls this
    /// from the worker claim route, so claim polling doubles as liveness.
    pub fn note_worker_kinds(&self, kinds: &[&str], now_ms: u64) -> ServerResult<()> {
        for kind in kinds {
            if !REGISTRY.iter().any(|d| d.executor_kind == *kind) {
                continue;
            }
            let mut s = self.db.prepare(
                "note worker kind",
                "INSERT INTO operation_worker_seen(kind, last_seen_ms) VALUES(?1, ?2)
                 ON CONFLICT(kind) DO UPDATE SET last_seen_ms=?2",
            )?;
            s.bind_text(1, kind)?;
            s.bind_u64(2, now_ms)?;
            s.run()?;
        }
        Ok(())
    }

    fn worker_alive(&self, executor_kind: &str, now_ms: u64) -> ServerResult<bool> {
        let mut s = self.db.prepare(
            "worker seen",
            "SELECT last_seen_ms FROM operation_worker_seen WHERE kind = ?1",
        )?;
        s.bind_text(1, executor_kind)?;
        if !s.step()? {
            return Ok(false);
        }
        let seen = s.column_u64(0);
        Ok(seen.saturating_add(self.budgets.operation_worker_liveness_ms) > now_ms)
    }

    /// Truthful availability of every registered operation.
    pub fn capabilities(&self, now_ms: u64) -> ServerResult<Vec<OperationAvailability>> {
        let mut out = Vec::with_capacity(REGISTRY.len());
        for def in REGISTRY {
            let alive = self.worker_alive(def.executor_kind, now_ms)?;
            out.push(OperationAvailability {
                def,
                available: alive,
                reason: if alive { None } else { Some("no live worker for this operation") },
            });
        }
        Ok(out)
    }

    // ---- creation ----------------------------------------------------------

    pub fn create(
        &self,
        req: &OperationCreateRequest<'_>,
        now_ms: u64,
    ) -> ServerResult<OperationCreateOutcome> {
        let def = operation_def(req.kind)
            .ok_or(ServerError::NotFound { what: "operation kind" })?;
        validate_namespace(req.namespace)?;
        validate_idempotency_key(req.idempotency_key)?;
        if req.inputs.len() as u64 > self.budgets.max_operation_inputs as u64 {
            return Err(ServerError::OverBudget {
                what: "operation inputs",
                limit: self.budgets.max_operation_inputs as u64,
                found: req.inputs.len() as u64,
            });
        }
        let params = resolve_params(def, req.params)?;
        check_slot_counts(def, req.inputs)?;
        if let OperationPublication::PublishAndAlias { alias, .. } = &req.publication {
            if alias.namespace() != req.namespace {
                return Err(ServerError::Conflict { what: "operation alias namespace" });
            }
        }

        self.db.tx(|db| {
            // Truthful availability is a creation gate, not just a listing.
            if !self.worker_alive(def.executor_kind, now_ms)? {
                return Err(ServerError::InvalidState {
                    what: "operation kind",
                    state: "unavailable",
                });
            }
            let mut pinned = Vec::with_capacity(req.inputs.len());
            for binding in req.inputs {
                pinned.push(self.resolve_input(def, binding)?);
            }
            let spec = encode_spec(def, req.namespace, &pinned, &params, &req.publication);
            if spec.len() as u64 > self.budgets.max_operation_spec_bytes as u64 {
                return Err(ServerError::OverBudget {
                    what: "operation spec bytes",
                    limit: self.budgets.max_operation_spec_bytes as u64,
                    found: spec.len() as u64,
                });
            }
            let spec_digest = makepad_asset_data::sha256(&spec);

            if let Some(existing) =
                self.row_by_idem(&req.owner, req.namespace, req.idempotency_key)?
            {
                if existing.spec_digest != spec_digest {
                    return Err(ServerError::Conflict { what: "operation idempotency key" });
                }
                let existing = self.sync_row(db, existing, now_ms)?;
                let rearm = if existing.state == OperationState::Queued
                    && self.jobs().state(&existing.job_id)?.is_none()
                {
                    Some(ArmedJob {
                        job_id: existing.job_id,
                        kind: def.executor_kind,
                        round: existing.round,
                    })
                } else {
                    None
                };
                let snapshot = self.snapshot_of(existing)?;
                return Ok(OperationCreateOutcome::Joined { snapshot, rearm });
            }

            let job_id = operation_job_id(&req.operation_id, 0);
            let mut s = db.prepare(
                "insert operation",
                "INSERT INTO operations(operation_id, owner, namespace, kind, def_revision,
                                        idempotency_key, spec_digest, spec, state, round,
                                        job_id, created_ms, updated_ms)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', 0, ?9, ?10, ?10)",
            )?;
            s.bind_blob(1, &req.operation_id.0)?;
            s.bind_blob(2, &req.owner.0)?;
            s.bind_text(3, req.namespace)?;
            s.bind_text(4, def.kind)?;
            s.bind_i64(5, def.revision as i64)?;
            s.bind_text(6, req.idempotency_key)?;
            s.bind_blob(7, &spec_digest)?;
            s.bind_blob(8, &spec)?;
            s.bind_blob(9, &job_id.0)?;
            s.bind_u64(10, now_ms)?;
            s.run()?;
            drop(s);
            self.append_event(db, &req.operation_id, "created", def.kind, now_ms)?;

            let row = self
                .row_by_id(&req.operation_id)?
                .ok_or(ServerError::NotFound { what: "operation row" })?;
            let snapshot = self.snapshot_of(row)?;
            Ok(OperationCreateOutcome::Created {
                snapshot,
                job: ArmedJob { job_id, kind: def.executor_kind, round: 0 },
            })
        })
    }

    /// Resolve one binding against the catalog and pin the exact file.
    fn resolve_input(
        &self,
        def: &OperationDef,
        binding: &OperationInputBinding,
    ) -> ServerResult<PinnedInput> {
        let slot = def
            .inputs
            .iter()
            .find(|s| s.name == binding.slot)
            .ok_or(ServerError::InvalidInput { what: "operation input slot" })?;
        if !slot.accepted_roles.contains(&binding.role) {
            return Err(ServerError::InvalidInput { what: "operation input role" });
        }
        let catalog = self.catalog();
        let manifest_bytes = catalog
            .asset_revision_manifest(&binding.revision)?
            .ok_or(ServerError::NotFound { what: "operation input revision" })?;
        let manifest = AssetManifest::from_canonical_bytes(&manifest_bytes)?;
        if manifest.asset_id != binding.asset_id {
            return Err(ServerError::Conflict { what: "operation input asset" });
        }
        match catalog.asset_candidate_state(&binding.asset_id, &binding.revision)? {
            Some(CandidateState::Published) => {}
            Some(_) => {
                return Err(ServerError::InvalidState {
                    what: "operation input",
                    state: "not published",
                })
            }
            None => return Err(ServerError::NotFound { what: "operation input candidate" }),
        }
        if !slot.accepted_kinds.contains(&manifest.kind) {
            return Err(ServerError::InvalidInput { what: "operation input kind" });
        }
        // Derivative-rights gate BEFORE any job is armed.
        if manifest.rights.derivatives == DerivativePolicy::Forbidden {
            return Err(ServerError::InvalidState {
                what: "operation input rights",
                state: "derivatives forbidden",
            });
        }
        let matches: Vec<&AssetFile> = manifest
            .files
            .iter()
            .filter(|f| {
                f.role == binding.role
                    && binding.tier.map(|t| f.tier == t).unwrap_or(true)
                    && binding.lod.map(|l| f.lod == l).unwrap_or(true)
            })
            .collect();
        let file = match matches.as_slice() {
            [] => return Err(ServerError::NotFound { what: "operation input file" }),
            [one] => **one,
            _ => return Err(ServerError::InvalidInput { what: "operation input ambiguous" }),
        };
        if !slot.accepted_media.contains(&file.media) {
            return Err(ServerError::InvalidInput { what: "operation input media" });
        }
        if let Some(expected) = binding.expected_media {
            if file.media != expected {
                return Err(ServerError::Conflict { what: "operation input media" });
            }
        }
        Ok(PinnedInput {
            slot: binding.slot.clone(),
            asset_id: binding.asset_id,
            revision: binding.revision,
            role: file.role,
            tier: file.tier,
            lod: file.lod,
            media: file.media,
            blob: file.blob,
            byte_len: file.byte_len,
        })
    }

    // ---- reads -------------------------------------------------------------

    /// Owner-scoped snapshot. Cross-owner reads are NotFound — operation ids
    /// must not leak across owners. Lazily reconciles a queued row whose
    /// executor job died terminally.
    pub fn get(
        &self,
        owner: &PrincipalId,
        id: &OperationId,
        now_ms: u64,
    ) -> ServerResult<OperationSnapshot> {
        self.db.tx(|db| {
            let row = self.owned_row(owner, id)?;
            let row = self.sync_row(db, row, now_ms)?;
            self.snapshot_of(row)
        })
    }

    /// Owner-scoped durable event page, `seq > after`, oldest first.
    pub fn events(
        &self,
        owner: &PrincipalId,
        id: &OperationId,
        after: u64,
        limit: u32,
        now_ms: u64,
    ) -> ServerResult<Vec<OperationEventRow>> {
        if limit == 0 || limit > self.budgets.max_operation_events {
            return Err(ServerError::InvalidInput { what: "operation event limit" });
        }
        self.db.tx(|db| {
            let row = self.owned_row(owner, id)?;
            let _ = self.sync_row(db, row, now_ms)?;
            let mut s = db.prepare(
                "operation events",
                "SELECT seq, kind, detail, created_ms FROM operation_events
                 WHERE operation_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
            )?;
            s.bind_blob(1, &id.0)?;
            s.bind_u64(2, after)?;
            s.bind_i64(3, limit as i64)?;
            let mut out = Vec::new();
            while s.step()? {
                out.push(OperationEventRow {
                    seq: s.column_u64(0),
                    kind: s.column_text(1),
                    detail: s.column_text(2),
                    created_ms: s.column_u64(3),
                });
            }
            Ok(out)
        })
    }

    /// Executor-facing snapshot, NOT owner-scoped: the finalize route reads
    /// it after `finalize` succeeded, under the worker capability it already
    /// proved for the operation's namespace. Never exposed on a client route.
    pub fn executor_snapshot(&self, id: &OperationId) -> ServerResult<OperationSnapshot> {
        let row = self
            .row_by_id(id)?
            .ok_or(ServerError::NotFound { what: "operation" })?;
        self.snapshot_of(row)
    }

    /// The operation an executor job belongs to, if any. The transport uses
    /// this to refuse raw `worker_succeed` on operation jobs (finalize is the
    /// only success path) and to route liveness.
    pub fn operation_of_job(&self, job: &JobId) -> ServerResult<Option<OperationId>> {
        let mut s = self.db.prepare(
            "operation of job",
            "SELECT operation_id FROM operations WHERE job_id = ?1",
        )?;
        s.bind_blob(1, &job.0)?;
        if !s.step()? {
            return Ok(None);
        }
        Ok(Some(OperationId(fixed16(&s.column_blob(0), "operation row")?)))
    }

    // ---- cancel / retry ----------------------------------------------------

    /// Owner-scoped cancel. Idempotent: returns false when already terminal.
    /// The executor job is cancelled first (dropping its lease so a stale
    /// worker's finalize refuses), then the row flips; a crash between the
    /// two self-heals through the lazy sync, which reads a cancelled job as
    /// a cancelled operation.
    pub fn cancel(
        &self,
        owner: &PrincipalId,
        id: &OperationId,
        now_ms: u64,
    ) -> ServerResult<bool> {
        let row = self
            .db
            .tx(|db| {
                let row = self.owned_row(owner, id)?;
                self.sync_row(db, row, now_ms)
            })?;
        if row.state.is_terminal() {
            return Ok(false);
        }
        if self.jobs().state(&row.job_id)?.is_some() {
            self.jobs().cancel(&row.job_id, now_ms)?;
        }
        self.db.tx(|db| {
            let row = self.owned_row(owner, id)?;
            if row.state == OperationState::Queued {
                self.set_state(db, id, OperationState::Cancelled, None, now_ms)?;
                self.append_event(db, id, "cancelled", "", now_ms)?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    /// Owner-scoped retry from a terminal failure/cancellation: arms the next
    /// round under a fresh deterministic job id. The transport must enqueue
    /// the returned job.
    pub fn retry(
        &self,
        owner: &PrincipalId,
        id: &OperationId,
        now_ms: u64,
    ) -> ServerResult<(OperationSnapshot, ArmedJob)> {
        self.db.tx(|db| {
            let row = self.owned_row(owner, id)?;
            let row = self.sync_row(db, row, now_ms)?;
            let def = operation_def(&row.kind)
                .ok_or(ServerError::NotFound { what: "operation kind" })?;
            match row.state {
                OperationState::Failed | OperationState::Cancelled => {}
                OperationState::Succeeded => {
                    return Err(ServerError::InvalidState {
                        what: "operation retry",
                        state: "succeeded",
                    })
                }
                OperationState::Queued => {
                    return Err(ServerError::InvalidState {
                        what: "operation retry",
                        state: "in flight",
                    })
                }
            }
            if !self.worker_alive(def.executor_kind, now_ms)? {
                return Err(ServerError::InvalidState {
                    what: "operation kind",
                    state: "unavailable",
                });
            }
            let round = row
                .round
                .checked_add(1)
                .ok_or(ServerError::InvalidState { what: "operation round", state: "overflow" })?;
            if round >= self.budgets.max_operation_rounds {
                return Err(ServerError::InvalidState {
                    what: "operation retry",
                    state: "rounds exhausted",
                });
            }
            let job_id = operation_job_id(id, round);
            let mut s = db.prepare(
                "retry operation",
                "UPDATE operations SET state='queued', round=?2, job_id=?3, error=NULL,
                                       updated_ms=?4
                 WHERE operation_id=?1",
            )?;
            s.bind_blob(1, &id.0)?;
            s.bind_i64(2, round as i64)?;
            s.bind_blob(3, &job_id.0)?;
            s.bind_u64(4, now_ms)?;
            s.run()?;
            drop(s);
            self.append_event(db, id, "retried", &format!("round {round}"), now_ms)?;
            let row = self
                .row_by_id(id)?
                .ok_or(ServerError::NotFound { what: "operation row" })?;
            let snapshot = self.snapshot_of(row)?;
            Ok((snapshot, ArmedJob { job_id, kind: def.executor_kind, round }))
        })
    }

    // ---- finalize ----------------------------------------------------------

    /// Validate a worker's typed completion facts and finalize the operation
    /// ATOMICALLY: immutable revision (server-built manifest with exact
    /// parents, inherited rights, actual model facts and the server-computed
    /// spec digest), publication, optional alias compare-and-set, recorded
    /// output, succeeded state, and job success — one transaction. A stale
    /// lease, a superseded round, a cancelled operation, or any lying fact
    /// refuses without publishing anything.
    pub fn finalize(
        &self,
        id: &OperationId,
        job: &JobId,
        worker: &str,
        facts: &OperationResultFacts,
        now_ms: u64,
    ) -> ServerResult<(AssetId, AssetRevisionId)> {
        let jobs = self.jobs();
        let catalog = self.catalog();
        self.db.tx(|db| {
            let row = self
                .row_by_id(id)?
                .ok_or(ServerError::NotFound { what: "operation" })?;
            let def = operation_def(&row.kind)
                .ok_or(ServerError::NotFound { what: "operation kind" })?;
            let spec = decode_spec(&row.spec)?;

            // Rebuild the manifest the server is willing to publish. This is
            // also the idempotent-replay comparison form.
            let (manifest_bytes, asset_id) =
                self.build_output_manifest(&row, def, &spec, facts)?;
            let revision = AssetRevisionId::hash_of(&manifest_bytes);

            if row.state == OperationState::Succeeded {
                // Late duplicate: identical result is an idempotent success;
                // different bytes can never replace the recorded output.
                return match row.result {
                    Some((recorded_asset, recorded_rev)) if recorded_rev == revision => {
                        Ok((recorded_asset, recorded_rev))
                    }
                    _ => Err(ServerError::Conflict { what: "late duplicate operation result" }),
                };
            }
            match row.state {
                OperationState::Queued => {}
                OperationState::Cancelled => {
                    return Err(ServerError::InvalidState {
                        what: "operation finalize",
                        state: "cancelled",
                    })
                }
                _ => {
                    return Err(ServerError::InvalidState {
                        what: "operation finalize",
                        state: row.state.as_str(),
                    })
                }
            }
            // A newer round superseded this job: its report can never land.
            if row.job_id != *job {
                return Err(ServerError::LeaseLost { what: "superseded operation job" });
            }
            // The reporting worker must hold the live lease RIGHT NOW.
            let attempt = jobs.require_live_lease(job, worker, now_ms)?;

            // Publish: register the deterministic output asset, stage the
            // manifest, publish the candidate.
            catalog.register_asset(&asset_id, &row.namespace, now_ms)?;
            let staged = catalog.stage_asset_revision_in_tx(db, &manifest_bytes, now_ms)?;
            debug_assert_eq!(staged, revision);
            catalog.transition_in_tx(
                db,
                "asset",
                asset_id.as_bytes(),
                revision.as_bytes(),
                &[CandidateState::Staged],
                CandidateState::Published,
                now_ms,
            )?;

            // Optional alias compare-and-set, INSIDE the same transaction: a
            // failed expectation rolls everything back — no published
            // revision, no succeeded operation, no alias movement.
            if let OperationPublication::PublishAndAlias { alias, expect } = &spec.publication {
                let current = catalog.resolve_asset_alias(alias)?;
                match (expect, &current) {
                    (AliasExpect::Unconditional, _) => {}
                    (AliasExpect::Absent, None) => {}
                    (AliasExpect::Absent, Some(_)) => {
                        return Err(ServerError::Conflict { what: "operation alias occupied" })
                    }
                    (AliasExpect::Head(want), Some(cur)) if cur.revision == *want => {}
                    (AliasExpect::Head(_), _) => {
                        return Err(ServerError::Conflict { what: "operation alias head" })
                    }
                }
                catalog.set_asset_alias_in_tx(
                    db,
                    alias,
                    &makepad_asset_data::AssetRevisionRef { asset_id, revision },
                    now_ms,
                )?;
            }

            let mut s = db.prepare(
                "succeed operation",
                "UPDATE operations SET state='succeeded', result_asset=?2,
                                       result_revision=?3, error=NULL, updated_ms=?4
                 WHERE operation_id=?1",
            )?;
            s.bind_blob(1, &id.0)?;
            s.bind_blob(2, asset_id.as_bytes())?;
            s.bind_blob(3, revision.as_bytes())?;
            s.bind_u64(4, now_ms)?;
            s.run()?;
            drop(s);
            self.append_event(db, id, "succeeded", &revision.to_string(), now_ms)?;
            jobs.finish(db, job, attempt, "succeeded", "succeeded", now_ms)?;
            Ok((asset_id, revision))
        })
    }

    /// Validate worker facts against the definition and the store, and build
    /// the canonical output manifest. Pure over recorded state — safe to run
    /// again for the idempotent-replay comparison.
    fn build_output_manifest(
        &self,
        row: &OperationRow,
        def: &OperationDef,
        spec: &DecodedSpec,
        facts: &OperationResultFacts,
    ) -> ServerResult<(Vec<u8>, AssetId)> {
        let catalog = self.catalog();
        // Model facts: bounded, non-empty, and the seed must be the one the
        // spec pinned — a worker cannot claim another run's parameters.
        for (what, value) in [
            ("operation model generator", &facts.generator),
            ("operation model name", &facts.model),
            ("operation model version", &facts.version),
        ] {
            let _ = what;
            if value.is_empty() || value.len() > MAX_NAME_BYTES {
                return Err(ServerError::InvalidInput { what: "operation model facts" });
            }
        }
        if def.supports_seed {
            let pinned_seed = spec
                .params
                .iter()
                .find_map(|(name, value)| match (name.as_str(), value) {
                    ("seed", ParamValue::Int(v)) => Some(*v),
                    _ => None,
                })
                .unwrap_or(0);
            if facts.seed != pinned_seed as u64 {
                return Err(ServerError::Conflict { what: "operation model seed" });
            }
        }
        // Exactly the definition's output set, by name.
        if facts.outputs.len() != def.outputs.len() {
            return Err(ServerError::InvalidInput { what: "operation output set" });
        }
        let out_spec = &def.outputs[0];
        let out = facts
            .outputs
            .iter()
            .find(|o| o.name == out_spec.name)
            .ok_or(ServerError::InvalidInput { what: "operation output name" })?;
        if out.files.is_empty() {
            return Err(ServerError::InvalidInput { what: "operation output files" });
        }
        for f in &out.files {
            if !out_spec.allowed_roles.contains(&f.role) {
                return Err(ServerError::InvalidInput { what: "operation output role" });
            }
        }
        for required in out_spec.required_roles {
            if !out.files.iter().any(|f| f.role == *required) {
                return Err(ServerError::InvalidInput { what: "operation output role missing" });
            }
        }
        // Every claimed blob must exist in the store at the declared size.
        for f in &out.files {
            catalog.require_blob_sized(
                &f.blob,
                f.byte_len,
                "operation output blob",
                "operation output blob size",
            )?;
        }
        if let Some(t) = &out.thumbnail {
            catalog.require_blob_sized(
                &t.blob,
                t.byte_len,
                "operation thumbnail blob",
                "operation thumbnail blob size",
            )?;
        }
        // Metrics must account for exactly the reported bytes.
        let declared: u64 = out
            .files
            .iter()
            .map(|f| f.byte_len)
            .chain(out.thumbnail.iter().map(|t| t.byte_len))
            .sum();
        if out.metrics.total_bytes != declared {
            return Err(ServerError::Conflict { what: "operation output metrics" });
        }

        // Exact parent lineage + inherited rights from the pinned inputs.
        // The first slice's definitions take exactly one input; its rights
        // record transfers verbatim (nothing weakened, nothing dropped).
        let mut parents: Vec<AssetRevisionId> =
            spec.inputs.iter().map(|i| i.revision).collect();
        parents.sort();
        parents.dedup();
        let parent = spec
            .inputs
            .first()
            .ok_or(ServerError::InvalidState { what: "operation inputs", state: "empty" })?;
        let parent_manifest_bytes = catalog
            .asset_revision_manifest(&parent.revision)?
            .ok_or(ServerError::NotFound { what: "operation input revision" })?;
        let parent_manifest = AssetManifest::from_canonical_bytes(&parent_manifest_bytes)?;
        // A parent pulled since creation must not gain new derivatives.
        if let Some(CandidateState::Quarantined) =
            catalog.asset_candidate_state(&parent.asset_id, &parent.revision)?
        {
            return Err(ServerError::InvalidState {
                what: "operation input",
                state: "quarantined",
            });
        }

        let asset_id = operation_asset_id(&row.id, out_spec.name);
        let mut manifest = AssetManifest {
            asset_id,
            kind: out_spec.kind,
            files: out.files.clone(),
            dependencies: vec![],
            thumbnail: out.thumbnail.clone(),
            metrics: out.metrics,
            coordinate_system: CoordinateSystem {
                units_per_meter: 1.0,
                up: Axis::YPos,
                forward: Axis::ZNeg,
                pivot: Pivot::Origin,
            },
            bounds: out.bounds.unwrap_or(Bounds {
                min: Vec3::new(-0.5, -0.5, -0.5),
                max: Vec3::new(0.5, 0.5, 0.5),
            }),
            anchors: Vec::<Anchor>::new(),
            capabilities: Capabilities {
                rigged: false,
                animated: false,
                collidable: out.files.iter().any(|f| f.role == FileRole::Collider),
                loopable: false,
                spawnable: false,
            },
            spawn_recipe: None,
            provenance: Some(Provenance {
                generator: facts.generator.clone(),
                model: facts.model.clone(),
                version: facts.version.clone(),
                seed: facts.seed,
                parents,
                params_digest: Some(row.spec_digest),
            }),
            rights: parent_manifest.rights.clone(),
        };
        manifest.canonicalize();
        manifest.validate()?;
        let bytes = manifest.to_canonical_bytes()?;
        Ok((bytes, asset_id))
    }

    // ---- internals ---------------------------------------------------------

    fn owned_row(&self, owner: &PrincipalId, id: &OperationId) -> ServerResult<OperationRow> {
        let row = self
            .row_by_id(id)?
            .ok_or(ServerError::NotFound { what: "operation" })?;
        if row.owner != *owner {
            // Hidden, not refused: ids must not leak across owners.
            return Err(ServerError::NotFound { what: "operation" });
        }
        Ok(row)
    }

    /// Lazily reconcile a queued row whose executor job reached a terminal
    /// state without finalization (compute failure, cancellation, or an
    /// executor that bypassed the finalizer).
    fn sync_row(&self, db: &Db, row: OperationRow, now_ms: u64) -> ServerResult<OperationRow> {
        if row.state != OperationState::Queued {
            return Ok(row);
        }
        let flipped = match self.jobs().state(&row.job_id)? {
            None | Some(JobState::Pending) | Some(JobState::Running) => return Ok(row),
            Some(JobState::Failed) => {
                self.set_state(
                    db,
                    &row.id,
                    OperationState::Failed,
                    Some("executor failed terminally"),
                    now_ms,
                )?;
                self.append_event(db, &row.id, "failed", "executor failed terminally", now_ms)?;
                true
            }
            Some(JobState::Cancelled) => {
                self.set_state(db, &row.id, OperationState::Cancelled, None, now_ms)?;
                self.append_event(db, &row.id, "cancelled", "executor job cancelled", now_ms)?;
                true
            }
            Some(JobState::Succeeded) => {
                // Success is only reachable through finalize (which flips the
                // row in the same transaction); a succeeded job under a
                // queued row means the executor bypassed the finalizer.
                self.set_state(
                    db,
                    &row.id,
                    OperationState::Failed,
                    Some("executor bypassed finalization"),
                    now_ms,
                )?;
                self.append_event(db, &row.id, "failed", "executor bypassed finalization", now_ms)?;
                true
            }
        };
        if flipped {
            self.row_by_id(&row.id)?
                .ok_or(ServerError::NotFound { what: "operation row" })
        } else {
            Ok(row)
        }
    }

    fn set_state(
        &self,
        db: &Db,
        id: &OperationId,
        state: OperationState,
        error: Option<&str>,
        now_ms: u64,
    ) -> ServerResult<()> {
        let mut s = db.prepare(
            "set operation state",
            "UPDATE operations SET state=?2, error=?3, updated_ms=?4 WHERE operation_id=?1",
        )?;
        s.bind_blob(1, &id.0)?;
        s.bind_text(2, state.as_str())?;
        match error {
            Some(e) => s.bind_text(3, e)?,
            None => s.bind_null(3)?,
        }
        s.bind_u64(4, now_ms)?;
        s.run()
    }

    fn append_event(
        &self,
        db: &Db,
        id: &OperationId,
        kind: &str,
        detail: &str,
        now_ms: u64,
    ) -> ServerResult<()> {
        let mut s = db.prepare(
            "operation event seq",
            "SELECT COALESCE(MAX(seq), 0) FROM operation_events WHERE operation_id = ?1",
        )?;
        s.bind_blob(1, &id.0)?;
        let last = if s.step()? { s.column_u64(0) } else { 0 };
        drop(s);
        if last >= self.budgets.max_operation_events as u64 {
            return Err(ServerError::OverBudget {
                what: "operation events",
                limit: self.budgets.max_operation_events as u64,
                found: last + 1,
            });
        }
        let mut detail = detail;
        if detail.len() > 200 {
            let mut end = 200;
            while !detail.is_char_boundary(end) {
                end -= 1;
            }
            detail = &detail[..end];
        }
        let mut s = db.prepare(
            "append operation event",
            "INSERT INTO operation_events(operation_id, seq, kind, detail, created_ms)
             VALUES(?1, ?2, ?3, ?4, ?5)",
        )?;
        s.bind_blob(1, &id.0)?;
        s.bind_u64(2, last + 1)?;
        s.bind_text(3, kind)?;
        s.bind_text(4, detail)?;
        s.bind_u64(5, now_ms)?;
        s.run()
    }

    fn row_by_id(&self, id: &OperationId) -> ServerResult<Option<OperationRow>> {
        let mut s = self.db.prepare(
            "operation by id",
            "SELECT operation_id, owner, namespace, kind, def_revision, idempotency_key,
                    spec_digest, spec, state, round, job_id, error, result_asset,
                    result_revision, created_ms, updated_ms
             FROM operations WHERE operation_id = ?1",
        )?;
        s.bind_blob(1, &id.0)?;
        if !s.step()? {
            return Ok(None);
        }
        Ok(Some(Self::row_from_stmt(&s)?))
    }

    fn row_by_idem(
        &self,
        owner: &PrincipalId,
        namespace: &str,
        key: &str,
    ) -> ServerResult<Option<OperationRow>> {
        let mut s = self.db.prepare(
            "operation by idem",
            "SELECT operation_id, owner, namespace, kind, def_revision, idempotency_key,
                    spec_digest, spec, state, round, job_id, error, result_asset,
                    result_revision, created_ms, updated_ms
             FROM operations WHERE owner = ?1 AND namespace = ?2 AND idempotency_key = ?3",
        )?;
        s.bind_blob(1, &owner.0)?;
        s.bind_text(2, namespace)?;
        s.bind_text(3, key)?;
        if !s.step()? {
            return Ok(None);
        }
        Ok(Some(Self::row_from_stmt(&s)?))
    }

    fn row_from_stmt(s: &crate::sqlite::Stmt<'_>) -> ServerResult<OperationRow> {
        let corrupt = |state: &'static str| ServerError::InvalidState { what: "operation row", state };
        Ok(OperationRow {
            id: OperationId(fixed16(&s.column_blob(0), "operation row")?),
            owner: PrincipalId(fixed16(&s.column_blob(1), "operation row")?),
            namespace: s.column_text(2),
            kind: s.column_text(3),
            def_revision: u32::try_from(s.column_u64(4)).map_err(|_| corrupt("def revision"))?,
            idempotency_key: s.column_text(5),
            spec_digest: fixed32(&s.column_blob(6), "operation row")?,
            spec: s.column_blob(7),
            state: OperationState::parse(&s.column_text(8)).ok_or(corrupt("state"))?,
            round: u32::try_from(s.column_u64(9)).map_err(|_| corrupt("round"))?,
            job_id: JobId(fixed16(&s.column_blob(10), "operation row")?),
            error: if s.column_is_null(11) { None } else { Some(s.column_text(11)) },
            result: if s.column_is_null(12) || s.column_is_null(13) {
                None
            } else {
                Some((
                    AssetId::from_bytes(fixed16(&s.column_blob(12), "operation row")?),
                    AssetRevisionId::from_bytes(fixed32(&s.column_blob(13), "operation row")?),
                ))
            },
            created_ms: s.column_u64(14),
            updated_ms: s.column_u64(15),
        })
    }

    fn snapshot_of(&self, row: OperationRow) -> ServerResult<OperationSnapshot> {
        let spec = decode_spec(&row.spec)?;
        let job_state = self.jobs().state(&row.job_id)?;
        let display_state = match row.state {
            OperationState::Succeeded => "succeeded",
            OperationState::Failed => "failed",
            OperationState::Cancelled => "cancelled",
            OperationState::Queued => match job_state {
                Some(JobState::Running) => "running",
                _ => "queued",
            },
        };
        Ok(OperationSnapshot {
            id: row.id,
            owner: row.owner,
            namespace: row.namespace,
            kind: row.kind,
            def_revision: row.def_revision,
            idempotency_key: row.idempotency_key,
            spec_digest: row.spec_digest,
            state: row.state,
            display_state,
            round: row.round,
            job_id: row.job_id,
            job_state,
            error: row.error,
            result: row.result,
            created_ms: row.created_ms,
            updated_ms: row.updated_ms,
            inputs: spec.inputs,
            params: spec.params,
            publication: spec.publication,
        })
    }
}

// ---------------------------------------------------------------------------
// validation helpers
// ---------------------------------------------------------------------------

/// Idempotency keys are caller-chosen retry handles: printable ASCII,
/// 1..=128 bytes. They scope under `{owner, namespace}`, so no global
/// uniqueness or charset ceremony is needed beyond safe storage/display.
fn validate_idempotency_key(key: &str) -> ServerResult<()> {
    if key.is_empty() || key.len() > 128 {
        return Err(ServerError::InvalidInput { what: "operation idempotency key length" });
    }
    if !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ServerError::InvalidInput { what: "operation idempotency key charset" });
    }
    Ok(())
}

/// Resolve caller params against the definition: unknown names refuse, types
/// and bounds are enforced, defaults fill the gaps, and the result is the
/// COMPLETE set in definition order (canonical for digesting).
fn resolve_params(
    def: &OperationDef,
    given: &[(String, ParamValue)],
) -> ServerResult<Vec<(String, ParamValue)>> {
    for (name, _) in given {
        if !def.params.iter().any(|p| p.name == name) {
            return Err(ServerError::InvalidInput { what: "operation parameter unknown" });
        }
    }
    for (index, (name, _)) in given.iter().enumerate() {
        if given[..index].iter().any(|(other, _)| other == name) {
            return Err(ServerError::InvalidInput { what: "operation parameter duplicate" });
        }
    }
    let mut out = Vec::with_capacity(def.params.len());
    for spec in def.params {
        let provided = given.iter().find(|(name, _)| name == spec.name);
        let value = match (&spec.kind, provided) {
            (ParamSpecKind::Int { min, max, .. }, Some((_, ParamValue::Int(v)))) => {
                if v < min || v > max {
                    return Err(ServerError::InvalidInput { what: "operation parameter range" });
                }
                ParamValue::Int(*v)
            }
            (ParamSpecKind::Text { max_bytes, .. }, Some((_, ParamValue::Text(v)))) => {
                if v.len() > *max_bytes || v.chars().any(char::is_control) {
                    return Err(ServerError::InvalidInput { what: "operation parameter text" });
                }
                ParamValue::Text(v.clone())
            }
            (ParamSpecKind::Bool { .. }, Some((_, ParamValue::Bool(v)))) => ParamValue::Bool(*v),
            (_, Some(_)) => {
                return Err(ServerError::InvalidInput { what: "operation parameter type" })
            }
            (ParamSpecKind::Int { default, .. }, None) => ParamValue::Int(*default),
            (ParamSpecKind::Text { default, .. }, None) => ParamValue::Text(default.to_string()),
            (ParamSpecKind::Bool { default }, None) => ParamValue::Bool(*default),
        };
        out.push((spec.name.to_string(), value));
    }
    Ok(out)
}

/// Per-slot binding counts must match the definition.
fn check_slot_counts(def: &OperationDef, inputs: &[OperationInputBinding]) -> ServerResult<()> {
    for binding in inputs {
        if !def.inputs.iter().any(|s| s.name == binding.slot) {
            return Err(ServerError::InvalidInput { what: "operation input slot" });
        }
    }
    for slot in def.inputs {
        let count = inputs.iter().filter(|b| b.slot == slot.name).count() as u32;
        if count < slot.min_count || count > slot.max_count {
            return Err(ServerError::InvalidInput { what: "operation input count" });
        }
    }
    Ok(())
}
