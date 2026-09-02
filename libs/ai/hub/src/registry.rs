//! Data-driven model registry: model id -> HF repo/files -> local cache layout.
//!
//! The registry is the extension point for new models: adding one means adding
//! an entry to registry.json (or a registry file passed with `--registry`) and,
//! if it needs a new runtime, a `ContentBackend` impl matched by the entry's
//! `backend` string. The checked-in seed registry is embedded in the binary;
//! a `registry.json` in the cache dir overrides it, so boxes can carry extra
//! models without a rebuild.
//!
//! Cache layout convention: `cache_as` paths use '/' and mirror the ComfyUI
//! model roots (`unet/`, `vae/`, `text_encoders/`, `checkpoints/`) so that
//! libs/diffusion's `ComfyModelRoots::new(cache_dir)` resolves files straight
//! out of our cache.

use crate::error::AssetAiError;
use makepad_micro_serde::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const EMBEDDED_REGISTRY: &str = include_str!("../registry.json");

// ---------------------------------------------------------------------------
// Wire format (exactly what registry.json contains)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct RegistryFileJson {
    pub models: Vec<RegistryModelJson>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct RegistryModelJson {
    pub id: String,
    pub domain: String,
    pub backend: String,
    pub available: bool,
    pub gated: bool,
    pub vram_gb: Option<f64>,
    /// HARD requirement gate: the backend refuses to load this model on a
    /// box whose total VRAM is below this (fail closed — an unknown GPU also
    /// refuses). `vram_gb` above stays informational/scheduling only.
    pub min_vram_gb: Option<f64>,
    /// HARD requirement gate on the CUDA compute capability (e.g. 12.0 for
    /// Blackwell-only NVFP4 checkpoints).
    pub min_compute_cap: Option<f64>,
    pub note: Option<String>,
    /// Weight-license record the UI must show before this model is cleared
    /// for download or generation. Optional on the wire so a cache-dir
    /// override registry from an older box still parses; the embedded
    /// registry requires it (see tests).
    pub license: Option<ModelLicenseJson>,
    pub files: Vec<RegistryEntryFileJson>,
}

/// Registry JSON shape of a model-weight license. `restriction` is one of
/// `none` (permissive), `non-commercial`, `community`, `restricted`.
#[derive(Clone, Debug, SerJson, DeJson, PartialEq, Eq)]
pub struct ModelLicenseJson {
    pub name: String,
    pub url: String,
    pub summary: String,
    pub restriction: String,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct RegistryEntryFileJson {
    /// Semantic name used by runtimes to resolve an artifact without
    /// guessing from its file name. New native model manifests should set a
    /// role for every runtime-read artifact; old manifests remain valid.
    pub role: Option<String>,
    /// HuggingFace repo, e.g. "black-forest-labs/FLUX.1-schnell".
    pub repo: String,
    /// Path inside the repo, downloaded from
    /// `https://huggingface.co/<repo>/resolve/main/<path>`.
    pub path: String,
    /// Immutable HuggingFace commit revision. Legacy entries omit this and
    /// retain the historical `main` behavior. When present it must be a full
    /// commit id and the entry must also pin size and SHA-256.
    pub revision: Option<String>,
    /// Where the file lands relative to the cache dir; '/'-separated, split
    /// and joined per-platform so Windows paths come out right.
    pub cache_as: String,
    /// Expected size in bytes if known; also learned from Content-Length and
    /// used for download progress totals.
    pub size: Option<u64>,
    /// Optional integrity check, lowercase hex.
    pub sha256: Option<String>,
    /// True for weights that only exist in a locally-converted format and
    /// have no downloadable source: the downloader never hits HF for these
    /// and instead errors helpfully when the file is missing from the cache.
    /// `repo`/`path` may then be empty or point at the upstream source for
    /// reference.
    pub local: Option<bool>,
    /// True for artifacts a model can run WITHOUT (Music3's reference-audio
    /// encoder): a pull still fetches them, but a download failure does not
    /// fail the model, readiness ignores them, and the backend names the
    /// role when a job actually needs the file. Requires `role`.
    pub optional: Option<bool>,
    /// Cache-relative path ('/'-separated) of the converted form this
    /// download is turned into by the backend (e.g. Kokoro's upstream `.pth`
    /// -> `tts/kokoro-v1_0.mktts`). A file whose converted form exists counts
    /// as present — boxes carrying pre-converted weights never re-download
    /// the upstream source.
    pub converts_to: Option<String>,
    /// Validated conversion identity for new native artifacts. This is
    /// additive to `converts_to`, which remains supported for legacy
    /// backends whose converted output has no recorded provenance yet.
    pub conversion: Option<RegistryConversionJson>,
}

#[derive(Clone, Debug, SerJson, DeJson, PartialEq, Eq)]
pub struct RegistryConversionJson {
    /// Cache-relative final artifact path.
    pub cache_as: String,
    pub size: u64,
    pub sha256: String,
    pub converter_id: String,
    pub converter_version: String,
}

// ---------------------------------------------------------------------------
// Validated in-memory form
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Image,
    Mesh,
    Video,
    Audio,
    /// LLM text generation (the prompt expander).
    Text,
    /// Text-to-speech.
    Speech,
    /// Image/text -> walkable 3D gaussian-splat scene (FlashWorld).
    World,
    /// Image -> RGBA cutout with a soft alpha matte (BiRefNet).
    Matte,
    /// Image -> metric depthmap (Depth-Anything-3).
    Depth,
    /// Image/frame -> structured human body pose packet (SAM 3D Body).
    Body,
    /// Image + text prompt -> instance mask PNG + RGBA cutout (SAM 3.1).
    Segment,
    /// Mesh GLB -> skinned/rigged GLB (SkinTokens).
    Rig,
    /// Rigged GLB + prompt -> animated GLB with named clips (HY-Motion).
    Motion,
    /// Lyrics + music description -> full song wav (MiniMax-Music3 now,
    /// ACE-Step 1.5 planned as a second backend in the same domain).
    Music,
    /// Mesh GLB + reference image -> PBR-textured GLB + semantic material
    /// maps (albedo/normal/ORM) + provenance manifest (Hunyuan3D-Paint-2.1;
    /// deterministic paint-test tier ships everywhere).
    Paint,
    /// Reference image + instruction -> edited image (FLUX.2 klein). Its own
    /// domain so image-domain affinity never routes a text-to-image job to
    /// a model that fails closed without a reference.
    Edit,
    /// Image -> 4x upscaled image (RealESRGAN x4plus). Its own domain, like
    /// `Edit`, so image-domain affinity never routes a text-to-image job to
    /// a model that fails closed without an input image.
    Upscale,
    /// Structure-conditioned image generation: a control image (depth map or
    /// Canny edge map) + text prompt -> a new image matching that structure
    /// (FLUX.1-Depth-dev / FLUX.1-Canny-dev). Its own domain, like `Edit`/
    /// `Upscale`, so image-domain affinity never routes a text-to-image job
    /// to a model that fails closed without a control image.
    Control,
    /// Image + mask + prompt -> inpainted/outpainted image (FLUX.1-Fill-dev).
    /// Its own domain, like `Edit`/`Upscale`, so image-domain affinity never
    /// routes a text-to-image job to a model that fails closed without an
    /// image+mask pair.
    Inpaint,
    /// Video -> video, decoded once and re-encoded once, with RealESRGAN
    /// upscaling, RIFE frame-rate multiplication and/or a playback motion
    /// sidecar fused in the middle (video-enhance). Its own domain so video
    /// affinity never routes a text-to-video job to a model that fails
    /// closed without an input clip — and so a box can be dedicated to
    /// post-processing without advertising generation.
    Enhance,
    /// Single object image -> 3D gaussian splat PLY (TripoSplat). Distinct
    /// from `World`, which reconstructs a walkable SCENE from an image or
    /// prompt: this is one object, reconstructed at the requested gaussian
    /// budget, and the two are neither substitutable nor comparable in cost.
    Splat,
    /// Image + prompt -> text answer (a VLM: the language model with its
    /// vision tower attached). Its own domain rather than a mode of `Text`,
    /// for the same reason `Edit` is not a mode of `Image`: a vision model
    /// fails closed without an input image, so text affinity must never
    /// route a plain prompt-expansion job to it. The serving shape is the
    /// chat shape — one prompt in, one answer out, weights resident between
    /// requests — which is why a box that serves chat can serve this.
    Vision,
    /// Scanned page -> HTML transcription (Chandra 2 on the vision tower).
    Ocr,
    /// Speech audio -> timed transcript (Whisper). The `stt.whisper` pipe;
    /// `Speech` stays text-to-speech, so the two never share affinity.
    Stt,
    /// Audio -> beat and downbeat tracking JSON.
    Beats,
    /// Stereo music -> four separated stems.
    Stems,
    /// Audio -> polyphonic note transcription JSON/MIDI.
    Notes,
    /// Audio -> music-structure sections.
    Sections,
    /// Image -> sewing-pattern JSON.
    Garment,
}

impl Domain {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "image" => Some(Domain::Image),
            "mesh" => Some(Domain::Mesh),
            "video" => Some(Domain::Video),
            "audio" => Some(Domain::Audio),
            "text" => Some(Domain::Text),
            "speech" => Some(Domain::Speech),
            "world" => Some(Domain::World),
            "matte" => Some(Domain::Matte),
            "depth" => Some(Domain::Depth),
            "body" => Some(Domain::Body),
            "segment" => Some(Domain::Segment),
            "rig" => Some(Domain::Rig),
            "motion" => Some(Domain::Motion),
            "music" => Some(Domain::Music),
            "paint" => Some(Domain::Paint),
            "edit" => Some(Domain::Edit),
            "upscale" => Some(Domain::Upscale),
            "control" => Some(Domain::Control),
            "inpaint" => Some(Domain::Inpaint),
            "enhance" => Some(Domain::Enhance),
            "splat" => Some(Domain::Splat),
            "vision" => Some(Domain::Vision),
            "ocr" => Some(Domain::Ocr),
            "stt" => Some(Domain::Stt),
            "beats" => Some(Domain::Beats),
            "stems" => Some(Domain::Stems),
            "notes" => Some(Domain::Notes),
            "sections" => Some(Domain::Sections),
            "garment" => Some(Domain::Garment),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Domain::Image => "image",
            Domain::Mesh => "mesh",
            Domain::Video => "video",
            Domain::Audio => "audio",
            Domain::Text => "text",
            Domain::Speech => "speech",
            Domain::World => "world",
            Domain::Matte => "matte",
            Domain::Depth => "depth",
            Domain::Body => "body",
            Domain::Segment => "segment",
            Domain::Rig => "rig",
            Domain::Motion => "motion",
            Domain::Music => "music",
            Domain::Paint => "paint",
            Domain::Edit => "edit",
            Domain::Upscale => "upscale",
            Domain::Control => "control",
            Domain::Inpaint => "inpaint",
            Domain::Enhance => "enhance",
            Domain::Splat => "splat",
            Domain::Vision => "vision",
            Domain::Ocr => "ocr",
            Domain::Stt => "stt",
            Domain::Beats => "beats",
            Domain::Stems => "stems",
            Domain::Notes => "notes",
            Domain::Sections => "sections",
            Domain::Garment => "garment",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversionSpec {
    pub cache_as: String,
    pub size: u64,
    pub sha256: String,
    pub converter_id: String,
    pub converter_version: String,
}

#[derive(Clone, Debug)]
pub struct FileSpec {
    pub role: Option<String>,
    pub repo: String,
    pub path: String,
    pub revision: Option<String>,
    pub cache_as: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    /// Local-only converted weights: never downloaded, expected in the cache.
    pub local: bool,
    /// Optional role: see [`RegistryEntryFileJson::optional`].
    pub optional: bool,
    /// Cache-relative path of the converted form the backend derives from
    /// this download (see [`RegistryEntryFileJson::converts_to`]).
    pub converts_to: Option<String>,
    /// Strict conversion identity used by native artifact preparation.
    pub conversion: Option<ConversionSpec>,
}

impl FileSpec {
    /// Absolute destination path inside the cache dir, '/'-split so it is
    /// correct on Windows.
    pub fn dest_path(&self, cache_dir: &Path) -> PathBuf {
        let mut out = cache_dir.to_path_buf();
        for part in self.cache_as.split('/') {
            out.push(part);
        }
        out
    }

    /// Absolute path of the converted form, when this file has one.
    pub fn converted_path(&self, cache_dir: &Path) -> Option<PathBuf> {
        let converts_to = self
            .conversion
            .as_ref()
            .map(|conversion| &conversion.cache_as)
            .or(self.converts_to.as_ref())?;
        let mut out = cache_dir.to_path_buf();
        for part in converts_to.split('/') {
            out.push(part);
        }
        Some(out)
    }

    /// True when this file needs no download: the downloaded form is in the
    /// cache, or its converted form is (pre-converted weights placed at the
    /// final path keep working without the upstream source ever landing).
    pub fn is_present(&self, cache_dir: &Path) -> bool {
        if let Some(converted) = self.converted_path(cache_dir) {
            if converted.is_file() {
                // Legacy conversions never had a provenance contract, so
                // preserve their existence-based startup behavior. Strict
                // structured conversions require the verifier receipt.
                return self.conversion.as_ref().map_or(true, |_| {
                    crate::download::converted_file_is_verified(self, cache_dir)
                });
            }
        }
        let dest = self.dest_path(cache_dir);
        dest.is_file()
            && if self.revision.is_some() || self.size.is_some() || self.sha256.is_some() {
                crate::download::source_file_is_verified(self, cache_dir)
            } else {
                // Completely unpinned legacy/local entries have no identity
                // against which bytes could be verified. Preserve their old
                // existence behavior; all new native manifests are strict.
                true
            }
    }

    /// Effective source revision used by the downloader. Only legacy
    /// entries are allowed to resolve the mutable `main` default.
    pub fn resolved_revision(&self) -> &str {
        self.revision.as_deref().unwrap_or("main")
    }
}

#[derive(Clone, Debug)]
pub struct ModelSpec {
    pub id: String,
    pub domain: Domain,
    pub backend: String,
    pub available: bool,
    pub gated: bool,
    pub vram_gb: Option<f64>,
    /// Hard fail-closed requirements (see the wire-format docs).
    pub min_vram_gb: Option<f64>,
    pub min_compute_cap: Option<f64>,
    pub note: Option<String>,
    pub license: Option<ModelLicense>,
    pub files: Vec<FileSpec>,
}

/// Validated weight-license identity. The UI keys acknowledgements on
/// `(model id, license.identity())` so a license-text change forces a
/// fresh ack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelLicense {
    pub name: String,
    pub url: String,
    pub summary: String,
    pub restriction: LicenseRestriction,
    pub sha256: Option<String>,
}

impl ModelLicense {
    /// Stable identity of the *text* the user accepted: sha256 when pinned,
    /// otherwise a hash of the licence name and canonical URL. A registry
    /// correction to either value therefore prompts again.
    pub fn identity(&self) -> String {
        self.sha256
            .clone()
            .unwrap_or_else(|| {
                crate::sha256::sha256_hex(format!("{}\0{}", self.name, self.url).as_bytes())
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LicenseRestriction {
    /// Apache / MIT / BSD and similar permissive weight licenses.
    None,
    NonCommercial,
    Community,
    Restricted,
}

impl LicenseRestriction {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "none" => Some(Self::None),
            "non-commercial" => Some(Self::NonCommercial),
            "community" => Some(Self::Community),
            "restricted" => Some(Self::Restricted),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::NonCommercial => "non-commercial",
            Self::Community => "community",
            Self::Restricted => "restricted",
        }
    }

    pub fn needs_emphasis(self) -> bool {
        !matches!(self, Self::None)
    }
}

impl ModelSpec {
    /// True when every registry file is present in the cache (in downloaded
    /// or converted form — see [`FileSpec::is_present`]). Optional roles do
    /// not gate readiness.
    pub fn files_present(&self, cache_dir: &Path) -> bool {
        self.files
            .iter()
            .filter(|file| !file.optional)
            .all(|file| file.is_present(cache_dir))
    }

    pub fn file_by_role(&self, role: &str) -> Option<&FileSpec> {
        self.files
            .iter()
            .find(|file| file.role.as_deref() == Some(role))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Registry {
    pub models: Vec<ModelSpec>,
}

impl Registry {
    pub fn parse(text: &str) -> Result<Self, AssetAiError> {
        let wire = RegistryFileJson::deserialize_json(text)
            .map_err(|e| AssetAiError::Registry(format!("registry json: {e:?}")))?;
        let mut models = Vec::new();
        // Registry-wide logical cache paths are shared intentionally by
        // several models. They are safe only when every byte/provenance field
        // agrees; otherwise startup fails instead of silently aliasing two
        // artifacts to one file.
        let mut cache_identities: HashMap<String, (String, CacheIdentity)> = HashMap::new();
        for model in wire.models {
            let domain = Domain::parse(&model.domain).ok_or_else(|| {
                AssetAiError::Registry(format!(
                    "model {}: unknown domain {:?} (expected image|mesh|video|audio|text|speech|world|matte|depth|body|segment|rig|motion|music|paint|edit|upscale|control|inpaint|enhance|splat|vision|ocr|beats|stems|notes|sections|garment)",
                    model.id, model.domain
                ))
            })?;
            if model.id.is_empty() || model.backend.is_empty() {
                return Err(AssetAiError::Registry(format!(
                    "model {:?}: id and backend must be non-empty",
                    model.id
                )));
            }
            if models
                .iter()
                .any(|existing: &ModelSpec| existing.id == model.id)
            {
                return Err(AssetAiError::Registry(format!(
                    "duplicate model id {:?}",
                    model.id
                )));
            }
            let mut files = Vec::new();
            let mut roles = HashMap::<String, usize>::new();
            for file in model.files {
                let local = file.local.unwrap_or(false);
                // Local-only files are never fetched, so repo/path are
                // informational; downloadable files need all three.
                if file.cache_as.is_empty()
                    || (!local && (file.repo.is_empty() || file.path.is_empty()))
                {
                    return Err(AssetAiError::Registry(format!(
                        "model {}: file entries need repo, path and cache_as (local files: cache_as)",
                        model.id
                    )));
                }
                validate_cache_path(&file.cache_as).map_err(|message| {
                    AssetAiError::Registry(format!(
                        "model {}: bad cache_as {:?}: {message}",
                        model.id, file.cache_as
                    ))
                })?;
                if let Some(role) = &file.role {
                    if role.trim().is_empty() || role != role.trim() {
                        return Err(AssetAiError::Registry(format!(
                            "model {}: file role must be non-empty and trimmed",
                            model.id
                        )));
                    }
                    if roles.insert(role.clone(), files.len()).is_some() {
                        return Err(AssetAiError::Registry(format!(
                            "model {}: duplicate file role {:?}",
                            model.id, role
                        )));
                    }
                }
                if let Some(size) = file.size {
                    if size == 0 {
                        return Err(AssetAiError::Registry(format!(
                            "model {}: {} has zero declared size",
                            model.id, file.cache_as
                        )));
                    }
                }
                if let Some(sha256) = &file.sha256 {
                    validate_sha256(sha256).map_err(|message| {
                        AssetAiError::Registry(format!(
                            "model {}: {}: {message}",
                            model.id, file.cache_as
                        ))
                    })?;
                }
                if let Some(revision) = &file.revision {
                    if !is_commit_revision(revision) {
                        return Err(AssetAiError::Registry(format!(
                            "model {}: {} revision must be a full 40- or 64-hex immutable commit, got {:?}",
                            model.id, file.cache_as, revision
                        )));
                    }
                    if file.size.is_none() || file.sha256.is_none() {
                        return Err(AssetAiError::Registry(format!(
                            "model {}: pinned file {} requires nonzero size and 64-hex sha256",
                            model.id, file.cache_as
                        )));
                    }
                    if !file.path.starts_with("http://") && !file.path.starts_with("https://") {
                        validate_source_path(&file.repo, &file.path).map_err(|message| {
                            AssetAiError::Registry(format!(
                                "model {}: pinned file {}: {message}",
                                model.id, file.cache_as
                            ))
                        })?;
                    }
                }
                if file.converts_to.is_some() && file.conversion.is_some() {
                    return Err(AssetAiError::Registry(format!(
                        "model {}: {} declares both legacy converts_to and structured conversion",
                        model.id, file.cache_as
                    )));
                }
                if file.conversion.is_some()
                    && (file.revision.is_none() || file.size.is_none() || file.sha256.is_none())
                {
                    return Err(AssetAiError::Registry(format!(
                        "model {}: structured conversion for {} requires pinned source revision, size and sha256",
                        model.id, file.cache_as
                    )));
                }
                if let Some(converts_to) = &file.converts_to {
                    validate_cache_path(converts_to).map_err(|message| {
                        AssetAiError::Registry(format!(
                            "model {}: bad converts_to {:?}: {message}",
                            model.id, converts_to
                        ))
                    })?;
                }
                let conversion = file
                    .conversion
                    .map(|conversion| validate_conversion(&model.id, conversion))
                    .transpose()?;
                let optional = file.optional.unwrap_or(false);
                if optional && file.role.is_none() {
                    return Err(AssetAiError::Registry(format!(
                        "model {}: optional file {} must declare a role",
                        model.id, file.cache_as
                    )));
                }
                let spec = FileSpec {
                    role: file.role,
                    repo: file.repo,
                    path: file.path,
                    revision: file.revision.map(|value| value.to_ascii_lowercase()),
                    cache_as: file.cache_as,
                    size: file.size,
                    sha256: file.sha256.map(|s| s.to_ascii_lowercase()),
                    local,
                    optional,
                    converts_to: file.converts_to,
                    conversion,
                };
                validate_cache_identity(
                    &mut cache_identities,
                    &model.id,
                    &spec.cache_as,
                    CacheIdentity::Source(SourceIdentity::from_file(&spec)),
                )?;
                if let Some(conversion) = &spec.conversion {
                    validate_cache_identity(
                        &mut cache_identities,
                        &model.id,
                        &conversion.cache_as,
                        CacheIdentity::Converted(ConvertedIdentity {
                            source: SourceIdentity::from_file(&spec),
                            output_size: conversion.size,
                            output_sha256: conversion.sha256.clone(),
                            converter_id: conversion.converter_id.clone(),
                            converter_version: conversion.converter_version.clone(),
                        }),
                    )?;
                }
                files.push(spec);
            }
            // `0.0` is the documented explicit VRAM-free sentinel used by
            // testpattern; every other declared estimate must be positive.
            // Reject negative/non-finite override values instead of letting
            // residency silently reinterpret a typo as "no byte gate".
            if let Some(value) = model.vram_gb {
                if !value.is_finite() || value < 0.0 {
                    return Err(AssetAiError::Registry(format!(
                        "model {}: vram_gb must be a finite non-negative number, got {value}",
                        model.id
                    )));
                }
            }
            for (field, value) in [
                ("min_vram_gb", model.min_vram_gb),
                ("min_compute_cap", model.min_compute_cap),
            ] {
                if let Some(value) = value {
                    if !value.is_finite() || value <= 0.0 {
                        return Err(AssetAiError::Registry(format!(
                            "model {}: {field} must be a positive finite number, got {value}",
                            model.id
                        )));
                    }
                }
            }
            let license = match model.license {
                None => None,
                Some(license) => Some(validate_license(&model.id, license)?),
            };
            models.push(ModelSpec {
                id: model.id,
                domain,
                backend: model.backend,
                available: model.available,
                gated: model.gated,
                vram_gb: model.vram_gb,
                min_vram_gb: model.min_vram_gb,
                min_compute_cap: model.min_compute_cap,
                note: model.note,
                license,
                files,
            });
        }
        Ok(Registry { models })
    }

    pub fn embedded() -> Result<Self, AssetAiError> {
        Self::parse(EMBEDDED_REGISTRY)
    }

    pub fn load_file(path: &Path) -> Result<Self, AssetAiError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| AssetAiError::Registry(format!("read {}: {e}", path.display())))?;
        Self::parse(&text)
    }

    pub fn find(&self, id: &str) -> Option<&ModelSpec> {
        self.models.iter().find(|model| model.id == id)
    }
}

/// License record for `id` from the embedded registry. Used by the Asset UI
/// when a live box is old enough that `GET /models` has no license fields.
pub fn license_for_model(id: &str) -> Option<ModelLicense> {
    Registry::embedded()
        .ok()?
        .find(id)
        .and_then(|model| model.license.clone())
}

fn validate_license(model_id: &str, license: ModelLicenseJson) -> Result<ModelLicense, AssetAiError> {
    if license.name.trim().is_empty() || license.url.trim().is_empty() || license.summary.trim().is_empty()
    {
        return Err(AssetAiError::Registry(format!(
            "model {model_id}: license name, url and summary must be non-empty"
        )));
    }
    if !license.url.starts_with("https://") && !license.url.starts_with("http://") {
        return Err(AssetAiError::Registry(format!(
            "model {model_id}: license url must be http(s), got {:?}",
            license.url
        )));
    }
    let restriction = LicenseRestriction::parse(&license.restriction).ok_or_else(|| {
        AssetAiError::Registry(format!(
            "model {model_id}: unknown license restriction {:?} (expected none|non-commercial|community|restricted)",
            license.restriction
        ))
    })?;
    if let Some(sha256) = &license.sha256 {
        validate_sha256(sha256).map_err(|message| {
            AssetAiError::Registry(format!("model {model_id}: license sha256: {message}"))
        })?;
    }
    Ok(ModelLicense {
        name: license.name,
        url: license.url,
        summary: license.summary,
        restriction,
        sha256: license.sha256.map(|s| s.to_ascii_lowercase()),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CacheIdentity {
    Source(SourceIdentity),
    Converted(ConvertedIdentity),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceIdentity {
    repo: String,
    path: String,
    revision: String,
    size: Option<u64>,
    sha256: Option<String>,
    local: bool,
}

impl SourceIdentity {
    fn from_file(file: &FileSpec) -> Self {
        Self {
            repo: file.repo.clone(),
            path: file.path.clone(),
            revision: file.resolved_revision().to_string(),
            size: file.size,
            sha256: file.sha256.clone(),
            local: file.local,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConvertedIdentity {
    source: SourceIdentity,
    output_size: u64,
    output_sha256: String,
    converter_id: String,
    converter_version: String,
}

fn validate_cache_identity(
    seen: &mut HashMap<String, (String, CacheIdentity)>,
    model_id: &str,
    cache_as: &str,
    identity: CacheIdentity,
) -> Result<(), AssetAiError> {
    if let Some((previous_model, previous)) = seen.get(cache_as) {
        if previous != &identity {
            return Err(AssetAiError::Registry(format!(
                "cache path {cache_as:?} has conflicting identities in models {previous_model:?} and {model_id:?}"
            )));
        }
    } else {
        seen.insert(cache_as.to_string(), (model_id.to_string(), identity));
    }
    Ok(())
}

fn validate_cache_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return Err("must be a non-empty portable relative path");
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("must be normalized (no empty, '.' or '..' components)");
    }
    Ok(())
}

fn validate_source_path(repo: &str, path: &str) -> Result<(), &'static str> {
    if repo.is_empty()
        || repo.starts_with('/')
        || repo.contains('\\')
        || repo
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("repo must be a normalized relative HuggingFace repository name");
    }
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("source path must be normalized and relative");
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), &'static str> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("sha256 must contain exactly 64 hexadecimal digits");
    }
    Ok(())
}

fn is_commit_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_conversion(
    model_id: &str,
    conversion: RegistryConversionJson,
) -> Result<ConversionSpec, AssetAiError> {
    validate_cache_path(&conversion.cache_as).map_err(|message| {
        AssetAiError::Registry(format!(
            "model {model_id}: bad conversion cache_as {:?}: {message}",
            conversion.cache_as
        ))
    })?;
    if conversion.size == 0 {
        return Err(AssetAiError::Registry(format!(
            "model {model_id}: conversion {} has zero declared size",
            conversion.cache_as
        )));
    }
    validate_sha256(&conversion.sha256).map_err(|message| {
        AssetAiError::Registry(format!(
            "model {model_id}: conversion {}: {message}",
            conversion.cache_as
        ))
    })?;
    if conversion.converter_id.trim().is_empty()
        || conversion.converter_version.trim().is_empty()
        || conversion.converter_id != conversion.converter_id.trim()
        || conversion.converter_version != conversion.converter_version.trim()
    {
        return Err(AssetAiError::Registry(format!(
            "model {model_id}: conversion {} needs non-empty trimmed converter_id and converter_version",
            conversion.cache_as
        )));
    }
    Ok(ConversionSpec {
        cache_as: conversion.cache_as,
        size: conversion.size,
        sha256: conversion.sha256.to_ascii_lowercase(),
        converter_id: conversion.converter_id,
        converter_version: conversion.converter_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_registry_parses() {
        let registry = Registry::embedded().unwrap();
        assert!(registry.models.len() >= 4);

        // Music: the MiniMax-Music3 diffusers file set must stay fully
        // pinned (immutable revision + size + sha256 on every file) so pull
        // jobs are reproducible and resumable on any box.
        assert!(
            registry.find("pbr-testpattern").is_none(),
            "deterministic paint-test is crate-internal and must not advertise"
        );
        let beats = registry.find("beat-this").unwrap();
        assert_eq!(beats.domain, Domain::Beats);
        assert_eq!(beats.backend, "beats");
        assert_eq!(beats.vram_gb, Some(0.5));
        assert_eq!(beats.files.len(), 2);
        let final_weights = beats.file_by_role("weights").unwrap();
        assert_eq!(final_weights.size, Some(81_058_141));
        assert_eq!(
            final_weights.sha256.as_deref(),
            Some("8c328b45f59d8dd3dff219253ff6a8d6482be57d0133a29140e2febbf8eb8331")
        );
        assert!(final_weights.path.starts_with("https://cloud.cp.jku.at/"));
        assert!(beats.file_by_role("weights-small").unwrap().optional);
        let stems = registry.find("bs-roformer-4stem").unwrap();
        assert_eq!(stems.domain, Domain::Stems);
        assert_eq!(stems.backend, "stems");
        assert_eq!(stems.files.len(), 1);
        assert_eq!(stems.file_by_role("weights").unwrap().size, Some(527_385_512));
        let hunyuan = registry.find("hunyuan3d-paint-2.1").unwrap();
        assert_eq!(hunyuan.domain, Domain::Paint);
        assert_eq!(hunyuan.backend, "paint");
        let hunyuan_license = hunyuan.license.as_ref().expect("hunyuan license");
        assert_eq!(
            hunyuan_license.name,
            "Tencent Hunyuan 3D 2.1 Community License Agreement"
        );
        assert_eq!(
            hunyuan_license.restriction,
            LicenseRestriction::Community
        );
        assert_eq!(
            hunyuan_license.sha256.as_deref(),
            Some("5bd08f93b2d280bb26ff3eed5d3996fe47a9698b5f7785163928668d7fd578c6")
        );
        assert_eq!(hunyuan.files.len(), 3);
        for role in ["unet", "vae", "dino-conditioner"] {
            let file = hunyuan.file_by_role(role).unwrap();
            assert!(file.size.is_some() && file.sha256.is_some(), "{role}");
            assert!(
                file.cache_as.starts_with("paint21/"),
                "{role} cache_as {}",
                file.cache_as
            );
        }
        assert_eq!(
            hunyuan.file_by_role("vae").unwrap().cache_as,
            "paint21/vae/diffusion_pytorch_model.bin"
        );

        let music3 = registry.find("minimax-music3").unwrap();
        assert_eq!(music3.domain, Domain::Music);
        assert_eq!(music3.backend, "music3");
        assert!(music3.available && !music3.gated);
        assert_eq!(music3.files.len(), 27);
        for file in &music3.files {
            assert!(file.size.is_some() && file.sha256.is_some(), "{}", file.cache_as);
            assert!(
                file.cache_as.starts_with("music/MiniMax-Music3/"),
                "{}",
                file.cache_as
            );
            if !file.optional {
                assert_eq!(file.repo, "MiniMaxAI/MiniMax-Music3");
                assert_eq!(
                    file.revision.as_deref(),
                    Some("bd348f9c49ea3c1b39f33ace3436f8fad435f24e"),
                    "{} must pin the audited revision",
                    file.cache_as
                );
            }
        }
        // The diffusers component subset only — the ~29 GB of sglang-omni
        // serving artifacts must not sneak into the REQUIRED pull set. The
        // one .pth that does ride along is the DAV encoder for reference
        // audio, and only as an optional role.
        assert!(!music3.files.iter().any(|f| f.path.starts_with("qwen_7B/")
            || (f.path.ends_with(".pth") && !f.optional)));
        let required: u64 = music3
            .files
            .iter()
            .filter(|f| !f.optional)
            .map(|f| f.size.unwrap())
            .sum();
        assert_eq!(required, 28_517_609_106);
        // Reference-audio roles (music3.md): the official dav.pth encoder
        // half at the same audited revision, plus the SimpleTuner RVQ v4
        // encoder + its config pinned to one immutable commit. Optional:
        // text-only generation never needs them.
        let dav = music3.file_by_role("dav-pth").unwrap();
        assert!(dav.optional);
        assert_eq!(dav.repo, "MiniMaxAI/MiniMax-Music3");
        assert_eq!(dav.path, "dav.pth");
        assert_eq!(dav.revision.as_deref(), Some("bd348f9c49ea3c1b39f33ace3436f8fad435f24e"));
        assert_eq!(dav.size, Some(491_817_450));
        let rvq = music3.file_by_role("rvq-encoder").unwrap();
        assert!(rvq.optional);
        assert_eq!(rvq.repo, "SimpleTuner/open-rvq-encoder-minimax-music3");
        assert_eq!(rvq.revision.as_deref(), Some("326964c2f4edcc642c1ea116274dd2dd94081713"));
        assert_eq!(rvq.size, Some(676_055_232));
        assert!(rvq.cache_as.ends_with("_v4_169m_autoregressive_depth_recommended.safetensors"));
        let rvq_cfg = music3.file_by_role("rvq-encoder-config").unwrap();
        assert!(rvq_cfg.optional);
        assert_eq!(rvq_cfg.revision, rvq.revision);
        assert_eq!(rvq_cfg.size, Some(595));
        assert_eq!(
            music3.files.iter().filter(|f| f.optional).count(),
            3,
            "only the three reference roles are optional"
        );

        // Music Q4 tier: the official audio.cpp GGUF pack (default mix
        // Q4_0 LM + Q4_0 DiT + BF16 RVQ + F32 cond/vocoder + sidecar
        // tokenizer + LICENSE), every file pinned to one immutable revision.
        let music3_q4 = registry.find("minimax-music3-q4").unwrap();
        assert_eq!(music3_q4.domain, Domain::Music);
        assert_eq!(music3_q4.backend, "music3");
        assert!(music3_q4.available && !music3_q4.gated);
        assert_eq!(music3_q4.files.len(), 8);
        for file in &music3_q4.files {
            assert_eq!(file.repo, "audio-cpp/MiniMax-Music3-GGUF");
            assert_eq!(
                file.revision.as_deref(),
                Some("ed915d0748225e39b2b9b4eab354a20f66e30bc2"),
                "{} must pin the audited revision",
                file.cache_as
            );
            assert!(file.size.is_some() && file.sha256.is_some(), "{}", file.cache_as);
            assert!(file.role.is_some(), "{}", file.cache_as);
            assert!(
                file.cache_as.starts_with("music/MiniMax-Music3-Q4/"),
                "{}",
                file.cache_as
            );
        }
        // The default audio.cpp mix exactly — no q8/q4_k alternates in the
        // pull set, and the LICENSE ships with the weights.
        let q4_lm = music3_q4.file_by_role("lm-gguf").unwrap();
        assert_eq!(q4_lm.path, "language_model_q4_0.gguf");
        assert_eq!(q4_lm.size, Some(6_006_866_496));
        assert_eq!(
            music3_q4.file_by_role("dit-gguf").unwrap().path,
            "transformer_q4_0.gguf"
        );
        assert_eq!(
            music3_q4.file_by_role("rvq-gguf").unwrap().path,
            "rvq_depth_decoder_bf16.gguf"
        );
        assert!(music3_q4.file_by_role("license").is_some());
        let q4_total: u64 = music3_q4.files.iter().map(|f| f.size.unwrap()).sum();
        assert_eq!(q4_total, 9_024_133_343);

        let ace = registry.find("ace-step-1.5-xl").unwrap();
        assert_eq!(ace.domain, Domain::Music);
        assert_eq!(ace.backend, "ace");
        assert!(ace.available && !ace.gated);
        assert_eq!(ace.files.len(), 18);
        for file in &ace.files {
            assert_eq!(file.repo, "ACE-Step/acestep-v15-xl-turbo-diffusers");
            assert_eq!(
                file.revision.as_deref(),
                Some("b795de13d747aa94b5fd0ba81603f0316132c8b7"),
                "{} must pin the audited revision",
                file.cache_as
            );
            assert!(file.size.is_some() && file.sha256.is_some(), "{}", file.cache_as);
            assert!(
                file.cache_as.starts_with("music/ACE-Step-1.5-XL/"),
                "{}",
                file.cache_as
            );
        }
        let ace_total: u64 = ace.files.iter().map(|f| f.size.unwrap()).sum();
        assert_eq!(ace_total, 11_101_497_774);

        // The canonical flux IDs are combined single-file FP8 checkpoints:
        // exactly ONE file each, with the immutable Comfy-Org revision, the
        // exact byte size and the exact sha256 of the audited LFS object
        // pinned. The former split-model BF16 bundles (unet/ + vae/ +
        // text_encoders/ cache entries) are RETIRED — nothing may quietly
        // reintroduce a full-weight download route behind these IDs.
        let schnell = registry.find("flux1-schnell").unwrap();
        assert_eq!(schnell.domain, Domain::Image);
        assert_eq!(schnell.backend, "flux");
        assert!(schnell.available && !schnell.gated);
        assert_eq!(schnell.files.len(), 1);
        let schnell_ckpt = &schnell.files[0];
        assert_eq!(schnell_ckpt.repo, "Comfy-Org/flux1-schnell");
        assert_eq!(schnell_ckpt.path, "flux1-schnell-fp8.safetensors");
        assert_eq!(
            schnell_ckpt.cache_as,
            "checkpoints/flux1-schnell-fp8.safetensors"
        );
        assert_eq!(
            schnell_ckpt.revision.as_deref(),
            Some("44ea96fbcead75dfa908449883350ada44601791")
        );
        assert_eq!(schnell_ckpt.size, Some(17_236_328_572));
        assert_eq!(
            schnell_ckpt.sha256.as_deref(),
            Some("ead426278b49030e9da5df862994f25ce94ab2ee4df38b556ddddb3db093bf72")
        );

        // flux1-dev: same combined-FP8 contract; `gated: true` records the
        // FLUX.1 [dev] Non-Commercial License (the Comfy-Org mirror itself
        // is ungated).
        let dev = registry.find("flux1-dev").unwrap();
        assert!(dev.available);
        assert!(dev.gated);
        assert_eq!(dev.backend, "flux");
        assert_eq!(dev.files.len(), 1);
        let dev_ckpt = &dev.files[0];
        assert_eq!(dev_ckpt.repo, "Comfy-Org/flux1-dev");
        assert_eq!(dev_ckpt.path, "flux1-dev-fp8.safetensors");
        assert_eq!(dev_ckpt.cache_as, "checkpoints/flux1-dev-fp8.safetensors");
        assert_eq!(
            dev_ckpt.revision.as_deref(),
            Some("7ec07cd0cd2cb88298a80c905ad38250e1389880")
        );
        assert_eq!(dev_ckpt.size, Some(17_246_524_772));
        assert_eq!(
            dev_ckpt.sha256.as_deref(),
            Some("8e91b68084b53a7fc44ed2a3756d821e355ac1a7b6fe29be760c1db532f3d88a")
        );
        for model in [&schnell, &dev] {
            assert!(
                model.files.iter().all(|f| {
                    !f.cache_as.starts_with("unet/") && !f.cache_as.starts_with("text_encoders/")
                }),
                "{}: split-model BF16 routes are retired",
                model.id
            );
        }

        let klein = registry.find("flux2-klein-4b").unwrap();
        assert_eq!(klein.domain, Domain::Edit);
        assert_eq!(klein.backend, "flux2");
        assert!(klein.available && !klein.gated);
        assert_eq!(klein.files.len(), 5);
        assert_eq!(klein.files[0].path, "flux-2-klein-4b.safetensors");
        assert_eq!(klein.files[0].size, Some(7_751_105_712));
        assert_eq!(
            klein.files[0].sha256.as_deref(),
            Some("ec3d4e733a771f61c052fb4856c48b336c55eaf2c65487c2a1faeb9bbda7a343")
        );

        let trellis = registry.find("trellis-2").unwrap();
        assert_eq!(trellis.domain, Domain::Mesh);
        assert_eq!(trellis.backend, "trellis");
        assert!(trellis.available);
        assert_eq!(trellis.files.len(), 9);
        assert!(trellis.files.iter().all(|file| {
            file.role.is_some()
                && file.revision.is_some()
                && file.size.is_some()
                && file.sha256.as_deref().is_some_and(|sha| sha.len() == 64)
        }));
        let matte = trellis.file_by_role("native-matte").unwrap();
        assert_eq!(matte.repo, "ZhengPeng7/BiRefNet_HR-matting");
        assert_eq!(matte.size, Some(444_473_596));

        // Rig domain: production is the resident native runtime. The old
        // Torch/bpy path is deliberately a separately addressed oracle and
        // can never be selected as a fallback for the canonical model id.
        let rig = registry.find("skintokens").unwrap();
        assert_eq!(rig.domain, Domain::Rig);
        assert_eq!(rig.backend, "rig-native");
        assert!(rig.available);
        assert_eq!(rig.files.len(), 1);
        let checkpoint = rig.file_by_role("tokenrig-checkpoint").unwrap();
        assert_eq!(checkpoint.repo, "VAST-AI/SkinTokens");
        assert_eq!(
            checkpoint.revision.as_deref(),
            Some("79736cad0fd84de384d5eede659b4ebd24effe33")
        );
        assert_eq!(checkpoint.size, Some(1_131_603_979));
        let conversion = checkpoint.conversion.as_ref().unwrap();
        assert_eq!(
            conversion.cache_as,
            "rig/skintokens/tokenrig.bf16.safetensors"
        );
        assert_eq!(conversion.size, 1_190_606_876);
        assert_eq!(
            conversion.converter_id,
            "makepad-skintokens-lightning-bf16"
        );
        let rig_oracle = registry.find("skintokens-oracle").unwrap();
        assert_eq!(rig_oracle.backend, "rig-oracle");
        assert!(rig_oracle.files.is_empty());

        // Text domain: Qwen3.8 is the preferred prompt expander. Its audited
        // 24-GB-friendly quant is immutable and fully integrity-pinned; the
        // older 3.6 entry remains addressable for already provisioned boxes.
        let qwen38 = registry.find("qwen3.8-27b").unwrap();
        assert_eq!(qwen38.domain, Domain::Text);
        assert_eq!(qwen38.backend, "llm");
        assert!(qwen38.available && !qwen38.gated);
        assert_eq!(qwen38.vram_gb, Some(19.0));
        assert_eq!(qwen38.files.len(), 1);
        let qwen38_gguf = qwen38.file_by_role("llm-gguf").unwrap();
        assert_eq!(qwen38_gguf.repo, "unsloth/Qwen3.8-27B-GGUF");
        assert_eq!(qwen38_gguf.path, "Qwen3.8-27B-Q4_K_M.gguf");
        assert_eq!(
            qwen38_gguf.revision.as_deref(),
            Some("fe1e2a23d973adb629709749dc4f6756df66ef10")
        );
        assert_eq!(qwen38_gguf.size, Some(17_106_775_008));
        assert_eq!(
            qwen38_gguf.sha256.as_deref(),
            Some("7e78da5d7e3ae28d178121f58646953305f3e5bd3cb46f4a75584e8b6c6fe169")
        );
        assert!(!qwen38_gguf.local);

        // Speech domain: Kokoro downloads the upstream .pth/.pt and converts
        // in-process to the .mktts/.mkvoice format the loader reads.
        let kokoro = registry.find("kokoro").unwrap();
        assert_eq!(kokoro.domain, Domain::Speech);
        assert_eq!(kokoro.backend, "kokoro");
        assert!(kokoro.available);
        // 1 model + all 28 English voice packs.
        assert_eq!(kokoro.files.len(), 29);
        assert!(kokoro.files.iter().all(|file| !file.local));
        assert!(kokoro
            .files
            .iter()
            .all(|file| file.repo == "hexgrad/Kokoro-82M"));
        // Every upstream file lands under tts/upstream/ and declares its
        // converted form at the pre-existing cache paths, so boxes carrying
        // converted files never re-download.
        assert!(kokoro
            .files
            .iter()
            .all(|file| file.cache_as.starts_with("tts/upstream/")));
        let model = kokoro
            .files
            .iter()
            .find(|file| file.path == "kokoro-v1_0.pth")
            .unwrap();
        assert_eq!(model.cache_as, "tts/upstream/kokoro-v1_0.pth");
        assert_eq!(model.converts_to.as_deref(), Some("tts/kokoro-v1_0.mktts"));
        assert_eq!(model.size, Some(327_212_226));
        let daniel = kokoro
            .files
            .iter()
            .find(|file| file.path == "voices/bm_daniel.pt")
            .unwrap();
        assert_eq!(daniel.converts_to.as_deref(), Some("tts/bm_daniel.mkvoice"));
        assert_eq!(daniel.size, Some(523_430));

        // Video domain: MiniMax H3 through the in-repo diffusion port.
        let h3 = registry.find("minimax-h3").unwrap();
        assert_eq!(h3.domain, Domain::Video);
        assert_eq!(h3.backend, "h3");
        assert!(h3.available);
        // 61 upstream MiniMax files + the shared RIFE flownet.
        assert_eq!(h3.files.len(), 62);
        assert!(h3
            .files
            .iter()
            .filter(|f| f.role.as_deref() != Some("interpolate"))
            .all(|f| f.repo == "MiniMaxAI/MiniMax-H3"));
        // The dir anchor the backend resolves the model root from.
        assert!(h3
            .files
            .iter()
            .any(|f| f.cache_as == "video/MiniMax-H3/model_index.json"));
        // The pieces the pipeline reads.
        for prefix in [
            "video/MiniMax-H3/tokenizer/",
            "video/MiniMax-H3/text_encoder/",
            "video/MiniMax-H3/transformer/",
            "video/MiniMax-H3/vae/",
            "video/MiniMax-H3/audio_vae/",
        ] {
            assert!(
                h3.files.iter().any(|f| f.cache_as.starts_with(prefix)),
                "missing registry files under {prefix}"
            );
        }
        // Everything is downloadable from HF (no local-only conversions).
        assert!(h3.files.iter().all(|f| !f.local));
        let total: u64 = h3.files.iter().map(|f| f.size.unwrap_or(0)).sum();
        assert!(total > 130_000_000_000, "h3 file set should be ~134 GiB, got {total}");

        // Video quantized tiers: distinct pinned manifests per VRAM class,
        // selected by file ROLES (dit-gguf / dit-nvfp4) and hard-gated on
        // GPU capability. Every quantized-tier file is fully pinned
        // (immutable revision + size + sha256) so the peer cache can
        // distribute and verify them.
        let q4 = registry.find("minimax-h3-q4-24g").unwrap();
        assert_eq!(q4.domain, Domain::Video);
        assert_eq!(q4.backend, "h3");
        assert!(q4.available && !q4.gated);
        // Peak excludes the service's separate 2 GiB safety reserve. Using
        // the marketing card size here would double-count that reserve and
        // make this 24GB tier impossible to advertise on its measured 4090.
        assert_eq!(q4.vram_gb, Some(20.0));
        assert_eq!(q4.min_vram_gb, Some(22.0));
        assert_eq!(q4.min_compute_cap, Some(8.9));
        assert_eq!(q4.files.len(), 7);
        for file in &q4.files {
            assert!(
                file.role.is_some()
                    && file.revision.is_some()
                    && file.size.is_some()
                    && file.sha256.as_deref().is_some_and(|sha| sha.len() == 64),
                "q4 tier file {} must be fully pinned",
                file.cache_as
            );
            // The RIFE flownet is shared by every tier, so it lives outside
            // the tier root (one on-disk copy for the whole video domain).
            let expected_root = match file.role.as_deref() {
                Some("interpolate") => "video/rife/",
                _ => "video/MiniMax-H3-tiers/",
            };
            assert!(file.cache_as.starts_with(expected_root), "{}", file.cache_as);
        }
        let dit = q4.file_by_role("dit-gguf").unwrap();
        assert_eq!(dit.repo, "unsloth/MiniMax-H3-GGUF");
        assert_eq!(dit.path, "minimax_h3_fl2va_pruned-Q4_K.gguf");
        assert_eq!(dit.size, Some(11_420_663_904));
        let te = q4.file_by_role("te-gguf").unwrap();
        assert_eq!(te.size, Some(18_218_065_024));
        assert!(q4.file_by_role("tokenizer-json").is_some());
        assert!(q4.file_by_role("audio-vae-config").is_some());

        let nv4 = registry.find("minimax-h3-nvfp4-32g").unwrap();
        assert_eq!(nv4.backend, "h3");
        assert_eq!(nv4.vram_gb, Some(28.0));
        assert_eq!(nv4.min_vram_gb, Some(30.0));
        assert_eq!(nv4.min_compute_cap, Some(12.0));
        assert_eq!(nv4.files.len(), 7);
        let nv_dit = nv4.file_by_role("dit-nvfp4").unwrap();
        assert_eq!(nv_dit.repo, "Abiray/Minimax-H3-nvfp4-INT4-INT8-Convrot");
        assert_eq!(nv_dit.size, Some(12_528_636_865));
        let nv_te = nv4.file_by_role("te-nvfp4").unwrap();
        assert_eq!(nv_te.size, Some(15_687_142_619));
        // The VAE/tokenizer files are byte-identical across both quant tiers
        // (same cache path + identity -> one on-disk copy).
        for shared_role in ["video-vae", "audio-vae", "audio-vae-config", "tokenizer-json"] {
            let a = q4.file_by_role(shared_role).unwrap();
            let b = nv4.file_by_role(shared_role).unwrap();
            assert_eq!(a.cache_as, b.cache_as, "{shared_role}");
            assert_eq!(a.sha256, b.sha256, "{shared_role}");
            assert_eq!(a.revision, b.revision, "{shared_role}");
        }

        // bf16-96g mirrors the legacy minimax-h3 file set (same identities,
        // deliberately unpinned — seeded boxes carry no receipts) and adds
        // the hard VRAM gate.
        let bf16 = registry.find("minimax-h3-bf16-96g").unwrap();
        assert_eq!(bf16.backend, "h3");
        assert_eq!(bf16.min_vram_gb, Some(90.0));
        assert_eq!(bf16.files.len(), h3.files.len());
        for (a, b) in bf16.files.iter().zip(&h3.files) {
            assert_eq!(a.cache_as, b.cache_as);
            assert_eq!(a.repo, b.repo);
            assert_eq!(a.size, b.size);
        }
        // The legacy id keeps its no-gate behavior (running boxes/apps).
        assert_eq!(h3.min_vram_gb, None);
        assert_eq!(h3.min_compute_cap, None);

        // The fast video lane: FastVideo FastH3 (DMD2-distilled MiniMax H3)
        // = the bf16 tree with ONLY the transformer swapped. Every FastH3
        // file is pinned to the audited revision; every shared component
        // is the minimax-h3 entry verbatim (same cache path, same identity
        // — the registry refuses conflicting identities on one path), so a
        // box holding minimax-h3 pulls only the 70 GB transformer.
        let fast = registry.find("fasth3-4step").unwrap();
        assert_eq!(fast.domain, Domain::Video);
        assert_eq!(fast.backend, "fast");
        assert!(fast.available && !fast.gated);
        assert_eq!(fast.min_vram_gb, Some(90.0));
        assert_eq!(fast.min_compute_cap, Some(8.9));
        let fast_license = fast.license.as_ref().expect("fast license");
        assert_eq!(fast_license.name, "MiniMax H3 Community License");
        assert_eq!(fast_license.restriction, LicenseRestriction::Community);
        assert_eq!(
            fast_license.sha256.as_deref(),
            Some("59b99642b95ea21630e311198ddbfffbfe05aadba0c2f5d884cbdf4efcc90f44")
        );
        const FAST_REPO: &str = "FastVideo/FastVideo-FastH3-4-step-Preview-v1-VSA-DataFree";
        const FAST_REV: &str = "b65818d41939b5085451074fe8ca8b799f8d4921";
        let (own, shared): (Vec<_>, Vec<_>) = fast
            .files
            .iter()
            .partition(|f| f.cache_as.starts_with("video/FastH3-4step/"));
        // 14 shards + index + config + LICENSE + provenance + contract.
        assert_eq!(own.len(), 19);
        for file in &own {
            assert_eq!(file.repo, FAST_REPO, "{}", file.cache_as);
            assert_eq!(file.revision.as_deref(), Some(FAST_REV), "{}", file.cache_as);
            assert!(file.size.is_some() && file.sha256.is_some(), "{}", file.cache_as);
            assert!(!file.local);
        }
        for role in ["license", "provenance", "inference-contract", "dit-bf16"] {
            assert!(fast.file_by_role(role).is_some(), "fast lacks the {role} role");
        }
        assert_eq!(
            fast.file_by_role("dit-bf16").unwrap().cache_as,
            "video/FastH3-4step/transformer/diffusion_pytorch_model.safetensors.index.json"
        );
        assert_eq!(fast.file_by_role("license").unwrap().size, Some(17_604));
        let transformer_bytes: u64 = own
            .iter()
            .filter(|f| f.path.ends_with(".safetensors"))
            .map(|f| f.size.unwrap())
            .sum();
        assert_eq!(transformer_bytes, 70_099_582_760);
        assert_eq!(
            own.iter().filter(|f| f.path.ends_with(".safetensors")).count(),
            14
        );
        // Shared: every non-transformer minimax-h3 file, verbatim, plus the
        // RIFE flownet; none of the base transformer.
        let base_shared: Vec<&FileSpec> = h3
            .files
            .iter()
            .filter(|f| !f.cache_as.starts_with("video/MiniMax-H3/transformer/"))
            .collect();
        assert_eq!(shared.len(), base_shared.len());
        for (a, b) in shared.iter().zip(&base_shared) {
            assert_eq!(a.cache_as, b.cache_as);
            assert_eq!(a.repo, b.repo);
            assert_eq!(a.path, b.path);
            assert_eq!(a.size, b.size);
            assert_eq!(a.sha256, b.sha256);
            assert_eq!(a.revision, b.revision);
        }
        assert!(!fast
            .files
            .iter()
            .any(|f| f.cache_as.starts_with("video/MiniMax-H3/transformer/")));
        assert!(fast
            .files
            .iter()
            .any(|f| f.cache_as == "video/MiniMax-H3/model_index.json"));
        assert!(fast.note.as_deref().unwrap_or("").contains("DENSE attention"));

        // The RIFE v4.26 flownet is an AUXILIARY FILE of every video tier,
        // never a model of its own: the domain must keep exactly one
        // selectable generator per tier, so nothing may register it as an
        // entry the UI model list can pick.
        assert!(registry.find("rife").is_none());
        assert!(registry.models.iter().all(|model| !model.id.contains("rife")));
        let mut interpolate_files = Vec::new();
        for tier in [&h3, &q4, &nv4, &bf16, &fast] {
            let file = tier
                .file_by_role("interpolate")
                .unwrap_or_else(|| panic!("{} carries no interpolate role", tier.id));
            assert_eq!(file.repo, "Comfy-Org/frame_interpolation");
            assert_eq!(file.path, "frame_interpolation/rife_v4.26.safetensors");
            assert_eq!(
                file.revision.as_deref(),
                Some("9bca6366a22473ccee25602fa82b224d78413960")
            );
            assert_eq!(file.cache_as, "video/rife/rife_v4.26.safetensors");
            assert_eq!(file.size, Some(22_674_688));
            assert_eq!(
                file.sha256.as_deref(),
                Some("151874592c877740e5db11522f4514df569eeafb0a0fcb2696f16e9e8d317c94")
            );
            assert!(!file.local);
            interpolate_files.push(file);
        }
        // One identity, so one on-disk copy no matter how many tiers a box
        // pulls.
        for file in &interpolate_files {
            assert_eq!(file.cache_as, interpolate_files[0].cache_as);
            assert_eq!(file.sha256, interpolate_files[0].sha256);
        }

        // Audio domain #3: Woosh ships as GitHub release zips — absolute
        // URLs in `path` (the downloader uses them verbatim), each zip
        // declaring its extracted safetensors as the converted form so
        // boxes carrying extracted weights never re-download.
        let woosh = registry.find("woosh-sfx").unwrap();
        assert_eq!(woosh.domain, Domain::Audio);
        assert_eq!(woosh.backend, "woosh");
        assert!(woosh.available);
        assert_eq!(woosh.files.len(), 4);
        let zips: Vec<&FileSpec> = woosh
            .files
            .iter()
            .filter(|f| f.cache_as.ends_with(".zip"))
            .collect();
        assert_eq!(zips.len(), 3);
        for zip in &zips {
            assert!(zip.path.starts_with("https://github.com/SonyResearch/Woosh/releases/"));
            assert!(zip
                .converts_to
                .as_deref()
                .is_some_and(|c| c.starts_with("audio/woosh/checkpoints/")
                    && c.ends_with("/weights.safetensors")));
            assert!(zip.sha256.as_deref().is_some_and(|sha| sha.len() == 64));
            assert!(zip.size.is_some());
        }
        let roberta = woosh
            .files
            .iter()
            .find(|f| f.cache_as == "audio/woosh/tokenizer.json")
            .unwrap();
        assert_eq!(roberta.repo, "FacebookAI/roberta-large");
        assert!(roberta.converts_to.is_none());
        // The NC license guard lives in the note.
        assert!(woosh.note.as_deref().unwrap().contains("CC-BY-NC-4.0"));

        // Matte domain: pinned native BiRefNet CUDA artifact, shared at the
        // same cache path as TRELLIS's native matte stage.
        let matte = registry.find("birefnet-hr").unwrap();
        assert_eq!(matte.domain, Domain::Matte);
        assert_eq!(matte.backend, "matte-native");
        assert!(matte.available);
        assert_eq!(matte.files.len(), 1);
        let matte_weights = matte.file_by_role("native-matte").unwrap();
        assert_eq!(matte_weights.size, Some(444_473_596));
        assert_eq!(
            matte_weights.revision.as_deref(),
            Some("5d6b6f8adcb5b417c871b1d84ceaae9871355b7f")
        );
        assert_eq!(matte.vram_gb, Some(2.0));

        // Depth domain: pinned native DA3 CUDA artifact.
        let depth = registry.find("da3-metric-large").unwrap();
        assert_eq!(depth.domain, Domain::Depth);
        assert_eq!(depth.backend, "depth-native");
        assert!(depth.available);
        assert_eq!(depth.files.len(), 1);
        let depth_weights = depth.file_by_role("native-depth").unwrap();
        assert_eq!(depth_weights.size, Some(1_336_734_448));
        assert_eq!(
            depth_weights.revision.as_deref(),
            Some("4010e39f3634a45bc60553321fb49fb760bd594e")
        );
        assert_eq!(depth.vram_gb, Some(3.0));
        // The licensing guard lives in the note: the x.1 refreshes are NC.
        assert!(depth.note.as_deref().unwrap().contains("Apache-2.0"));

        // Body domain: the pinned native artifact.
        let native_body = registry.find("sam3dbody").unwrap();
        assert_eq!(native_body.domain, Domain::Body);
        assert_eq!(native_body.backend, "body-native");
        assert!(native_body.available && !native_body.gated);
        assert_eq!(native_body.vram_gb, Some(4.5));
        // The checkpoint plus the optional SAM 3.1 detector for `detect`
        // (the same artifact the segment entry pins, so one cache file).
        assert_eq!(native_body.files.len(), 2);
        let detector = native_body.file_by_role("native-segment").unwrap();
        assert!(detector.optional);
        assert_eq!(
            detector.cache_as,
            registry
                .find("sam3-1-multiplex")
                .unwrap()
                .file_by_role("native-segment")
                .unwrap()
                .cache_as
        );
        let body_weights = native_body.file_by_role("native-body").unwrap();
        assert_eq!(body_weights.repo, "Comfy-Org/sam-3d-body");
        assert_eq!(
            body_weights.path,
            "detection/sam_3d_body_dinov3_bf16.safetensors"
        );
        assert_eq!(
            body_weights.revision.as_deref(),
            Some("60476aced0b8de0a0e82a318c79a85061cc97434")
        );
        assert_eq!(body_weights.size, Some(2_830_737_652));
        assert_eq!(
            body_weights.sha256.as_deref(),
            Some("59fa45200c504c5b56625004d7d3385daf48c616613e88099e43bf83b3e249cf")
        );
        assert_eq!(
            body_weights.cache_as,
            "body/sam3dbody/sam_3d_body_dinov3_bf16.safetensors"
        );
        assert!(!body_weights.repo.starts_with("facebook/"));

        // Segment domain: pinned Comfy-Org SAM 3.1 multiplex CUDA artifact.
        let segment = registry.find("sam3-1-multiplex").unwrap();
        assert_eq!(segment.domain, Domain::Segment);
        assert_eq!(segment.backend, "segment-native");
        assert!(segment.available);
        assert_eq!(segment.files.len(), 1);
        let segment_weights = segment.file_by_role("native-segment").unwrap();
        assert_eq!(segment_weights.size, Some(1_745_546_848));
        assert_eq!(
            segment_weights.revision.as_deref(),
            Some("f38cd62b71494b53ac2b56ca36e24f3c8d565581")
        );
        assert_eq!(
            segment_weights.sha256.as_deref(),
            Some("9ba99c92703c2e8b4f47de2d34a539bb8e18923049e238b780d70dbe6368eb03")
        );
        assert_eq!(segment_weights.repo, "Comfy-Org/sam3.1");
        assert!(!segment_weights.repo.starts_with("facebook/"));
        assert_eq!(segment.vram_gb, Some(4.0));

        // Upscale domain: pinned native RealESRGAN x4plus CUDA artifact.
        let upscale = registry.find("realesrgan-x4plus").unwrap();
        assert_eq!(upscale.domain, Domain::Upscale);
        assert_eq!(upscale.backend, "upscale-native");
        assert!(upscale.available && !upscale.gated);
        assert_eq!(upscale.files.len(), 1);
        let upscale_weights = upscale.file_by_role("native-upscale").unwrap();
        assert_eq!(upscale_weights.repo, "Comfy-Org/Real-ESRGAN_repackaged");
        assert_eq!(upscale_weights.path, "RealESRGAN_x4plus.safetensors");
        assert_eq!(
            upscale_weights.revision.as_deref(),
            Some("ea19b4cd14f85a5b914eee8aa7ff77bc371039a0")
        );
        assert_eq!(upscale_weights.size, Some(66_857_836));
        assert_eq!(
            upscale_weights.sha256.as_deref(),
            Some("37f9a931c215f040aa6d50f711f2cb115f713c46df1d0d6469a8bd7bfe9a60bb")
        );
        assert!(upscale_weights.cache_as.starts_with("upscale/"));

        // Inpaint domain: flux1-fill-dev, a 4-file split bundle (no
        // Comfy-Org combined-checkpoint repack exists for Fill).
        let fill = registry.find("flux1-fill-dev").unwrap();
        assert_eq!(fill.domain, Domain::Inpaint);
        assert_eq!(fill.backend, "flux-fill");
        assert!(fill.available && fill.gated);
        assert_eq!(fill.files.len(), 4);
        let dit = fill.file_by_role("diffusion_model").unwrap();
        assert_eq!(dit.repo, "cudabenchmarktest/flux1-fill-dev-fp8");
        assert_eq!(dit.size, Some(11_902_539_328));
        assert_eq!(
            dit.sha256.as_deref(),
            Some("0320d505ca42bca99c5bd600b1839ced2b2e980ea985917965d411d98a710729")
        );
        assert!(dit.cache_as.starts_with("unet/"));
        let vae = fill.file_by_role("vae").unwrap();
        assert_eq!(vae.repo, "Kijai/flux-fp8");
        assert!(vae.cache_as.starts_with("vae/"));
        let clip_l = fill.file_by_role("clip_l").unwrap();
        assert_eq!(clip_l.repo, "comfyanonymous/flux_text_encoders");
        assert!(clip_l.cache_as.starts_with("text_encoders/"));
        let t5xxl = fill.file_by_role("t5xxl").unwrap();
        assert_eq!(t5xxl.repo, "comfyanonymous/flux_text_encoders");
        assert!(t5xxl.cache_as.starts_with("text_encoders/"));
        // Every pinned file has both size and sha256 (the RegistryEntryFileJson
        // doc contract: a revision pin must carry both).
        for file in &fill.files {
            assert!(file.revision.is_some());
            assert!(file.size.is_some());
            assert!(file.sha256.is_some());
        }
    }

    #[test]
    fn dest_path_splits_on_slash() {
        let file = FileSpec {
            role: None,
            repo: "r".into(),
            path: "p".into(),
            revision: None,
            cache_as: "unet/flux1-schnell.safetensors".into(),
            size: None,
            sha256: None,
            local: false,
            optional: false,
            converts_to: None,
            conversion: None,
        };
        let dest = file.dest_path(Path::new("cache"));
        let mut expected = PathBuf::from("cache");
        expected.push("unet");
        expected.push("flux1-schnell.safetensors");
        assert_eq!(dest, expected);
    }

    #[test]
    fn converted_form_counts_as_present() {
        let dir = std::env::temp_dir().join(format!(
            "makepad-asset-ai-registry-present-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tts")).unwrap();
        let file = FileSpec {
            role: None,
            repo: "hexgrad/Kokoro-82M".into(),
            path: "voices/af_heart.pt".into(),
            revision: None,
            cache_as: "tts/upstream/af_heart.pt".into(),
            size: None,
            sha256: None,
            local: false,
            optional: false,
            converts_to: Some("tts/af_heart.mkvoice".into()),
            conversion: None,
        };
        // Neither form on disk: absent.
        assert!(!file.is_present(&dir));
        // Only the converted form (the existing-box case): present.
        std::fs::write(dir.join("tts").join("af_heart.mkvoice"), b"x").unwrap();
        assert!(file.is_present(&dir));
        // Only the upstream form: also present (conversion needs no network).
        std::fs::remove_file(dir.join("tts").join("af_heart.mkvoice")).unwrap();
        std::fs::create_dir_all(dir.join("tts").join("upstream")).unwrap();
        std::fs::write(dir.join("tts").join("upstream").join("af_heart.pt"), b"x").unwrap();
        assert!(file.is_present(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_registries_without_converts_to_still_parse() {
        // The exact shape cache-dir override registries had before
        // converts_to existed (kokoro's old local:true entries).
        let old = "{\"models\":[{\"id\":\"kokoro\",\"domain\":\"speech\",\"backend\":\"kokoro\",\"available\":true,\"gated\":false,\"vram_gb\":0.5,\"note\":null,\"files\":[
            {\"repo\":\"hexgrad/Kokoro-82M\",\"path\":\"kokoro-v1_0.pth\",\"cache_as\":\"tts/kokoro-v1_0.mktts\",\"size\":null,\"sha256\":null,\"local\":true}]}]}";
        let registry = Registry::parse(old).unwrap();
        let file = &registry.find("kokoro").unwrap().files[0];
        assert!(file.local);
        assert!(file.converts_to.is_none());
        // No converted form declared: presence falls back to cache_as.
        assert!(file.converted_path(Path::new("cache")).is_none());
    }

    #[test]
    fn rejects_bad_converts_to() {
        let escape = "{\"models\":[{\"id\":\"x\",\"domain\":\"speech\",\"backend\":\"b\",\"available\":true,\"gated\":false,\"vram_gb\":null,\"note\":null,\"files\":[
            {\"repo\":\"r\",\"path\":\"p\",\"cache_as\":\"tts/upstream/x.pt\",\"size\":null,\"sha256\":null,\"local\":false,\"converts_to\":\"../evil\"}]}]}";
        assert!(Registry::parse(escape).is_err());
        let empty = "{\"models\":[{\"id\":\"x\",\"domain\":\"speech\",\"backend\":\"b\",\"available\":true,\"gated\":false,\"vram_gb\":null,\"note\":null,\"files\":[
            {\"repo\":\"r\",\"path\":\"p\",\"cache_as\":\"tts/upstream/x.pt\",\"size\":null,\"sha256\":null,\"local\":false,\"converts_to\":\"\"}]}]}";
        assert!(Registry::parse(empty).is_err());
    }

    #[test]
    fn rejects_bad_registries() {
        assert!(Registry::parse("{\"models\":[{\"id\":\"x\",\"domain\":\"nope\",\"backend\":\"b\",\"available\":true,\"gated\":false,\"vram_gb\":null,\"note\":null,\"files\":[]}]}").is_err());
        // duplicate ids
        let dup = "{\"models\":[
            {\"id\":\"x\",\"domain\":\"image\",\"backend\":\"b\",\"available\":true,\"gated\":false,\"vram_gb\":null,\"note\":null,\"files\":[]},
            {\"id\":\"x\",\"domain\":\"image\",\"backend\":\"b\",\"available\":true,\"gated\":false,\"vram_gb\":null,\"note\":null,\"files\":[]}]}";
        assert!(Registry::parse(dup).is_err());
        // path escape
        let escape = "{\"models\":[{\"id\":\"x\",\"domain\":\"image\",\"backend\":\"b\",\"available\":true,\"gated\":false,\"vram_gb\":null,\"note\":null,\"files\":[
            {\"repo\":\"r\",\"path\":\"p\",\"cache_as\":\"../evil\",\"size\":null,\"sha256\":null}]}]}";
        assert!(Registry::parse(escape).is_err());

        // A negative estimate used to be treated as VRAM-free by residency,
        // turning a registry typo into a fail-open admission policy. Explicit
        // zero remains valid for genuinely GPU-free models such as testpattern.
        let negative_vram = "{\"models\":[{\"id\":\"x\",\"domain\":\"image\",\"backend\":\"b\",\"available\":true,\"gated\":false,\"vram_gb\":-1.0,\"note\":null,\"files\":[]}]}";
        assert!(Registry::parse(negative_vram).is_err());
        let zero_vram = "{\"models\":[{\"id\":\"x\",\"domain\":\"image\",\"backend\":\"b\",\"available\":true,\"gated\":false,\"vram_gb\":0.0,\"note\":null,\"files\":[]}]}";
        assert!(Registry::parse(zero_vram).is_ok());
    }

    #[test]
    fn pinned_role_and_structured_conversion_parse() {
        let sha = "ab".repeat(32);
        let output_sha = "cd".repeat(32);
        let json = format!(r#"{{"models":[{{"id":"native","domain":"rig","backend":"rig-native","available":true,"gated":false,"vram_gb":1.0,"note":null,"files":[{{
            "role":"checkpoint_source","repo":"org/repo","path":"weights/model.ckpt",
            "revision":"79736cad0fd84de384d5eede659b4ebd24effe33","cache_as":"rig/upstream/model.ckpt",
            "size":123,"sha256":"{sha}","conversion":{{"cache_as":"rig/model.safetensors","size":99,
            "sha256":"{output_sha}","converter_id":"native-bf16","converter_version":"1"}}
        }}]}}]}}"#);
        let registry = Registry::parse(&json).unwrap();
        let model = registry.find("native").unwrap();
        let file = model.file_by_role("checkpoint_source").unwrap();
        assert_eq!(file.resolved_revision(), "79736cad0fd84de384d5eede659b4ebd24effe33");
        let conversion = file.conversion.as_ref().unwrap();
        assert_eq!(conversion.converter_id, "native-bf16");
        assert_eq!(conversion.cache_as, "rig/model.safetensors");
    }

    #[test]
    fn strict_pins_hashes_roles_and_paths_are_validated() {
        let base = |file: &str| format!(r#"{{"models":[{{"id":"x","domain":"rig","backend":"b","available":true,"gated":false,"vram_gb":null,"note":null,"files":[{file}]}}]}}"#);
        let good_sha = "11".repeat(32);
        for bad in [
            base(r#"{"role":"r","repo":"o/r","path":"p","revision":"main","cache_as":"x/p","size":1,"sha256":"1111"}"#),
            base(&format!(r#"{{"role":"r","repo":"o/r","path":"p","revision":"{}","cache_as":"x/p","size":0,"sha256":"{good_sha}"}}"#, "1".repeat(40))),
            base(&format!(r#"{{"role":"r","repo":"o/r","path":"../p","revision":"{}","cache_as":"x/p","size":1,"sha256":"{good_sha}"}}"#, "1".repeat(40))),
            base(r#"{"role":" ","repo":"o/r","path":"p","cache_as":"x/p","size":null,"sha256":null}"#),
        ] {
            assert!(Registry::parse(&bad).is_err(), "accepted {bad}");
        }
        let duplicate_role = base(r#"{"role":"same","repo":"o/r","path":"a","cache_as":"x/a","size":null,"sha256":null},{"role":"same","repo":"o/r","path":"b","cache_as":"x/b","size":null,"sha256":null}"#);
        assert!(Registry::parse(&duplicate_role).is_err());
    }

    #[test]
    fn registry_wide_cache_collisions_require_identical_source_identity() {
        let sha = "22".repeat(32);
        let revision = "2".repeat(40);
        let model = |id: &str, path: &str, sha: &str| format!(r#"{{"id":"{id}","domain":"image","backend":"b","available":true,"gated":false,"vram_gb":null,"note":null,"files":[{{"repo":"o/r","path":"{path}","revision":"{revision}","cache_as":"shared/w","size":7,"sha256":"{sha}"}}]}}"#);
        let identical = format!(r#"{{"models":[{},{}]}}"#, model("a", "w", &sha), model("b", "w", &sha));
        assert!(Registry::parse(&identical).is_ok());
        let conflict = format!(r#"{{"models":[{},{}]}}"#, model("a", "w", &sha), model("b", "other", &sha));
        let message = Registry::parse(&conflict).unwrap_err().to_string();
        assert!(message.contains("conflicting identities"), "{message}");
    }

    #[test]
    fn one_source_may_feed_distinct_structured_outputs() {
        let source_sha = "33".repeat(32);
        let output_a = "44".repeat(32);
        let output_b = "55".repeat(32);
        let revision = "3".repeat(40);
        let entry = |role: &str, output: &str, output_sha: &str| format!(r#"{{"role":"{role}","repo":"o/r","path":"w","revision":"{revision}","cache_as":"shared/source","size":7,"sha256":"{source_sha}","conversion":{{"cache_as":"{output}","size":9,"sha256":"{output_sha}","converter_id":"c","converter_version":"1"}}}}"#);
        let json = format!(r#"{{"models":[{{"id":"x","domain":"rig","backend":"b","available":true,"gated":false,"vram_gb":null,"note":null,"files":[{},{}]}}]}}"#, entry("a", "out/a", &output_a), entry("b", "out/b", &output_b));
        assert!(Registry::parse(&json).is_ok());
    }

    #[test]
    fn embedded_registry_requires_a_license_on_every_model() {
        let registry = Registry::embedded().unwrap();
        assert!(registry.models.len() >= 4);
        for model in &registry.models {
            let license = model
                .license
                .as_ref()
                .unwrap_or_else(|| panic!("model {} is missing a license record", model.id));
            assert!(
                !license.name.trim().is_empty(),
                "{} empty license name",
                model.id
            );
            assert!(
                license.url.starts_with("https://") || license.url.starts_with("http://"),
                "{} license url {}",
                model.id,
                license.url
            );
            assert!(
                !license.summary.trim().is_empty(),
                "{} empty license summary",
                model.id
            );
        }
        let flux_dev = license_for_model("flux1-dev").expect("flux1-dev license");
        assert_eq!(flux_dev.restriction, LicenseRestriction::NonCommercial);
        let schnell = license_for_model("flux1-schnell").expect("flux1-schnell license");
        assert_eq!(schnell.restriction, LicenseRestriction::None);
        assert!(license_for_model("no-such-model").is_none());
    }

    #[test]
    fn unknown_license_restriction_fails_closed() {
        let json = r#"{"models":[{"id":"x","domain":"image","backend":"b","available":true,"gated":false,"vram_gb":null,"note":null,"license":{"name":"X","url":"https://example.com/l","summary":"s","restriction":"copyleft"},"files":[]}]}"#;
        let message = Registry::parse(json).unwrap_err().to_string();
        assert!(message.contains("unknown license restriction"), "{message}");
    }

    #[test]
    fn local_app_domains_round_trip() {
        for (text, domain) in [
            ("beats", Domain::Beats),
            ("stems", Domain::Stems),
            ("notes", Domain::Notes),
            ("sections", Domain::Sections),
            ("garment", Domain::Garment),
        ] {
            assert_eq!(Domain::parse(text), Some(domain));
            assert_eq!(domain.as_str(), text);
        }
    }
}
