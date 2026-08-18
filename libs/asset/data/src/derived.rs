//! Canonical processing recipes, immutable derived variants, frozen variant
//! sets, and deterministic per-client variant resolution.
//!
//! On-demand processing never edits a published `AssetManifest`. It creates an
//! immutable [`DerivedVariantManifest`] tied to one exact base revision and
//! one canonical [`ProcessingRecipe`]; approved results are collected into an
//! immutable [`VariantSetManifest`] that a release lock pins. The derivation
//! cache key ([`derivation_key`]) covers the base revision, recipe digest, and
//! ordered input digests, so identical requests dedupe and a processor
//! upgrade changes the key instead of poisoning an older cache entry.
//!
//! Determinism law: these manifests carry only deterministic content facts.
//! Job IDs, worker identities, attempts, and timestamps are control-plane
//! records — two clean servers that run the same recipe over the same input
//! must produce byte-identical manifests and IDs.

use crate::asset::{AssetFile, FileRole, MediaType, Metrics, Rights, ThumbnailMedia, ThumbnailMeta};
use crate::codec::{canon_enum, check_sorted_unique, dockind, CanonReader, CanonWriter};
use crate::error::AssetDataError;
use crate::id::{
    check_name, AssetRevisionRef, BlobId, ClientProfileDigest, DerivationKey, DerivedVariantId,
    RecipeDigest, ResolvedMapDigest, VariantSetId,
};
use crate::limits::*;
use crate::sha256::Sha256;

/// Version of the deterministic resolution policy implemented by
/// [`resolve_variants`]. A profile or set carrying another version refuses.
pub const RESOLUTION_POLICY_V1: u32 = 1;

/// Version of the derived-output contract. Unknown versions refuse.
pub const OUTPUT_SCHEMA_V1: u32 = 1;

/// The exact processor identity a recipe pins: ID, version, build, and
/// whether its output is deterministic. An upgrade changes the recipe digest.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolClosure {
    pub processor: String,
    pub version: String,
    pub build: String,
    pub deterministic: bool,
}

impl ToolClosure {
    fn validate(&self) -> Result<(), AssetDataError> {
        check_name(&self.processor, "tool processor")?;
        for (what, s) in [("tool version", &self.version), ("tool build", &self.build)] {
            if s.is_empty() || s.len() > MAX_RECIPE_PARAM_BYTES {
                return Err(AssetDataError::Malformed { what });
            }
        }
        // v1 admits only deterministic tools: the derivation key promises
        // that one key names one output, and a nondeterministic tool without
        // a pinned seed/nonce could publish divergent bytes under it. The
        // field stays on the wire so a future version can admit seeded
        // nondeterminism explicitly.
        if !self.deterministic {
            return Err(AssetDataError::Malformed {
                what: "nondeterministic tool closure",
            });
        }
        Ok(())
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.str(&self.processor);
        w.str(&self.version);
        w.str(&self.build);
        w.bool(self.deterministic);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            processor: r.str("tool processor", MAX_NAME_BYTES)?,
            version: r.str("tool version", MAX_RECIPE_PARAM_BYTES)?,
            build: r.str("tool build", MAX_RECIPE_PARAM_BYTES)?,
            deterministic: r.bool("tool deterministic")?,
        })
    }
}

/// Closed set of supported processing kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecipeKind {
    MeshThumbnail,
    MeshLod,
    MeshCollider,
    MeshAo,
    MeshRemesh,
    ImageResize,
    ImageTranscode,
}

canon_enum!(RecipeKind {
    MeshThumbnail = 0,
    MeshLod = 1,
    MeshCollider = 2,
    MeshAo = 3,
    MeshRemesh = 4,
    ImageResize = 5,
    ImageTranscode = 6,
});

impl RecipeKind {
    /// Job kind string for the typed worker queue, `[a-z0-9_.]`.
    pub fn job_kind(self) -> &'static str {
        match self {
            RecipeKind::MeshThumbnail => "derive.mesh_thumbnail",
            RecipeKind::MeshLod => "derive.mesh_lod",
            RecipeKind::MeshCollider => "derive.mesh_collider",
            RecipeKind::MeshAo => "derive.mesh_ao",
            RecipeKind::MeshRemesh => "derive.mesh_remesh",
            RecipeKind::ImageResize => "derive.image_resize",
            RecipeKind::ImageTranscode => "derive.image_transcode",
        }
    }
}

/// Every quality-affecting setting of one recipe kind, typed and closed —
/// there is no free-form parameter bag to smuggle nondeterminism through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecipeSettings {
    /// Render the mandatory-style thumbnail from the base render mesh.
    MeshThumbnail {
        width: u32,
        height: u32,
        media: ThumbnailMedia,
    },
    /// Triangle-budget decimation producing one LOD mesh.
    MeshLod { lod: u8, target_triangles: u32 },
    /// Convex-hull collider generation.
    MeshCollider { max_hull_vertices: u32 },
    /// Ambient-occlusion / shadow data bake.
    MeshAo { resolution: u32 },
    /// Topology-preserving remesh of the render mesh.
    MeshRemesh { target_triangles: u32 },
    /// Resize one image role to exact dimensions.
    ImageResize {
        source_role: FileRole,
        width: u32,
        height: u32,
        media: ThumbnailMedia,
    },
    /// Transcode one image role to another codec. `quality` is 1..=100 for
    /// JPEG and must be 0 for PNG (lossless has no quality knob).
    ImageTranscode {
        source_role: FileRole,
        media: ThumbnailMedia,
        quality: u8,
    },
}

fn image_source_role(role: FileRole) -> Result<(), AssetDataError> {
    match role {
        FileRole::Albedo | FileRole::Normal | FileRole::Orm | FileRole::Texture => Ok(()),
        _ => Err(AssetDataError::Mismatch {
            what: "image recipe source role",
        }),
    }
}

fn check_dim(dim: u32, what: &'static str) -> Result<(), AssetDataError> {
    if dim == 0 {
        return Err(AssetDataError::Malformed { what });
    }
    if dim > MAX_TEXTURE_DIM {
        return Err(AssetDataError::OverBudget {
            what,
            limit: MAX_TEXTURE_DIM as u64,
            found: dim as u64,
        });
    }
    Ok(())
}

impl RecipeSettings {
    pub fn kind(&self) -> RecipeKind {
        match self {
            RecipeSettings::MeshThumbnail { .. } => RecipeKind::MeshThumbnail,
            RecipeSettings::MeshLod { .. } => RecipeKind::MeshLod,
            RecipeSettings::MeshCollider { .. } => RecipeKind::MeshCollider,
            RecipeSettings::MeshAo { .. } => RecipeKind::MeshAo,
            RecipeSettings::MeshRemesh { .. } => RecipeKind::MeshRemesh,
            RecipeSettings::ImageResize { .. } => RecipeKind::ImageResize,
            RecipeSettings::ImageTranscode { .. } => RecipeKind::ImageTranscode,
        }
    }

    /// The single base-revision file role this recipe consumes.
    pub fn input_role(&self) -> FileRole {
        match self {
            RecipeSettings::MeshThumbnail { .. }
            | RecipeSettings::MeshLod { .. }
            | RecipeSettings::MeshCollider { .. }
            | RecipeSettings::MeshAo { .. }
            | RecipeSettings::MeshRemesh { .. } => FileRole::RenderGlb,
            RecipeSettings::ImageResize { source_role, .. }
            | RecipeSettings::ImageTranscode { source_role, .. } => *source_role,
        }
    }

    fn validate(&self) -> Result<(), AssetDataError> {
        match *self {
            RecipeSettings::MeshThumbnail { width, height, .. } => {
                for (what, dim) in [
                    ("recipe thumbnail width", width),
                    ("recipe thumbnail height", height),
                ] {
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
                Ok(())
            }
            RecipeSettings::MeshLod {
                lod,
                target_triangles,
            } => {
                // Only the explicit LOD roles exist; lod maps onto them.
                if lod < 1 || lod > 2 {
                    return Err(AssetDataError::Malformed { what: "recipe lod" });
                }
                if target_triangles < MIN_LOD_TRIANGLES {
                    return Err(AssetDataError::TooSmall {
                        what: "recipe target_triangles",
                        min: MIN_LOD_TRIANGLES as u64,
                        found: target_triangles as u64,
                    });
                }
                if target_triangles > MAX_TRIANGLES {
                    return Err(AssetDataError::OverBudget {
                        what: "recipe target_triangles",
                        limit: MAX_TRIANGLES as u64,
                        found: target_triangles as u64,
                    });
                }
                Ok(())
            }
            RecipeSettings::MeshCollider { max_hull_vertices } => {
                if max_hull_vertices == 0 || max_hull_vertices > MAX_VERTICES {
                    return Err(AssetDataError::Malformed {
                        what: "recipe max_hull_vertices",
                    });
                }
                Ok(())
            }
            RecipeSettings::MeshAo { resolution } => check_dim(resolution, "recipe ao resolution"),
            RecipeSettings::MeshRemesh { target_triangles } => {
                if target_triangles < MIN_LOD_TRIANGLES || target_triangles > MAX_TRIANGLES {
                    return Err(AssetDataError::Malformed {
                        what: "recipe target_triangles",
                    });
                }
                Ok(())
            }
            RecipeSettings::ImageResize {
                source_role,
                width,
                height,
                ..
            } => {
                image_source_role(source_role)?;
                check_dim(width, "recipe image width")?;
                check_dim(height, "recipe image height")
            }
            RecipeSettings::ImageTranscode {
                source_role,
                media,
                quality,
            } => {
                image_source_role(source_role)?;
                match media {
                    ThumbnailMedia::Jpeg => {
                        if quality == 0 || quality > MAX_TRANSCODE_QUALITY {
                            return Err(AssetDataError::Malformed {
                                what: "recipe transcode quality",
                            });
                        }
                    }
                    ThumbnailMedia::Png => {
                        if quality != 0 {
                            return Err(AssetDataError::Mismatch {
                                what: "quality on lossless transcode",
                            });
                        }
                    }
                }
                Ok(())
            }
        }
    }

    fn encode(&self, w: &mut CanonWriter) {
        w.u8(self.kind().tag());
        match *self {
            RecipeSettings::MeshThumbnail {
                width,
                height,
                media,
            } => {
                w.u32(width);
                w.u32(height);
                w.u8(media.tag());
            }
            RecipeSettings::MeshLod {
                lod,
                target_triangles,
            } => {
                w.u8(lod);
                w.u32(target_triangles);
            }
            RecipeSettings::MeshCollider { max_hull_vertices } => w.u32(max_hull_vertices),
            RecipeSettings::MeshAo { resolution } => w.u32(resolution),
            RecipeSettings::MeshRemesh { target_triangles } => w.u32(target_triangles),
            RecipeSettings::ImageResize {
                source_role,
                width,
                height,
                media,
            } => {
                w.u8(source_role.tag());
                w.u32(width);
                w.u32(height);
                w.u8(media.tag());
            }
            RecipeSettings::ImageTranscode {
                source_role,
                media,
                quality,
            } => {
                w.u8(source_role.tag());
                w.u8(media.tag());
                w.u8(quality);
            }
        }
    }

    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(match RecipeKind::decode(r)? {
            RecipeKind::MeshThumbnail => RecipeSettings::MeshThumbnail {
                width: r.u32("recipe thumbnail width")?,
                height: r.u32("recipe thumbnail height")?,
                media: ThumbnailMedia::decode(r)?,
            },
            RecipeKind::MeshLod => RecipeSettings::MeshLod {
                lod: r.u8("recipe lod")?,
                target_triangles: r.u32("recipe target_triangles")?,
            },
            RecipeKind::MeshCollider => RecipeSettings::MeshCollider {
                max_hull_vertices: r.u32("recipe max_hull_vertices")?,
            },
            RecipeKind::MeshAo => RecipeSettings::MeshAo {
                resolution: r.u32("recipe ao resolution")?,
            },
            RecipeKind::MeshRemesh => RecipeSettings::MeshRemesh {
                target_triangles: r.u32("recipe target_triangles")?,
            },
            RecipeKind::ImageResize => RecipeSettings::ImageResize {
                source_role: FileRole::decode(r)?,
                width: r.u32("recipe image width")?,
                height: r.u32("recipe image height")?,
                media: ThumbnailMedia::decode(r)?,
            },
            RecipeKind::ImageTranscode => RecipeSettings::ImageTranscode {
                source_role: FileRole::decode(r)?,
                media: ThumbnailMedia::decode(r)?,
                quality: r.u8("recipe transcode quality")?,
            },
        })
    }
}

/// The canonical processing recipe: typed settings plus the exact tool
/// closure. Its digest is the recipe identity in every cache key.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessingRecipe {
    pub settings: RecipeSettings,
    pub tool: ToolClosure,
    pub output_schema: u32,
}

impl ProcessingRecipe {
    pub fn kind(&self) -> RecipeKind {
        self.settings.kind()
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        self.settings.validate()?;
        self.tool.validate()?;
        if self.output_schema != OUTPUT_SCHEMA_V1 {
            return Err(AssetDataError::UnsupportedSchema {
                found: self.output_schema as u16,
            });
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::PROCESSING_RECIPE);
        self.settings.encode(&mut w);
        self.tool.encode(&mut w);
        w.u32(self.output_schema);
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::PROCESSING_RECIPE)?;
        let settings = RecipeSettings::decode(&mut r)?;
        let tool = ToolClosure::decode(&mut r)?;
        let output_schema = r.u32("recipe output schema")?;
        r.finish()?;
        let recipe = Self {
            settings,
            tool,
            output_schema,
        };
        recipe.validate()?;
        Ok(recipe)
    }

    pub fn digest(&self) -> Result<RecipeDigest, AssetDataError> {
        Ok(RecipeDigest::hash_of(&self.to_canonical_bytes()?))
    }

    /// Check a completed result against this recipe's contract: output roles,
    /// media, exact dimensions, and metric budgets. The server refuses to
    /// publish a variant that fails here, whatever the worker claims.
    pub fn validate_result(&self, variant: &DerivedVariantManifest) -> Result<(), AssetDataError> {
        variant.validate()?;
        if variant.kind != self.kind() {
            return Err(AssetDataError::Mismatch {
                what: "variant kind vs recipe",
            });
        }
        if variant.recipe != self.digest()? {
            return Err(AssetDataError::Mismatch {
                what: "variant recipe digest",
            });
        }
        // The variant must actually declare the input this recipe consumes;
        // a result that omits its own input role has untracked lineage.
        if !variant
            .inputs
            .iter()
            .any(|i| i.role == self.settings.input_role())
        {
            return Err(AssetDataError::Missing {
                what: "recipe input role in variant inputs",
            });
        }
        match self.settings {
            RecipeSettings::MeshThumbnail {
                width,
                height,
                media,
            } => {
                let t = variant.thumbnail.as_ref().ok_or(AssetDataError::Missing {
                    what: "variant thumbnail",
                })?;
                if t.width != width || t.height != height || t.media != media {
                    return Err(AssetDataError::Mismatch {
                        what: "thumbnail vs recipe settings",
                    });
                }
            }
            RecipeSettings::MeshLod {
                lod,
                target_triangles,
            } => {
                let expected_role = if lod == 1 {
                    FileRole::Lod1Glb
                } else {
                    FileRole::Lod2Glb
                };
                let out = single_output(variant)?;
                if out.role != expected_role || out.media != MediaType::Glb || out.lod != lod {
                    return Err(AssetDataError::Mismatch {
                        what: "lod output vs recipe settings",
                    });
                }
                if variant.metrics.triangles == 0
                    || variant.metrics.triangles > target_triangles
                {
                    return Err(AssetDataError::Mismatch {
                        what: "lod triangles vs target",
                    });
                }
            }
            RecipeSettings::MeshCollider { max_hull_vertices } => {
                let out = single_output(variant)?;
                if out.role != FileRole::Collider {
                    return Err(AssetDataError::Mismatch {
                        what: "collider output role",
                    });
                }
                if variant.metrics.vertices == 0
                    || variant.metrics.vertices > max_hull_vertices
                {
                    return Err(AssetDataError::Mismatch {
                        what: "collider vertices vs hull budget",
                    });
                }
            }
            RecipeSettings::MeshAo { .. } => {
                let out = single_output(variant)?;
                if !matches!(out.role, FileRole::AoMesh | FileRole::ShadowSdf) {
                    return Err(AssetDataError::Mismatch { what: "ao output role" });
                }
            }
            RecipeSettings::MeshRemesh { target_triangles } => {
                let out = single_output(variant)?;
                if out.role != FileRole::RenderGlb || out.media != MediaType::Glb {
                    return Err(AssetDataError::Mismatch {
                        what: "remesh output vs recipe settings",
                    });
                }
                if variant.metrics.triangles == 0
                    || variant.metrics.triangles > target_triangles
                {
                    return Err(AssetDataError::Mismatch {
                        what: "remesh triangles vs target",
                    });
                }
            }
            RecipeSettings::ImageResize {
                source_role,
                width,
                height,
                media,
            } => {
                let out = single_output(variant)?;
                let expected_media = match media {
                    ThumbnailMedia::Png => MediaType::Png,
                    ThumbnailMedia::Jpeg => MediaType::Jpeg,
                };
                if out.role != source_role || out.media != expected_media {
                    return Err(AssetDataError::Mismatch {
                        what: "resize output vs recipe settings",
                    });
                }
                if out.dims != Some(crate::asset::ImageDims { width, height }) {
                    return Err(AssetDataError::Mismatch {
                        what: "resize dims vs recipe settings",
                    });
                }
            }
            RecipeSettings::ImageTranscode {
                source_role, media, ..
            } => {
                let out = single_output(variant)?;
                let expected_media = match media {
                    ThumbnailMedia::Png => MediaType::Png,
                    ThumbnailMedia::Jpeg => MediaType::Jpeg,
                };
                if out.role != source_role || out.media != expected_media {
                    return Err(AssetDataError::Mismatch {
                        what: "transcode output vs recipe settings",
                    });
                }
            }
        }
        Ok(())
    }
}

fn single_output(variant: &DerivedVariantManifest) -> Result<&AssetFile, AssetDataError> {
    if variant.outputs.len() != 1 {
        return Err(AssetDataError::Mismatch {
            what: "variant output count",
        });
    }
    Ok(&variant.outputs[0])
}

/// One exact input a derivation consumed: a role of the base revision pinned
/// to its exact blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DerivedInput {
    pub role: FileRole,
    pub blob: BlobId,
}

impl DerivedInput {
    fn sort_key(&self) -> (u8, BlobId) {
        (self.role.tag(), self.blob)
    }
    fn encode(&self, w: &mut CanonWriter) {
        w.u8(self.role.tag());
        self.blob.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            role: FileRole::decode(r)?,
            blob: BlobId::decode(r)?,
        })
    }
}

/// The immutable manifest of one validated processing result. Its digest is
/// the [`DerivedVariantId`].
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedVariantManifest {
    /// The exact base asset revision this was derived from.
    pub base: AssetRevisionRef,
    pub kind: RecipeKind,
    pub recipe: RecipeDigest,
    /// Ordered exact inputs, canonical `(role, blob)` order.
    pub inputs: Vec<DerivedInput>,
    /// Output blob roles, canonical `(role, tier, lod)` order. Empty exactly
    /// when the recipe produces a thumbnail instead.
    pub outputs: Vec<AssetFile>,
    /// The produced thumbnail for thumbnail recipes, mirroring the asset
    /// manifest's typed mandatory-thumbnail design.
    pub thumbnail: Option<ThumbnailMeta>,
    /// Measured metrics of the outputs; `total_bytes` must equal their sum.
    pub metrics: Metrics,
    /// Rights inherited from the base revision plus derivative notices.
    pub rights: Rights,
}

impl DerivedVariantManifest {
    pub fn canonicalize(&mut self) {
        self.inputs.sort_by_key(|i| i.sort_key());
        self.outputs.sort_by_key(|f| f.sort_key());
    }

    /// The semantic role this variant plays during resolution.
    pub fn role(&self) -> VariantRole {
        match self.kind {
            RecipeKind::MeshThumbnail => VariantRole::Thumbnail,
            _ => self
                .outputs
                .first()
                .map(|f| VariantRole::File(f.role))
                .unwrap_or(VariantRole::Thumbnail),
        }
    }

    /// Every blob this variant needs, canonical order.
    pub fn blob_closure(&self) -> Vec<BlobId> {
        let mut blobs: Vec<BlobId> = self
            .outputs
            .iter()
            .map(|f| f.blob)
            .chain(self.thumbnail.iter().map(|t| t.blob))
            .collect();
        blobs.sort();
        blobs.dedup();
        blobs
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        if self.inputs.is_empty() {
            return Err(AssetDataError::Missing {
                what: "variant inputs",
            });
        }
        if self.inputs.len() > MAX_DERIVED_INPUTS {
            return Err(AssetDataError::OverBudget {
                what: "variant inputs",
                limit: MAX_DERIVED_INPUTS as u64,
                found: self.inputs.len() as u64,
            });
        }
        check_sorted_unique(&self.inputs, |i| i.sort_key(), "variant input")?;
        if self.outputs.len() > MAX_DERIVED_OUTPUTS {
            return Err(AssetDataError::OverBudget {
                what: "variant outputs",
                limit: MAX_DERIVED_OUTPUTS as u64,
                found: self.outputs.len() as u64,
            });
        }
        for f in &self.outputs {
            f.validate()?;
        }
        check_sorted_unique(&self.outputs, |f| f.sort_key(), "variant output (role, tier, lod)")?;
        // One variant fills exactly one semantic role; mixed output roles
        // would make `role()` ambiguous and let one cache entry answer two
        // different resolution questions.
        if let Some(first) = self.outputs.first() {
            if self.outputs.iter().any(|f| f.role != first.role) {
                return Err(AssetDataError::Mismatch {
                    what: "mixed variant output roles",
                });
            }
        }
        if let Some(t) = &self.thumbnail {
            t.validate()?;
        }
        // A thumbnail recipe produces the thumbnail and nothing else; every
        // other kind produces file outputs and no thumbnail.
        match (self.kind, self.thumbnail.is_some(), self.outputs.is_empty()) {
            (RecipeKind::MeshThumbnail, true, true) => {}
            (RecipeKind::MeshThumbnail, _, _) => {
                return Err(AssetDataError::Mismatch {
                    what: "thumbnail recipe outputs",
                })
            }
            (_, false, false) => {}
            (_, _, _) => {
                return Err(AssetDataError::Mismatch {
                    what: "variant outputs vs kind",
                })
            }
        }
        self.metrics.validate()?;
        let sum = self
            .outputs
            .iter()
            .map(|f| f.byte_len)
            .chain(self.thumbnail.iter().map(|t| t.byte_len))
            .try_fold(0u64, |a, b| a.checked_add(b))
            .ok_or(AssetDataError::Malformed {
                what: "variant byte_len sum",
            })?;
        if sum != self.metrics.total_bytes {
            return Err(AssetDataError::Mismatch {
                what: "variant total_bytes vs output sum",
            });
        }
        self.rights.validate()
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::DERIVED_VARIANT);
        self.base.encode(&mut w);
        w.u8(self.kind.tag());
        self.recipe.encode(&mut w);
        w.u32(self.inputs.len() as u32);
        for i in &self.inputs {
            i.encode(&mut w);
        }
        w.u32(self.outputs.len() as u32);
        for f in &self.outputs {
            f.encode(&mut w);
        }
        match &self.thumbnail {
            None => w.bool(false),
            Some(t) => {
                w.bool(true);
                t.encode(&mut w);
            }
        }
        self.metrics.encode(&mut w);
        self.rights.encode(&mut w);
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::DERIVED_VARIANT)?;
        let base = AssetRevisionRef::decode(&mut r)?;
        let kind = RecipeKind::decode(&mut r)?;
        let recipe = RecipeDigest::decode(&mut r)?;
        let n = r.count("variant inputs", MAX_DERIVED_INPUTS)?;
        let mut inputs = Vec::with_capacity(n);
        for _ in 0..n {
            inputs.push(DerivedInput::decode(&mut r)?);
        }
        let n = r.count("variant outputs", MAX_DERIVED_OUTPUTS)?;
        let mut outputs = Vec::with_capacity(n);
        for _ in 0..n {
            outputs.push(AssetFile::decode(&mut r)?);
        }
        let thumbnail = if r.bool("variant thumbnail present")? {
            Some(ThumbnailMeta::decode(&mut r)?)
        } else {
            None
        };
        let metrics = Metrics::decode(&mut r)?;
        let rights = Rights::decode(&mut r)?;
        r.finish()?;
        let manifest = Self {
            base,
            kind,
            recipe,
            inputs,
            outputs,
            thumbnail,
            metrics,
            rights,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// The immutable identity of this exact result.
    pub fn id(&self) -> Result<DerivedVariantId, AssetDataError> {
        Ok(DerivedVariantId::hash_of(&self.to_canonical_bytes()?))
    }
}

/// The single-flight derivation cache key: base revision + recipe + ordered
/// exact inputs, domain-separated. Never derived from a filesystem mtime.
pub fn derivation_key(
    base: &AssetRevisionRef,
    recipe: &RecipeDigest,
    inputs: &[DerivedInput],
) -> Result<DerivationKey, AssetDataError> {
    if inputs.is_empty() || inputs.len() > MAX_DERIVED_INPUTS {
        return Err(AssetDataError::Malformed {
            what: "derivation key inputs",
        });
    }
    let mut sorted = inputs.to_vec();
    sorted.sort_by_key(|i| i.sort_key());
    sorted.dedup();
    if sorted.len() != inputs.len() {
        return Err(AssetDataError::Duplicate {
            what: "derivation key input",
        });
    }
    let mut h = Sha256::new();
    h.update(b"mpc1:derivation-key:v1\0");
    h.update(base.asset_id.as_bytes());
    h.update(base.revision.as_bytes());
    h.update(recipe.as_bytes());
    h.update(&(sorted.len() as u32).to_be_bytes());
    for i in &sorted {
        h.update(&[i.role.tag()]);
        h.update(i.blob.as_bytes());
    }
    Ok(DerivationKey::from_bytes(h.finalize()))
}

/// The immutable frozen set of variants one release may resolve among.
/// Creating another derivative later creates a NEW set digest; it never
/// rewrites this one.
#[derive(Clone, Debug, PartialEq)]
pub struct VariantSetManifest {
    pub base: AssetRevisionRef,
    /// Approved variants, canonical sorted order.
    pub variants: Vec<DerivedVariantId>,
    pub policy_version: u32,
}

impl VariantSetManifest {
    pub fn canonicalize(&mut self) {
        self.variants.sort();
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        if self.variants.is_empty() {
            return Err(AssetDataError::Missing {
                what: "variant set variants",
            });
        }
        if self.variants.len() > MAX_VARIANTS_PER_SET {
            return Err(AssetDataError::OverBudget {
                what: "variant set variants",
                limit: MAX_VARIANTS_PER_SET as u64,
                found: self.variants.len() as u64,
            });
        }
        check_sorted_unique(&self.variants, |v| *v, "variant set entry")?;
        if self.policy_version != RESOLUTION_POLICY_V1 {
            return Err(AssetDataError::UnsupportedSchema {
                found: self.policy_version as u16,
            });
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::VARIANT_SET);
        self.base.encode(&mut w);
        w.u32(self.variants.len() as u32);
        for v in &self.variants {
            v.encode(&mut w);
        }
        w.u32(self.policy_version);
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::VARIANT_SET)?;
        let base = AssetRevisionRef::decode(&mut r)?;
        let n = r.count("variant set variants", MAX_VARIANTS_PER_SET)?;
        let mut variants = Vec::with_capacity(n);
        for _ in 0..n {
            variants.push(DerivedVariantId::decode(&mut r)?);
        }
        let policy_version = r.u32("variant set policy version")?;
        r.finish()?;
        let set = Self {
            base,
            variants,
            policy_version,
        };
        set.validate()?;
        Ok(set)
    }

    pub fn id(&self) -> Result<VariantSetId, AssetDataError> {
        Ok(VariantSetId::hash_of(&self.to_canonical_bytes()?))
    }
}

/// The semantic slot a variant fills during resolution: the mandatory
/// thumbnail, or one typed file role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantRole {
    Thumbnail,
    File(FileRole),
}

impl VariantRole {
    fn sort_key(&self) -> (u8, u8) {
        match self {
            VariantRole::Thumbnail => (0, 0),
            VariantRole::File(role) => (1, role.tag()),
        }
    }
    fn encode(&self, w: &mut CanonWriter) {
        match self {
            VariantRole::Thumbnail => w.u8(0),
            VariantRole::File(role) => {
                w.u8(1);
                w.u8(role.tag());
            }
        }
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        match r.u8("variant role")? {
            0 => Ok(VariantRole::Thumbnail),
            1 => Ok(VariantRole::File(FileRole::decode(r)?)),
            found => Err(AssetDataError::BadTag {
                what: "VariantRole",
                found,
            }),
        }
    }
}

/// The bounded canonical inputs of deterministic per-client resolution. Two
/// clients declaring the same profile resolve the same set to the same map
/// regardless of install contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientProfile {
    pub policy_version: u32,
    /// Requested quality ceiling; `Any` accepts every tier.
    pub tier: crate::asset::DeviceTier,
    pub max_texture_dim: u32,
    pub max_triangles: u32,
    /// Per-variant total byte budget.
    pub max_variant_bytes: u64,
    pub accept_png: bool,
    pub accept_jpeg: bool,
    pub accept_glb: bool,
    pub accept_bin: bool,
}

impl ClientProfile {
    pub fn validate(&self) -> Result<(), AssetDataError> {
        if self.policy_version != RESOLUTION_POLICY_V1 {
            return Err(AssetDataError::UnsupportedSchema {
                found: self.policy_version as u16,
            });
        }
        check_dim(self.max_texture_dim, "profile max_texture_dim")?;
        if self.max_triangles == 0 || self.max_variant_bytes == 0 {
            return Err(AssetDataError::Malformed {
                what: "profile budgets",
            });
        }
        if !(self.accept_png || self.accept_jpeg || self.accept_glb || self.accept_bin) {
            return Err(AssetDataError::Malformed {
                what: "profile accepts nothing",
            });
        }
        Ok(())
    }

    fn accepts(&self, media: MediaType) -> bool {
        match media {
            MediaType::Png => self.accept_png,
            MediaType::Jpeg => self.accept_jpeg,
            MediaType::Glb => self.accept_glb,
            MediaType::Bin => self.accept_bin,
            _ => false,
        }
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::CLIENT_PROFILE);
        w.u32(self.policy_version);
        w.u8(self.tier.tag());
        w.u32(self.max_texture_dim);
        w.u32(self.max_triangles);
        w.u64(self.max_variant_bytes);
        let bits = (self.accept_png as u8)
            | (self.accept_jpeg as u8) << 1
            | (self.accept_glb as u8) << 2
            | (self.accept_bin as u8) << 3;
        w.u8(bits);
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::CLIENT_PROFILE)?;
        let policy_version = r.u32("profile policy version")?;
        let tier = crate::asset::DeviceTier::decode(&mut r)?;
        let max_texture_dim = r.u32("profile max_texture_dim")?;
        let max_triangles = r.u32("profile max_triangles")?;
        let max_variant_bytes = r.u64("profile max_variant_bytes")?;
        let bits = r.u8("profile accept bits")?;
        if bits & !0b1111 != 0 {
            return Err(AssetDataError::Malformed {
                what: "profile accept bits",
            });
        }
        r.finish()?;
        let profile = Self {
            policy_version,
            tier,
            max_texture_dim,
            max_triangles,
            max_variant_bytes,
            accept_png: bits & 1 != 0,
            accept_jpeg: bits & 2 != 0,
            accept_glb: bits & 4 != 0,
            accept_bin: bits & 8 != 0,
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn digest(&self) -> Result<ClientProfileDigest, AssetDataError> {
        Ok(ClientProfileDigest::hash_of(&self.to_canonical_bytes()?))
    }
}

/// One resolved role: the winning variant and its exact blob closure.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedEntry {
    pub role: VariantRole,
    pub variant: DerivedVariantId,
    pub blobs: Vec<BlobId>,
}

/// The canonical result of deterministic resolution for ONE asset: a set of
/// ATOMIC OVERRIDES over the base revision named by `VariantSet.base`.
///
/// Each entry replaces exactly its named semantic role; every base-manifest
/// role WITHOUT an entry here is retained unchanged from the base
/// `AssetManifest` — the map never removes, renames, or partially rewrites a
/// role it does not name. A peer acknowledges the map digest as one unit:
/// applying some overrides and not others is never valid. Binds the exact
/// set AND the exact profile so a digest can never be replayed across either.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedVariantMap {
    pub set: VariantSetId,
    pub profile: ClientProfileDigest,
    pub entries: Vec<ResolvedEntry>,
}

impl ResolvedVariantMap {
    /// The override this map applies to one semantic role, or `None` when
    /// that role is retained unchanged from the base revision's manifest.
    pub fn override_for(&self, role: VariantRole) -> Option<&ResolvedEntry> {
        self.entries
            .binary_search_by(|e| e.role.sort_key().cmp(&role.sort_key()))
            .ok()
            .map(|i| &self.entries[i])
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        if self.entries.is_empty() {
            return Err(AssetDataError::Missing {
                what: "resolved entries",
            });
        }
        if self.entries.len() > MAX_RESOLVED_ENTRIES {
            return Err(AssetDataError::OverBudget {
                what: "resolved entries",
                limit: MAX_RESOLVED_ENTRIES as u64,
                found: self.entries.len() as u64,
            });
        }
        check_sorted_unique(&self.entries, |e| e.role.sort_key(), "resolved role")?;
        for e in &self.entries {
            if e.blobs.is_empty() {
                return Err(AssetDataError::Missing {
                    what: "resolved entry blobs",
                });
            }
            // Mirrors the decode-side cap exactly: outputs plus thumbnail.
            if e.blobs.len() > MAX_DERIVED_OUTPUTS + 1 {
                return Err(AssetDataError::OverBudget {
                    what: "resolved entry blobs",
                    limit: (MAX_DERIVED_OUTPUTS + 1) as u64,
                    found: e.blobs.len() as u64,
                });
            }
            check_sorted_unique(&e.blobs, |b| *b, "resolved entry blob")?;
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::RESOLVED_VARIANT_MAP);
        self.set.encode(&mut w);
        self.profile.encode(&mut w);
        w.u32(self.entries.len() as u32);
        for e in &self.entries {
            e.role.encode(&mut w);
            e.variant.encode(&mut w);
            w.u32(e.blobs.len() as u32);
            for b in &e.blobs {
                b.encode(&mut w);
            }
        }
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::RESOLVED_VARIANT_MAP)?;
        let set = VariantSetId::decode(&mut r)?;
        let profile = ClientProfileDigest::decode(&mut r)?;
        let n = r.count("resolved entries", MAX_RESOLVED_ENTRIES)?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let role = VariantRole::decode(&mut r)?;
            let variant = DerivedVariantId::decode(&mut r)?;
            let bn = r.count("resolved entry blobs", MAX_DERIVED_OUTPUTS + 1)?;
            let mut blobs = Vec::with_capacity(bn);
            for _ in 0..bn {
                blobs.push(BlobId::decode(&mut r)?);
            }
            entries.push(ResolvedEntry {
                role,
                variant,
                blobs,
            });
        }
        r.finish()?;
        let map = Self {
            set,
            profile,
            entries,
        };
        map.validate()?;
        Ok(map)
    }

    pub fn digest(&self) -> Result<ResolvedMapDigest, AssetDataError> {
        Ok(ResolvedMapDigest::hash_of(&self.to_canonical_bytes()?))
    }
}

fn tier_rank(tier: crate::asset::DeviceTier) -> u8 {
    tier.tag()
}

/// Deterministic per-client resolution, [`RESOLUTION_POLICY_V1`].
///
/// Pure over `{set, variant manifests, profile}`: it never schedules work,
/// consults local files, or invents a substitute. Every provided manifest is
/// re-hashed against the set (self-authenticating input), filtered against
/// the profile, and the winner per role is chosen by
/// `(quality desc, byte cost asc, variant id asc)`. A role whose variants are
/// all incompatible fails closed.
pub fn resolve_variants(
    set: &VariantSetManifest,
    variants: &[DerivedVariantManifest],
    profile: &ClientProfile,
) -> Result<ResolvedVariantMap, AssetDataError> {
    set.validate()?;
    profile.validate()?;
    if profile.policy_version != set.policy_version {
        return Err(AssetDataError::Mismatch {
            what: "profile policy version vs set",
        });
    }
    if variants.len() != set.variants.len() {
        return Err(AssetDataError::Mismatch {
            what: "variant manifests vs set entries",
        });
    }
    // Verify the provided manifests are exactly the set's members.
    let mut ids = Vec::with_capacity(variants.len());
    for v in variants {
        if v.base != set.base {
            return Err(AssetDataError::Mismatch {
                what: "variant base vs set base",
            });
        }
        ids.push((v.id()?, v));
    }
    ids.sort_by_key(|(id, _)| *id);
    for (given, expected) in ids.iter().zip(set.variants.iter()) {
        if given.0 != *expected {
            return Err(AssetDataError::Mismatch {
                what: "variant manifest not in set",
            });
        }
    }

    // Group by semantic role in canonical role order.
    let mut roles: Vec<(u8, u8)> = ids.iter().map(|(_, v)| v.role().sort_key()).collect();
    roles.sort();
    roles.dedup();

    let profile_tier = tier_rank(profile.tier);
    let mut entries = Vec::new();
    for role_key in roles {
        // Candidates for this role that the profile admits.
        let mut compatible: Vec<(u8, u64, DerivedVariantId, &DerivedVariantManifest)> = Vec::new();
        for (id, v) in &ids {
            if v.role().sort_key() != role_key {
                continue;
            }
            let mut ok = true;
            let mut quality = 0u8;
            for f in &v.outputs {
                if !profile.accepts(f.media) {
                    ok = false;
                }
                if let Some(d) = f.dims {
                    if d.width > profile.max_texture_dim || d.height > profile.max_texture_dim {
                        ok = false;
                    }
                }
                let rank = tier_rank(f.tier);
                // Tier 0 is `Any`; a tiered file must fit the ceiling.
                if rank != 0 && profile_tier != 0 && rank > profile_tier {
                    ok = false;
                }
                quality = quality.max(rank);
            }
            if let Some(t) = &v.thumbnail {
                let media = match t.media {
                    ThumbnailMedia::Png => MediaType::Png,
                    ThumbnailMedia::Jpeg => MediaType::Jpeg,
                };
                if !profile.accepts(media) {
                    ok = false;
                }
                if t.width > profile.max_texture_dim || t.height > profile.max_texture_dim {
                    ok = false;
                }
            }
            if v.metrics.triangles > profile.max_triangles {
                ok = false;
            }
            if v.metrics.total_bytes > profile.max_variant_bytes {
                ok = false;
            }
            if ok {
                compatible.push((quality, v.metrics.total_bytes, *id, v));
            }
        }
        if compatible.is_empty() {
            // Variants exist for this role but none fit: refuse rather than
            // substitute or expand the set.
            return Err(AssetDataError::Missing {
                what: "compatible variant for role",
            });
        }
        compatible.sort_by(|a, b| {
            b.0.cmp(&a.0) // quality desc
                .then(a.1.cmp(&b.1)) // byte cost asc
                .then(a.2.cmp(&b.2)) // exact id asc — final total tie-break
        });
        let (_, _, id, v) = compatible[0];
        entries.push(ResolvedEntry {
            role: v.role(),
            variant: id,
            blobs: v.blob_closure(),
        });
    }

    let map = ResolvedVariantMap {
        set: set.id()?,
        profile: profile.digest()?,
        entries,
    };
    map.validate()?;
    Ok(map)
}

/// One asset's line in the aggregate realm resolution: the exact base
/// revision, the exact pinned variant set, and this client's exact per-asset
/// resolved map over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RealmResolutionEntry {
    pub base: AssetRevisionRef,
    pub variant_set: VariantSetId,
    pub resolved_map: ResolvedMapDigest,
}

impl RealmResolutionEntry {
    fn encode(&self, w: &mut CanonWriter) {
        self.base.encode(w);
        self.variant_set.encode(w);
        self.resolved_map.encode(w);
    }
    fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(Self {
            base: AssetRevisionRef::decode(r)?,
            variant_set: VariantSetId::decode(r)?,
            resolved_map: ResolvedMapDigest::decode(r)?,
        })
    }
}

/// The aggregate per-client resolution over EVERY variant set a content set
/// pins — one entry per asset that carries a pinned set, sorted by base.
/// Its digest is what readiness messages acknowledge: a single per-asset map
/// digest proves one asset, this proves the realm.
///
/// The `(base, variant_set)` pairs are not chosen by the client: they must
/// equal the content lock's pinned variant-set table exactly
/// ([`RealmResolution::verify_against`]), so a peer can neither skip an
/// asset nor resolve against a set the release never pinned.
#[derive(Clone, Debug, PartialEq)]
pub struct RealmResolution {
    pub content_set: crate::id::ContentSetId,
    pub profile: ClientProfileDigest,
    pub entries: Vec<RealmResolutionEntry>,
}

impl RealmResolution {
    pub fn canonicalize(&mut self) {
        self.entries.sort_by_key(|e| e.base);
    }

    pub fn validate(&self) -> Result<(), AssetDataError> {
        // An empty table is legal: a realm whose lock pins no variant sets
        // still produces a canonical content_set/profile-bound digest, so
        // no-variant realms can ready. `verify_against` still refuses an
        // empty resolution for a lock that DOES pin sets.
        if self.entries.len() > MAX_REALM_RESOLUTION_ENTRIES {
            return Err(AssetDataError::OverBudget {
                what: "realm resolution entries",
                limit: MAX_REALM_RESOLUTION_ENTRIES as u64,
                found: self.entries.len() as u64,
            });
        }
        check_sorted_unique(&self.entries, |e| e.base, "realm resolution base")?;
        // One resolution per asset: two revisions of one asset cannot both
        // be resolved in one realm.
        check_sorted_unique(
            &self.entries,
            |e| e.base.asset_id,
            "realm resolution asset",
        )
    }

    /// Prove this aggregate covers the lock's pinned variant-set table
    /// exactly: every pinned `(base, set)` resolved, nothing extra, nothing
    /// substituted. Coverage failures are typed refusals, never downgrades.
    pub fn verify_against(&self, lock: &crate::game::ContentLock) -> Result<(), AssetDataError> {
        self.validate()?;
        if self.entries.len() != lock.variant_sets.len() {
            return Err(AssetDataError::Mismatch {
                what: "realm resolution coverage",
            });
        }
        for (entry, pinned) in self.entries.iter().zip(lock.variant_sets.iter()) {
            if entry.base != pinned.base || entry.variant_set != pinned.set {
                return Err(AssetDataError::Mismatch {
                    what: "realm resolution pinned set",
                });
            }
        }
        Ok(())
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, AssetDataError> {
        self.validate()?;
        let mut w = CanonWriter::new(dockind::REALM_RESOLUTION);
        self.content_set.encode(&mut w);
        self.profile.encode(&mut w);
        w.u32(self.entries.len() as u32);
        for e in &self.entries {
            e.encode(&mut w);
        }
        w.finish()
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetDataError> {
        let mut r = CanonReader::new(bytes, dockind::REALM_RESOLUTION)?;
        let content_set = crate::id::ContentSetId::decode(&mut r)?;
        let profile = ClientProfileDigest::decode(&mut r)?;
        let n = r.count("realm resolution entries", MAX_REALM_RESOLUTION_ENTRIES)?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            entries.push(RealmResolutionEntry::decode(&mut r)?);
        }
        r.finish()?;
        let resolution = Self {
            content_set,
            profile,
            entries,
        };
        resolution.validate()?;
        Ok(resolution)
    }

    pub fn digest(&self) -> Result<crate::id::RealmResolutionDigest, AssetDataError> {
        Ok(crate::id::RealmResolutionDigest::hash_of(
            &self.to_canonical_bytes()?,
        ))
    }
}
