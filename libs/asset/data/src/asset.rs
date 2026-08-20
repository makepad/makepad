//! The immutable asset revision manifest.
//!
//! An asset is a manifest, not a file: typed blob roles, exact dependencies,
//! measured metrics, coordinate system, bounds, anchors, capabilities, an
//! optional data-only spawn recipe, provenance, and rights. Changing anything
//! here creates a new revision; published revisions are never edited in place.
//!
//! Mutable catalog text (title, tags, lifecycle, ACLs, alias heads) is
//! deliberately NOT here — it lives in control-plane records so it cannot
//! change a runtime identity.

use crate::codec::{canon_enum, check_sorted_unique, dockind, CanonReader, CanonWriter};
use crate::error::AssetDataError;
use crate::geom::Bounds;
use crate::geom::Transform;
use crate::id::{check_name, AssetId, AssetRevisionId, AssetRevisionRef, BlobId};
use crate::limits::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    Mesh,
    Character,
    Weapon,
    Vehicle,
    Prop,
    Texture,
    Material,
    Audio,
    Video,
    Skybox,
    World,
    Prefab,
    /// Camera-facing sprite content (Doom/Quake billboards). Payload is
    /// keyed PNG frames with real alpha — not a mesh. The engine draws the
    /// facing quad; do not bake one into a GLB.
    Billboard,
    /// A playable game: splash source text (`FileRole::Source`, `Text`)
    /// that the sandbox runs. Everything it references (models, audio)
    /// is resolved through the catalog — a game never embeds bytes.
    Game,
}

canon_enum!(AssetKind {
    Mesh = 0,
    Character = 1,
    Weapon = 2,
    Vehicle = 3,
    Prop = 4,
    Texture = 5,
    Material = 6,
    Audio = 7,
    Video = 8,
    Skybox = 9,
    World = 10,
    Prefab = 11,
    Billboard = 12,
    Game = 13,
});

impl AssetKind {
    /// Kinds that carry a 3D mesh and therefore require the mandatory
    /// thumbnail and a render GLB role.
    pub fn has_mesh(self) -> bool {
        matches!(
            self,
            AssetKind::Mesh
                | AssetKind::Character
                | AssetKind::Weapon
                | AssetKind::Vehicle
                | AssetKind::Prop
        )
    }
}

/// Explicit file roles. Derived products (collider, AO mesh, shadow SDF, LODs)
/// are explicit roles, never inferred adjacent filenames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileRole {
    RenderGlb,
    Lod1Glb,
    Lod2Glb,
    Collider,
    AoMesh,
    ShadowSdf,
    Albedo,
    Normal,
    Orm,
    Texture,
    PreviewFront,
    PreviewSide,
    Turntable,
    Audio,
    Video,
    /// Original supplied artifact retained for future derivation.
    Source,
    /// 16-bit grayscale PNG metric depth in millimeters (0 = invalid).
    Depth,
    /// 3D Gaussian splat scene (PLY): the render payload of a splat
    /// `World`. Drawn by the splat renderer, never meshed.
    Splat,
    /// Baked per-asset ambient-occlusion atlas (grayscale PNG) that the
    /// `AoMesh` role's `ao_uv` lane samples. Published beside the render
    /// GLB so a game streams the bake with the model instead of re-baking.
    AoTexture,
}

canon_enum!(FileRole {
    RenderGlb = 0,
    Lod1Glb = 1,
    Lod2Glb = 2,
    Collider = 3,
    AoMesh = 4,
    ShadowSdf = 5,
    Albedo = 6,
    Normal = 7,
    Orm = 8,
    Texture = 9,
    PreviewFront = 10,
    PreviewSide = 11,
    Turntable = 12,
    Audio = 13,
    Video = 14,
    Source = 15,
    Depth = 16,
    Splat = 17,
    AoTexture = 18,
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MediaType {
    Png,
    Jpeg,
    Glb,
    Wav,
    Ogg,
    Mp4,
    /// Opaque validated binary (collider data, SDF, KTX-style textures).
    Bin,
    /// Validated UTF-8 text (for retained sources only).
    Text,
    /// Gaussian-splat point cloud (`ply` header; ascii or binary body).
    Ply,
    /// MPEG-1/2/2.5 Layer III audio (the music library's native container).
    Mp3,
}

canon_enum!(MediaType {
    Png = 0,
    Jpeg = 1,
    Glb = 2,
    Wav = 3,
    Ogg = 4,
    Mp4 = 5,
    Bin = 6,
    Text = 7,
    Ply = 8,
    Mp3 = 9,
});

impl FileRole {
    /// Media types a role may legally carry. The server sniffs actual bytes;
    /// this is the contract-level agreement both sides enforce.
    pub fn allows(self, media: MediaType) -> bool {
        use FileRole::*;
        use MediaType::*;
        match self {
            RenderGlb | Lod1Glb | Lod2Glb => matches!(media, Glb),
            Collider | AoMesh | ShadowSdf => matches!(media, Bin | Glb),
            Albedo | Normal | Orm | Texture => matches!(media, Png | Jpeg | Bin),
            PreviewFront | PreviewSide => matches!(media, Png | Jpeg),
            Turntable | Video => matches!(media, Mp4),
            Audio => matches!(media, Wav | Ogg | Mp3),
            Source => true,
            Depth => matches!(media, Png),
            Splat => matches!(media, Ply),
            AoTexture => matches!(media, Png),
        }
    }
}

/// Intended device tier for one file variant. `Any` serves every tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeviceTier {
    Any,
    Low,
    Medium,
    High,
}

canon_enum!(DeviceTier {
    Any = 0,
    Low = 1,
    Medium = 2,
    High = 3,
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageDims {
    pub width: u32,
    pub height: u32,
}

/// One typed blob role plus its device/quality variant coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssetFile {
    pub role: FileRole,
    pub tier: DeviceTier,
    /// LOD index within the role, `0..=MAX_LOD`.
    pub lod: u8,
    pub media: MediaType,
    pub blob: BlobId,
    pub byte_len: u64,
    /// Required for PNG/JPEG files, absent otherwise.
    pub dims: Option<ImageDims>,
}

impl AssetFile {
    pub(crate) fn sort_key(&self) -> (u8, u8, u8) {
        (self.role.tag(), self.tier.tag(), self.lod)
    }

    pub(crate) fn validate(&self) -> Result<(), AssetDataError> {
        if !self.role.allows(self.media) {
            return Err(AssetDataError::Mismatch {
                what: "file role does not allow media type",
            });
        }
        if self.lod > MAX_LOD {
            return Err(AssetDataError::OverBudget {
                what: "file lod",
                limit: MAX_LOD as u64,
                found: self.lod as u64,
            });
        }
        if self.byte_len == 0 {
            return Err(AssetDataError::Malformed { what: "file byte_len" });
        }
        if self.byte_len > MAX_FILE_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "file byte_len",
                limit: MAX_FILE_BYTES,
                found: self.byte_len,
            });
        }
        let is_image = matches!(self.media, MediaType::Png | MediaType::Jpeg);
        match (is_image, self.dims) {
            (true, None) => return Err(AssetDataError::Missing { what: "image dims" }),
            (false, Some(_)) => {
                return Err(AssetDataError::Mismatch {
                    what: "dims on non-image media",
                })
            }
            (true, Some(d)) => {
                if d.width == 0 || d.height == 0 {
                    return Err(AssetDataError::Malformed { what: "image dims" });
                }
                if d.width > MAX_TEXTURE_DIM || d.height > MAX_TEXTURE_DIM {
                    return Err(AssetDataError::OverBudget {
                        what: "image dims",
                        limit: MAX_TEXTURE_DIM as u64,
                        found: d.width.max(d.height) as u64,
                    });
                }
            }
            (false, None) => {}
        }
        Ok(())
    }

    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        w.u8(self.role.tag());
        w.u8(self.tier.tag());
        w.u8(self.lod);
        w.u8(self.media.tag());
        self.blob.encode(w);
        w.u64(self.byte_len);
        match self.dims {
            None => w.bool(false),
            Some(d) => {
                w.bool(true);
                w.u32(d.width);
                w.u32(d.height);
            }
        }
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            role: FileRole::decode(r)?,
            tier: DeviceTier::decode(r)?,
            lod: r.u8("file lod")?,
            media: MediaType::decode(r)?,
            blob: BlobId::decode(r)?,
            byte_len: r.u64("file byte_len")?,
            dims: if r.bool("file dims present")? {
                Some(ImageDims {
                    width: r.u32("image dims")?,
                    height: r.u32("image dims")?,
                })
            } else {
                None
            },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailMedia {
    Png,
    Jpeg,
}

canon_enum!(ThumbnailMedia {
    Png = 0,
    Jpeg = 1,
});

/// The mandatory primary thumbnail of every published mesh-bearing asset:
/// its own content-addressed blob, PNG or JPEG, at least 256x256, stored
/// outside the GLB so search results can draw a grid without fetching meshes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbnailMeta {
    pub blob: BlobId,
    pub media: ThumbnailMedia,
    pub width: u32,
    pub height: u32,
    pub byte_len: u64,
}

impl ThumbnailMeta {
    pub fn validate(&self) -> Result<(), AssetDataError> {
        for (what, dim) in [("thumbnail width", self.width), ("thumbnail height", self.height)] {
            if dim < THUMBNAIL_MIN_DIM {
                return Err(AssetDataError::TooSmall {
                    what,
                    min: THUMBNAIL_MIN_DIM as u64,
                    found: dim as u64,
                });
            }
            if dim > THUMBNAIL_MAX_DIM {
                return Err(AssetDataError::OverBudget {
                    what,
                    limit: THUMBNAIL_MAX_DIM as u64,
                    found: dim as u64,
                });
            }
        }
        if self.byte_len == 0 {
            return Err(AssetDataError::Malformed {
                what: "thumbnail byte_len",
            });
        }
        if self.byte_len > MAX_FILE_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "thumbnail byte_len",
                limit: MAX_FILE_BYTES,
                found: self.byte_len,
            });
        }
        Ok(())
    }

    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        self.blob.encode(w);
        w.u8(self.media.tag());
        w.u32(self.width);
        w.u32(self.height);
        w.u64(self.byte_len);
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            blob: BlobId::decode(r)?,
            media: ThumbnailMedia::decode(r)?,
            width: r.u32("thumbnail width")?,
            height: r.u32("thumbnail height")?,
            byte_len: r.u64("thumbnail byte_len")?,
        })
    }
}

/// Server-measured metrics. Validators measure them at admission; clients
/// recheck against the same ceilings before allocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    /// Sum of all file byte lengths including the thumbnail.
    pub total_bytes: u64,
    pub triangles: u32,
    pub vertices: u32,
    pub joints: u16,
    pub clips: u16,
    pub max_texture_dim: u32,
    pub media_millis: u32,
}

impl Metrics {
    pub(crate) fn validate(&self) -> Result<(), AssetDataError> {
        let checks: [(&'static str, u64, u64); 7] = [
            ("metrics total_bytes", self.total_bytes, MAX_ASSET_BYTES),
            ("metrics triangles", self.triangles as u64, MAX_TRIANGLES as u64),
            ("metrics vertices", self.vertices as u64, MAX_VERTICES as u64),
            ("metrics joints", self.joints as u64, MAX_JOINTS as u64),
            ("metrics clips", self.clips as u64, MAX_CLIPS as u64),
            (
                "metrics max_texture_dim",
                self.max_texture_dim as u64,
                MAX_TEXTURE_DIM as u64,
            ),
            (
                "metrics media_millis",
                self.media_millis as u64,
                MAX_MEDIA_MILLIS as u64,
            ),
        ];
        for (what, found, limit) in checks {
            if found > limit {
                return Err(AssetDataError::OverBudget { what, limit, found });
            }
        }
        Ok(())
    }

    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        w.u64(self.total_bytes);
        w.u32(self.triangles);
        w.u32(self.vertices);
        w.u16(self.joints);
        w.u16(self.clips);
        w.u32(self.max_texture_dim);
        w.u32(self.media_millis);
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            total_bytes: r.u64("metrics")?,
            triangles: r.u32("metrics")?,
            vertices: r.u32("metrics")?,
            joints: r.u16("metrics")?,
            clips: r.u16("metrics")?,
            max_texture_dim: r.u32("metrics")?,
            media_millis: r.u32("metrics")?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    XPos,
    XNeg,
    YPos,
    YNeg,
    ZPos,
    ZNeg,
}

canon_enum!(Axis {
    XPos = 0,
    XNeg = 1,
    YPos = 2,
    YNeg = 3,
    ZPos = 4,
    ZNeg = 5,
});

impl Axis {
    fn line(self) -> u8 {
        self.tag() / 2
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pivot {
    Origin,
    BoundsCenter,
    BoundsBottom,
}

canon_enum!(Pivot {
    Origin = 0,
    BoundsCenter = 1,
    BoundsBottom = 2,
});

/// Units, axes, and pivot policy. Physics and anchors are interpreted in this
/// system on every device — it is part of the locked identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoordinateSystem {
    pub units_per_meter: f32,
    pub up: Axis,
    pub forward: Axis,
    pub pivot: Pivot,
}

impl CoordinateSystem {
    fn validate(&self) -> Result<(), AssetDataError> {
        if !self.units_per_meter.is_finite()
            || self.units_per_meter <= 0.0
            || self.units_per_meter > MAX_UNITS_PER_METER
        {
            return Err(AssetDataError::Malformed {
                what: "units_per_meter",
            });
        }
        if self.up.line() == self.forward.line() {
            return Err(AssetDataError::Mismatch {
                what: "up and forward on the same axis",
            });
        }
        Ok(())
    }

    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        w.f32(self.units_per_meter);
        w.u8(self.up.tag());
        w.u8(self.forward.tag());
        w.u8(self.pivot.tag());
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            units_per_meter: r.f32("units_per_meter")?,
            up: Axis::decode(r)?,
            forward: Axis::decode(r)?,
            pivot: Pivot::decode(r)?,
        })
    }
}

/// A named attachment transform: `weapon_grip`, `weapon_muzzle`, hand sockets,
/// seats. Invariant across visual variants.
#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    pub name: String,
    pub transform: Transform,
}

impl Anchor {
    fn validate(&self) -> Result<(), AssetDataError> {
        check_name(&self.name, "anchor name")?;
        self.transform.validate("anchor transform")
    }
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        w.str(&self.name);
        self.transform.encode(w);
    }
    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            name: r.str("anchor name", MAX_NAME_BYTES)?,
            transform: Transform::decode(r, "anchor transform")?,
        })
    }
}

/// What this asset can do, encoded as a closed bitmask — unknown bits refuse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub rigged: bool,
    pub animated: bool,
    pub collidable: bool,
    pub loopable: bool,
    pub spawnable: bool,
}

impl Capabilities {
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        let bits = (self.rigged as u8)
            | (self.animated as u8) << 1
            | (self.collidable as u8) << 2
            | (self.loopable as u8) << 3
            | (self.spawnable as u8) << 4;
        w.u8(bits);
    }
    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let bits = r.u8("capabilities")?;
        if bits & !0b1_1111 != 0 {
            return Err(AssetDataError::Malformed { what: "capabilities" });
        }
        Ok(Self {
            rigged: bits & 1 != 0,
            animated: bits & 2 != 0,
            collidable: bits & 4 != 0,
            loopable: bits & 8 != 0,
            spawnable: bits & 16 != 0,
        })
    }
}

/// Safe prefab classes the catalog may declare. Damage, fire rate, and
/// permission to spawn remain host-validated game data; uploaded scripts can
/// never become executable prefabs through this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefabClass {
    Prop,
    Weapon,
    Vehicle,
    Character,
    Pickup,
    Emitter,
}

canon_enum!(PrefabClass {
    Prop = 0,
    Weapon = 1,
    Vehicle = 2,
    Character = 3,
    Pickup = 4,
    Emitter = 5,
});

#[derive(Clone, Debug, PartialEq)]
pub struct SpawnParam {
    pub name: String,
    pub value: crate::value::Value,
}

/// Bounded, data-only description of how this asset may be spawned.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnRecipe {
    pub class: PrefabClass,
    pub params: Vec<SpawnParam>,
}

impl SpawnRecipe {
    fn validate(&self) -> Result<(), AssetDataError> {
        if self.params.len() > MAX_SPAWN_PARAMS {
            return Err(AssetDataError::OverBudget {
                what: "spawn params",
                limit: MAX_SPAWN_PARAMS as u64,
                found: self.params.len() as u64,
            });
        }
        for p in &self.params {
            check_name(&p.name, "spawn param name")?;
            p.value.validate("spawn param value")?;
        }
        check_sorted_unique(&self.params, |p| p.name.clone(), "spawn param name")
    }
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        w.u8(self.class.tag());
        w.u32(self.params.len() as u32);
        for p in &self.params {
            w.str(&p.name);
            p.value.encode(w);
        }
    }
    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let class = PrefabClass::decode(r)?;
        let n = r.count("spawn params", MAX_SPAWN_PARAMS)?;
        let mut params = Vec::with_capacity(n);
        for _ in 0..n {
            params.push(SpawnParam {
                name: r.str("spawn param name", MAX_NAME_BYTES)?,
                value: crate::value::Value::decode(r)?,
            });
        }
        Ok(Self { class, params })
    }
}

/// Immutable generation/derivation provenance, kept in the manifest so an
/// export cannot lose it. Raw prompts are private metadata and do not belong
/// here.
#[derive(Clone, Debug, PartialEq)]
pub struct Provenance {
    pub generator: String,
    pub model: String,
    pub version: String,
    pub seed: u64,
    /// Exact parent revisions this was derived from, canonical order.
    pub parents: Vec<AssetRevisionId>,
    /// Recipe digest over tool ID/version, canonical settings, and ordered
    /// input digests — never a filesystem mtime.
    pub params_digest: Option<[u8; 32]>,
}

impl Provenance {
    fn validate(&self) -> Result<(), AssetDataError> {
        for (what, s) in [
            ("provenance generator", &self.generator),
            ("provenance model", &self.model),
            ("provenance version", &self.version),
        ] {
            if s.is_empty() || s.len() > MAX_NAME_BYTES {
                return Err(AssetDataError::Malformed { what });
            }
        }
        if self.parents.len() > MAX_PROVENANCE_PARENTS {
            return Err(AssetDataError::OverBudget {
                what: "provenance parents",
                limit: MAX_PROVENANCE_PARENTS as u64,
                found: self.parents.len() as u64,
            });
        }
        check_sorted_unique(&self.parents, |p| *p, "provenance parent")
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.str(&self.generator);
        w.str(&self.model);
        w.str(&self.version);
        w.u64(self.seed);
        w.u32(self.parents.len() as u32);
        for p in &self.parents {
            p.encode(w);
        }
        match &self.params_digest {
            None => w.bool(false),
            Some(d) => {
                w.bool(true);
                w.raw32(d);
            }
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        let generator = r.str("provenance generator", MAX_NAME_BYTES)?;
        let model = r.str("provenance model", MAX_NAME_BYTES)?;
        let version = r.str("provenance version", MAX_NAME_BYTES)?;
        let seed = r.u64("provenance seed")?;
        let n = r.count("provenance parents", MAX_PROVENANCE_PARENTS)?;
        let mut parents = Vec::with_capacity(n);
        for _ in 0..n {
            parents.push(AssetRevisionId::decode(r)?);
        }
        let params_digest = if r.bool("provenance params_digest present")? {
            Some(r.raw32("provenance params_digest")?)
        } else {
            None
        };
        Ok(Self {
            generator,
            model,
            version,
            seed,
            parents,
            params_digest,
        })
    }
}

/// Redistribution policy of licensed content. Unknown or forbidden rights
/// fail closed before publication; there is no "assume it is fine" default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redistribution {
    /// Free redistribution (for example CC0).
    Allowed,
    /// Redistribution requires retaining the credits string.
    AttributionRequired,
    /// May not be redistributed: never enters a public catalog or lock.
    Forbidden,
    /// User-owned content that may be served on the owner's own LAN (this
    /// asset server and the clients it discovers) but must never leave it:
    /// no internet-facing catalog, no content-set lock shipped elsewhere,
    /// no peer transfer beyond the LAN. Shareware game data the user holds
    /// a copy of is the canonical case.
    LanLocal,
}

canon_enum!(Redistribution {
    Allowed = 0,
    AttributionRequired = 1,
    Forbidden = 2,
    LanLocal = 3,
});

/// Whether derivatives (AO bakes, LODs, transcodes, remixes) may be produced
/// from this content, and under what condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivativePolicy {
    Allowed,
    /// Derivatives are permitted but must retain the credits string.
    AttributionRequired,
    /// No derivative may be produced; derivation requests fail closed.
    Forbidden,
    /// Derivatives only for local preview/use (thumbnails, AO, LODs,
    /// transcodes served on the owner's LAN); nothing derived may leave
    /// the LAN, same boundary as [`Redistribution::LanLocal`].
    LocalPreview,
}

canon_enum!(DerivativePolicy {
    Allowed = 0,
    AttributionRequired = 1,
    Forbidden = 2,
    LocalPreview = 3,
});

/// The complete rights record of one piece of content: exact license
/// identity, pinned immutable terms, required attribution, upstream
/// provenance, and the source-determined redistribution/derivative policy.
///
/// License is mandatory: content with unknown rights is refused at the
/// boundary, not published quietly. For imported content every field here is
/// AUTHORITATIVELY the registered source collection's terms — an importer
/// can never select laxer terms than the approval records.
#[derive(Clone, Debug, PartialEq)]
pub struct Rights {
    /// Exact license identifier including its version, e.g. `CC-BY-4.0`.
    pub license: String,
    /// Terms revision qualifier when the identifier alone is not exact
    /// (publisher EULA date, custom terms revision). May be empty.
    pub license_revision: String,
    /// SHA-256 of the immutable license terms text, when pinned.
    pub terms_digest: Option<[u8; 32]>,
    /// URL of the immutable terms document. May be empty.
    pub terms_url: String,
    /// Required attribution/credits line. Mandatory whenever redistribution
    /// or derivatives require attribution.
    pub credits: String,
    /// Upstream origin of the content: URL/repo/locator of the ORIGINAL
    /// source. Internal collection/pack locators live in provenance and the
    /// import record — they never overwrite this.
    pub source: String,
    /// SHA-256 of the exact upstream archive/revision the bytes came from,
    /// when pinned.
    pub source_archive: Option<[u8; 32]>,
    pub redistribution: Redistribution,
    pub derivatives: DerivativePolicy,
}

impl Rights {
    pub(crate) fn validate(&self) -> Result<(), AssetDataError> {
        if self.license.is_empty() {
            return Err(AssetDataError::Missing { what: "license" });
        }
        if self.license.len() > MAX_LICENSE_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "rights license",
                limit: MAX_LICENSE_BYTES as u64,
                found: self.license.len() as u64,
            });
        }
        if self.license_revision.len() > MAX_LICENSE_REVISION_BYTES {
            return Err(AssetDataError::OverBudget {
                what: "rights license_revision",
                limit: MAX_LICENSE_REVISION_BYTES as u64,
                found: self.license_revision.len() as u64,
            });
        }
        for (what, s) in [
            ("rights terms_url", &self.terms_url),
            ("rights credits", &self.credits),
            ("rights source", &self.source),
        ] {
            if s.len() > MAX_STRING_BYTES {
                return Err(AssetDataError::OverBudget {
                    what,
                    limit: MAX_STRING_BYTES as u64,
                    found: s.len() as u64,
                });
            }
        }
        // Attribution without a credits line cannot be honored by anyone.
        if (self.redistribution == Redistribution::AttributionRequired
            || self.derivatives == DerivativePolicy::AttributionRequired)
            && self.credits.is_empty()
        {
            return Err(AssetDataError::Missing {
                what: "credits for attribution-required rights",
            });
        }
        Ok(())
    }
    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        w.str(&self.license);
        w.str(&self.license_revision);
        match &self.terms_digest {
            None => w.bool(false),
            Some(d) => {
                w.bool(true);
                w.raw32(d);
            }
        }
        w.str(&self.terms_url);
        w.str(&self.credits);
        w.str(&self.source);
        match &self.source_archive {
            None => w.bool(false),
            Some(d) => {
                w.bool(true);
                w.raw32(d);
            }
        }
        w.u8(self.redistribution.tag());
        w.u8(self.derivatives.tag());
    }
    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            license: r.str("rights license", MAX_LICENSE_BYTES)?,
            license_revision: r.str("rights license_revision", MAX_LICENSE_REVISION_BYTES)?,
            terms_digest: if r.bool("rights terms_digest present")? {
                Some(r.raw32("rights terms_digest")?)
            } else {
                None
            },
            terms_url: r.str("rights terms_url", MAX_STRING_BYTES)?,
            credits: r.str("rights credits", MAX_STRING_BYTES)?,
            source: r.str("rights source", MAX_STRING_BYTES)?,
            source_archive: if r.bool("rights source_archive present")? {
                Some(r.raw32("rights source_archive")?)
            } else {
                None
            },
            redistribution: Redistribution::decode(r)?,
            derivatives: DerivativePolicy::decode(r)?,
        })
    }
}

/// The canonical, versioned asset revision manifest. Its revision ID is the
/// SHA-256 of its canonical bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetManifest {
    pub asset_id: AssetId,
    pub kind: AssetKind,
    /// Typed blob roles, canonical order `(role, tier, lod)`. The primary
    /// thumbnail is NOT in this list — it is the typed field below, so it can
    /// never be absent-by-omission on a mesh.
    pub files: Vec<AssetFile>,
    /// Exact asset revisions, never floating aliases. Canonical order, one
    /// revision per dependency asset.
    pub dependencies: Vec<AssetRevisionRef>,
    /// Mandatory for every mesh-bearing kind.
    pub thumbnail: Option<ThumbnailMeta>,
    pub metrics: Metrics,
    pub coordinate_system: CoordinateSystem,
    pub bounds: Bounds,
    /// Canonical order by name.
    pub anchors: Vec<Anchor>,
    pub capabilities: Capabilities,
    pub spawn_recipe: Option<SpawnRecipe>,
    pub provenance: Option<Provenance>,
    pub rights: Rights,
}

impl AssetManifest {
    /// Sort every repeated field into canonical order. Producers that built
    /// their vectors from maps or in discovery order call this once before
    /// `validate`/`to_canonical_bytes`; duplicates still refuse there.
    pub fn canonicalize(&mut self) {
        self.files.sort_by_key(|f| f.sort_key());
        self.dependencies.sort();
        self.anchors.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(recipe) = &mut self.spawn_recipe {
            recipe.params.sort_by(|a, b| a.name.cmp(&b.name));
        }
        if let Some(p) = &mut self.provenance {
            p.parents.sort();
        }
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        if self.files.len() > MAX_FILES_PER_ASSET {
            return Err(AssetDataError::OverBudget {
                what: "asset files",
                limit: MAX_FILES_PER_ASSET as u64,
                found: self.files.len() as u64,
            });
        }
        if self.dependencies.len() > MAX_DEPENDENCIES_PER_ASSET {
            return Err(AssetDataError::OverBudget {
                what: "asset dependencies",
                limit: MAX_DEPENDENCIES_PER_ASSET as u64,
                found: self.dependencies.len() as u64,
            });
        }
        for f in &self.files {
            f.validate()?;
        }
        check_sorted_unique(&self.files, |f| f.sort_key(), "asset file (role, tier, lod)")?;
        // One exact revision per dependency asset: sorted by the full ref but
        // unique by asset_id, so a manifest cannot pin two revisions of one
        // dependency and let peers resolve differently.
        check_sorted_unique(&self.dependencies, |d| *d, "asset dependency")?;
        check_sorted_unique(
            &self.dependencies,
            |d| d.asset_id,
            "asset dependency asset_id",
        )?;
        for d in &self.dependencies {
            if d.asset_id == self.asset_id {
                return Err(AssetDataError::Malformed {
                    what: "self-dependency",
                });
            }
        }
        if let Some(t) = &self.thumbnail {
            t.validate()?;
        }
        self.metrics.validate()?;
        self.coordinate_system.validate()?;
        self.bounds.validate("asset bounds")?;
        if self.anchors.len() > MAX_ANCHORS_PER_ASSET {
            return Err(AssetDataError::OverBudget {
                what: "asset anchors",
                limit: MAX_ANCHORS_PER_ASSET as u64,
                found: self.anchors.len() as u64,
            });
        }
        for a in &self.anchors {
            a.validate()?;
        }
        check_sorted_unique(&self.anchors, |a| a.name.clone(), "anchor name")?;
        if let Some(recipe) = &self.spawn_recipe {
            recipe.validate()?;
        }
        if let Some(p) = &self.provenance {
            p.validate()?;
        }
        self.rights.validate()?;

        // Spawnability is a promise about a recipe, not a loose flag: a
        // spawnable asset must carry its bounded data-only recipe, and a
        // recipe on a non-spawnable asset is a contradiction. Prefabs exist
        // only to be spawned.
        match (self.capabilities.spawnable, &self.spawn_recipe) {
            (true, None) => {
                return Err(AssetDataError::Missing {
                    what: "spawn recipe on spawnable asset",
                })
            }
            (false, Some(_)) => {
                return Err(AssetDataError::Mismatch {
                    what: "spawn recipe on non-spawnable asset",
                })
            }
            _ => {}
        }
        if self.kind == AssetKind::Prefab && self.spawn_recipe.is_none() {
            return Err(AssetDataError::Missing {
                what: "prefab spawn recipe",
            });
        }

        // Per-kind requirements: every published 3D mesh has a thumbnail and
        // a render mesh; media kinds carry their media role.
        if self.kind.has_mesh() {
            if self.thumbnail.is_none() {
                return Err(AssetDataError::Missing { what: "mesh thumbnail" });
            }
            if !self.files.iter().any(|f| f.role == FileRole::RenderGlb) {
                return Err(AssetDataError::Missing { what: "render_glb role" });
            }
            if self.metrics.triangles == 0 || self.metrics.vertices < 3 {
                return Err(AssetDataError::Malformed { what: "mesh metrics" });
            }
        }
        if self.kind == AssetKind::Audio {
            if !self.files.iter().any(|f| f.role == FileRole::Audio) {
                return Err(AssetDataError::Missing { what: "audio role" });
            }
            if self.metrics.media_millis == 0 {
                return Err(AssetDataError::Malformed { what: "audio metrics" });
            }
        }
        if self.kind == AssetKind::Video && !self.files.iter().any(|f| f.role == FileRole::Video) {
            return Err(AssetDataError::Missing { what: "video role" });
        }
        if self.kind == AssetKind::Game
            && !self
                .files
                .iter()
                .any(|f| f.role == FileRole::Source && f.media == MediaType::Text)
        {
            return Err(AssetDataError::Missing { what: "game source role" });
        }
        if matches!(
            self.kind,
            AssetKind::Texture | AssetKind::Skybox | AssetKind::Billboard
        ) && !self.files.iter().any(|f| {
            matches!(
                f.role,
                FileRole::Texture | FileRole::Albedo | FileRole::Depth
            )
        }) {
            return Err(AssetDataError::Missing { what: "texture role" });
        }

        // The declared total must be exactly the sum of the parts, so a
        // client can budget a download from the manifest alone and detect a
        // lying manifest before fetching anything.
        let sum = self
            .files
            .iter()
            .map(|f| f.byte_len)
            .chain(self.thumbnail.iter().map(|t| t.byte_len))
            .try_fold(0u64, |a, b| a.checked_add(b))
            .ok_or(AssetDataError::Malformed {
                what: "file byte_len sum",
            })?;
        if sum != self.metrics.total_bytes {
            return Err(AssetDataError::Mismatch {
                what: "metrics total_bytes vs file sum",
            });
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::ASSET_MANIFEST);
        self.asset_id.encode(&mut w);
        w.u8(self.kind.tag());
        w.u32(self.files.len() as u32);
        for f in &self.files {
            f.encode(&mut w);
        }
        w.u32(self.dependencies.len() as u32);
        for d in &self.dependencies {
            d.encode(&mut w);
        }
        match &self.thumbnail {
            None => w.bool(false),
            Some(t) => {
                w.bool(true);
                t.encode(&mut w);
            }
        }
        self.metrics.encode(&mut w);
        self.coordinate_system.encode(&mut w);
        self.bounds.encode(&mut w);
        w.u32(self.anchors.len() as u32);
        for a in &self.anchors {
            a.encode(&mut w);
        }
        self.capabilities.encode(&mut w);
        match &self.spawn_recipe {
            None => w.bool(false),
            Some(s) => {
                w.bool(true);
                s.encode(&mut w);
            }
        }
        match &self.provenance {
            None => w.bool(false),
            Some(p) => {
                w.bool(true);
                p.encode(&mut w);
            }
        }
        self.rights.encode(&mut w);
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::ASSET_MANIFEST)?;
        let asset_id = AssetId::decode(&mut r)?;
        let kind = AssetKind::decode(&mut r)?;
        let n = r.count("asset files", MAX_FILES_PER_ASSET)?;
        let mut files = Vec::with_capacity(n);
        for _ in 0..n {
            files.push(AssetFile::decode(&mut r)?);
        }
        let n = r.count("asset dependencies", MAX_DEPENDENCIES_PER_ASSET)?;
        let mut dependencies = Vec::with_capacity(n);
        for _ in 0..n {
            dependencies.push(AssetRevisionRef::decode(&mut r)?);
        }
        let thumbnail = if r.bool("thumbnail present")? {
            Some(ThumbnailMeta::decode(&mut r)?)
        } else {
            None
        };
        let metrics = Metrics::decode(&mut r)?;
        let coordinate_system = CoordinateSystem::decode(&mut r)?;
        let bounds = Bounds::decode(&mut r, "asset bounds")?;
        let n = r.count("asset anchors", MAX_ANCHORS_PER_ASSET)?;
        let mut anchors = Vec::with_capacity(n);
        for _ in 0..n {
            anchors.push(Anchor::decode(&mut r)?);
        }
        let capabilities = Capabilities::decode(&mut r)?;
        let spawn_recipe = if r.bool("spawn recipe present")? {
            Some(SpawnRecipe::decode(&mut r)?)
        } else {
            None
        };
        let provenance = if r.bool("provenance present")? {
            Some(Provenance::decode(&mut r)?)
        } else {
            None
        };
        let rights = Rights::decode(&mut r)?;
        r.finish()?;
        let manifest = Self {
            asset_id,
            kind,
            files,
            dependencies,
            thumbnail,
            metrics,
            coordinate_system,
            bounds,
            anchors,
            capabilities,
            spawn_recipe,
            provenance,
            rights,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// The immutable identity of this exact manifest.
    pub fn revision(&self) -> Result<AssetRevisionId, AssetDataError> {
        Ok(AssetRevisionId::hash_of(&self.to_canonical_bytes()?))
    }

    pub fn revision_ref(&self) -> Result<AssetRevisionRef, AssetDataError> {
        Ok(AssetRevisionRef {
            asset_id: self.asset_id,
            revision: self.revision()?,
        })
    }
}
