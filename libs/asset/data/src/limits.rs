//! Frozen admission budgets for the content contract.
//!
//! Every count, length, and metric a manifest or DTO may carry is bounded by a
//! named constant here. Validators enforce these before publication, decoders
//! enforce them before allocation, and clients recheck them before use. There
//! is no unlimited field anywhere in the contract.

/// Hard ceiling for one canonical document (manifest, lock, plan, or DTO).
/// Blobs themselves are unbounded by this; documents only reference them.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// General ceiling for any length-prefixed string a decoder will allocate.
pub const MAX_STRING_BYTES: usize = 4096;

/// Human names/titles (asset anchors, components, params, game name, author).
pub const MAX_NAME_BYTES: usize = 64;
/// Game/asset description text.
pub const MAX_DESCRIPTION_BYTES: usize = 4096;
/// A string stored inside a typed `Value`.
pub const MAX_VALUE_STR_BYTES: usize = 1024;

/// Readable alias: total bytes, segment count, and per-segment bytes.
pub const MAX_ALIAS_BYTES: usize = 128;
pub const MIN_ALIAS_SEGMENTS: usize = 2;
pub const MAX_ALIAS_SEGMENTS: usize = 8;
pub const MAX_ALIAS_SEGMENT_BYTES: usize = 48;

/// Stable scene/state key: total bytes, segment count, per-segment bytes.
pub const MAX_KEY_BYTES: usize = 96;
pub const MAX_KEY_SEGMENTS: usize = 8;
pub const MAX_KEY_SEGMENT_BYTES: usize = 48;

/// Asset manifest shape.
pub const MAX_FILES_PER_ASSET: usize = 64;
pub const MAX_DEPENDENCIES_PER_ASSET: usize = 64;
/// Maximum dependency-chain depth an asset closure may declare.
pub const MAX_DEPENDENCY_DEPTH: u32 = 8;
/// Anchors per asset. 64, not 32: a converted Doom map publishes its player
/// starts, walk heights, exit, keys, every door, every lift and every
/// teleport pad, and E1M8-sized maps pass 32 on doors alone — a level that
/// silently drops half its doors is worse than one that refuses.
pub const MAX_ANCHORS_PER_ASSET: usize = 64;
pub const MAX_SPAWN_PARAMS: usize = 64;
pub const MAX_PROVENANCE_PARENTS: usize = 8;

/// Per-file and per-asset byte ceilings.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_ASSET_BYTES: u64 = 1024 * 1024 * 1024;

/// Mesh admission ceilings, invariant across visual variants.
pub const MAX_TRIANGLES: u32 = 2_000_000;
pub const MAX_VERTICES: u32 = 4_000_000;
pub const MAX_JOINTS: u16 = 256;
pub const MAX_CLIPS: u16 = 256;
pub const MAX_TEXTURE_DIM: u32 = 8192;
/// Audio/video duration ceiling in milliseconds.
pub const MAX_MEDIA_MILLIS: u32 = 15 * 60 * 1000;
/// Highest LOD index a file variant may declare.
pub const MAX_LOD: u8 = 7;

/// Mandatory mesh thumbnail contract: canonical 512, accepted 256..=4096.
pub const THUMBNAIL_MIN_DIM: u32 = 256;
pub const THUMBNAIL_CANONICAL_DIM: u32 = 512;
pub const THUMBNAIL_MAX_DIM: u32 = 4096;

/// Regions one thumbnail may declare about itself. Eight is generous for
/// everything the catalog bakes today — an audio composite declares two, a
/// sprite sheet one — and keeps the added metadata a rounding error against
/// `MAX_DOCUMENT_BYTES`.
pub const MAX_THUMBNAIL_VIEWS: usize = 8;
/// Ceiling for a declared cycling rate. Above this a "frame" is shorter than
/// a display refresh, so it is a producer bug, not a fast animation.
pub const MAX_THUMBNAIL_FPS: f32 = 240.0;

/// Splash source admission ceiling for a game revision.
pub const MAX_SPLASH_SOURCE_BYTES: u64 = 1024 * 1024;

/// Content lock and content set shape.
pub const MAX_LOCK_ENTRIES: usize = 4096;
pub const MAX_LOCK_CLOSURE: usize = 16384;
pub const MAX_CONTENT_SET_SLOTS: usize = 65536;

/// Scene plan shape.
pub const MAX_SCENE_OBJECTS: usize = 65536;
pub const MAX_COMPONENTS_PER_OBJECT: usize = 32;
pub const MAX_PARAMS_PER_COMPONENT: usize = 64;
pub const MAX_STATE_SCHEMAS: usize = 4096;

/// Migration plan shape.
pub const MAX_MIGRATION_OPS: usize = 65536;
pub const MAX_STATE_MIGRATIONS: usize = 4096;
pub const MAX_MIGRATION_REASONS: usize = 256;
pub const MAX_REASON_DETAIL_BYTES: usize = 256;
pub const MAX_COMPONENT_RULES_PER_OP: usize = 32;

/// Activation DTO shape.
pub const MAX_ORIGINS: usize = 8;
pub const MAX_ORIGIN_BYTES: usize = 256;
pub const MAX_CAPABILITY_BYTES: usize = 512;
pub const MAX_DYNAMIC_SPAWNS: usize = 1024;
pub const MAX_REQUIRED_DELTA: usize = 1024;

/// `units_per_meter` sanity ceiling for a declared coordinate system.
pub const MAX_UNITS_PER_METER: f32 = 10_000.0;

/// Snapshot framing. One chunk stays well below the transport's 256 KiB frame
/// ceiling; the whole snapshot is bounded so a hostile host cannot grow a
/// joining client without limit.
pub const MAX_SNAPSHOT_CHUNK_BYTES: usize = 192 * 1024;
pub const MAX_SNAPSHOT_RECORDS_PER_SECTION: u32 = 262_144;
pub const MAX_SNAPSHOT_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// Content/readiness refusal shape.
pub const MAX_REFUSAL_MISSING: usize = 64;
pub const MAX_REFUSAL_DETAIL_BYTES: usize = 256;

/// External-pack import shape. A pack path is the normalized archive-relative
/// source path of one entry file; entry keys reuse the stable-key budget but
/// are capped at fewer segments so `source/pack/key...` still fits an alias.
pub const MAX_IMPORT_ASSETS: usize = 1024;
pub const MAX_IMPORT_FILES_PER_ASSET: usize = 32;
pub const MAX_PACK_PATH_BYTES: usize = 256;
pub const MAX_PACK_VERSION_BYTES: usize = 32;
pub const MAX_IMPORT_KEY_SEGMENTS: usize = 6;

/// Processing recipe / derived variant / variant set shape.
pub const MAX_RECIPE_PARAM_BYTES: usize = 64;
pub const MAX_DERIVED_INPUTS: usize = 16;
pub const MAX_DERIVED_OUTPUTS: usize = 16;
pub const MAX_VARIANTS_PER_SET: usize = 256;
pub const MAX_RESOLVED_ENTRIES: usize = 64;
/// One aggregate realm resolution entry per asset carrying a pinned variant
/// set. Sized so the document stays under `MAX_DOCUMENT_BYTES`.
pub const MAX_REALM_RESOLUTION_ENTRIES: usize = 8192;

/// Recipe parameter domains.
pub const MIN_LOD_TRIANGLES: u32 = 8;
pub const MAX_TRANSCODE_QUALITY: u8 = 100;

/// Rights record shape: exact license identifier (SPDX-style, with version)
/// and an optional terms-revision qualifier.
pub const MAX_LICENSE_BYTES: usize = 128;
pub const MAX_LICENSE_REVISION_BYTES: usize = 64;

/// Readiness deadline ceiling for prepare transactions (10 minutes).
pub const MAX_READINESS_DEADLINE_MILLIS: u32 = 600_000;
