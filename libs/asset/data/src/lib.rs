//! Shared canonical content contract for the Makepad Asset Server.
//!
//! One crate defines the identities, immutable manifests, admission budgets,
//! scene/migration plans, and activation DTOs that the Asset Server, the
//! client cache, the scene compiler, and the headless World Server all share.
//! It deliberately has zero dependencies: no UI `Cx`, no GPU, no database, no
//! HTTP — headless hosts and tests use it as-is.
//!
//! Contract rules, frozen for `CONTENT_SCHEMA_VERSION` 1:
//! - Bytes are content-addressed (`BlobId`); manifests are canonical documents
//!   and every revision ID is the SHA-256 of the manifest's canonical bytes.
//! - Canonical encoding is deterministic across map order, process restart,
//!   and platform: no maps, sorted-unique repeated fields, fixed-width
//!   big-endian integers, finite floats with one bit pattern per value.
//! - Decoding is total and fail-closed. Malformed, over-budget, non-canonical,
//!   unknown-tag, or trailing input is a structured [`AssetDataError`]; nothing
//!   here infers an ID, substitutes content, or guesses.
//! - Every published mesh-bearing asset revision carries its mandatory
//!   PNG/JPEG thumbnail metadata as a typed field.
//! - Aliases are for authoring; exact `{asset_id, revision}` pairs are for
//!   multiplayer. Content sets map append-only `AssetSlot` numbers to exact
//!   revisions and never reinterpret an existing slot.
//! - Authored objects and typed persistent state carry required stable keys;
//!   migration plans may escalate their activation mode but can never
//!   downgrade below what their own findings require.

pub mod activation;
pub mod asset;
pub mod codec;
pub mod content_set;
pub mod derived;
pub mod error;
pub mod game;
pub mod geom;
pub mod id;
pub mod import;
pub mod limits;
pub mod migration;
pub mod scene;
pub mod sha256;
pub mod stateful_billboard;
pub mod snapshot;
pub mod value;
pub mod world_place;

pub use activation::{
    CommitContentChange, CommitRealm, CommitSceneChange, ContentChangeReady, ContentRefusal,
    ContentRefusalCode, JoinContentReady, JoinTicket, PrepareContentChange, PrepareRealm,
    PrepareSceneChange, ReadinessPolicy, RealmDescriptor, RealmEpoch, RoomContentTuple,
    SceneApplied, SceneContentReady, SceneSequence, SceneTag, SceneTagDisposition, Tick,
    UnreadyPeerPolicy,
};
pub use asset::{
    Anchor, AssetFile, AssetKind, AssetManifest, Axis, Capabilities, CoordinateSystem,
    DerivativePolicy, DeviceTier, FileRole, ImageDims, MediaType, Metrics, Pivot, PrefabClass,
    Provenance, Redistribution, Rights, SpawnParam, SpawnRecipe, ThumbnailCells, ThumbnailLayout,
    ThumbnailMedia, ThumbnailMeta, ThumbnailRect, ThumbnailView, ThumbnailViewKind,
};
pub use codec::CONTENT_SCHEMA_VERSION;
pub use content_set::{AssetSlot, ContentSetManifest};
pub use derived::{
    derivation_key, resolve_variants, ClientProfile, DerivedInput, DerivedVariantManifest,
    ProcessingRecipe, RealmResolution, RealmResolutionEntry, RecipeKind, RecipeSettings,
    ResolvedEntry, ResolvedVariantMap, ToolClosure, VariantRole, VariantSetManifest,
    OUTPUT_SCHEMA_V1, RESOLUTION_POLICY_V1,
};
pub use error::AssetDataError;
pub use game::{ContentLock, GameRevisionManifest, LockEntry, LockVariantSet};
pub use geom::{Bounds, Quat, Transform, Vec3};
pub use id::{
    AssetAlias, AssetId, AssetRevisionId, AssetRevisionRef, BlobId, ClientProfileDigest,
    ContentSetId, DerivationKey, DerivedVariantId, GameAlias, GameId, GameRevisionId,
    ImportRevisionId, MigrationPlanDigest, PackEntryKey, RealmResolutionDigest, RecipeDigest,
    ResolvedMapDigest, SceneObjectKey, ScenePlanDigest, SnapshotDigest, SourceCollectionId,
    StateKey, TransactionId, VariantSetId,
};
pub use import::{
    ImportAsset, ImportFile, ImportManifest, ImportThumbnail, SourceCollection, SourceOrigin,
    IMPORT_ASSET_ID_POLICY_V1,
};
pub use migration::{
    ActivationMode, ComponentRule, MigrationReason, MigrationReasonCode, PreserveRule,
    RebuildScopes, SceneMigrationPlan, SceneOp, SceneOpKind, StateMigration, StateMigrationOp,
    TerrainPolicy,
};
pub use scene::{
    ComponentConfig, Param, SceneObject, ScenePlan, StateLifetime, StateSchema, TerrainDecl,
};
pub use sha256::{sha256, sha256_hex, Sha256};
pub use snapshot::{
    SnapshotAssembler, SnapshotBegin, SnapshotChunk, SnapshotCounts, SnapshotDigestBuilder,
    SnapshotEnd, SnapshotId, SnapshotReady, SnapshotSection,
};
pub use value::{Value, ValueType};
