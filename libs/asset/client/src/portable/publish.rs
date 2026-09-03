//! Portable publication vocabulary and typed refusal for blocking writes.

use crate::client::AssetClient;
use crate::error::{ClientError, ClientResult};
use crate::location::ClientMode;
use makepad_asset_data::{
    Anchor, AssetAlias, AssetFile, AssetId, AssetKind, AssetManifest, AssetRevisionId,
    AssetRevisionRef, Axis, BlobId, Bounds, Capabilities, CoordinateSystem, DerivativePolicy,
    DeviceTier, FileRole, ImageDims, MediaType, Metrics, Pivot, Provenance, Redistribution,
    Rights, ThumbnailMedia, ThumbnailMeta, ThumbnailView, Vec3,
};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishFile {
    pub bytes: Vec<u8>,
    pub media: MediaType,
    pub role: FileRole,
    pub media_millis: u32,
    pub dims: Option<(u32, u32)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublishStats {
    pub triangles: u32,
    pub vertices: u32,
    pub joints: u16,
    pub clips: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishProvenance {
    pub generator: String,
    pub model: String,
    pub version: String,
    pub seed: u64,
    pub parents: Vec<AssetRevisionId>,
    pub params_digest: Option<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PublishThumbnail {
    pub bytes: Vec<u8>,
    pub media: ThumbnailMedia,
    pub width: u32,
    pub height: u32,
    pub views: Vec<ThumbnailView>,
}

impl PublishThumbnail {
    pub fn plain(bytes: Vec<u8>, media: ThumbnailMedia, width: u32, height: u32) -> Self {
        Self { bytes, media, width, height, views: Vec::new() }
    }

    pub fn with_views(mut self, views: Vec<ThumbnailView>) -> Self {
        self.views = views;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishRights {
    pub license: String,
    pub license_revision: String,
    pub terms_digest: Option<[u8; 32]>,
    pub terms_url: String,
    pub credits: String,
    pub source: String,
    pub source_archive: Option<[u8; 32]>,
    pub redistribution: Redistribution,
    pub derivatives: DerivativePolicy,
}

impl PublishRights {
    pub fn declared(
        license: impl Into<String>,
        credits: impl Into<String>,
        source: impl Into<String>,
        redistribution: Redistribution,
        derivatives: DerivativePolicy,
    ) -> Self {
        Self {
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

    pub fn generated_cc0() -> Self {
        Self::declared(
            "CC0-1.0",
            "",
            "",
            Redistribution::Allowed,
            DerivativePolicy::Allowed,
        )
    }

    pub fn from_manifest(rights: &Rights) -> Self {
        Self {
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
    pub alias: Option<AssetAlias>,
    pub asset_id: Option<AssetId>,
    pub artifact: PublishFile,
    pub thumbnail: PublishThumbnail,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub creator: String,
    pub artist: String,
    pub artist_url: String,
    pub album: String,
    pub source_url: String,
    pub license: String,
    pub license_url: String,
    pub generator: String,
    pub backend: String,
    pub model: String,
    pub prompt: String,
    pub provenance: String,
    pub rights: PublishRights,
    pub private: bool,
    pub stats: PublishStats,
    pub manifest_provenance: Option<PublishProvenance>,
}

impl PublishRequest {
    pub fn new(
        namespace: impl Into<String>,
        kind: AssetKind,
        title: impl Into<String>,
        artifact: PublishFile,
        thumbnail: PublishThumbnail,
    ) -> Self {
        Self {
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
            artist: String::new(),
            artist_url: String::new(),
            album: String::new(),
            source_url: String::new(),
            license: String::new(),
            license_url: String::new(),
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

    /// Build the same canonical immutable revision the native publisher
    /// sends to a socket store. Browser-local services use this transport-
    /// free half, then commit it through the embedded store.
    pub fn manifest_for_asset(
        &self,
        asset_id: AssetId,
    ) -> ClientResult<(Vec<u8>, AssetRevisionId)> {
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
                dims: self.artifact.dims.map(|(width, height)| ImageDims { width, height }),
            }],
            dependencies: Vec::new(),
            thumbnail: Some(ThumbnailMeta {
                blob: BlobId::hash_of(&self.thumbnail.bytes),
                media: self.thumbnail.media,
                width: self.thumbnail.width,
                height: self.thumbnail.height,
                byte_len: self.thumbnail.bytes.len() as u64,
                views: self.thumbnail.views.clone(),
            }),
            metrics: Metrics {
                total_bytes: (self.artifact.bytes.len() + self.thumbnail.bytes.len()) as u64,
                triangles: self.stats.triangles,
                vertices: self.stats.vertices,
                joints: self.stats.joints,
                clips: self.stats.clips,
                max_texture_dim: self
                    .thumbnail
                    .width
                    .max(self.thumbnail.height)
                    .max(self.artifact.dims.map_or(0, |(width, height)| width.max(height))),
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
            anchors: Vec::new(),
            capabilities: Capabilities {
                loopable: matches!(self.kind, AssetKind::Audio | AssetKind::Video),
                ..Capabilities::default()
            },
            spawn_recipe: None,
            provenance: self.manifest_provenance.as_ref().map(|value| Provenance {
                generator: value.generator.clone(),
                model: value.model.clone(),
                version: value.version.clone(),
                seed: value.seed,
                parents: value.parents.clone(),
                params_digest: value.params_digest,
            }),
            rights: self.rights.as_manifest_rights(),
        };
        manifest.canonicalize();
        manifest.validate().map_err(ClientError::Content)?;
        let bytes = manifest.to_canonical_bytes().map_err(ClientError::Content)?;
        let revision = manifest.revision().map_err(ClientError::Content)?;
        Ok((bytes, revision))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
    pub alias: Option<AssetAlias>,
    pub artifact_blob: BlobId,
    pub thumbnail_blob: BlobId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishBundleFile {
    pub role: FileRole,
    pub tier: DeviceTier,
    pub lod: u8,
    pub media: MediaType,
    pub bytes: Vec<u8>,
    pub reference: Option<PathBuf>,
    pub dims: Option<(u32, u32)>,
}

impl PublishBundleFile {
    pub fn bytes(
        role: FileRole,
        media: MediaType,
        bytes: Vec<u8>,
        dims: Option<(u32, u32)>,
    ) -> Self {
        Self { role, tier: DeviceTier::Any, lod: 0, media, bytes, reference: None, dims }
    }

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

#[derive(Clone, Debug, PartialEq)]
pub struct PublishBundle {
    pub namespace: String,
    pub kind: AssetKind,
    pub title: String,
    pub description: String,
    pub alias: Option<AssetAlias>,
    pub asset_id: Option<AssetId>,
    pub files: Vec<PublishBundleFile>,
    pub thumbnail: PublishThumbnail,
    pub dependencies: Vec<AssetRevisionRef>,
    pub bounds: Bounds,
    pub coordinate_system: CoordinateSystem,
    pub anchors: Vec<Anchor>,
    pub capabilities: Capabilities,
    pub media_millis: u32,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub creator: String,
    pub artist: String,
    pub artist_url: String,
    pub album: String,
    pub source_url: String,
    pub license: String,
    pub license_url: String,
    pub generator: String,
    pub backend: String,
    pub model: String,
    pub prompt: String,
    pub provenance: String,
    pub rights: PublishRights,
    pub private: bool,
    pub stats: PublishStats,
    pub manifest_provenance: Option<PublishProvenance>,
}

impl PublishBundle {
    pub fn new(
        namespace: impl Into<String>,
        kind: AssetKind,
        title: impl Into<String>,
        files: Vec<PublishBundleFile>,
        thumbnail: PublishThumbnail,
        rights: PublishRights,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            kind,
            title: title.into(),
            description: String::new(),
            alias: None,
            asset_id: None,
            files,
            thumbnail,
            dependencies: Vec::new(),
            bounds: Bounds { min: Vec3::new(-0.5, -0.5, -0.5), max: Vec3::new(0.5, 0.5, 0.5) },
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
            artist: String::new(),
            artist_url: String::new(),
            album: String::new(),
            source_url: String::new(),
            license: String::new(),
            license_url: String::new(),
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishStage {
    Validating,
    RegisteringAsset,
    UploadingBlob { index: usize, of: usize, bytes: u64 },
    ReferencingFile { index: usize, of: usize },
    Annotating,
    Staging,
    Publishing,
    SettingAlias,
    Complete,
}

impl std::fmt::Display for PublishStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validating => write!(f, "validating"),
            Self::RegisteringAsset => write!(f, "registering-asset"),
            Self::UploadingBlob { index, of, bytes } => {
                write!(f, "uploading-blob {index}/{of} ({bytes} bytes)")
            }
            Self::ReferencingFile { index, of } => write!(f, "referencing-file {index}/{of}"),
            Self::Annotating => write!(f, "annotating"),
            Self::Staging => write!(f, "staging"),
            Self::Publishing => write!(f, "publishing"),
            Self::SettingAlias => write!(f, "setting-alias"),
            Self::Complete => write!(f, "complete"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedFile {
    pub role: FileRole,
    pub tier: DeviceTier,
    pub lod: u8,
    pub blob: BlobId,
    pub byte_len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedBundle {
    pub asset_id: AssetId,
    pub revision: AssetRevisionId,
    pub alias: Option<AssetAlias>,
    pub files: Vec<PublishedFile>,
    pub thumbnail_blob: BlobId,
}

impl AssetClient {
    fn publish_unavailable<T>(&self) -> ClientResult<T> {
        Err(ClientError::Unavailable {
            capability: "publish",
            mode: ClientMode::StaticWeb,
        })
    }

    pub fn publish_artifact(&mut self, _request: &PublishRequest) -> ClientResult<Published> {
        self.publish_unavailable()
    }

    pub fn publish_bundle(&mut self, _request: &PublishBundle) -> ClientResult<PublishedBundle> {
        self.publish_unavailable()
    }

    pub fn publish_bundles(
        &mut self,
        _requests: &[PublishBundle],
    ) -> ClientResult<Vec<PublishedBundle>> {
        self.publish_unavailable()
    }

    pub fn publish_bundles_with(
        &mut self,
        _requests: &[PublishBundle],
        _progress: Option<&mut dyn FnMut(&PublishStage)>,
        _abort: &dyn Fn() -> bool,
    ) -> ClientResult<Vec<PublishedBundle>> {
        self.publish_unavailable()
    }

    pub fn publish_bundle_with(
        &mut self,
        _request: &PublishBundle,
        _progress: Option<&mut dyn FnMut(&PublishStage)>,
        _abort: &dyn Fn() -> bool,
    ) -> ClientResult<PublishedBundle> {
        self.publish_unavailable()
    }
}
