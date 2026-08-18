//! Pinned specification of the Hunyuan3D-Paint-2.1 backend: exact checkpoint
//! identity (revisions, byte sizes, sha256), architecture constants extracted
//! from the pinned configs/sources, runtime defaults, the honest channel
//! declaration, and license identity surfaced as provenance. Merely inspecting
//! this metadata is unrestricted, but obtaining a provisioning manifest for
//! the model blobs requires an explicit acknowledgement of the exact pinned
//! license digest. This is a fail-closed integration guard, not legal advice
//! or an attempt to interpret/enforce the license's substantive terms.
//!
//! Pinned upstream sources used for these specifications:
//! * weights: `tencent/Hunyuan3D-2.1` @ 0b94677654c57bb9a6b6845cd7b704ccf551d327
//! * pipeline code: `Tencent-Hunyuan/Hunyuan3D-2.1` @ 82920d643c0dc2f7bfd7255f45f62d386edfe60c
//! * conditioner: `facebook/dinov2-giant` @ 611a9d42f2335e0f921f1e313ad3c1b7178d206d

use crate::contract::{ChannelOrigin, PbrChannel};

pub const MODEL_ID: &str = "hunyuan3d-paint-2.1";

pub const WEIGHTS_REPO: &str = "tencent/Hunyuan3D-2.1";
pub const WEIGHTS_REVISION: &str = "0b94677654c57bb9a6b6845cd7b704ccf551d327";
pub const WEIGHTS_SUBFOLDER: &str = "hunyuan3d-paintpbr-v2-1";
pub const UPSTREAM_CODE_REPO: &str = "Tencent-Hunyuan/Hunyuan3D-2.1";
pub const UPSTREAM_CODE_REVISION: &str = "82920d643c0dc2f7bfd7255f45f62d386edfe60c";
pub const DINO_REPO: &str = "facebook/dinov2-giant";
pub const DINO_REVISION: &str = "611a9d42f2335e0f921f1e313ad3c1b7178d206d";

/// Whether a component is needed for inference. `LikelyUnusedVerify` marks the
/// CLIP text/image encoders: the pinned pipeline builds prompt embeddings from
/// learned per-material tokens and conditions on DINOv2, so both appear unused
/// at inference — this must be confirmed against the frozen oracle before the
/// components are dropped from provisioning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferenceUse {
    Required,
    LikelyUnusedVerify,
}

#[derive(Clone, Copy, Debug)]
pub struct ComponentFile {
    pub repo: &'static str,
    pub path: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub usage: InferenceUse,
}

/// Exact blob pins (Hugging Face LFS oids at the pinned revisions).
pub fn component_manifest() -> Vec<ComponentFile> {
    vec![
        ComponentFile {
            repo: WEIGHTS_REPO,
            path: "hunyuan3d-paintpbr-v2-1/unet/diffusion_pytorch_model.bin",
            bytes: 3_925_293_863,
            sha256: "675a1b5cd0098b2002637c443946529c03c5cd54427f40245263350feb3dd5b8",
            usage: InferenceUse::Required,
        },
        ComponentFile {
            repo: WEIGHTS_REPO,
            path: "hunyuan3d-paintpbr-v2-1/vae/diffusion_pytorch_model.bin",
            bytes: 334_707_217,
            sha256: "1b4889b6b1d4ce7ae320a02dedaeff1780ad77d415ea0d744b476155c6377ddc",
            usage: InferenceUse::Required,
        },
        ComponentFile {
            repo: DINO_REPO,
            path: "model.safetensors",
            bytes: 4_546_005_432,
            sha256: "917d3c470db999d32a312f8542149be91c7cbac61ee8fb4b67ae3d82b79ce21f",
            usage: InferenceUse::Required,
        },
        ComponentFile {
            repo: WEIGHTS_REPO,
            path: "hunyuan3d-paintpbr-v2-1/text_encoder/pytorch_model.bin",
            bytes: 1_361_671_895,
            sha256: "c3e254d7b61353497ea0be2c4013df4ea8f739ee88cffa0ba58cd085459ed565",
            usage: InferenceUse::LikelyUnusedVerify,
        },
        ComponentFile {
            repo: WEIGHTS_REPO,
            path: "hunyuan3d-paintpbr-v2-1/image_encoder/model.safetensors",
            bytes: 1_264_217_240,
            sha256: "ae616c24393dd1854372b0639e5541666f7521cbe219669255e865cb7f89466a",
            usage: InferenceUse::LikelyUnusedVerify,
        },
    ]
}

/// UNet2p5D architecture constants (unet/config.json + modules.py @ pins).
#[derive(Clone, Copy, Debug)]
pub struct UnetArch {
    /// conv_in channels: 4 noise + 4 normal-map latent + 4 position-map latent.
    pub in_channels: u32,
    pub out_channels: u32,
    pub block_out_channels: [u32; 4],
    pub attention_head_dim: [u32; 4],
    pub cross_attention_dim: u32,
    pub layers_per_block: u32,
    /// Latent spatial size at view resolution 512.
    pub sample_size: u32,
    /// Reference branch is a full UNet copy (dual-stream).
    pub dual_stream_reference: bool,
    /// Per-material attention weight clones (suffix `_mr`).
    pub material_attention: bool,
    /// DINOv2-giant feature dim and projected token count.
    pub dino_hidden_dim: u32,
    pub dino_proj_tokens: u32,
    /// Multiview 3D-RoPE multires grids (latent) and voxel resolutions.
    pub rope_grid_resolutions: [u32; 4],
    pub rope_voxel_resolutions: [u32; 4],
}

pub fn unet_arch() -> UnetArch {
    UnetArch {
        in_channels: 12,
        out_channels: 4,
        block_out_channels: [320, 640, 1280, 1280],
        attention_head_dim: [5, 10, 20, 20],
        cross_attention_dim: 1024,
        layers_per_block: 2,
        sample_size: 64,
        dual_stream_reference: true,
        material_attention: true,
        dino_hidden_dim: 1536,
        dino_proj_tokens: 4,
        rope_grid_resolutions: [64, 32, 16, 8],
        rope_voxel_resolutions: [512, 256, 128, 64],
    }
}

/// Runtime defaults from `Hunyuan3DPaintConfig` / `HunyuanPaintPipeline.__call__`.
#[derive(Clone, Copy, Debug)]
pub struct PaintDefaults {
    pub num_inference_steps: u32,
    pub guidance_scale: f32,
    /// CFG runs a 3-way batch: (negative, cond, cond).
    pub cfg_batch: u32,
    pub view_size: u32,
    pub num_view: u32,
    pub max_view: u32,
    pub render_size: u32,
    pub texture_size: u32,
    pub bake_exp: f32,
    pub camera_distance: f32,
    /// SD VAE scaling factor; from AutoencoderKL config family. Verified
    /// against the actual vae config at provision time.
    pub vae_scaling_factor: f32,
    /// Two PBR materials generated jointly: albedo + packed metallic-roughness.
    pub pbr_materials: u32,
}

pub fn defaults() -> PaintDefaults {
    PaintDefaults {
        num_inference_steps: 15,
        guidance_scale: 3.0,
        cfg_batch: 3,
        view_size: 512,
        num_view: 6,
        max_view: 9,
        render_size: 2048,
        texture_size: 4096,
        bake_exp: 4.0,
        camera_distance: 1.45,
        vae_scaling_factor: 0.18215,
        pbr_materials: 2,
    }
}

/// The model-level channel truth: what the diffusion model itself produces.
/// Normal and occlusion are *not* generated; the engine may later fill them
/// via geometry bakes, which changes their origin to `GeometryDerived` at the
/// integration layer — never silently.
pub fn declared_channels() -> [(PbrChannel, ChannelOrigin); 5] {
    [
        (
            PbrChannel::Albedo,
            ChannelOrigin::Generated {
                model: MODEL_ID.to_string(),
            },
        ),
        (
            PbrChannel::Normal,
            ChannelOrigin::Absent {
                reason: "hunyuan3d-paint-2.1 does not generate normal maps; engine may bake mesh normals".to_string(),
            },
        ),
        (
            PbrChannel::Roughness,
            ChannelOrigin::Generated {
                model: MODEL_ID.to_string(),
            },
        ),
        (
            PbrChannel::Metallic,
            ChannelOrigin::Generated {
                model: MODEL_ID.to_string(),
            },
        ),
        (
            PbrChannel::Occlusion,
            ChannelOrigin::Absent {
                reason: "not generated; per-asset AO baker available engine-side; packed ORM R is neutral 255".to_string(),
            },
        ),
    ]
}

pub const LICENSE_NAME: &str = "Tencent Hunyuan 3D 2.1 Community License Agreement";
/// sha256 of the exact LICENSE text at [`WEIGHTS_REVISION`]. Pinned so
/// provenance can state precisely which license revision shipped with the
/// weights.
pub const LICENSE_TEXT_SHA256: &str = "5bd08f93b2d280bb26ff3eed5d3996fe47a9698b5f7785163928668d7fd578c6";
/// Immutable upstream URL of that exact license text.
pub const LICENSE_URL: &str =
    "https://huggingface.co/tencent/Hunyuan3D-2.1/raw/0b94677654c57bb9a6b6845cd7b704ccf551d327/LICENSE";
/// Informational only — no enforcement. The license text names territory
/// exclusions (§1.l, §5(c)); this is surfaced verbatim in provenance so the
/// experimental UI and stored outputs are honest about the terms.
pub const LICENSE_TERRITORY_NOTE: &str =
    "license territory excludes the European Union, United Kingdom and South Korea, including outputs";

/// Opaque proof that the caller explicitly accepted the exact license text
/// pinned by [`LICENSE_TEXT_SHA256`]. The private field prevents downstream
/// code from manufacturing an acknowledgement without calling
/// [`acknowledge_license`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicenseAcknowledgement {
    license_text_sha256: &'static str,
}

impl LicenseAcknowledgement {
    pub fn license_text_sha256(&self) -> &'static str {
        self.license_text_sha256
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LicenseAcknowledgementError {
    NotAccepted,
    DigestMismatch { provided: String },
}

impl std::fmt::Display for LicenseAcknowledgementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAccepted => write!(
                f,
                "Hunyuan checkpoint provisioning requires explicit acceptance of {LICENSE_NAME}"
            ),
            Self::DigestMismatch { provided } => write!(
                f,
                "Hunyuan license digest mismatch: provided {provided}, expected {LICENSE_TEXT_SHA256}"
            ),
        }
    }
}

impl std::error::Error for LicenseAcknowledgementError {}

/// Construct an acknowledgement only after the caller has shown the pinned
/// license and received an explicit affirmative choice. Passing a stale or
/// different digest fails closed so a license update cannot be accepted by a
/// checkbox recorded for older text.
pub fn acknowledge_license(
    accepted: bool,
    presented_license_sha256: &str,
) -> Result<LicenseAcknowledgement, LicenseAcknowledgementError> {
    if !accepted {
        return Err(LicenseAcknowledgementError::NotAccepted);
    }
    if presented_license_sha256 != LICENSE_TEXT_SHA256 {
        return Err(LicenseAcknowledgementError::DigestMismatch {
            provided: presented_license_sha256.to_string(),
        });
    }
    Ok(LicenseAcknowledgement {
        license_text_sha256: LICENSE_TEXT_SHA256,
    })
}

/// The only model-blob provisioning entry point. Metadata callers may inspect
/// [`component_manifest`] without accepting anything, but download/cache code
/// must hold this acknowledgement before it receives a provisioning plan.
pub fn provisioning_manifest(
    acknowledgement: &LicenseAcknowledgement,
) -> Vec<ComponentFile> {
    debug_assert_eq!(
        acknowledgement.license_text_sha256,
        LICENSE_TEXT_SHA256
    );
    component_manifest()
}

/// Provenance pairs for the material manifest and UI: exact model identity,
/// pinned revisions, and license identity. This does not itself claim the
/// weights were provisioned or executed.
pub fn provenance() -> Vec<(String, String)> {
    vec![
        ("model".to_string(), MODEL_ID.to_string()),
        ("weights_repo".to_string(), WEIGHTS_REPO.to_string()),
        ("weights_revision".to_string(), WEIGHTS_REVISION.to_string()),
        ("code_repo".to_string(), UPSTREAM_CODE_REPO.to_string()),
        ("code_revision".to_string(), UPSTREAM_CODE_REVISION.to_string()),
        (
            "conditioner".to_string(),
            format!("{DINO_REPO}@{DINO_REVISION}"),
        ),
        ("license".to_string(), LICENSE_NAME.to_string()),
        ("license_sha256".to_string(), LICENSE_TEXT_SHA256.to_string()),
        ("license_url".to_string(), LICENSE_URL.to_string()),
        ("license_note".to_string(), LICENSE_TERRITORY_NOTE.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::candidate_views;

    #[test]
    fn manifest_totals_match_pins() {
        let manifest = component_manifest();
        assert_eq!(manifest.len(), 5);
        let total: u64 = manifest.iter().map(|c| c.bytes).sum();
        assert_eq!(total, 11_431_895_647);
        let required: u64 = manifest
            .iter()
            .filter(|c| c.usage == InferenceUse::Required)
            .map(|c| c.bytes)
            .sum();
        assert_eq!(required, 8_806_006_512);
        // A selectable model must carry an exact immutable pin for every blob.
        for c in &manifest {
            assert_eq!(c.sha256.len(), 64, "{} sha length", c.path);
            assert!(c.sha256.chars().all(|ch| ch.is_ascii_hexdigit()), "{}", c.path);
        }
    }

    #[test]
    fn revisions_are_full_shas() {
        for rev in [WEIGHTS_REVISION, UPSTREAM_CODE_REVISION, DINO_REVISION] {
            assert_eq!(rev.len(), 40);
            assert!(rev.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn arch_constants_consistent() {
        let arch = unet_arch();
        assert_eq!(arch.in_channels, 12);
        assert_eq!(arch.block_out_channels[0] / arch.attention_head_dim[0], 64);
        assert_eq!(arch.block_out_channels[3] / arch.attention_head_dim[3], 64);
        // RoPE voxel resolutions are 8x their latent grids.
        for (g, v) in arch
            .rope_grid_resolutions
            .iter()
            .zip(arch.rope_voxel_resolutions.iter())
        {
            assert_eq!(g * 8, *v);
        }
        let d = defaults();
        assert_eq!(d.view_size / 8, arch.sample_size);
        assert_eq!(d.num_inference_steps, 15);
        assert_eq!(d.cfg_batch, 3);
        assert_eq!(candidate_views().len(), 30);
        assert!(d.num_view >= 6 && d.max_view <= 9);
    }

    #[test]
    fn provenance_exposes_exact_pins() {
        let p = provenance();
        let get = |k: &str| p.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(get("weights_revision"), WEIGHTS_REVISION);
        assert_eq!(get("license_sha256").len(), 64);
        assert!(get("license_sha256").chars().all(|c| c.is_ascii_hexdigit()));
        assert!(get("license_url").starts_with("https://huggingface.co/tencent/Hunyuan3D-2.1/raw/0b94677"));
        assert!(get("license_note").contains("European Union"));
        assert!(get("conditioner").contains(DINO_REVISION));
    }

    #[test]
    fn provisioning_requires_explicit_current_license_acknowledgement() {
        assert_eq!(
            acknowledge_license(false, LICENSE_TEXT_SHA256),
            Err(LicenseAcknowledgementError::NotAccepted)
        );
        assert!(matches!(
            acknowledge_license(true, &"00".repeat(32)),
            Err(LicenseAcknowledgementError::DigestMismatch { .. })
        ));
        let acknowledgement = acknowledge_license(true, LICENSE_TEXT_SHA256).unwrap();
        assert_eq!(
            acknowledgement.license_text_sha256(),
            LICENSE_TEXT_SHA256
        );
        assert_eq!(
            provisioning_manifest(&acknowledgement).len(),
            component_manifest().len()
        );
    }

    #[test]
    fn declared_channels_are_honest() {
        let channels = declared_channels();
        let by_name = |c: PbrChannel| {
            channels
                .iter()
                .find(|(ch, _)| *ch == c)
                .map(|(_, o)| o.clone())
                .unwrap()
        };
        assert!(matches!(by_name(PbrChannel::Albedo), ChannelOrigin::Generated { .. }));
        assert!(matches!(by_name(PbrChannel::Roughness), ChannelOrigin::Generated { .. }));
        assert!(matches!(by_name(PbrChannel::Metallic), ChannelOrigin::Generated { .. }));
        assert!(matches!(by_name(PbrChannel::Normal), ChannelOrigin::Absent { .. }));
        assert!(matches!(by_name(PbrChannel::Occlusion), ChannelOrigin::Absent { .. }));
    }
}
