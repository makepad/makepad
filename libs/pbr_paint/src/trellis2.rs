//! Pinned specification of the TRELLIS.2 retexture backend — the permissively
//! licensed default for the PBR domain (MIT code + MIT weights; conditioner is
//! Meta's DINOv3 under its own custom license, surfaced in provenance).
//!
//! TRELLIS.2 ships an official mesh+image texturing mode
//! (`texturing_pipeline.json`, `Trellis2TexturingPipeline`): the existing
//! mesh is voxelized to O-Voxel structure, encoded to a shape SLAT by the
//! shape encoder, and the `imgshape2tex` flow DiT generates a texture SLAT
//! conditioned on DINOv3 image features; the sparse texture decoder emits
//! **6 per-voxel PBR channels: albedo RGB (0..2), metallic (3), roughness (4),
//! alpha (5)** which bake onto the mesh's UV atlas. This maps 1:1 onto our
//! ORM contract; normal and occlusion are honestly absent at the model level.
//!
//! Runtime reuse plan: the existing mesh-domain TRELLIS.2 stack has the same
//! tex flow + tex decoder primitives; the PBR domain still needs mesh
//! voxelization + shape-encoder + retexture orchestration.
//!
//! Oracle/canary plan (deterministic):
//! * oracle = upstream `Trellis2TexturingPipeline` @ the pinned code revision
//!   on a 24 GB CUDA reference host;
//! * taps: T1 DINOv3 tokens, T2 voxelized structure coords (exact-set match),
//!   T3 shape SLAT after encoder+normalization, T4 tex-flow output at steps
//!   {0,6,11} with fixed seed, T5 decoded per-voxel 6-channel attributes,
//!   T6 baked albedo/ORM texture digests;
//! * gates: T2 exact; neural taps cosine >= 0.999; bake within quantization;
//!   warm latency not slower than oracle; peak VRAM recorded on 24 GB, with a
//!   separate honest 16 GB canary (weights ~5.5 GB; unproven until measured).

use crate::contract::{ChannelOrigin, PbrChannel};

pub const MODEL_ID: &str = "trellis2-pbr";

pub const WEIGHTS_REPO: &str = "microsoft/TRELLIS.2-4B";
pub const WEIGHTS_REVISION: &str = "af44b45f2e35a493886929c6d786e563ec68364d";
pub const CODE_REPO: &str = "microsoft/TRELLIS.2";
pub const CODE_REVISION: &str = "75fbf0183001ed9876c8dbb35de6b68552ee08bd";
pub const WEIGHTS_LICENSE: &str = "MIT";
pub const CODE_LICENSE: &str = "MIT";

/// DINOv3 conditioner. Canonical repo is HF-gated; the fleet pins an ungated
/// mirror blob byte-identical by sha256. Meta's DINOv3 license is custom
/// (commercial use permitted with conditions) — surfaced, not enforced.
pub const DINO_REPO: &str = "facebook/dinov3-vitl16-pretrain-lvd1689m";
pub const DINO_CANONICAL_REVISION: &str = "ea8dc2863c51be0a264bab82070e3e8836b02d51";
pub const DINO_MIRROR_REPO: &str = "visualbruno/dinov3-vitl16-pretrain-lvd1689m";
pub const DINO_MIRROR_REVISION: &str = "8463e34549282813c2cbf67241b27c6fe8fa6321";
pub const DINO_LICENSE: &str = "DINOv3 License (Meta custom; commercial use permitted with conditions)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentUse {
    Required,
    /// 512-resolution tier variant (optional lower tier).
    OptionalTier,
    /// Only needed to build oracle fixtures (e.g. the texture encoder).
    OracleOnly,
}

#[derive(Clone, Copy, Debug)]
pub struct ComponentFile {
    pub repo: &'static str,
    pub path: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub license: &'static str,
    pub usage: ComponentUse,
}

/// Exact blob pins (HF LFS oids at the pinned revisions; the four fleet-shared
/// blobs match libs/game/asset-ai/registry.json byte-for-byte).
pub fn component_manifest() -> Vec<ComponentFile> {
    vec![
        ComponentFile {
            repo: WEIGHTS_REPO,
            path: "ckpts/shape_enc_next_dc_f16c32_fp16.safetensors",
            bytes: 708_797_208,
            sha256: "f37c5ff5b983b68e9946060000f09bc131f3e84318a2c8b7430a81e4b4636c41",
            license: WEIGHTS_LICENSE,
            usage: ComponentUse::Required,
        },
        ComponentFile {
            repo: WEIGHTS_REPO,
            path: "ckpts/slat_flow_imgshape2tex_dit_1_3B_1024_bf16.safetensors",
            bytes: 2_584_672_728,
            sha256: "580401269059a339b8318ab9ced459a13ba63391721c83a6c383198c29e77686",
            license: WEIGHTS_LICENSE,
            usage: ComponentUse::Required,
        },
        ComponentFile {
            repo: WEIGHTS_REPO,
            path: "ckpts/tex_dec_next_dc_f16c32_fp16.safetensors",
            bytes: 948_458_812,
            sha256: "97ea69addea2ecd9312910f5f548234665eef51c088386180b7cd5b258645e3c",
            license: WEIGHTS_LICENSE,
            usage: ComponentUse::Required,
        },
        ComponentFile {
            repo: DINO_MIRROR_REPO,
            path: "model.safetensors",
            bytes: 1_212_559_808,
            sha256: "dcb2e45127cccbf1601e5f42fef165eea275c8e5213197e8dcf3f48822718179",
            license: DINO_LICENSE,
            usage: ComponentUse::Required,
        },
        ComponentFile {
            repo: WEIGHTS_REPO,
            path: "ckpts/slat_flow_imgshape2tex_dit_1_3B_512_bf16.safetensors",
            bytes: 2_584_672_728,
            sha256: "8371aa1c5d13be79dcd5ddfd2cf3835e902e204dc34427169a1c702828e1a94d",
            license: WEIGHTS_LICENSE,
            usage: ComponentUse::OptionalTier,
        },
        ComponentFile {
            repo: WEIGHTS_REPO,
            path: "ckpts/tex_enc_next_dc_f16c32_fp16.safetensors",
            bytes: 708_797_208,
            sha256: "dd109f75f84b90fa411554ed6b0e4a87f430841163156fc0ebda2ebdc4752493",
            license: WEIGHTS_LICENSE,
            usage: ComponentUse::OracleOnly,
        },
    ]
}

/// Per-voxel PBR attribute layout of the texture decoder output
/// (upstream `pbr_attr_layout`).
pub const ATTR_ALBEDO_R: usize = 0;
pub const ATTR_ALBEDO_G: usize = 1;
pub const ATTR_ALBEDO_B: usize = 2;
pub const ATTR_METALLIC: usize = 3;
pub const ATTR_ROUGHNESS: usize = 4;
pub const ATTR_ALPHA: usize = 5;
pub const DECODER_OUT_CHANNELS: usize = 6;

/// `SLatFlowModel` config for the imgshape2tex DiT (1.3B, bf16).
#[derive(Clone, Copy, Debug)]
pub struct TexFlowArch {
    pub resolution: u32,
    pub in_channels: u32,
    pub out_channels: u32,
    pub model_channels: u32,
    pub cond_channels: u32,
    pub num_blocks: u32,
    pub num_heads: u32,
    pub mlp_ratio: f32,
    pub qk_rms_norm: bool,
}

pub fn tex_flow_arch() -> TexFlowArch {
    TexFlowArch {
        resolution: 64,
        in_channels: 64,
        out_channels: 32,
        model_channels: 1536,
        cond_channels: 1024,
        num_blocks: 30,
        num_heads: 12,
        mlp_ratio: 5.3334,
        qk_rms_norm: true,
    }
}

/// `FlowEulerGuidanceIntervalSampler` parameters from `texturing_pipeline.json`.
#[derive(Clone, Copy, Debug)]
pub struct TexSampler {
    pub steps: u32,
    pub guidance_strength: f32,
    pub guidance_interval: (f32, f32),
    pub guidance_rescale: f32,
    pub rescale_t: f32,
    pub sigma_min: f32,
}

pub fn tex_sampler() -> TexSampler {
    TexSampler {
        steps: 12,
        guidance_strength: 1.0,
        guidance_interval: (0.6, 0.9),
        guidance_rescale: 0.0,
        rescale_t: 3.0,
        sigma_min: 1e-5,
    }
}

/// SLAT channel normalization (mean/std, 32 channels each) pinned from
/// `texturing_pipeline.json` @ [`WEIGHTS_REVISION`].
pub const SHAPE_SLAT_MEAN: [f32; 32] = [
    0.781296, 0.018091, -0.495192, -0.558457, 1.060530, 0.093252, 1.518149, -0.933218,
    -0.732996, 2.604095, -0.118341, -2.143904, 0.495076, -2.179512, -2.130751, -0.996944,
    0.261421, -2.217463, 1.260067, -0.150213, 3.790713, 1.481266, -1.046058, -1.523667,
    -0.059621, 2.220780, 1.621212, 0.877230, 0.567247, -3.175944, -3.186688, 1.578665,
];
pub const SHAPE_SLAT_STD: [f32; 32] = [
    5.972266, 4.706852, 5.445010, 5.209927, 5.320220, 4.547237, 5.020802, 5.444004,
    5.226681, 5.683095, 4.831436, 5.286469, 5.652043, 5.367606, 5.525084, 4.730578,
    4.805265, 5.124013, 5.530808, 5.619001, 5.103930, 5.417670, 5.269677, 5.547194,
    5.634698, 5.235274, 6.110351, 5.511298, 6.237273, 4.879207, 5.347008, 5.405691,
];
pub const TEX_SLAT_MEAN: [f32; 32] = [
    3.501659, 2.212398, 2.226094, 0.251093, -0.026248, -0.687364, 0.439898, -0.928075,
    0.029398, -0.339596, -0.869527, 1.038479, -0.972385, 0.126042, -1.129303, 0.455149,
    -1.209521, 2.069067, 0.544735, 2.569128, -0.323407, 2.293000, -1.925608, -1.217717,
    1.213905, 0.971588, -0.023631, 0.106750, 2.021786, 0.250524, -0.662387, -0.768862,
];
pub const TEX_SLAT_STD: [f32; 32] = [
    2.665652, 2.743913, 2.765121, 2.595319, 3.037293, 2.291316, 2.144656, 2.911822,
    2.969419, 2.501689, 2.154811, 3.163343, 2.621215, 2.381943, 3.186697, 3.021588,
    2.295916, 3.234985, 3.233086, 2.260140, 2.874801, 2.810596, 3.292720, 2.674999,
    2.680878, 2.372054, 2.451546, 2.353556, 2.995195, 2.379849, 2.786195, 2.775190,
];

/// Conservative admission from the accepted fleet canary: the complete
/// TRELLIS.2 path reached approximately 19.5 GiB. Texture-only execution is
/// not assigned a smaller number until it has its own measurement.
pub fn admission_estimate_gb() -> f32 {
    19.5
}

/// The model-level channel truth for the ORM contract.
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
                reason: "TRELLIS.2 texture decoder does not emit normals; engine may bake mesh normals".to_string(),
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

/// Provenance pairs for manifests/UI: exact pins and license identities.
pub fn provenance() -> Vec<(String, String)> {
    vec![
        ("model".to_string(), MODEL_ID.to_string()),
        ("weights_repo".to_string(), WEIGHTS_REPO.to_string()),
        ("weights_revision".to_string(), WEIGHTS_REVISION.to_string()),
        ("weights_license".to_string(), WEIGHTS_LICENSE.to_string()),
        ("code_repo".to_string(), CODE_REPO.to_string()),
        ("code_revision".to_string(), CODE_REVISION.to_string()),
        ("code_license".to_string(), CODE_LICENSE.to_string()),
        (
            "conditioner".to_string(),
            format!("{DINO_REPO}@{DINO_CANONICAL_REVISION} (mirror {DINO_MIRROR_REPO}@{DINO_MIRROR_REVISION})"),
        ),
        ("conditioner_license".to_string(), DINO_LICENSE.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_totals_match_pins() {
        let manifest = component_manifest();
        assert_eq!(manifest.len(), 6);
        let total: u64 = manifest.iter().map(|c| c.bytes).sum();
        assert_eq!(total, 8_747_958_492);
        let required: u64 = manifest
            .iter()
            .filter(|c| c.usage == ComponentUse::Required)
            .map(|c| c.bytes)
            .sum();
        assert_eq!(required, 5_454_488_556);
        for c in &manifest {
            assert_eq!(c.sha256.len(), 64, "{}", c.path);
            assert!(c.sha256.chars().all(|ch| ch.is_ascii_hexdigit()));
        }
        // Exactly one non-MIT component: the DINOv3 conditioner.
        let non_mit: Vec<_> = manifest.iter().filter(|c| c.license != "MIT").collect();
        assert_eq!(non_mit.len(), 1);
        assert_eq!(non_mit[0].repo, DINO_MIRROR_REPO);
    }

    #[test]
    fn admission_uses_measured_full_pipeline_peak() {
        assert_eq!(admission_estimate_gb(), 19.5);
    }

    #[test]
    fn revisions_are_full_shas() {
        for rev in [
            WEIGHTS_REVISION,
            CODE_REVISION,
            DINO_CANONICAL_REVISION,
            DINO_MIRROR_REVISION,
        ] {
            assert_eq!(rev.len(), 40);
            assert!(rev.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn arch_and_sampler_consistent() {
        let arch = tex_flow_arch();
        assert_eq!(arch.model_channels / arch.num_heads, 128);
        assert_eq!(arch.out_channels, 32);
        assert_eq!(SHAPE_SLAT_MEAN.len(), 32);
        assert_eq!(TEX_SLAT_STD.len(), 32);
        // Spot values against the pinned pipeline json.
        assert!((SHAPE_SLAT_MEAN[0] - 0.781296).abs() < 1e-6);
        assert!((TEX_SLAT_MEAN[0] - 3.501659).abs() < 1e-6);
        assert!((TEX_SLAT_STD[31] - 2.775190).abs() < 1e-6);
        let s = tex_sampler();
        assert_eq!(s.steps, 12);
        assert_eq!(s.guidance_interval, (0.6, 0.9));
        assert!((s.rescale_t - 3.0).abs() < 1e-6);
    }

    #[test]
    fn attr_layout_covers_decoder_channels() {
        let all = [
            ATTR_ALBEDO_R,
            ATTR_ALBEDO_G,
            ATTR_ALBEDO_B,
            ATTR_METALLIC,
            ATTR_ROUGHNESS,
            ATTR_ALPHA,
        ];
        for (i, a) in all.iter().enumerate() {
            assert_eq!(*a, i);
        }
        assert_eq!(all.len(), DECODER_OUT_CHANNELS);
    }

    #[test]
    fn declared_channels_are_honest_and_provenance_pinned() {
        let channels = declared_channels();
        assert!(matches!(channels[0].1, ChannelOrigin::Generated { .. }));
        assert!(matches!(channels[1].1, ChannelOrigin::Absent { .. }));
        assert!(matches!(channels[4].1, ChannelOrigin::Absent { .. }));
        let p = provenance();
        let get = |k: &str| p.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(get("weights_license"), "MIT");
        assert!(get("conditioner").contains("dinov3-vitl16"));
        assert!(get("conditioner_license").contains("DINOv3"));
        assert!(admission_estimate_gb() <= 24.0);
    }
}
