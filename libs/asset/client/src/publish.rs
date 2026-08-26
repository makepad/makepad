//! Typed artifact publication: one call turns a generated media file + its
//! thumbnail into a complete, searchable, playable catalog asset.
//!
//! The sequence (every step over the existing wire contract, every identity
//! verified locally before it is trusted):
//!
//! 1. build + validate the canonical [`AssetManifest`] (fail-closed through
//!    the content contract, so a bad publish dies before any byte moves),
//! 2. upload the artifact and thumbnail blobs (content-addressed; the
//!    server's echoed identity must equal the locally hashed one),
//! 3. register the asset id (server-minted, or reuse a caller-supplied id —
//!    an "already exists" 409 is expected on re-publish),
//! 4. write the search annotation FIRST (title/kind/categories/tags/prompt),
//!    so the later publish emits a catalog event that already carries the
//!    content kind — kind-filtered subscribers (the VJ) see it immediately,
//! 5. inspect the candidate lifecycle, then stage/publish only when needed;
//!    this makes a retry after a landed publish continue to the missing alias
//!    instead of trying to re-stage immutable published content,
//! 6. optionally point a stable alias at the published head.
//!
//! Server-side catalog events (`asset_published`, `alias_set`,
//! `annotation_set`) are emitted by the transport after each commit — no
//! extra notification path exists or is needed.

use crate::api::AnnotationUpload;
use crate::client::AssetClient;
use std::path::PathBuf;
use crate::dto::CandidateStateDto;
use crate::error::{ClientError, ClientResult};
use crate::wire;
use makepad_asset_data::limits::{
    MAX_ASSET_BYTES, MAX_FILES_PER_ASSET, MAX_FILE_BYTES, MAX_LICENSE_BYTES,
    MAX_LICENSE_REVISION_BYTES, MAX_LOD, MAX_STRING_BYTES,
};
use makepad_asset_data::{
    Anchor, AssetAlias, AssetFile, AssetId, AssetKind, AssetManifest, AssetRevisionId,
    AssetRevisionRef, Axis, BlobId, Bounds, Capabilities, CoordinateSystem, DerivativePolicy,
    DeviceTier, FileRole, ImageDims, MediaType, Metrics, Pivot, Provenance, Redistribution,
    Rights, ThumbnailMedia, ThumbnailMeta, ThumbnailView, Vec3,
};

/// The playable media file being published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishFile {
    pub bytes: Vec<u8>,
    pub media: MediaType,
    /// `FileRole::Video` / `FileRole::Audio` / `FileRole::RenderGlb`…
    pub role: FileRole,
    /// Playback length in milliseconds when known (drives catalog metrics).
    pub media_millis: u32,
    /// Pixel dimensions — REQUIRED by the content contract for PNG/JPEG
    /// artifacts, refused for other media.
    pub dims: Option<(u32, u32)>,
}

/// Measured geometry/animation stats for the catalog metrics block. Zeros
/// mean "not applicable/unmeasured" — never fabricate values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublishStats {
    pub triangles: u32,
    pub vertices: u32,
    pub joints: u16,
    pub clips: u16,
}

/// Typed manifest provenance. Only construct this from REAL generation
/// records; absent legacy provenance publishes as `None`, never as guessed
/// values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishProvenance {
    pub generator: String,
    pub model: String,
    pub version: String,
    pub seed: u64,
    /// Exact parent revisions this output was derived from (the derivation
    /// lineage), when known. Canonical order is enforced at manifest build.
    pub parents: Vec<AssetRevisionId>,
    pub params_digest: Option<[u8; 32]>,
}

/// The mandatory preview image (PNG or JPEG, 256–4096 px per side).
///
/// One picture. [`views`](Self::views) declares what its regions ARE — the
/// spectrogram and wave halves of an audio composite, the packed cell layout
/// of a sprite sheet — so consumers read the layout off the manifest instead
/// of measuring pixels. A producer that has nothing to declare leaves it
/// empty, which is what every publication meant before the field existed.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishThumbnail {
    pub bytes: Vec<u8>,
    pub media: ThumbnailMedia,
    pub width: u32,
    pub height: u32,
    pub views: Vec<ThumbnailView>,
}

impl PublishThumbnail {
    /// A picture with nothing declared about its regions.
    pub fn plain(bytes: Vec<u8>, media: ThumbnailMedia, width: u32, height: u32) -> Self {
        Self { bytes, media, width, height, views: Vec::new() }
    }

    /// The same picture, declaring what its regions are.
    pub fn with_views(mut self, views: Vec<ThumbnailView>) -> Self {
        self.views = views;
        self
    }
}

/// The explicit typed rights declaration of a publication — the COMPLETE
/// record the content schema preserves inside the immutable revision
/// identity: exact license id (+revision qualifier), pinned terms digest
/// and URL, required attribution, upstream source and archive identity, and
/// the redistribution/derivative policies. There is NO blanket default:
/// imported and derived content states its terms (or inherits them from an
/// exact source revision); a missing license, or an attribution-required
/// policy without credits, refuses before any byte moves. The one named
/// exception is [`PublishRights::generated_cc0`], which spells out the
/// pre-existing born-in-pipeline generated-media contract instead of
/// hiding it.
///
/// For imported content every field is AUTHORITATIVELY the registered
/// source collection's terms; a later publication can restate them, never
/// relax them (see the rights-immutability guard on both publish paths).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishRights {
    /// Exact license identifier including its version, e.g. `CC-BY-4.0`.
    /// Mandatory.
    pub license: String,
    /// Terms revision qualifier when the identifier alone is not exact
    /// (publisher EULA date, custom terms revision). May be empty.
    pub license_revision: String,
    /// SHA-256 of the immutable license terms text, when pinned.
    pub terms_digest: Option<[u8; 32]>,
    /// URL of the immutable terms document. May be empty.
    pub terms_url: String,
    /// Required attribution/credits line. Mandatory whenever
    /// redistribution or derivatives require attribution.
    pub credits: String,
    /// Upstream origin of the content (URL/repo/locator of the ORIGINAL
    /// source). Empty only for content born in this pipeline.
    pub source: String,
    /// SHA-256 of the exact upstream archive/revision, when pinned.
    pub source_archive: Option<[u8; 32]>,
    pub redistribution: Redistribution,
    pub derivatives: DerivativePolicy,
}

impl PublishRights {
    /// Fully explicit declaration: identifier, attribution, upstream
    /// origin, and BOTH policies stated by the caller. The optional pins
    /// (`license_revision`, `terms_digest`, `terms_url`, `source_archive`)
    /// start empty/None — an absent pin is honest; set them when the
    /// registration records them.
    pub fn declared(
        license: impl Into<String>,
        credits: impl Into<String>,
        source: impl Into<String>,
        redistribution: Redistribution,
        derivatives: DerivativePolicy,
    ) -> PublishRights {
        PublishRights {
            license: license.into(),
            license_revision: String::new(),
            terms_digest: None,
            terms_url: String::new(),
            credits: credits.into(),
            source: source.into(),
            source_archive: None,
            redistribution,
            derivatives,
        }
    }

    /// The ONLY blanket grant, and it is named: content generated inside
    /// this pipeline (no upstream source, no required attribution) that the
    /// operator publishes as CC0. Never applies to imported or derived
    /// content — those flows require explicit or inherited terms.
    pub fn generated_cc0() -> PublishRights {
        PublishRights::declared(
            "CC0-1.0",
            "",
            "",
            Redistribution::Allowed,
            DerivativePolicy::Allowed,
        )
    }

    /// The exact terms of an already published manifest — the inheritance
    /// path for derivatives and the comparison form for the
    /// rights-immutability guard.
    pub fn from_manifest(rights: &Rights) -> PublishRights {
        PublishRights {
            license: rights.license.clone(),
            license_revision: rights.license_revision.clone(),
            terms_digest: rights.terms_digest,
            terms_url: rights.terms_url.clone(),
            credits: rights.credits.clone(),
            source: rights.source.clone(),
            source_archive: rights.source_archive,
            redistribution: rights.redistribution,
            derivatives: rights.derivatives,
        }
    }

    fn validate(&self) -> ClientResult<()> {
        if self.license.trim().is_empty() {
            return Err(ClientError::InvalidInput { what: "publish rights license missing" });
        }
        if self.license.len() > MAX_LICENSE_BYTES {
            return Err(ClientError::InvalidInput {
                what: "publish rights license over budget",
            });
        }
        if self.license_revision.len() > MAX_LICENSE_REVISION_BYTES {
            return Err(ClientError::InvalidInput {
                what: "publish rights license_revision over budget",
            });
        }
        for text in [
            &self.license,
            &self.license_revision,
            &self.terms_url,
            &self.credits,
            &self.source,
        ] {
            if text.len() > MAX_STRING_BYTES {
                return Err(ClientError::InvalidInput { what: "publish rights over budget" });
            }
            if text.chars().any(char::is_control) {
                return Err(ClientError::InvalidInput { what: "publish rights control chars" });
            }
        }
        // Mirror the contract law locally for a friendly early refusal:
        // attribution nobody can render is not a grant.
        if (self.redistribution == Redistribution::AttributionRequired
            || self.derivatives == DerivativePolicy::AttributionRequired)
            && self.credits.is_empty()
        {
            return Err(ClientError::InvalidInput {
                what: "publish rights credits required for attribution",
            });
        }
        Ok(())
    }

    fn as_manifest_rights(&self) -> Rights {
        Rights {
            license: self.license.clone(),
            license_revision: self.license_revision.clone(),
            terms_digest: self.terms_digest,
            terms_url: self.terms_url.clone(),
            credits: self.credits.clone(),
            source: self.source.clone(),
            source_archive: self.source_archive,
            redistribution: self.redistribution,
            derivatives: self.derivatives,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublishRequest {
    pub namespace: String,
    pub kind: AssetKind,
    pub title: String,
    pub description: String,
    /// Stable alias to point at the new head (e.g. `gen/track-neon-drift`).
    pub alias: Option<AssetAlias>,
    /// Reuse an existing asset id (re-publish = new revision); None mints.
    pub asset_id: Option<AssetId>,
    pub artifact: PublishFile,
    pub thumbnail: PublishThumbnail,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub creator: String,
    pub generator: String,
    pub backend: String,
    pub model: String,
    /// Generation prompt — indexed owner-only server-side.
    pub prompt: String,
    pub provenance: String,
    /// The immutable manifest rights block, typed and explicit. `new()`
    /// starts it at the NAMED generated-content grant
    /// ([`PublishRights::generated_cc0`]); import/derivation flows must
    /// overwrite it with the real declared or inherited terms.
    pub rights: PublishRights,
    pub private: bool,
    /// Measured stats for the manifest metrics block.
    pub stats: PublishStats,
    /// Typed manifest provenance (real records only).
    pub manifest_provenance: Option<PublishProvenance>,
}

impl PublishRequest {
    /// A minimally filled request; callers set metadata on top.
    pub fn new(
        namespace: impl Into<String>,
        kind: AssetKind,
        title: impl Into<String>,
        artifact: PublishFile,
        thumbnail: PublishThumbnail,
    ) -> PublishRequest {
        PublishRequest {
            namespace: namespace.into(),
            kind,
            title: title.into(),
            description: String::new(),
            alias: None,
            asset_id: None,
            artifact,
            thumbnail,
            categories: Vec::new(),
            tags: Vec::new(),
            creator: String::new(),
            generator: String::new(),
            backend: String::new(),
            model: String::new(),
            prompt: String::new(),
            provenance: String::new(),
            rights: PublishRights::generated_cc0(),
            private: false,
            stats: PublishStats::default(),
            manifest_provenance: None,
        }
    }

    fn validate(&self) -> ClientResult<()> {
        if self.namespace.is_empty()
            || self.namespace.len() > wire::MAX_NAMESPACE_BYTES
            || !wire::query_value_ok(&self.namespace)
        {
            return Err(ClientError::InvalidInput { what: "publish namespace" });
        }
        self.rights.validate()?;
        if self.title.is_empty() || self.title.len() > wire::MAX_TITLE_BYTES {
            return Err(ClientError::InvalidInput { what: "publish title" });
        }
        if self.artifact.bytes.is_empty() {
            return Err(ClientError::InvalidInput { what: "publish artifact empty" });
        }
        if self.thumbnail.bytes.is_empty() {
            return Err(ClientError::InvalidInput { what: "publish thumbnail empty" });
        }
        // Mirror the content contract's thumbnail bounds up front so the
        // refusal is a friendly local error, not a late contract violation.
        for dim in [self.thumbnail.width, self.thumbnail.height] {
            if !(256..=4096).contains(&dim) {
                return Err(ClientError::InvalidInput { what: "publish thumbnail dims" });
            }
        }
        // Image artifacts carry mandatory dimensions; others must not.
        let is_image = matches!(self.artifact.media, MediaType::Png | MediaType::Jpeg);
        match (is_image, self.artifact.dims) {
            (true, None) => {
                return Err(ClientError::InvalidInput { what: "publish image needs dims" })
            }
            (false, Some(_)) => {
                return Err(ClientError::InvalidInput { what: "publish dims on non-image" })
            }
            _ => {}
        }
        Ok(())
    }

    /// The canonical manifest for this request under `asset_id`.
    fn manifest(&self, asset_id: AssetId) -> ClientResult<(Vec<u8>, AssetRevisionId)> {
        let mut manifest = AssetManifest {
            asset_id,
            kind: self.kind,
            files: vec![AssetFile {
                role: self.artifact.role,
                tier: DeviceTier::Any,
                lod: 0,
                media: self.artifact.media,
                blob: BlobId::hash_of(&self.artifact.bytes),
                byte_len: self.artifact.bytes.len() as u64,
                dims: self
                    .artifact
                    .dims
                    .map(|(width, height)| ImageDims { width, height }),
            }],
            dependencies: vec![],
            thumbnail: Some(ThumbnailMeta {
                blob: BlobId::hash_of(&self.thumbnail.bytes),
                media: self.thumbnail.media,
                width: self.thumbnail.width,
                height: self.thumbnail.height,
                byte_len: self.thumbnail.bytes.len() as u64,
                views: self.thumbnail.views.clone(),
            }),
            metrics: Metrics {
                total_bytes: self.artifact.bytes.len() as u64
                    + self.thumbnail.bytes.len() as u64,
                triangles: self.stats.triangles,
                vertices: self.stats.vertices,
                joints: self.stats.joints,
                clips: self.stats.clips,
                max_texture_dim: self
                    .thumbnail
                    .width
                    .max(self.thumbnail.height)
                    .max(self.artifact.dims.map(|(w, h)| w.max(h)).unwrap_or(0)),
                media_millis: self.artifact.media_millis,
            },
            coordinate_system: CoordinateSystem {
                units_per_meter: 1.0,
                up: Axis::YPos,
                forward: Axis::ZNeg,
                pivot: Pivot::Origin,
            },
            bounds: Bounds {
                min: Vec3::new(-0.5, -0.5, -0.5),
                max: Vec3::new(0.5, 0.5, 0.5),
            },
            anchors: vec![],
            capabilities: Capabilities {
                rigged: false,
                animated: false,
                collidable: false,
                // Media clips loop by player policy; declare the capability
                // for the media kinds so game consumers may loop them.
                loopable: matches!(self.kind, AssetKind::Audio | AssetKind::Video),
                spawnable: false,
            },
            spawn_recipe: None,
            provenance: self.manifest_provenance.as_ref().map(|p| Provenance {
                generator: p.generator.clone(),
                model: p.model.clone(),
                version: p.version.clone(),
                seed: p.seed,
                parents: p.parents.clone(),
                params_digest: p.params_digest,
            }),
            rights: self.rights.as_manifest_rights(),
        };
        manifest.canonicalize();
        manifest.validate().map_err(ClientError::Content)?;
        let bytes = manifest.to_canonical_bytes().map_err(ClientError::Content)?;
        let revision = manifest.revision().map_err(ClientError::Content)?;
        Ok((bytes, revision))
    }

    fn annotation(&self) -> AnnotationUpload {
        AnnotationUpload {
            title: self.title.clone(),
            description: self.description.clone(),
            kind: Some(self.kind),
            categories: self.categories.clone(),
            tags: self.tags.clone(),
            creator: self.creator.clone(),
            generator: self.generator.clone(),
            backend: self.backend.clone(),
            model: self.model.clone(),
            prompt: self.prompt.clone(),
            provenance: self.provenance.clone(),
            private: self.private,
        }
    }
}

/// The published identities, ready to hand to any consumer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
    pub alias: Option<AssetAlias>,
    pub artifact_blob: BlobId,
    pub thumbnail_blob: BlobId,
}

// ---- multi-file bundle publication -------------------------------------------

/// One typed file slot of a multi-file publication: exact role, device tier,
/// LOD index, media type, payload, and (for images) mandatory dimensions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishBundleFile {
    pub role: FileRole,
    pub tier: DeviceTier,
    /// LOD index within the role, `0..=MAX_LOD`.
    pub lod: u8,
    pub media: MediaType,
    /// The payload, when the client holds it. EMPTY exactly when
    /// `reference` is set — a referenced file's bytes are never read here.
    pub bytes: Vec<u8>,
    /// Publish this slot BY REFERENCE instead of by upload: a path on the
    /// SERVER's filesystem that the store will hash in place and catalogue
    /// without copying.
    ///
    /// Only sound when client and store share a filesystem, which in
    /// practice means an app hosting its own store on loopback. The digest
    /// and byte length in the manifest then come from the server's own
    /// measurement, so the manifest is indistinguishable from an uploaded
    /// one — everything downstream (dedup, search, playback, other clients)
    /// is untouched.
    ///
    /// A server without reference support answers 404 and the publish
    /// refuses rather than silently uploading a file the caller asked not to
    /// copy.
    pub reference: Option<PathBuf>,
    /// Pixel dimensions — REQUIRED for PNG/JPEG, refused for other media.
    pub dims: Option<(u32, u32)>,
}

impl PublishBundleFile {
    /// A slot whose bytes the caller holds.
    pub fn bytes(
        role: FileRole,
        media: MediaType,
        bytes: Vec<u8>,
        dims: Option<(u32, u32)>,
    ) -> Self {
        Self { role, tier: DeviceTier::Any, lod: 0, media, bytes, reference: None, dims }
    }

    /// A slot the store should catalogue in place, at `path`, without
    /// copying. `path` is interpreted by the SERVER.
    pub fn reference(
        role: FileRole,
        media: MediaType,
        path: PathBuf,
        dims: Option<(u32, u32)>,
    ) -> Self {
        Self {
            role,
            tier: DeviceTier::Any,
            lod: 0,
            media,
            bytes: Vec::new(),
            reference: Some(path),
            dims,
        }
    }

    pub fn is_reference(&self) -> bool {
        self.reference.is_some()
    }
}

/// A canonical bounded multi-file publication: one request that uploads and
/// deduplicates every blob, builds ONE deterministic [`AssetManifest`]
/// carrying every `(role, tier, lod)` slot, stages it, publishes it
/// all-or-nothing, and returns the exact resulting refs.
///
/// The single-file convenience path ([`PublishRequest`]) remains for plain
/// media artifacts; this is the canonical shape for derived mesh/PBR sets.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishBundle {
    pub namespace: String,
    pub kind: AssetKind,
    pub title: String,
    pub description: String,
    /// Stable alias to point at the new head.
    pub alias: Option<AssetAlias>,
    /// Reuse an existing asset id (re-publish = new revision); None mints.
    pub asset_id: Option<AssetId>,
    /// Every typed file of the revision. Order is irrelevant — the manifest
    /// canonicalizes — but each `(role, tier, lod)` slot must be unique.
    pub files: Vec<PublishBundleFile>,
    /// The mandatory preview image (PNG or JPEG, 256–4096 px per side).
    pub thumbnail: PublishThumbnail,
    /// Exact dependency revisions, never floating aliases.
    pub dependencies: Vec<AssetRevisionRef>,
    pub bounds: Bounds,
    pub coordinate_system: CoordinateSystem,
    pub anchors: Vec<Anchor>,
    pub capabilities: Capabilities,
    /// Playback length in ms when the bundle carries timed media.
    pub media_millis: u32,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    /// Catalog annotation fields (searchable, mutable control-plane text).
    pub creator: String,
    pub generator: String,
    pub backend: String,
    pub model: String,
    /// Generation prompt — indexed owner-only server-side.
    pub prompt: String,
    pub provenance: String,
    /// The immutable manifest rights block, an EXPLICIT constructor input:
    /// a bundle (the import/derivation shape) has no default license at
    /// all. The terms travel INSIDE the revision identity, unlike the
    /// mutable annotation fields above.
    pub rights: PublishRights,
    pub private: bool,
    /// Measured stats for the manifest metrics block (zeros = unmeasured).
    pub stats: PublishStats,
    /// Typed manifest provenance (real records only).
    pub manifest_provenance: Option<PublishProvenance>,
}

impl PublishBundle {
    /// A minimally filled bundle; callers set metadata and geometry on top.
    /// Geometry defaults mirror [`PublishRequest::new`] (meter units,
    /// Y-up/-Z-forward, origin pivot, unit bounds, loopable for the timed
    /// media kinds) — but rights have NO default: the import/derivation
    /// shape states its terms or does not construct.
    pub fn new(
        namespace: impl Into<String>,
        kind: AssetKind,
        title: impl Into<String>,
        files: Vec<PublishBundleFile>,
        thumbnail: PublishThumbnail,
        rights: PublishRights,
    ) -> PublishBundle {
        PublishBundle {
            namespace: namespace.into(),
            kind,
            title: title.into(),
            description: String::new(),
            alias: None,
            asset_id: None,
            files,
            thumbnail,
            dependencies: Vec::new(),
            bounds: Bounds {
                min: Vec3::new(-0.5, -0.5, -0.5),
                max: Vec3::new(0.5, 0.5, 0.5),
            },
            coordinate_system: CoordinateSystem {
                units_per_meter: 1.0,
                up: Axis::YPos,
                forward: Axis::ZNeg,
                pivot: Pivot::Origin,
            },
            anchors: Vec::new(),
            capabilities: Capabilities {
                loopable: matches!(kind, AssetKind::Audio | AssetKind::Video),
                ..Capabilities::default()
            },
            media_millis: 0,
            categories: Vec::new(),
            tags: Vec::new(),
            creator: String::new(),
            generator: String::new(),
            backend: String::new(),
            model: String::new(),
            prompt: String::new(),
            provenance: String::new(),
            rights,
            private: false,
            stats: PublishStats::default(),
            manifest_provenance: None,
        }
    }

    /// Local fail-closed validation before any byte moves. The content
    /// contract re-validates everything at manifest build; these checks
    /// exist to refuse with a precise, friendly reason first.
    fn validate(&self) -> ClientResult<()> {
        if self.namespace.is_empty()
            || self.namespace.len() > wire::MAX_NAMESPACE_BYTES
            || !wire::query_value_ok(&self.namespace)
        {
            return Err(ClientError::InvalidInput { what: "publish namespace" });
        }
        if self.title.is_empty() || self.title.len() > wire::MAX_TITLE_BYTES {
            return Err(ClientError::InvalidInput { what: "publish title" });
        }
        self.rights.validate()?;
        if self.files.is_empty() {
            return Err(ClientError::InvalidInput { what: "publish bundle has no files" });
        }
        if self.files.len() > MAX_FILES_PER_ASSET {
            return Err(ClientError::OverBudget {
                what: "publish bundle file count",
                limit: MAX_FILES_PER_ASSET as u64,
                found: self.files.len() as u64,
            });
        }
        let mut total: u64 = 0;
        for file in &self.files {
            match &file.reference {
                // A referenced slot's length is not knowable here — only the
                // server can measure a file only the server can see — so the
                // budget check for it happens after admission, against the
                // size the store reports.
                Some(path) => {
                    if !file.bytes.is_empty() {
                        return Err(ClientError::InvalidInput {
                            what: "publish bundle reference carries bytes",
                        });
                    }
                    let text = path.to_str().unwrap_or_default();
                    if text.is_empty() || text.len() > wire::MAX_BLOB_REF_PATH_BYTES {
                        return Err(ClientError::InvalidInput {
                            what: "publish bundle reference path",
                        });
                    }
                    if !path.is_absolute() {
                        return Err(ClientError::InvalidInput {
                            what: "publish bundle reference path not absolute",
                        });
                    }
                }
                None => {
                    if file.bytes.is_empty() {
                        return Err(ClientError::InvalidInput {
                            what: "publish bundle empty file",
                        });
                    }
                    if file.bytes.len() as u64 > MAX_FILE_BYTES {
                        return Err(ClientError::OverBudget {
                            what: "publish bundle file bytes",
                            limit: MAX_FILE_BYTES,
                            found: file.bytes.len() as u64,
                        });
                    }
                }
            }
            if file.lod > MAX_LOD {
                return Err(ClientError::InvalidInput { what: "publish bundle file lod" });
            }
            if !file.role.allows(file.media) {
                return Err(ClientError::InvalidInput { what: "publish bundle role/media" });
            }
            let is_image = matches!(file.media, MediaType::Png | MediaType::Jpeg);
            match (is_image, file.dims) {
                (true, None) => {
                    return Err(ClientError::InvalidInput { what: "publish bundle image needs dims" })
                }
                (false, Some(_)) => {
                    return Err(ClientError::InvalidInput { what: "publish bundle dims on non-image" })
                }
                (true, Some((w, h))) if w == 0 || h == 0 => {
                    return Err(ClientError::InvalidInput { what: "publish bundle image dims" })
                }
                _ => {}
            }
            total = total
                .checked_add(file.bytes.len() as u64)
                .ok_or(ClientError::InvalidInput { what: "publish bundle byte overflow" })?;
        }
        // Duplicate (role, tier, lod) slots are ambiguous content: refuse
        // here with a precise reason instead of a late canonical-order error.
        let mut slots: Vec<(FileRole, DeviceTier, u8)> =
            self.files.iter().map(|f| (f.role, f.tier, f.lod)).collect();
        slots.sort();
        if slots.windows(2).any(|w| w[0] == w[1]) {
            return Err(ClientError::InvalidInput { what: "publish bundle duplicate slot" });
        }
        if self.thumbnail.bytes.is_empty() {
            return Err(ClientError::InvalidInput { what: "publish thumbnail empty" });
        }
        for dim in [self.thumbnail.width, self.thumbnail.height] {
            if !(256..=4096).contains(&dim) {
                return Err(ClientError::InvalidInput { what: "publish thumbnail dims" });
            }
        }
        total = total
            .checked_add(self.thumbnail.bytes.len() as u64)
            .ok_or(ClientError::InvalidInput { what: "publish bundle byte overflow" })?;
        if total > MAX_ASSET_BYTES {
            return Err(ClientError::OverBudget {
                what: "publish bundle total bytes",
                limit: MAX_ASSET_BYTES,
                found: total,
            });
        }
        Ok(())
    }

    /// The ONE deterministic canonical manifest for this bundle under
    /// `asset_id`, plus the exact per-file refs it pins. Canonicalization
    /// makes the revision identity independent of the caller's file order.
    ///
    /// `resolved` carries one `(digest, byte_len)` per file in `self.files`
    /// order. For a byte-carrying slot that is simply the hash of its bytes;
    /// for a REFERENCED slot it is what the store measured when it hashed
    /// the file in place, because the client never saw those bytes. Both
    /// produce the same manifest shape — which is exactly why nothing
    /// downstream needs to know the difference.
    fn manifest_with(
        &self,
        asset_id: AssetId,
        resolved: &[(BlobId, u64)],
    ) -> ClientResult<(Vec<u8>, AssetRevisionId, Vec<PublishedFile>)> {
        if resolved.len() != self.files.len() {
            return Err(ClientError::InvalidInput { what: "publish bundle resolution width" });
        }
        let refs: Vec<PublishedFile> = self
            .files
            .iter()
            .zip(resolved)
            .map(|(f, (blob, byte_len))| PublishedFile {
                role: f.role,
                tier: f.tier,
                lod: f.lod,
                blob: *blob,
                byte_len: *byte_len,
            })
            .collect();
        let files: Vec<AssetFile> = self
            .files
            .iter()
            .zip(&refs)
            .map(|(f, r)| AssetFile {
                role: f.role,
                tier: f.tier,
                lod: f.lod,
                media: f.media,
                blob: r.blob,
                byte_len: r.byte_len,
                dims: f.dims.map(|(width, height)| ImageDims { width, height }),
            })
            .collect();
        let total_bytes = files.iter().map(|f| f.byte_len).sum::<u64>()
            + self.thumbnail.bytes.len() as u64;
        let max_texture_dim = self
            .files
            .iter()
            .filter_map(|f| f.dims.map(|(w, h)| w.max(h)))
            .chain([self.thumbnail.width.max(self.thumbnail.height)])
            .max()
            .unwrap_or(0);
        let mut manifest = AssetManifest {
            asset_id,
            kind: self.kind,
            files,
            dependencies: self.dependencies.clone(),
            thumbnail: Some(ThumbnailMeta {
                blob: BlobId::hash_of(&self.thumbnail.bytes),
                media: self.thumbnail.media,
                width: self.thumbnail.width,
                height: self.thumbnail.height,
                byte_len: self.thumbnail.bytes.len() as u64,
                views: self.thumbnail.views.clone(),
            }),
            metrics: Metrics {
                total_bytes,
                triangles: self.stats.triangles,
                vertices: self.stats.vertices,
                joints: self.stats.joints,
                clips: self.stats.clips,
                max_texture_dim,
                media_millis: self.media_millis,
            },
            coordinate_system: self.coordinate_system,
            bounds: self.bounds,
            anchors: self.anchors.clone(),
            capabilities: self.capabilities,
            spawn_recipe: None,
            provenance: self.manifest_provenance.as_ref().map(|p| Provenance {
                generator: p.generator.clone(),
                model: p.model.clone(),
                version: p.version.clone(),
                seed: p.seed,
                parents: p.parents.clone(),
                params_digest: p.params_digest,
            }),
            // The explicit typed rights, never the mutable annotation text:
            // an imported licensed pack keeps its exact terms inside the
            // immutable revision identity.
            rights: self.rights.as_manifest_rights(),
        };
        manifest.canonicalize();
        manifest.validate().map_err(ClientError::Content)?;
        let bytes = manifest.to_canonical_bytes().map_err(ClientError::Content)?;
        let revision = manifest.revision().map_err(ClientError::Content)?;
        Ok((bytes, revision, refs))
    }

    fn annotation(&self) -> AnnotationUpload {
        AnnotationUpload {
            title: self.title.clone(),
            description: self.description.clone(),
            kind: Some(self.kind),
            categories: self.categories.clone(),
            tags: self.tags.clone(),
            creator: self.creator.clone(),
            generator: self.generator.clone(),
            backend: self.backend.clone(),
            model: self.model.clone(),
            prompt: self.prompt.clone(),
            provenance: self.provenance.clone(),
            private: self.private,
        }
    }
}

/// Coarse publication stages, distinct from byte-level blob progress: a UI
/// renders "uploading 3/5", a worker heartbeats the same stage into its job
/// note. Emitted in order; a resumed retry legitimately skips stages whose
/// work already landed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishStage {
    Validating,
    RegisteringAsset,
    /// Uploading one deduplicated blob (`index` is 1-based out of `of`
    /// unique blobs; `bytes` is that blob's size).
    UploadingBlob { index: usize, of: usize, bytes: u64 },
    /// Having the store hash a file in place instead of copying it. The cost
    /// is a full read of that file ON THE SERVER, so it deserves its own
    /// visible stage rather than hiding inside "validating".
    ReferencingFile { index: usize, of: usize },
    Annotating,
    Staging,
    Publishing,
    SettingAlias,
    Complete,
}

impl std::fmt::Display for PublishStage {
    /// Heartbeat-note-safe rendering: short, lowercase, control-free.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validating => write!(f, "validating"),
            Self::RegisteringAsset => write!(f, "registering-asset"),
            Self::UploadingBlob { index, of, bytes } => {
                write!(f, "uploading-blob {index}/{of} ({bytes} bytes)")
            }
            Self::ReferencingFile { index, of } => {
                write!(f, "referencing-file {index}/{of}")
            }
            Self::Annotating => write!(f, "annotating"),
            Self::Staging => write!(f, "staging"),
            Self::Publishing => write!(f, "publishing"),
            Self::SettingAlias => write!(f, "setting-alias"),
            Self::Complete => write!(f, "complete"),
        }
    }
}

/// One exact published file ref: the slot coordinates and the verified blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedFile {
    pub role: FileRole,
    pub tier: DeviceTier,
    pub lod: u8,
    pub blob: BlobId,
    pub byte_len: u64,
}

/// Everything a bundle publication produced, ready to hand to any consumer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedBundle {
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
    pub alias: Option<AssetAlias>,
    /// Exact per-file refs in the bundle's original file order.
    pub files: Vec<PublishedFile>,
    pub thumbnail_blob: BlobId,
}

impl AssetClient {
    /// Rights immutability per asset: a publication that would change the
    /// license/credits/source of an ALREADY PUBLISHED asset refuses with
    /// [`ClientError::RightsConflict`]. The comparison is against the
    /// latest published head's immutable manifest, so replaying a lost
    /// response (same revision, same terms) passes and a silent downgrade
    /// cannot. Different terms are a different asset — publish a new id.
    fn guard_rights_unchanged(
        &mut self,
        detail: &crate::dto::AssetDetailDto,
        new_revision: &AssetRevisionId,
        new_rights: &Rights,
    ) -> ClientResult<()> {
        let Some(prev) = detail.latest_published() else {
            return Ok(());
        };
        if prev.revision == *new_revision {
            return Ok(());
        }
        let previous = match self.fetch_asset_manifest(&prev.revision) {
            Ok(previous) => previous,
            // The predecessor cannot be read under today's contract — a
            // schema bump. The guard exists to stop a SILENT downgrade, and
            // a schema bump is not a silent anything: it is a deliberate,
            // announced, whole-catalog event, and the re-import that carries
            // the catalog across it is the only way the head becomes legible
            // again. Refusing here would make the catalog permanently
            // un-migratable, which is a worse failure than not being able to
            // compare one field. It is said out loud rather than passed over.
            Err(error) if error.is_unreadable_stored_document() => {
                eprintln!(
                    "[publish] rights guard: the published head of this asset cannot be read \
                     under content schema v{} ({error}) — republishing without comparing terms",
                    makepad_asset_data::CONTENT_SCHEMA_VERSION
                );
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if previous.rights != *new_rights {
            return Err(ClientError::RightsConflict {
                what: "published asset rights would change",
            });
        }
        Ok(())
    }

    /// Publish one generated artifact end to end. Blocking (network); run it
    /// through [`crate::ClientRuntime`] from UI hosts.
    pub fn publish_artifact(&mut self, request: &PublishRequest) -> ClientResult<Published> {
        request.validate()?;
        let ns = request.namespace.clone();

        // Register (or adopt) the asset id first: the manifest embeds it.
        let asset_id = match request.asset_id {
            None => self.api().register_asset(&ns, None)?,
            Some(id) => match self.api().register_asset(&ns, Some(&id)) {
                Ok(got) => got,
                // Already registered (re-publish): the 409 is expected.
                Err(ClientError::Server { status: 409, .. }) => id,
                Err(e) => return Err(e),
            },
        };
        let (canonical, revision) = request.manifest(asset_id)?;

        // Bytes before catalog rows: a failed upload leaves nothing staged.
        let artifact_blob = self.api().upload_blob(&ns, &request.artifact.bytes)?;
        let thumbnail_blob = self.api().upload_blob(&ns, &request.thumbnail.bytes)?;

        // Annotation BEFORE publish so the publish event carries the kind.
        self.api().put_annotation(&asset_id, &request.annotation())?;

        // A previous attempt may have committed the immutable revision and
        // then lost its response (or failed while setting the alias). The
        // server intentionally refuses re-staging a Published candidate, so
        // resume from the typed lifecycle state instead of replaying blindly.
        let detail = self.api().asset_detail(&asset_id)?;
        self.guard_rights_unchanged(&detail, &revision, &request.rights.as_manifest_rights())?;
        let candidate_state = detail
            .candidates
            .into_iter()
            .find(|candidate| candidate.revision == revision)
            .map(|candidate| candidate.state);
        match candidate_state {
            None => {
                self.api().stage_asset_revision(&asset_id, &canonical)?;
                self.api().publish_asset_revision(&asset_id, &revision)?;
            }
            Some(CandidateStateDto::Staged) => {
                self.api().publish_asset_revision(&asset_id, &revision)?;
            }
            Some(CandidateStateDto::Published) => {
                // Already durable; continue to the idempotent alias write.
            }
            Some(CandidateStateDto::Quarantined) | Some(CandidateStateDto::Retired) => {
                // Pulled or deleted content is never re-published under the
                // same immutable revision; a caller that wants it back
                // publishes new content.
                return Err(ClientError::InvalidInput {
                    what: "publish revision quarantined",
                });
            }
        }
        if let Some(alias) = &request.alias {
            self.api().put_alias(alias, &asset_id, &revision)?;
        }
        Ok(Published {
            asset_id,
            revision,
            alias: request.alias.clone(),
            artifact_blob,
            thumbnail_blob,
        })
    }

    /// Publish a multi-file bundle end to end. Blocking (network); run it
    /// through [`crate::ClientRuntime`] from UI hosts, or use
    /// [`Self::publish_bundle_with`] from a worker that must heartbeat.
    pub fn publish_bundle(&mut self, request: &PublishBundle) -> ClientResult<PublishedBundle> {
        self.publish_bundle_with(request, None, &|| false)
    }

    /// Publish MANY bundles in one batched exchange: every unique blob goes
    /// up in bulk (`POST /v1/blobs/batch`, one catalog transaction), then the
    /// whole page publishes through `POST /v1/publish/batch` — ONE
    /// state-thread visit and ONE catalog transaction for every asset,
    /// annotation and alias in it, all-or-nothing.
    ///
    /// This is the bulk lane the split [`Self::publish_bundle`] ceremony is
    /// not: two round trips a page instead of ~ten per asset. At most
    /// [`wire::MAX_PUBLISH_BATCH_ITEMS`] bundles per call — callers page
    /// larger sets and stream results between pages. Bundles must carry
    /// their bytes (a `reference` slot needs the split flow), and a bundle
    /// without an `asset_id` has one minted locally.
    ///
    /// Idempotent per page: replaying a landed page re-uploads nothing (the
    /// CAS dedups) and re-publishing an identical revision refreshes its
    /// annotation and alias without minting anything new.
    pub fn publish_bundles(
        &mut self,
        requests: &[PublishBundle],
    ) -> ClientResult<Vec<PublishedBundle>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        if requests.len() > wire::MAX_PUBLISH_BATCH_ITEMS {
            return Err(ClientError::OverBudget {
                what: "publish batch bundles",
                limit: wire::MAX_PUBLISH_BATCH_ITEMS as u64,
                found: requests.len() as u64,
            });
        }
        for request in requests {
            request.validate()?;
            if request.files.iter().any(PublishBundleFile::is_reference) {
                return Err(ClientError::InvalidInput {
                    what: "publish batch reference slot",
                });
            }
        }
        // Build every manifest locally first — identity before any byte
        // moves — and collect the unique blobs per namespace.
        let mut items: Vec<crate::api::PublishBatchWireItem> = Vec::with_capacity(requests.len());
        let mut results: Vec<PublishedBundle> = Vec::with_capacity(requests.len());
        let mut per_ns: Vec<(String, Vec<(BlobId, &[u8])>)> = Vec::new();
        for request in requests {
            let asset_id = match request.asset_id {
                Some(id) => id,
                None => mint_asset_id(),
            };
            let resolved: Vec<(BlobId, u64)> = request
                .files
                .iter()
                .map(|f| (BlobId::hash_of(&f.bytes), f.bytes.len() as u64))
                .collect();
            let (canonical, revision, file_refs) = request.manifest_with(asset_id, &resolved)?;
            let thumbnail_blob = BlobId::hash_of(&request.thumbnail.bytes);
            let ns_blobs = match per_ns.iter_mut().find(|(ns, _)| *ns == request.namespace) {
                Some((_, blobs)) => blobs,
                None => {
                    per_ns.push((request.namespace.clone(), Vec::new()));
                    &mut per_ns.last_mut().expect("just pushed").1
                }
            };
            for (file, (blob, _)) in request.files.iter().zip(&resolved) {
                if !ns_blobs.iter().any(|(b, _)| b == blob) {
                    ns_blobs.push((*blob, &file.bytes));
                }
            }
            if !ns_blobs.iter().any(|(b, _)| *b == thumbnail_blob) {
                ns_blobs.push((thumbnail_blob, &request.thumbnail.bytes));
            }
            items.push(crate::api::PublishBatchWireItem {
                namespace: request.namespace.clone(),
                manifest: canonical,
                alias: request.alias.clone(),
                annotation: request.annotation(),
            });
            results.push(PublishedBundle {
                asset_id,
                revision,
                alias: request.alias.clone(),
                files: file_refs,
                thumbnail_blob,
            });
        }
        // Bytes before catalog rows, in bounded pages per namespace. The
        // page budget stays under the server's batch byte cap.
        const UPLOAD_PAGE_BYTES: usize = 8 * 1024 * 1024;
        for (ns, blobs) in &per_ns {
            let mut page: Vec<&[u8]> = Vec::new();
            let mut page_bytes = 0usize;
            for (_, bytes) in blobs {
                if !page.is_empty()
                    && (page.len() >= wire::MAX_UPLOAD_BATCH_ITEMS
                        || page_bytes + bytes.len() > UPLOAD_PAGE_BYTES)
                {
                    self.api().upload_blob_batch(ns, &page)?;
                    page.clear();
                    page_bytes = 0;
                }
                page.push(bytes);
                page_bytes += bytes.len();
            }
            if !page.is_empty() {
                self.api().upload_blob_batch(ns, &page)?;
            }
        }
        // One request publishes the page; echoed identities must match the
        // locally computed ones.
        let outcomes = self.api().publish_batch(&items)?;
        for ((asset_id, revision, _already), local) in outcomes.iter().zip(&results) {
            if *asset_id != local.asset_id || *revision != local.revision {
                return Err(ClientError::Protocol { what: "publish batch identity mismatch" });
            }
        }
        Ok(results)
    }

    /// As [`Self::publish_bundle`], with explicit operation-stage progress
    /// and cooperative cancellation. `progress` observes each
    /// [`PublishStage`] as it begins; `abort` is consulted before every
    /// network step and ends the publication with
    /// [`ClientError::Cancelled`].
    ///
    /// All-or-nothing: nothing catalog-visible exists until the single
    /// stage→publish transaction lands. An abort or crash can leave
    /// content-addressed blobs (invisible) or a staged candidate
    /// (unpublished); retrying the same bundle resumes idempotently from the
    /// typed candidate state and can never commit a second revision of the
    /// same content.
    pub fn publish_bundle_with(
        &mut self,
        request: &PublishBundle,
        mut progress: Option<&mut dyn FnMut(&PublishStage)>,
        abort: &dyn Fn() -> bool,
    ) -> ClientResult<PublishedBundle> {
        let mut emit = |stage: PublishStage| {
            if let Some(cb) = progress.as_deref_mut() {
                cb(&stage);
            }
        };
        let gate = || -> ClientResult<()> {
            if abort() {
                Err(ClientError::Cancelled)
            } else {
                Ok(())
            }
        };

        emit(PublishStage::Validating);
        request.validate()?;
        let ns = request.namespace.clone();

        // Register (or adopt) the asset id first: the manifest embeds it.
        gate()?;
        emit(PublishStage::RegisteringAsset);
        let asset_id = match request.asset_id {
            None => self.api().register_asset(&ns, None)?,
            Some(id) => match self.api().register_asset(&ns, Some(&id)) {
                Ok(got) => got,
                // Already registered (re-publish/retry): the 409 is expected.
                Err(ClientError::Server { status: 409, .. }) => id,
                Err(e) => return Err(e),
            },
        };
        // Resolve every slot to (digest, length) BEFORE the manifest exists,
        // because a referenced slot's digest is the server's measurement of
        // a file this process never opened. Reference admission is therefore
        // the first thing that touches bytes — and it is idempotent, so a
        // retried publish re-measures rather than duplicating anything.
        let mut resolved: Vec<(BlobId, u64)> = Vec::with_capacity(request.files.len());
        let reference_count = request.files.iter().filter(|f| f.is_reference()).count();
        let mut referenced_done = 0usize;
        for file in &request.files {
            match &file.reference {
                None => resolved.push((BlobId::hash_of(&file.bytes), file.bytes.len() as u64)),
                Some(path) => {
                    gate()?;
                    referenced_done += 1;
                    emit(PublishStage::ReferencingFile {
                        index: referenced_done,
                        of: reference_count,
                    });
                    let text = path.to_str().ok_or(ClientError::InvalidInput {
                        what: "publish bundle reference path encoding",
                    })?;
                    let admission = self.api().admit_blob_ref(&ns, text)?;
                    if admission.size > MAX_FILE_BYTES {
                        return Err(ClientError::OverBudget {
                            what: "publish bundle reference bytes",
                            limit: MAX_FILE_BYTES,
                            found: admission.size,
                        });
                    }
                    resolved.push((admission.blob, admission.size));
                }
            }
        }
        let (canonical, revision, file_refs) = request.manifest_with(asset_id, &resolved)?;
        let thumbnail_blob = BlobId::hash_of(&request.thumbnail.bytes);

        // Upload every unique blob once (bytes before catalog rows). A blob
        // the server already holds at the exact size — a dedupe hit or a
        // resumed retry — is skipped without moving its bytes again.
        // Referenced slots are already admitted and carry no bytes to move.
        let mut unique: Vec<(BlobId, &[u8])> = Vec::new();
        for (file, (blob, _)) in request.files.iter().zip(&resolved) {
            if file.is_reference() {
                continue;
            }
            if !unique.iter().any(|(b, _)| b == blob) {
                unique.push((*blob, &file.bytes));
            }
        }
        if !unique.iter().any(|(b, _)| *b == thumbnail_blob) {
            unique.push((thumbnail_blob, &request.thumbnail.bytes));
        }
        let of = unique.len();
        for (index, (blob, bytes)) in unique.iter().enumerate() {
            gate()?;
            emit(PublishStage::UploadingBlob {
                index: index + 1,
                of,
                bytes: bytes.len() as u64,
            });
            let present = match self.api().blob_head(blob) {
                Ok(head) => head.size == bytes.len() as u64 && head.etag_matches,
                Err(ClientError::NotFound { .. }) => false,
                Err(e) => return Err(e),
            };
            if !present {
                self.api().upload_blob(&ns, bytes)?;
            }
        }

        // Annotation BEFORE publish so the publish event carries the kind.
        gate()?;
        emit(PublishStage::Annotating);
        self.api().put_annotation(&asset_id, &request.annotation())?;

        // Resume from the typed candidate lifecycle exactly like the
        // single-file path: a lost response or an aborted earlier attempt
        // continues instead of replaying immutable steps blindly. The
        // rights guard runs on the same detail read: a re-publication that
        // would change an existing asset's terms dies here, before staging.
        let detail = self.api().asset_detail(&asset_id)?;
        self.guard_rights_unchanged(&detail, &revision, &request.rights.as_manifest_rights())?;
        let candidate_state = detail
            .candidates
            .into_iter()
            .find(|candidate| candidate.revision == revision)
            .map(|candidate| candidate.state);
        match candidate_state {
            None => {
                gate()?;
                emit(PublishStage::Staging);
                self.api().stage_asset_revision(&asset_id, &canonical)?;
                gate()?;
                emit(PublishStage::Publishing);
                self.api().publish_asset_revision(&asset_id, &revision)?;
            }
            Some(CandidateStateDto::Staged) => {
                gate()?;
                emit(PublishStage::Publishing);
                self.api().publish_asset_revision(&asset_id, &revision)?;
            }
            Some(CandidateStateDto::Published) => {
                // Already durable; continue to the idempotent alias write.
            }
            Some(CandidateStateDto::Quarantined) | Some(CandidateStateDto::Retired) => {
                // Pulled or deleted content is never re-published under the
                // same immutable revision; a caller that wants it back
                // publishes new content.
                return Err(ClientError::InvalidInput {
                    what: "publish revision quarantined",
                });
            }
        }
        if let Some(alias) = &request.alias {
            gate()?;
            emit(PublishStage::SettingAlias);
            self.api().put_alias(alias, &asset_id, &revision)?;
        }
        emit(PublishStage::Complete);
        Ok(PublishedBundle {
            asset_id,
            revision,
            alias: request.alias.clone(),
            files: file_refs,
            thumbnail_blob,
        })
    }
}

/// Mint a fresh 16-byte asset identity locally, so a batch page never pays a
/// per-asset register round trip. Entropy comes from the OS-seeded standard
/// hasher keys mixed with time and a process-wide counter — the same class
/// of uniqueness the server's own minting provides, and a true collision is
/// caught by the namespace-conflict check at registration.
fn mint_asset_id() -> AssetId {
    use std::hash::{BuildHasher, Hasher};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut bytes = [0u8; 16];
    for (i, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(now);
        hasher.write_u64(nonce);
        hasher.write_u64(std::process::id() as u64);
        hasher.write_u64(i as u64);
        chunk.copy_from_slice(&hasher.finish().to_be_bytes());
    }
    AssetId::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumbnail() -> PublishThumbnail {
        PublishThumbnail {
            bytes: vec![7; 900],
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            views: Vec::new(),
        }
    }

    fn request() -> PublishRequest {
        PublishRequest::new(
            "gen",
            AssetKind::Video,
            "Neon drift",
            PublishFile {
                bytes: vec![1; 4000],
                media: MediaType::Mp4,
                role: FileRole::Video,
                media_millis: 5200,
                dims: None,
            },
            thumbnail(),
        )
    }

    #[test]
    fn validation_refuses_bad_inputs_locally() {
        assert!(request().validate().is_ok());
        let mut r = request();
        r.namespace = "has space".into();
        assert!(r.validate().is_err());
        let mut r = request();
        r.title = String::new();
        assert!(r.validate().is_err());
        let mut r = request();
        r.artifact.bytes.clear();
        assert!(r.validate().is_err());
        let mut r = request();
        r.thumbnail.width = 100; // under the contract's 256px floor
        assert!(r.validate().is_err());
    }

    #[test]
    fn manifest_is_canonical_and_identity_stable() {
        let r = request();
        let asset = AssetId::from_bytes([9; 16]);
        let (bytes_a, rev_a) = r.manifest(asset).unwrap();
        let (bytes_b, rev_b) = r.manifest(asset).unwrap();
        // Deterministic canonical bytes → stable revision identity.
        assert_eq!(bytes_a, bytes_b);
        assert_eq!(rev_a, rev_b);
        // The manifest decodes back through the contract with the payload
        // file and the mandatory thumbnail intact.
        let decoded = AssetManifest::from_canonical_bytes(&bytes_a).unwrap();
        assert_eq!(decoded.asset_id, asset);
        assert_eq!(decoded.kind, AssetKind::Video);
        assert_eq!(decoded.files.len(), 1);
        assert_eq!(decoded.files[0].role, FileRole::Video);
        assert_eq!(decoded.files[0].byte_len, 4000);
        assert!(decoded.thumbnail.is_some());
        assert!(decoded.capabilities.loopable);
        // A different artifact is a different revision.
        let mut r2 = request();
        r2.artifact.bytes[0] ^= 0xff;
        let (_, rev_c) = r2.manifest(asset).unwrap();
        assert_ne!(rev_a, rev_c);
    }

    #[test]
    fn fidelity_fields_reach_the_manifest_and_are_gated() {
        // Image artifacts REQUIRE dims; non-images refuse them.
        let mut image = request();
        image.kind = AssetKind::Texture;
        image.artifact.media = MediaType::Png;
        image.artifact.role = FileRole::Texture;
        assert!(image.validate().is_err(), "png without dims must refuse");
        image.artifact.dims = Some((800, 600));
        assert!(image.validate().is_ok());
        let mut wrong = request();
        wrong.artifact.dims = Some((1, 1)); // dims on an mp4
        assert!(wrong.validate().is_err());

        // Dims, measured stats and typed provenance land in the manifest.
        let asset = AssetId::from_bytes([4; 16]);
        image.stats = PublishStats { triangles: 12, vertices: 8, joints: 3, clips: 2 };
        image.manifest_provenance = Some(PublishProvenance {
            generator: "h3".into(),
            model: "minimax-h3".into(),
            version: "1.0".into(),
            seed: 42,
            parents: vec![],
            params_digest: Some([7; 32]),
        });
        let (bytes, _rev) = image.manifest(asset).unwrap();
        let decoded = AssetManifest::from_canonical_bytes(&bytes).unwrap();
        let dims = decoded.files[0].dims.as_ref().expect("image dims");
        assert_eq!((dims.width, dims.height), (800, 600));
        assert_eq!(decoded.metrics.triangles, 12);
        assert_eq!(decoded.metrics.vertices, 8);
        assert_eq!(decoded.metrics.joints, 3);
        assert_eq!(decoded.metrics.clips, 2);
        assert!(decoded.metrics.max_texture_dim >= 800);
        let prov = decoded.provenance.as_ref().expect("typed provenance");
        assert_eq!(prov.generator, "h3");
        assert_eq!(prov.seed, 42);
        assert_eq!(prov.params_digest, Some([7; 32]));
        // Absent legacy provenance stays absent — never fabricated.
        let (bytes, _) = request().manifest(asset).unwrap();
        assert!(AssetManifest::from_canonical_bytes(&bytes).unwrap().provenance.is_none());
    }

    #[test]
    fn annotation_mirrors_request_metadata() {
        let mut r = request();
        r.categories = vec!["music".into()];
        r.tags = vec!["synthwave".into()];
        r.prompt = "a neon drift".into();
        let ann = r.annotation();
        assert_eq!(ann.kind, Some(AssetKind::Video));
        assert_eq!(ann.categories, vec!["music".to_string()]);
        assert_eq!(ann.prompt, "a neon drift");
        assert!(!ann.private);
    }

    // ---- multi-file bundle ---------------------------------------------------

    fn bundle_file(role: FileRole, media: MediaType, byte: u8, len: usize) -> PublishBundleFile {
        let dims = matches!(media, MediaType::Png | MediaType::Jpeg).then_some((512, 512));
        PublishBundleFile { role, tier: DeviceTier::Any, lod: 0, media, bytes: vec![byte; len], reference: None, dims }
    }

    /// Byte-carrying bundles resolve every slot from their own bytes; the
    /// tests exercise the manifest through that path.
    fn manifest_of(
        b: &PublishBundle,
        asset: AssetId,
    ) -> ClientResult<(Vec<u8>, AssetRevisionId, Vec<PublishedFile>)> {
        let resolved: Vec<(BlobId, u64)> = b
            .files
            .iter()
            .map(|f| (BlobId::hash_of(&f.bytes), f.bytes.len() as u64))
            .collect();
        b.manifest_with(asset, &resolved)
    }

    /// A prop with the full derived PBR set: render mesh + base color +
    /// normal + ORM + thumbnail. Rights are the explicit constructor input.
    fn bundle() -> PublishBundle {
        let mut b = PublishBundle::new(
            "gen",
            AssetKind::Prop,
            "Derived crate",
            vec![
                bundle_file(FileRole::RenderGlb, MediaType::Glb, 0x11, 8_000),
                bundle_file(FileRole::Albedo, MediaType::Png, 0x22, 4_000),
                bundle_file(FileRole::Normal, MediaType::Png, 0x33, 4_000),
                bundle_file(FileRole::Orm, MediaType::Png, 0x44, 2_000),
            ],
            thumbnail(),
            PublishRights::generated_cc0(),
        );
        b.stats = PublishStats { triangles: 12, vertices: 8, joints: 0, clips: 0 };
        b
    }

    #[test]
    fn missing_or_dishonest_rights_refuse_before_any_byte_moves() {
        // No license is an ERROR, never a silent CC0.
        let mut b = bundle();
        b.rights.license = String::new();
        assert_eq!(
            b.validate().unwrap_err(),
            ClientError::InvalidInput { what: "publish rights license missing" }
        );
        let mut b = bundle();
        b.rights.license = "   ".to_string();
        assert!(b.validate().is_err(), "whitespace license refused");
        // Attribution-required policies without a credits line refuse.
        let mut b = bundle();
        b.rights = PublishRights::declared(
            "CC-BY-4.0",
            "",
            "https://example.com/pack",
            Redistribution::AttributionRequired,
            DerivativePolicy::Allowed,
        );
        assert_eq!(
            b.validate().unwrap_err(),
            ClientError::InvalidInput { what: "publish rights credits required for attribution" }
        );
        // Control characters and over-budget identifiers refuse.
        let mut b = bundle();
        b.rights.credits = "a\u{7}b".to_string();
        assert!(b.validate().is_err());
        let mut b = bundle();
        b.rights.license = "x".repeat(MAX_LICENSE_BYTES + 1);
        assert!(b.validate().is_err());
        // The single-file path enforces the same law.
        let mut r = request();
        r.rights.license = String::new();
        assert!(r.validate().is_err());
        // And the named generated grant is exactly CC0 + free policies.
        let cc0 = PublishRights::generated_cc0();
        assert_eq!(cc0.license, "CC0-1.0");
        assert_eq!(cc0.redistribution, Redistribution::Allowed);
        assert_eq!(cc0.derivatives, DerivativePolicy::Allowed);
        assert!(cc0.validate().is_ok());
    }

    #[test]
    fn bundle_validation_refuses_bad_inputs_locally() {
        assert!(bundle().validate().is_ok());
        // Duplicate (role, tier, lod) slot.
        let mut b = bundle();
        b.files.push(bundle_file(FileRole::Albedo, MediaType::Png, 0x55, 100));
        assert_eq!(
            b.validate().unwrap_err(),
            ClientError::InvalidInput { what: "publish bundle duplicate slot" }
        );
        // Same role at another LOD is a different slot: legal.
        let mut b = bundle();
        let mut lod1 = bundle_file(FileRole::Lod1Glb, MediaType::Glb, 0x66, 3_000);
        lod1.lod = 1;
        b.files.push(lod1);
        assert!(b.validate().is_ok());
        // Image without dims / dims on non-image / zero dims.
        let mut b = bundle();
        b.files[1].dims = None;
        assert!(b.validate().is_err());
        let mut b = bundle();
        b.files[0].dims = Some((10, 10));
        assert!(b.validate().is_err());
        let mut b = bundle();
        b.files[1].dims = Some((0, 512));
        assert!(b.validate().is_err());
        // Role/media disagreement (audio bytes under a GLB role).
        let mut b = bundle();
        b.files[0].media = MediaType::Wav;
        assert!(b.validate().is_err());
        // Empty file list, empty file, LOD over cap, thumbnail floor.
        let mut b = bundle();
        b.files.clear();
        assert!(b.validate().is_err());
        let mut b = bundle();
        b.files[2].bytes.clear();
        assert!(b.validate().is_err());
        let mut b = bundle();
        b.files[3].lod = MAX_LOD + 1;
        assert!(b.validate().is_err());
        let mut b = bundle();
        b.thumbnail.width = 100;
        assert!(b.validate().is_err());
        // Over the per-asset file-count budget: unique slots via tier × lod.
        let mut b = bundle();
        b.files.clear();
        let tiers = [DeviceTier::Any, DeviceTier::Low, DeviceTier::Medium, DeviceTier::High];
        'fill: for role in [FileRole::Texture, FileRole::Albedo, FileRole::Normal] {
            for tier in tiers {
                for lod in 0..=MAX_LOD {
                    if b.files.len() > MAX_FILES_PER_ASSET {
                        break 'fill;
                    }
                    let mut f = bundle_file(role, MediaType::Png, 0x77, 16);
                    f.tier = tier;
                    f.lod = lod;
                    b.files.push(f);
                }
            }
        }
        assert!(b.files.len() > MAX_FILES_PER_ASSET);
        assert!(matches!(
            b.validate().unwrap_err(),
            ClientError::OverBudget { what: "publish bundle file count", .. }
        ));
    }

    #[test]
    fn bundle_manifest_is_deterministic_and_order_independent() {
        let b = bundle();
        let asset = AssetId::from_bytes([3; 16]);
        let (bytes_a, rev_a, refs_a) = manifest_of(&b, asset).unwrap();
        let (bytes_b, rev_b, _) = manifest_of(&b, asset).unwrap();
        assert_eq!(bytes_a, bytes_b);
        assert_eq!(rev_a, rev_b);
        // Caller file order does not change the identity…
        let mut shuffled = b.clone();
        shuffled.files.reverse();
        let (bytes_c, rev_c, refs_c) = manifest_of(&shuffled, asset).unwrap();
        assert_eq!(bytes_a, bytes_c);
        assert_eq!(rev_a, rev_c);
        // …while the returned refs stay in the caller's order.
        assert_eq!(refs_a.first().map(|r| r.role), Some(FileRole::RenderGlb));
        assert_eq!(refs_c.first().map(|r| r.role), Some(FileRole::Orm));
        // Any byte change is a different revision.
        let mut changed = b.clone();
        changed.files[3].bytes[0] ^= 0xff;
        let (_, rev_d, _) = manifest_of(&changed, asset).unwrap();
        assert_ne!(rev_a, rev_d);
    }

    #[test]
    fn bundle_manifest_round_trips_every_slot_and_metric() {
        let mut b = bundle();
        b.dependencies = vec![AssetRevisionRef {
            asset_id: AssetId::from_bytes([9; 16]),
            revision: AssetRevisionId::from_bytes([8; 32]),
        }];
        b.media_millis = 0;
        // Imported-licensed-content contract: the COMPLETE terms (id +
        // revision qualifier + pinned digests/URL + policies) travel inside
        // the immutable identity, independent of the annotation fields.
        b.rights = PublishRights {
            license: "CC-BY-4.0".to_string(),
            license_revision: "2013-11-25".to_string(),
            terms_digest: Some([0xAA; 32]),
            terms_url: "https://creativecommons.org/licenses/by/4.0/legalcode".to_string(),
            credits: "Kenney (kenney.nl)".to_string(),
            source: "https://kenney.nl/assets/space-kit".to_string(),
            source_archive: Some([0xBB; 32]),
            redistribution: Redistribution::AttributionRequired,
            derivatives: DerivativePolicy::AttributionRequired,
        };
        b.creator = "importer-bot".to_string();
        b.generator = "asset-worker".to_string();
        b.manifest_provenance = Some(PublishProvenance {
            generator: "makepad-asset-importer".into(),
            model: "scripted-pbr".into(),
            version: "0.1".into(),
            seed: 42,
            parents: vec![AssetRevisionId::from_bytes([8; 32])],
            params_digest: Some([5; 32]),
        });
        let asset = AssetId::from_bytes([4; 16]);
        let (bytes, rev, refs) = manifest_of(&b, asset).unwrap();
        let decoded = AssetManifest::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(decoded.asset_id, asset);
        assert_eq!(decoded.kind, AssetKind::Prop);
        // Canonical slot order (role, tier, lod) with exact media and blobs.
        let slots: Vec<_> = decoded
            .files
            .iter()
            .map(|f| (f.role, f.tier, f.lod, f.media, f.byte_len))
            .collect();
        assert_eq!(
            slots,
            vec![
                (FileRole::RenderGlb, DeviceTier::Any, 0, MediaType::Glb, 8_000),
                (FileRole::Albedo, DeviceTier::Any, 0, MediaType::Png, 4_000),
                (FileRole::Normal, DeviceTier::Any, 0, MediaType::Png, 4_000),
                (FileRole::Orm, DeviceTier::Any, 0, MediaType::Png, 2_000),
            ]
        );
        for decoded_file in &decoded.files {
            let r = refs
                .iter()
                .find(|r| (r.role, r.tier, r.lod) == (decoded_file.role, decoded_file.tier, decoded_file.lod))
                .expect("every slot has a returned ref");
            assert_eq!(r.blob, decoded_file.blob);
            assert_eq!(r.byte_len, decoded_file.byte_len);
        }
        let thumb = decoded.thumbnail.expect("mandatory thumbnail");
        assert_eq!((thumb.width, thumb.height), (512, 512));
        assert_eq!(
            decoded.metrics.total_bytes,
            8_000 + 4_000 + 4_000 + 2_000 + thumb.byte_len
        );
        assert_eq!(decoded.metrics.max_texture_dim, 512);
        assert_eq!(decoded.metrics.triangles, 12);
        assert_eq!(decoded.dependencies.len(), 1);
        let prov = decoded.provenance.expect("typed provenance");
        assert_eq!(prov.seed, 42);
        assert_eq!(prov.parents, vec![AssetRevisionId::from_bytes([8; 32])]);
        // The COMPLETE typed rights record round-trips losslessly — and it
        // is the explicit declaration, not the annotation text.
        assert_eq!(PublishRights::from_manifest(&decoded.rights), b.rights);
        assert_eq!(decoded.rights.license, "CC-BY-4.0");
        assert_eq!(decoded.rights.license_revision, "2013-11-25");
        assert_eq!(decoded.rights.terms_digest, Some([0xAA; 32]));
        assert_eq!(decoded.rights.credits, "Kenney (kenney.nl)");
        assert_eq!(decoded.rights.source, "https://kenney.nl/assets/space-kit");
        assert_eq!(decoded.rights.source_archive, Some([0xBB; 32]));
        assert_eq!(decoded.rights.redistribution, Redistribution::AttributionRequired);
        assert_eq!(decoded.rights.derivatives, DerivativePolicy::AttributionRequired);
        // The revision is the digest of the canonical bytes.
        assert_eq!(rev, AssetRevisionId::hash_of(&bytes));
    }

    #[test]
    fn bundle_kind_contracts_still_fail_closed_at_manifest() {
        // A mesh-bearing kind without its render mesh refuses at the
        // authoritative contract validation, not silently.
        let mut b = bundle();
        b.files.retain(|f| f.role != FileRole::RenderGlb);
        assert!(b.validate().is_ok(), "local checks are shape-only");
        assert!(matches!(
            manifest_of(&b, AssetId::from_bytes([1; 16])).unwrap_err(),
            ClientError::Content(_)
        ));
        // Mesh metrics must be measured, never zero.
        let mut b = bundle();
        b.stats.triangles = 0;
        assert!(manifest_of(&b, AssetId::from_bytes([1; 16])).is_err());
    }

    #[test]
    fn publish_stages_render_note_safe() {
        let stages = [
            PublishStage::Validating,
            PublishStage::RegisteringAsset,
            PublishStage::UploadingBlob { index: 3, of: 12, bytes: 123_456_789 },
            PublishStage::Annotating,
            PublishStage::Staging,
            PublishStage::Publishing,
            PublishStage::SettingAlias,
            PublishStage::Complete,
        ];
        for stage in stages {
            let text = stage.to_string();
            assert!(!text.is_empty() && text.len() <= 64, "{text}");
            assert!(!text.chars().any(char::is_control), "{text}");
        }
    }
}
