use crate::clip::{ClipTokenizedPrompt, ClipTokenizer};
use crate::comfy::{
    FluxGenerationConfig, FluxPrompts, FluxWorkflow, FluxWorkflowFiles, FluxWorkflowKind,
};
use crate::t5::{T5TokenizedPrompt, T5Tokenizer};
use crate::{DiffusionError, Result};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry};
use std::path::{Path, PathBuf};

pub const FLUX_CLIP_L_MAX_LENGTH: usize = 77;
pub const FLUX_T5XXL_MAX_LENGTH: usize = 256;

/// Tensor-name prefixes of the four components inside a combined single-file
/// FLUX checkpoint (ComfyUI CheckpointLoaderSimple layout, e.g. the
/// Comfy-Org flux1-{schnell,dev}-fp8 bundles). Scoping a component view to
/// its prefix yields exactly the tensor naming of the standalone component
/// files, so the per-component loaders work unchanged. The two outer
/// `text_encoders.*.logit_scale` scalars sit outside the `transformer.`
/// prefixes and are deliberately excluded (nothing consumes them).
pub const FLUX_CKPT_PREFIX_DIFFUSION: &str = "model.diffusion_model.";
pub const FLUX_CKPT_PREFIX_CLIP_L: &str = "text_encoders.clip_l.transformer.";
pub const FLUX_CKPT_PREFIX_T5XXL: &str = "text_encoders.t5xxl.transformer.";
pub const FLUX_CKPT_PREFIX_VAE: &str = "vae.";

#[derive(Clone, Debug)]
pub struct ComfyModelRoots {
    pub root_dir: PathBuf,
    pub unet_dir: PathBuf,
    pub vae_dir: PathBuf,
    pub text_encoders_dir: PathBuf,
    pub checkpoints_dir: PathBuf,
}

impl ComfyModelRoots {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        let root_dir = root_dir.as_ref().to_path_buf();
        let model_base = if root_dir.join("models").is_dir() {
            root_dir.join("models")
        } else {
            root_dir.clone()
        };
        Self {
            unet_dir: model_base.join("unet"),
            vae_dir: model_base.join("vae"),
            text_encoders_dir: model_base.join("text_encoders"),
            checkpoints_dir: model_base.join("checkpoints"),
            root_dir,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FluxResolvedBundle {
    pub kind: FluxWorkflowKind,
    pub diffusion_model_path: PathBuf,
    pub vae_path: Option<PathBuf>,
    pub clip_l_path: Option<PathBuf>,
    pub t5xxl_path: Option<PathBuf>,
}

/// Per-component tensor-name scope inside each component's file: `None` for
/// split-model bundles (standalone files are already scoped), the ComfyUI
/// combined-checkpoint prefixes when every path points at one checkpoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FluxComponentPrefixes {
    pub diffusion: Option<&'static str>,
    pub clip_l: Option<&'static str>,
    pub t5xxl: Option<&'static str>,
    pub vae: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct FluxBundleHeaders {
    pub diffusion_model: MlxSafetensorsHeader,
    pub vae: Option<MlxSafetensorsHeader>,
    pub clip_l: Option<MlxSafetensorsHeader>,
    pub t5xxl: Option<MlxSafetensorsHeader>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FluxTensorNameStyle {
    Canonical,
    Diffusers,
    Mixed,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FluxTransformerInspection {
    pub tensor_name_style: FluxTensorNameStyle,
    pub canonical_tensor_count: usize,
    pub config: FluxTransformerConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipLTextEncoderConfig {
    pub vocab_size: u32,
    pub max_position_embeddings: u32,
    pub hidden_size: u32,
    pub intermediate_size: u32,
    pub layer_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct T5TextEncoderConfig {
    pub vocab_size: u32,
    pub model_dim: u32,
    pub feedforward_dim: u32,
    pub layer_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FluxBundleInspection {
    pub transformer: FluxTransformerInspection,
    pub clip_l: Option<ClipLTextEncoderConfig>,
    pub t5xxl: Option<T5TextEncoderConfig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FluxTransformerConfig {
    pub patch_size: u32,
    pub in_channels: u32,
    pub out_channels: u32,
    pub vec_in_dim: u32,
    pub context_in_dim: u32,
    pub hidden_size: u32,
    pub num_heads: u32,
    pub depth: u32,
    pub depth_single_blocks: u32,
    pub theta: u32,
    pub guidance_embed: bool,
    pub qkv_bias: bool,
    pub axes_dim: [u32; 3],
}

impl FluxTransformerConfig {
    pub const fn flux1_dev() -> Self {
        Self {
            patch_size: 2,
            in_channels: 64,
            out_channels: 64,
            vec_in_dim: 768,
            context_in_dim: 4096,
            hidden_size: 3072,
            num_heads: 24,
            depth: 19,
            depth_single_blocks: 38,
            theta: 10_000,
            guidance_embed: true,
            qkv_bias: true,
            axes_dim: [16, 56, 56],
        }
    }

    pub const fn head_dim(self) -> u32 {
        self.hidden_size / self.num_heads
    }

    pub const fn axes_dim_sum(self) -> u32 {
        self.axes_dim[0] + self.axes_dim[1] + self.axes_dim[2]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FluxLatentShape {
    pub image_width: u32,
    pub image_height: u32,
    pub latent_width: u32,
    pub latent_height: u32,
    pub latent_channels: u32,
    pub packed_width: u32,
    pub packed_height: u32,
    pub transformer_channels: u32,
    pub image_token_count: u32,
}

impl FluxLatentShape {
    pub fn from_image_size(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(DiffusionError::workflow(
                "image width and height must both be non-zero",
            ));
        }
        if width % 16 != 0 || height % 16 != 0 {
            return Err(DiffusionError::workflow(format!(
                "FLUX image size must be divisible by 16, got {}x{}",
                width, height
            )));
        }

        let latent_width = width / 8;
        let latent_height = height / 8;
        let packed_width = latent_width / 2;
        let packed_height = latent_height / 2;
        let image_token_count = packed_width
            .checked_mul(packed_height)
            .ok_or_else(|| DiffusionError::workflow("FLUX packed token count overflow"))?;

        Ok(Self {
            image_width: width,
            image_height: height,
            latent_width,
            latent_height,
            latent_channels: 16,
            packed_width,
            packed_height,
            transformer_channels: 64,
            image_token_count,
        })
    }
}

pub fn pack_flux_latents_nchw(
    latents: &[f32],
    batch_size: u32,
    latent_height: u32,
    latent_width: u32,
) -> Result<Vec<f32>> {
    if latent_height % 2 != 0 || latent_width % 2 != 0 {
        return Err(DiffusionError::workflow(format!(
            "FLUX latent size must be even, got {}x{}",
            latent_width, latent_height
        )));
    }

    let channels = 16usize;
    let batch = batch_size as usize;
    let h = latent_height as usize;
    let w = latent_width as usize;
    let expected = batch
        .checked_mul(channels)
        .and_then(|value| value.checked_mul(h))
        .and_then(|value| value.checked_mul(w))
        .ok_or_else(|| DiffusionError::workflow("FLUX latent buffer size overflow"))?;
    if latents.len() != expected {
        return Err(DiffusionError::workflow(format!(
            "FLUX latent pack expected {} values for {}x{}x{}x{}, got {}",
            expected,
            batch_size,
            channels,
            latent_height,
            latent_width,
            latents.len()
        )));
    }

    let packed_h = h / 2;
    let packed_w = w / 2;
    let tokens = packed_h * packed_w;
    let mut packed = vec![0.0f32; batch * tokens * 64];

    for b in 0..batch {
        for c in 0..channels {
            for y in 0..h {
                for x in 0..w {
                    let token_y = y / 2;
                    let token_x = x / 2;
                    let token = token_y * packed_w + token_x;
                    let feature = c * 4 + (y % 2) * 2 + (x % 2);
                    let src = (((b * channels + c) * h + y) * w) + x;
                    let dst = ((b * tokens + token) * 64) + feature;
                    packed[dst] = latents[src];
                }
            }
        }
    }

    Ok(packed)
}

pub fn unpack_flux_latents_nchw(
    packed: &[f32],
    batch_size: u32,
    latent_height: u32,
    latent_width: u32,
) -> Result<Vec<f32>> {
    if latent_height % 2 != 0 || latent_width % 2 != 0 {
        return Err(DiffusionError::workflow(format!(
            "FLUX latent size must be even, got {}x{}",
            latent_width, latent_height
        )));
    }

    let channels = 16usize;
    let batch = batch_size as usize;
    let h = latent_height as usize;
    let w = latent_width as usize;
    let packed_h = h / 2;
    let packed_w = w / 2;
    let tokens = packed_h * packed_w;
    let expected = batch
        .checked_mul(tokens)
        .and_then(|value| value.checked_mul(64))
        .ok_or_else(|| DiffusionError::workflow("FLUX packed latent buffer size overflow"))?;
    if packed.len() != expected {
        return Err(DiffusionError::workflow(format!(
            "FLUX latent unpack expected {} packed values for {} tokens, got {}",
            expected,
            tokens,
            packed.len()
        )));
    }

    let mut latents = vec![0.0f32; batch * channels * h * w];
    for b in 0..batch {
        for token_y in 0..packed_h {
            for token_x in 0..packed_w {
                let token = token_y * packed_w + token_x;
                for c in 0..channels {
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let feature = c * 4 + dy * 2 + dx;
                            let dst_y = token_y * 2 + dy;
                            let dst_x = token_x * 2 + dx;
                            let src = ((b * tokens + token) * 64) + feature;
                            let dst = (((b * channels + c) * h + dst_y) * w) + dst_x;
                            latents[dst] = packed[src];
                        }
                    }
                }
            }
        }
    }

    Ok(latents)
}

#[derive(Clone, Debug)]
pub struct FluxPromptToImagePlan {
    pub workflow_path: PathBuf,
    pub kind: FluxWorkflowKind,
    pub bundle: FluxResolvedBundle,
    pub prompts: FluxPrompts,
    pub generation: FluxGenerationConfig,
    pub latent_shape: FluxLatentShape,
    pub transformer: FluxTransformerConfig,
}

impl FluxPromptToImagePlan {
    /// Asset-server entry: resolved files + request params. No Comfy graph.
    pub fn from_files(
        bundle: FluxResolvedBundle,
        prompts: FluxPrompts,
        generation: FluxGenerationConfig,
    ) -> Result<Self> {
        let latent_shape =
            FluxLatentShape::from_image_size(generation.width, generation.height)?;
        let transformer = inspect_diffusion_config(&bundle)?;
        Ok(Self {
            workflow_path: bundle.diffusion_model_path.clone(),
            kind: bundle.kind,
            bundle,
            prompts,
            generation,
            latent_shape,
            transformer,
        })
    }

    pub fn from_workflow(workflow: &FluxWorkflow, roots: &ComfyModelRoots) -> Result<Self> {
        let bundle =
            FluxResolvedBundle::from_workflow_files(workflow.kind, &workflow.files, roots)?;
        Self::from_files(bundle, workflow.prompts.clone(), workflow.generation.clone())
    }
}

fn inspect_diffusion_config(bundle: &FluxResolvedBundle) -> Result<FluxTransformerConfig> {
    let path = &bundle.diffusion_model_path;
    if crate::flux_gguf::is_gguf_path(path) {
        return Ok(crate::flux_gguf::inspect(path)?.transformer.config);
    }
    let diffusion_header = match bundle.component_prefixes().diffusion {
        Some(prefix) => {
            let full = MlxSafetensorsHeader::load(path)?;
            validate_flux_combined_checkpoint(&full)?;
            full.scoped_to_prefix(prefix)?
        }
        None => MlxSafetensorsHeader::load(path)?,
    };
    Ok(FluxTransformerInspection::from_header(&diffusion_header)?.config)
}

pub fn tokenize_flux_clip_l_prompt(prompt: &str) -> Result<ClipTokenizedPrompt> {
    ClipTokenizer::new()?.tokenize_chunks(prompt, FLUX_CLIP_L_MAX_LENGTH, true)
}

pub fn tokenize_flux_t5xxl_prompt(prompt: &str) -> Result<T5TokenizedPrompt> {
    T5Tokenizer::new()?.tokenize(prompt, FLUX_T5XXL_MAX_LENGTH, true)
}

impl FluxResolvedBundle {
    pub fn from_checkpoint(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let checkpoint = require_file(path.as_ref().to_path_buf(), "checkpoint")?;
        Ok(Self {
            kind: FluxWorkflowKind::Checkpoint,
            diffusion_model_path: checkpoint.clone(),
            vae_path: Some(checkpoint.clone()),
            clip_l_path: Some(checkpoint.clone()),
            t5xxl_path: Some(checkpoint),
        })
    }

    pub fn from_split(
        dit: impl AsRef<std::path::Path>,
        vae: impl AsRef<std::path::Path>,
        clip_l: Option<impl AsRef<std::path::Path>>,
        t5xxl: Option<impl AsRef<std::path::Path>>,
    ) -> Result<Self> {
        Ok(Self {
            kind: FluxWorkflowKind::SplitModel,
            diffusion_model_path: require_file(dit.as_ref().to_path_buf(), "diffusion model")?,
            vae_path: Some(require_file(vae.as_ref().to_path_buf(), "VAE")?),
            clip_l_path: clip_l
                .map(|p| require_file(p.as_ref().to_path_buf(), "clip_l"))
                .transpose()?,
            t5xxl_path: t5xxl
                .map(|p| require_file(p.as_ref().to_path_buf(), "t5xxl"))
                .transpose()?,
        })
    }

    pub fn from_workflow_files(
        kind: FluxWorkflowKind,
        files: &FluxWorkflowFiles,
        roots: &ComfyModelRoots,
    ) -> Result<Self> {
        match kind {
            FluxWorkflowKind::SplitModel => Ok(Self {
                kind,
                diffusion_model_path: require_file(
                    roots
                        .unet_dir
                        .join(require_name(&files.unet_name, "unet_name")?),
                    "diffusion model",
                )?,
                vae_path: Some(require_file(
                    roots
                        .vae_dir
                        .join(require_name(&files.vae_name, "vae_name")?),
                    "VAE",
                )?),
                clip_l_path: Some(require_file(
                    roots
                        .text_encoders_dir
                        .join(require_name(&files.clip_l_name, "clip_l_name")?),
                    "clip_l",
                )?),
                t5xxl_path: Some(require_file(
                    roots
                        .text_encoders_dir
                        .join(require_name(&files.t5xxl_name, "t5xxl_name")?),
                    "t5xxl",
                )?),
            }),
            FluxWorkflowKind::Checkpoint => {
                // One combined file carries all four components; every path
                // points at it so warm-reuse keys, device-cache namespaces
                // and whole-checkpoint eviction share one identity.
                let checkpoint = require_file(
                    roots
                        .checkpoints_dir
                        .join(require_name(&files.checkpoint_name, "ckpt_name")?),
                    "checkpoint",
                )?;
                Ok(Self {
                    kind,
                    diffusion_model_path: checkpoint.clone(),
                    vae_path: Some(checkpoint.clone()),
                    clip_l_path: Some(checkpoint.clone()),
                    t5xxl_path: Some(checkpoint),
                })
            }
        }
    }

    /// Tensor-name scoping for each component's file (see
    /// [`FluxComponentPrefixes`]).
    pub fn component_prefixes(&self) -> FluxComponentPrefixes {
        match self.kind {
            FluxWorkflowKind::SplitModel => FluxComponentPrefixes::default(),
            FluxWorkflowKind::Checkpoint => FluxComponentPrefixes {
                diffusion: Some(FLUX_CKPT_PREFIX_DIFFUSION),
                clip_l: Some(FLUX_CKPT_PREFIX_CLIP_L),
                t5xxl: Some(FLUX_CKPT_PREFIX_T5XXL),
                vae: Some(FLUX_CKPT_PREFIX_VAE),
            },
        }
    }

    pub fn inspect_headers(&self) -> Result<FluxBundleHeaders> {
        let prefixes = self.component_prefixes();
        Ok(FluxBundleHeaders {
            diffusion_model: flux_component_header(&self.diffusion_model_path, prefixes.diffusion)?,
            vae: self
                .vae_path
                .as_ref()
                .map(|path| flux_component_header(path, prefixes.vae))
                .transpose()?,
            clip_l: self
                .clip_l_path
                .as_ref()
                .map(|path| flux_component_header(path, prefixes.clip_l))
                .transpose()?,
            t5xxl: self
                .t5xxl_path
                .as_ref()
                .map(|path| flux_component_header(path, prefixes.t5xxl))
                .transpose()?,
        })
    }
}

impl FluxBundleHeaders {
    pub fn inspect_bundle(&self) -> Result<FluxBundleInspection> {
        Ok(FluxBundleInspection {
            transformer: FluxTransformerInspection::from_header(&self.diffusion_model)?,
            clip_l: self
                .clip_l
                .as_ref()
                .map(ClipLTextEncoderConfig::from_header)
                .transpose()?,
            t5xxl: self
                .t5xxl
                .as_ref()
                .map(T5TextEncoderConfig::from_header)
                .transpose()?,
        })
    }
}

impl FluxTransformerInspection {
    pub fn from_header(header: &MlxSafetensorsHeader) -> Result<Self> {
        let mut inferred = FluxTransformerConfig::flux1_dev();
        let mut canonical_hits = 0usize;
        let mut renamed_hits = 0usize;
        let mut max_double_block = None::<u32>;
        let mut max_single_block = None::<u32>;
        let mut hidden_size = None::<u32>;
        let mut context_in_dim = None::<u32>;
        let mut in_channels = None::<u32>;
        let mut out_channels = None::<u32>;
        let mut vec_in_dim = None::<u32>;
        let mut head_dim = None::<u32>;
        let mut guidance_embed = false;

        for (name, entry) in &header.tensors {
            let canonical = canonicalize_flux_diffusion_tensor_name(name);
            if canonical_name_recognized(&canonical) {
                canonical_hits += 1;
            }
            if canonical != *name {
                renamed_hits += 1;
            }

            if canonical == "txt_in.weight" {
                hidden_size = shape_dim(entry, 0);
                context_in_dim = shape_dim(entry, 1);
            } else if canonical == "img_in.weight" {
                in_channels = shape_dim(entry, 1);
            } else if canonical == "vector_in.in_layer.weight" {
                vec_in_dim = shape_dim(entry, 1);
            } else if canonical == "guidance_in.in_layer.weight" {
                guidance_embed = true;
            } else if canonical == "single_blocks.0.norm.key_norm.scale"
                || canonical == "double_blocks.0.txt_attn.norm.key_norm.scale"
            {
                head_dim = shape_dim(entry, 0);
            } else if canonical == "final_layer.linear.weight" {
                out_channels = shape_dim(entry, 0);
            } else if let Some(index) = block_index(&canonical, "double_blocks.") {
                max_double_block =
                    Some(max_double_block.map_or(index, |current| current.max(index)));
            } else if let Some(index) = block_index(&canonical, "single_blocks.") {
                max_single_block =
                    Some(max_single_block.map_or(index, |current| current.max(index)));
            }
        }

        inferred.hidden_size = hidden_size.ok_or_else(|| {
            DiffusionError::model(format!(
                "could not infer FLUX hidden_size from {}",
                header.path.display()
            ))
        })?;
        inferred.context_in_dim = context_in_dim.ok_or_else(|| {
            DiffusionError::model(format!(
                "could not infer FLUX context_in_dim from {}",
                header.path.display()
            ))
        })?;
        if let Some(value) = in_channels {
            inferred.in_channels = value;
        }
        if let Some(value) = out_channels {
            inferred.out_channels = value;
        }
        if let Some(value) = vec_in_dim {
            inferred.vec_in_dim = value;
        }
        if let Some(value) = max_double_block {
            inferred.depth = value + 1;
        }
        if let Some(value) = max_single_block {
            inferred.depth_single_blocks = value + 1;
        }
        inferred.guidance_embed = guidance_embed;

        let head_dim = head_dim.ok_or_else(|| {
            DiffusionError::model(format!(
                "could not infer FLUX head_dim from {}",
                header.path.display()
            ))
        })?;
        if head_dim == 0 || inferred.hidden_size % head_dim != 0 {
            return Err(DiffusionError::model(format!(
                "invalid FLUX head_dim {} for hidden_size {} in {}",
                head_dim,
                inferred.hidden_size,
                header.path.display()
            )));
        }
        inferred.num_heads = inferred.hidden_size / head_dim;

        let tensor_name_style = match (canonical_hits > 0, renamed_hits > 0) {
            (true, false) => FluxTensorNameStyle::Canonical,
            (true, true) => FluxTensorNameStyle::Mixed,
            (false, true) => FluxTensorNameStyle::Diffusers,
            (false, false) => FluxTensorNameStyle::Unknown,
        };

        Ok(Self {
            tensor_name_style,
            canonical_tensor_count: canonical_hits,
            config: inferred,
        })
    }
}

impl ClipLTextEncoderConfig {
    pub fn from_header(header: &MlxSafetensorsHeader) -> Result<Self> {
        let token_embedding = header
            .tensor("text_model.embeddings.token_embedding.weight")
            .ok_or_else(|| {
                DiffusionError::model(format!(
                    "clip_l token embedding missing in {}",
                    header.path.display()
                ))
            })?;
        let pos_embedding = header
            .tensor("text_model.embeddings.position_embedding.weight")
            .ok_or_else(|| {
                DiffusionError::model(format!(
                    "clip_l position embedding missing in {}",
                    header.path.display()
                ))
            })?;
        let mlp_fc1 = header
            .tensor("text_model.encoder.layers.0.mlp.fc1.weight")
            .ok_or_else(|| {
                DiffusionError::model(format!(
                    "clip_l MLP weight missing in {}",
                    header.path.display()
                ))
            })?;

        let mut max_layer = None::<u32>;
        for name in header.tensors.keys() {
            if let Some(index) = block_index(name, "text_model.encoder.layers.") {
                max_layer = Some(max_layer.map_or(index, |current| current.max(index)));
            }
        }

        Ok(Self {
            vocab_size: shape_dim(token_embedding, 0).ok_or_else(|| {
                DiffusionError::model("clip_l token embedding missing vocab dimension")
            })?,
            hidden_size: shape_dim(token_embedding, 1).ok_or_else(|| {
                DiffusionError::model("clip_l token embedding missing hidden dimension")
            })?,
            max_position_embeddings: shape_dim(pos_embedding, 0).ok_or_else(|| {
                DiffusionError::model("clip_l position embedding missing sequence dimension")
            })?,
            intermediate_size: shape_dim(mlp_fc1, 0).ok_or_else(|| {
                DiffusionError::model("clip_l MLP missing intermediate dimension")
            })?,
            layer_count: max_layer.map_or(0, |value| value + 1),
        })
    }
}

impl T5TextEncoderConfig {
    pub fn from_header(header: &MlxSafetensorsHeader) -> Result<Self> {
        let shared = header.tensor("shared.weight").ok_or_else(|| {
            DiffusionError::model(format!(
                "t5xxl shared embedding missing in {}",
                header.path.display()
            ))
        })?;
        let wi0 = header
            .tensor("encoder.block.0.layer.1.DenseReluDense.wi_0.weight")
            .ok_or_else(|| {
                DiffusionError::model(format!(
                    "t5xxl wi_0 weight missing in {}",
                    header.path.display()
                ))
            })?;

        let mut max_layer = None::<u32>;
        for name in header.tensors.keys() {
            if let Some(index) = block_index(name, "encoder.block.") {
                max_layer = Some(max_layer.map_or(index, |current| current.max(index)));
            }
        }

        Ok(Self {
            vocab_size: shape_dim(shared, 0).ok_or_else(|| {
                DiffusionError::model("t5xxl shared embedding missing vocab dimension")
            })?,
            model_dim: shape_dim(shared, 1)
                .ok_or_else(|| DiffusionError::model("t5xxl shared embedding missing model_dim"))?,
            feedforward_dim: shape_dim(wi0, 0)
                .ok_or_else(|| DiffusionError::model("t5xxl wi_0 missing feedforward dim"))?,
            layer_count: max_layer.map_or(0, |value| value + 1),
        })
    }
}

/// Loads a component header: the file's own header for standalone component
/// files, or the prefix-scoped view of a combined checkpoint (per-tensor
/// range reads mean only that component's byte ranges are ever read).
pub fn flux_component_header(
    path: &Path,
    prefix: Option<&str>,
) -> Result<MlxSafetensorsHeader> {
    let header = MlxSafetensorsHeader::load(path)?;
    match prefix {
        Some(prefix) => Ok(header.scoped_to_prefix(prefix)?),
        None => Ok(header),
    }
}

/// Contract audit of a combined single-file FLUX FP8 checkpoint header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FluxCombinedCheckpointAudit {
    pub diffusion_tensors: usize,
    pub clip_l_tensors: usize,
    pub t5xxl_tensors: usize,
    pub vae_tensors: usize,
    pub diffusion_bytes: u64,
    pub total_tensors: usize,
}

/// Validates that `header` is a complete combined FLUX FP8 checkpoint: all
/// four component prefixes present with their expected dtype classes
/// (diffusion + t5xxl entirely F8_E4M3, clip_l entirely F16, vae entirely
/// F32). This is the canonical-tier contract — a split/partial file, a BF16
/// combined file, or a mixed-precision repack all fail closed here instead
/// of streaming weights and failing (or silently degrading) later. The two
/// outer `text_encoders.*.logit_scale` F32 scalars are the only tensors
/// allowed outside the four prefixes.
pub fn validate_flux_combined_checkpoint(
    header: &MlxSafetensorsHeader,
) -> Result<FluxCombinedCheckpointAudit> {
    let mut audit = FluxCombinedCheckpointAudit {
        diffusion_tensors: 0,
        clip_l_tensors: 0,
        t5xxl_tensors: 0,
        vae_tensors: 0,
        diffusion_bytes: 0,
        total_tensors: 0,
    };
    let path = header.path.display();
    for (name, entry) in &header.tensors {
        audit.total_tensors += 1;
        let (component, expected, allowed): (&str, MlxDType, bool) =
            if name.starts_with(FLUX_CKPT_PREFIX_DIFFUSION) {
                audit.diffusion_tensors += 1;
                audit.diffusion_bytes += entry.data_len_bytes();
                ("diffusion model", MlxDType::F8E4M3, true)
            } else if name.starts_with(FLUX_CKPT_PREFIX_CLIP_L) {
                audit.clip_l_tensors += 1;
                ("clip_l", MlxDType::F16, true)
            } else if name.starts_with(FLUX_CKPT_PREFIX_T5XXL) {
                audit.t5xxl_tensors += 1;
                ("t5xxl", MlxDType::F8E4M3, true)
            } else if name.starts_with(FLUX_CKPT_PREFIX_VAE) {
                audit.vae_tensors += 1;
                ("vae", MlxDType::F32, true)
            } else if name == "text_encoders.clip_l.logit_scale"
                || name == "text_encoders.t5xxl.logit_scale"
            {
                ("logit_scale", MlxDType::F32, true)
            } else {
                ("", MlxDType::F32, false)
            };
        if !allowed {
            return Err(DiffusionError::model(format!(
                "combined FLUX checkpoint {} has unexpected tensor '{}' outside every component prefix",
                path, name
            )));
        }
        if entry.dtype != expected {
            return Err(DiffusionError::model(format!(
                "combined FLUX checkpoint {} {} tensor '{}' is {:?}, the FP8 contract requires {:?}",
                path, component, name, entry.dtype, expected
            )));
        }
    }
    for (label, count) in [
        ("diffusion model", audit.diffusion_tensors),
        ("clip_l", audit.clip_l_tensors),
        ("t5xxl", audit.t5xxl_tensors),
        ("vae", audit.vae_tensors),
    ] {
        if count == 0 {
            return Err(DiffusionError::model(format!(
                "combined FLUX checkpoint {} is missing its {} component",
                path, label
            )));
        }
    }
    Ok(audit)
}

fn require_name<'a>(value: &'a Option<String>, field: &str) -> Result<&'a str> {
    value.as_deref().ok_or_else(|| {
        DiffusionError::workflow(format!("missing '{}' in resolved workflow", field))
    })
}

fn require_file(path: PathBuf, label: &str) -> Result<PathBuf> {
    if path.is_file() {
        Ok(path)
    } else {
        Err(DiffusionError::model(format!(
            "{} file does not exist: {}",
            label,
            path.display()
        )))
    }
}

pub fn canonicalize_flux_diffusion_tensor_name(name: &str) -> String {
    let stripped = strip_flux_prefix(name);
    if canonical_name_recognized(stripped) {
        return stripped.to_string();
    }

    if let Some(rest) = stripped.strip_prefix("time_text_embed.timestep_embedder.linear_1.") {
        return format!("time_in.in_layer.{}", rest);
    }
    if let Some(rest) = stripped.strip_prefix("time_text_embed.timestep_embedder.linear_2.") {
        return format!("time_in.out_layer.{}", rest);
    }
    if let Some(rest) = stripped.strip_prefix("time_text_embed.text_embedder.linear_1.") {
        return format!("vector_in.in_layer.{}", rest);
    }
    if let Some(rest) = stripped.strip_prefix("time_text_embed.text_embedder.linear_2.") {
        return format!("vector_in.out_layer.{}", rest);
    }
    if let Some(rest) = stripped.strip_prefix("time_text_embed.guidance_embedder.linear_1.") {
        return format!("guidance_in.in_layer.{}", rest);
    }
    if let Some(rest) = stripped.strip_prefix("time_text_embed.guidance_embedder.linear_2.") {
        return format!("guidance_in.out_layer.{}", rest);
    }
    if let Some(rest) = stripped.strip_prefix("context_embedder.") {
        return format!("txt_in.{}", rest);
    }
    if let Some(rest) = stripped.strip_prefix("x_embedder.") {
        return format!("img_in.{}", rest);
    }
    if let Some(rest) = stripped.strip_prefix("proj_out.") {
        return format!("final_layer.linear.{}", rest);
    }
    if let Some(rest) = stripped.strip_prefix("norm_out.linear.") {
        return format!("final_layer.adaLN_modulation.1.{}", rest);
    }

    if let Some((index, rest)) = indexed_rest(stripped, "transformer_blocks.") {
        let dst = format!("double_blocks.{}.", index);
        if let Some(rest) = rest.strip_prefix("norm1.linear.") {
            return format!("{}img_mod.lin.{}", dst, rest);
        }
        if let Some(rest) = rest.strip_prefix("norm1_context.linear.") {
            return format!("{}txt_mod.lin.{}", dst, rest);
        }
        if let Some(mapped) = map_qkv_suffix(rest, "attn.to_q.", "img_attn.qkv.") {
            return format!("{}{}", dst, mapped);
        }
        if let Some(mapped) = map_qkv_suffix(rest, "attn.to_k.", "img_attn.qkv.") {
            return format!("{}{}", dst, with_suffix_index(&mapped, 1));
        }
        if let Some(mapped) = map_qkv_suffix(rest, "attn.to_v.", "img_attn.qkv.") {
            return format!("{}{}", dst, with_suffix_index(&mapped, 2));
        }
        if let Some(mapped) = map_qkv_suffix(rest, "attn.add_q_proj.", "txt_attn.qkv.") {
            return format!("{}{}", dst, mapped);
        }
        if let Some(mapped) = map_qkv_suffix(rest, "attn.add_k_proj.", "txt_attn.qkv.") {
            return format!("{}{}", dst, with_suffix_index(&mapped, 1));
        }
        if let Some(mapped) = map_qkv_suffix(rest, "attn.add_v_proj.", "txt_attn.qkv.") {
            return format!("{}{}", dst, with_suffix_index(&mapped, 2));
        }
        if rest == "attn.norm_q.weight" {
            return format!("{}img_attn.norm.query_norm.scale", dst);
        }
        if rest == "attn.norm_k.weight" {
            return format!("{}img_attn.norm.key_norm.scale", dst);
        }
        if rest == "attn.norm_added_q.weight" {
            return format!("{}txt_attn.norm.query_norm.scale", dst);
        }
        if rest == "attn.norm_added_k.weight" {
            return format!("{}txt_attn.norm.key_norm.scale", dst);
        }
        if let Some(rest) = rest.strip_prefix("ff.net.0.proj.") {
            return format!("{}img_mlp.0.{}", dst, rest);
        }
        if let Some(rest) = rest.strip_prefix("ff.net.2.") {
            return format!("{}img_mlp.2.{}", dst, rest);
        }
        if let Some(rest) = rest.strip_prefix("ff_context.net.0.proj.") {
            return format!("{}txt_mlp.0.{}", dst, rest);
        }
        if let Some(rest) = rest.strip_prefix("ff_context.net.2.") {
            return format!("{}txt_mlp.2.{}", dst, rest);
        }
        if let Some(rest) = rest.strip_prefix("attn.to_out.0.") {
            return format!("{}img_attn.proj.{}", dst, rest);
        }
        if let Some(rest) = rest.strip_prefix("attn.to_add_out.") {
            return format!("{}txt_attn.proj.{}", dst, rest);
        }
    }

    if let Some((index, rest)) = indexed_rest(stripped, "single_transformer_blocks.") {
        let dst = format!("single_blocks.{}.", index);
        if let Some(rest) = rest.strip_prefix("norm.linear.") {
            return format!("{}modulation.lin.{}", dst, rest);
        }
        if let Some(mapped) = map_qkv_suffix(rest, "attn.to_q.", "linear1.") {
            return format!("{}{}", dst, mapped);
        }
        if let Some(mapped) = map_qkv_suffix(rest, "attn.to_k.", "linear1.") {
            return format!("{}{}", dst, with_suffix_index(&mapped, 1));
        }
        if let Some(mapped) = map_qkv_suffix(rest, "attn.to_v.", "linear1.") {
            return format!("{}{}", dst, with_suffix_index(&mapped, 2));
        }
        if let Some(mapped) = map_qkv_suffix(rest, "proj_mlp.", "linear1.") {
            return format!("{}{}", dst, with_suffix_index(&mapped, 3));
        }
        if rest == "attn.norm_q.weight" {
            return format!("{}norm.query_norm.scale", dst);
        }
        if rest == "attn.norm_k.weight" {
            return format!("{}norm.key_norm.scale", dst);
        }
        if let Some(rest) = rest.strip_prefix("proj_out.") {
            return format!("{}linear2.{}", dst, rest);
        }
    }

    stripped.to_string()
}

fn strip_flux_prefix(name: &str) -> &str {
    for prefix in [
        "model.diffusion_model.",
        "diffusion_model.",
        "unet.",
        "transformer.",
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest;
        }
    }
    name
}

pub(crate) fn canonical_name_recognized(name: &str) -> bool {
    name.starts_with("double_blocks.")
        || name.starts_with("single_blocks.")
        || name.starts_with("img_in.")
        || name.starts_with("time_in.")
        || name.starts_with("vector_in.")
        || name.starts_with("guidance_in.")
        || name.starts_with("txt_in.")
        || name.starts_with("final_layer.")
        || name.starts_with("distilled_guidance_layer.")
        || name.starts_with("img_in_patch.")
}

fn indexed_rest<'a>(name: &'a str, prefix: &str) -> Option<(u32, &'a str)> {
    let rest = name.strip_prefix(prefix)?;
    let (index, rest) = rest.split_once('.')?;
    Some((index.parse().ok()?, rest))
}

fn block_index(name: &str, prefix: &str) -> Option<u32> {
    indexed_rest(name, prefix).map(|(index, _)| index)
}

fn map_qkv_suffix(rest: &str, src_prefix: &str, dst_prefix: &str) -> Option<String> {
    let suffix = rest.strip_prefix(src_prefix)?;
    Some(format!("{}{}", dst_prefix, suffix))
}

fn with_suffix_index(mapped: &str, index: u32) -> String {
    if let Some(rest) = mapped.strip_prefix("linear1.weight") {
        return format!("linear1.weight.{}{}", index, rest);
    }
    if let Some(rest) = mapped.strip_prefix("linear1.bias") {
        return format!("linear1.bias.{}{}", index, rest);
    }
    if let Some(rest) = mapped.strip_prefix("img_attn.qkv.weight") {
        return format!("img_attn.qkv.weight.{}{}", index, rest);
    }
    if let Some(rest) = mapped.strip_prefix("img_attn.qkv.bias") {
        return format!("img_attn.qkv.bias.{}{}", index, rest);
    }
    if let Some(rest) = mapped.strip_prefix("txt_attn.qkv.weight") {
        return format!("txt_attn.qkv.weight.{}{}", index, rest);
    }
    if let Some(rest) = mapped.strip_prefix("txt_attn.qkv.bias") {
        return format!("txt_attn.qkv.bias.{}{}", index, rest);
    }
    format!("{}.{}", mapped, index)
}

fn shape_dim(entry: &MlxTensorEntry, index: usize) -> Option<u32> {
    entry
        .shape
        .get(index)
        .copied()
        .and_then(|value| u32::try_from(value).ok())
}

#[cfg(test)]
mod combined_fp8_tests {
    use super::*;

    /// Minimal hand-rolled safetensors writer for combined-checkpoint tests:
    /// deterministic tensor order, exact offsets, no external JSON dep.
    struct SafetensorsBuilder {
        entries: Vec<(String, &'static str, Vec<u64>, Vec<u8>)>,
    }

    impl SafetensorsBuilder {
        fn new() -> Self {
            Self {
                entries: Vec::new(),
            }
        }

        fn tensor(
            mut self,
            name: &str,
            dtype: &'static str,
            shape: &[u64],
            payload: Vec<u8>,
        ) -> Self {
            self.entries
                .push((name.to_string(), dtype, shape.to_vec(), payload));
            self
        }

        fn write(self, file_name: &str) -> std::path::PathBuf {
            let mut header = String::from("{");
            let mut offset = 0u64;
            for (index, (name, dtype, shape, payload)) in self.entries.iter().enumerate() {
                if index > 0 {
                    header.push(',');
                }
                let shape_json = shape
                    .iter()
                    .map(|dim| dim.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let end = offset + payload.len() as u64;
                header.push_str(&format!(
                    "\"{}\":{{\"dtype\":\"{}\",\"shape\":[{}],\"data_offsets\":[{},{}]}}",
                    name, dtype, shape_json, offset, end
                ));
                offset = end;
            }
            header.push('}');
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&(header.len() as u64).to_le_bytes());
            bytes.extend_from_slice(header.as_bytes());
            for (_, _, _, payload) in &self.entries {
                bytes.extend_from_slice(payload);
            }
            let path = std::env::temp_dir().join(format!(
                "makepad_flux_fp8_test_{}_{}",
                std::process::id(),
                file_name
            ));
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }

    /// F8 payloads use golden-anchored bytes only (never 0x7f/0xff unless a
    /// test wants the NaN rejection to fire).
    fn tiny_combined_builder() -> SafetensorsBuilder {
        let d = FLUX_CKPT_PREFIX_DIFFUSION;
        let c = FLUX_CKPT_PREFIX_CLIP_L;
        let t = FLUX_CKPT_PREFIX_T5XXL;
        let v = FLUX_CKPT_PREFIX_VAE;
        SafetensorsBuilder::new()
            // Diffusion component: enough structure for config inference
            // (hidden_size 4, context 8, in_channels 4, head_dim 2, one
            // double + one single block, no guidance_in => schnell-class).
            .tensor(&format!("{d}txt_in.weight"), "F8_E4M3", &[4, 8], vec![0x38; 32])
            .tensor(&format!("{d}img_in.weight"), "F8_E4M3", &[4, 4], vec![0xB8; 16])
            .tensor(
                &format!("{d}vector_in.in_layer.weight"),
                "F8_E4M3",
                &[4, 4],
                vec![0x40; 16],
            )
            .tensor(
                &format!("{d}double_blocks.0.txt_attn.norm.key_norm.scale"),
                "F8_E4M3",
                &[2],
                vec![0x38, 0xB8],
            )
            .tensor(
                &format!("{d}double_blocks.0.img_attn.proj.weight"),
                "F8_E4M3",
                &[4, 4],
                vec![0x44; 16],
            )
            .tensor(
                &format!("{d}single_blocks.0.norm.key_norm.scale"),
                "F8_E4M3",
                &[2],
                vec![0x7e, 0xfe],
            )
            .tensor(
                &format!("{d}single_blocks.0.linear2.weight"),
                "F8_E4M3",
                &[4, 4],
                vec![0x3c; 16],
            )
            .tensor(
                &format!("{d}final_layer.linear.weight"),
                "F8_E4M3",
                &[4, 4],
                vec![0x48; 16],
            )
            // clip_l component (F16) + outer logit_scale scalar (F32).
            .tensor(
                &format!("{c}text_model.embeddings.token_embedding.weight"),
                "F16",
                &[4, 4],
                vec![0u8; 32],
            )
            .tensor("text_encoders.clip_l.logit_scale", "F32", &[], vec![0u8; 4])
            // t5xxl component (F8) + outer logit_scale scalar.
            .tensor(&format!("{t}shared.weight"), "F8_E4M3", &[4, 4], vec![0x50; 16])
            .tensor("text_encoders.t5xxl.logit_scale", "F32", &[], vec![0u8; 4])
            // vae component (F32).
            .tensor(&format!("{v}decoder.conv_in.bias"), "F32", &[2], vec![0u8; 8])
    }

    #[test]
    fn validates_and_scopes_combined_fp8_checkpoint() {
        let path = tiny_combined_builder().write("valid.safetensors");
        let header = MlxSafetensorsHeader::load(&path).unwrap();

        let audit = validate_flux_combined_checkpoint(&header).unwrap();
        assert_eq!(audit.diffusion_tensors, 8);
        assert_eq!(audit.clip_l_tensors, 1);
        assert_eq!(audit.t5xxl_tensors, 1);
        assert_eq!(audit.vae_tensors, 1);
        assert_eq!(audit.total_tensors, 13);
        assert_eq!(audit.diffusion_bytes, 32 + 16 + 16 + 2 + 16 + 2 + 16 + 16);

        // Component views strip the prefixes and read only their ranges.
        let diffusion = header.scoped_to_prefix(FLUX_CKPT_PREFIX_DIFFUSION).unwrap();
        assert_eq!(diffusion.tensors.len(), 8);
        assert!(diffusion.tensor("txt_in.weight").is_some());
        assert_eq!(
            diffusion.read_tensor_bytes("img_in.weight").unwrap(),
            vec![0xB8; 16]
        );
        let t5 = header.scoped_to_prefix(FLUX_CKPT_PREFIX_T5XXL).unwrap();
        assert_eq!(t5.tensors.len(), 1, "outer logit_scale must be excluded");
        assert_eq!(t5.read_tensor_bytes("shared.weight").unwrap(), vec![0x50; 16]);
        let clip = header.scoped_to_prefix(FLUX_CKPT_PREFIX_CLIP_L).unwrap();
        assert!(clip
            .tensor("text_model.embeddings.token_embedding.weight")
            .is_some());
        let vae = header.scoped_to_prefix(FLUX_CKPT_PREFIX_VAE).unwrap();
        assert!(vae.tensor("decoder.conv_in.bias").is_some());
        assert!(header.scoped_to_prefix("no.such.prefix.").is_err());

        // The scoped diffusion view drives config inference unchanged.
        let inspect = FluxTransformerInspection::from_header(&diffusion).unwrap();
        assert_eq!(inspect.config.hidden_size, 4);
        assert_eq!(inspect.config.context_in_dim, 8);
        assert_eq!(inspect.config.num_heads, 2);
        assert_eq!(inspect.config.depth, 1);
        assert_eq!(inspect.config.depth_single_blocks, 1);
        assert!(!inspect.config.guidance_embed, "no guidance_in => schnell");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn loads_fp8_diffusion_component_with_raw_matrices_and_promoted_norms() {
        use crate::flux_transformer::LoadedFluxTransformerWeights;
        let path = tiny_combined_builder().write("load.safetensors");

        let weights = LoadedFluxTransformerWeights::load_component_with_progress(
            &path,
            Some(FLUX_CKPT_PREFIX_DIFFUSION),
            None,
        )
        .unwrap();
        assert!(weights.f8_weights, "rank-2 F8 matrices must set the flag");
        assert_eq!(weights.path, path, "cache identity is the combined file");
        // Rank-1 F8 promotes to exact F32 on load (anchor bytes 448/-448).
        let single_scale = weights.tensor_id("single_blocks.0.norm.key_norm.scale").unwrap();
        let tensor = weights.ctx.tensor(single_scale).unwrap();
        assert_eq!(tensor.desc.ty, makepad_ggml::TensorType::F32);
        let bytes = weights.ctx.tensor_data(single_scale).unwrap();
        let values: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(values, vec![448.0, -448.0]);
        // Rank-2 F8 stays raw 1-byte resident.
        let txt_in = weights.tensor_id("txt_in.weight").unwrap();
        let tensor = weights.ctx.tensor(txt_in).unwrap();
        assert_eq!(tensor.desc.ty, makepad_ggml::TensorType::F8E4M3);
        assert_eq!(weights.ctx.tensor_data(txt_in).unwrap(), &[0x38u8; 32][..]);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_nan_bytes_mixed_dtypes_and_missing_components() {
        use crate::flux_transformer::LoadedFluxTransformerWeights;

        // A NaN byte (0x7f) inside an F8 matrix fails the load, fail-closed.
        let mut nan_payload = vec![0x38u8; 32];
        nan_payload[17] = 0x7f;
        let d = FLUX_CKPT_PREFIX_DIFFUSION;
        let nan_path = tiny_combined_builder()
            .tensor(&format!("{d}time_in.in_layer.weight"), "F8_E4M3", &[4, 8], nan_payload)
            .write("nan.safetensors");
        let error = LoadedFluxTransformerWeights::load_component_with_progress(
            &nan_path,
            Some(FLUX_CKPT_PREFIX_DIFFUSION),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("NaN"), "got: {error}");
        // The complete-checkpoint audit still passes structurally (NaN is a
        // payload property, caught by the loaders' byte screen).
        let header = MlxSafetensorsHeader::load(&nan_path).unwrap();
        validate_flux_combined_checkpoint(&header).unwrap();
        let _ = std::fs::remove_file(nan_path);

        // A BF16 tensor under the diffusion prefix breaks the FP8 contract.
        let mixed_path = tiny_combined_builder()
            .tensor(&format!("{d}time_in.out_layer.weight"), "BF16", &[2, 2], vec![0u8; 8])
            .write("mixed.safetensors");
        let header = MlxSafetensorsHeader::load(&mixed_path).unwrap();
        let error = validate_flux_combined_checkpoint(&header)
            .unwrap_err()
            .to_string();
        assert!(error.contains("FP8 contract requires"), "got: {error}");
        let _ = std::fs::remove_file(mixed_path);

        // A stray tensor outside every component prefix is rejected.
        let stray_path = tiny_combined_builder()
            .tensor("first_stage_model.mystery", "F32", &[1], vec![0u8; 4])
            .write("stray.safetensors");
        let header = MlxSafetensorsHeader::load(&stray_path).unwrap();
        let error = validate_flux_combined_checkpoint(&header)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unexpected tensor"), "got: {error}");
        let _ = std::fs::remove_file(stray_path);

        // A split/partial file (no vae component) is not a combined
        // checkpoint.
        let mut partial = SafetensorsBuilder::new();
        for (name, dtype, shape, payload) in tiny_combined_builder().entries {
            if !name.starts_with(FLUX_CKPT_PREFIX_VAE) {
                partial = partial.tensor(&name, dtype, &shape, payload);
            }
        }
        let partial_path = partial.write("partial.safetensors");
        let header = MlxSafetensorsHeader::load(&partial_path).unwrap();
        let error = validate_flux_combined_checkpoint(&header)
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing its vae"), "got: {error}");
        let _ = std::fs::remove_file(partial_path);
    }

    #[test]
    fn checkpoint_bundle_resolves_all_components_to_one_file() {
        use crate::comfy::{FluxWorkflowFiles, FluxWorkflowKind};

        let path = tiny_combined_builder().write("bundle.safetensors");
        let root = path.parent().unwrap().to_path_buf();
        // ComfyModelRoots expects checkpoints/<name>; point a synthetic root
        // at temp_dir and place the file name accordingly.
        let checkpoints_dir = root.join("checkpoints");
        std::fs::create_dir_all(&checkpoints_dir).unwrap();
        let file_name = path.file_name().unwrap().to_str().unwrap().to_string();
        let ckpt_path = checkpoints_dir.join(&file_name);
        std::fs::copy(&path, &ckpt_path).unwrap();

        let roots = ComfyModelRoots::new(&root);
        let files = FluxWorkflowFiles {
            checkpoint_name: Some(file_name),
            unet_name: None,
            vae_name: None,
            clip_l_name: None,
            t5xxl_name: None,
        };
        let bundle =
            FluxResolvedBundle::from_workflow_files(FluxWorkflowKind::Checkpoint, &files, &roots)
                .unwrap();
        assert_eq!(bundle.diffusion_model_path, ckpt_path);
        assert_eq!(bundle.vae_path.as_deref(), Some(ckpt_path.as_path()));
        assert_eq!(bundle.clip_l_path.as_deref(), Some(ckpt_path.as_path()));
        assert_eq!(bundle.t5xxl_path.as_deref(), Some(ckpt_path.as_path()));
        let prefixes = bundle.component_prefixes();
        assert_eq!(prefixes.diffusion, Some(FLUX_CKPT_PREFIX_DIFFUSION));
        assert_eq!(prefixes.vae, Some(FLUX_CKPT_PREFIX_VAE));
        assert_eq!(prefixes.clip_l, Some(FLUX_CKPT_PREFIX_CLIP_L));
        assert_eq!(prefixes.t5xxl, Some(FLUX_CKPT_PREFIX_T5XXL));

        // The bundle's scoped headers expose standalone-file naming.
        let headers = bundle.inspect_headers().unwrap();
        assert!(headers.t5xxl.unwrap().tensor("shared.weight").is_some());
        assert!(headers
            .vae
            .unwrap()
            .tensor("decoder.conv_in.bias")
            .is_some());

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(ckpt_path);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonicalize_flux_diffusion_tensor_name, pack_flux_latents_nchw, unpack_flux_latents_nchw,
        FluxLatentShape, FluxTransformerConfig,
    };

    #[test]
    fn computes_flux_latent_layout() {
        let shape = FluxLatentShape::from_image_size(1024, 1024).unwrap();
        assert_eq!(shape.latent_width, 128);
        assert_eq!(shape.latent_height, 128);
        assert_eq!(shape.packed_width, 64);
        assert_eq!(shape.packed_height, 64);
        assert_eq!(shape.transformer_channels, 64);
        assert_eq!(shape.image_token_count, 4096);
    }

    #[test]
    fn exposes_flux1_dev_defaults() {
        let config = FluxTransformerConfig::flux1_dev();
        assert_eq!(config.hidden_size, 3072);
        assert_eq!(config.num_heads, 24);
        assert_eq!(config.head_dim(), 128);
        assert_eq!(config.axes_dim, [16, 56, 56]);
        assert_eq!(config.axes_dim_sum(), 128);
        assert_eq!(config.depth, 19);
        assert_eq!(config.depth_single_blocks, 38);
    }

    #[test]
    fn canonicalizes_diffusers_flux_names() {
        assert_eq!(
            canonicalize_flux_diffusion_tensor_name("transformer_blocks.0.attn.add_k_proj.weight"),
            "double_blocks.0.txt_attn.qkv.weight.1"
        );
        assert_eq!(
            canonicalize_flux_diffusion_tensor_name("single_transformer_blocks.7.proj_mlp.bias"),
            "single_blocks.7.linear1.bias.3"
        );
        assert_eq!(
            canonicalize_flux_diffusion_tensor_name(
                "time_text_embed.timestep_embedder.linear_1.weight"
            ),
            "time_in.in_layer.weight"
        );
        assert_eq!(
            canonicalize_flux_diffusion_tensor_name("norm_out.linear.bias"),
            "final_layer.adaLN_modulation.1.bias"
        );
    }

    #[test]
    fn packs_and_unpacks_flux_latents_round_trip() {
        let batch = 1u32;
        let h = 4u32;
        let w = 4u32;
        let latents: Vec<f32> = (0..(batch * 16 * h * w))
            .map(|value| value as f32)
            .collect();

        let packed = pack_flux_latents_nchw(&latents, batch, h, w).unwrap();
        assert_eq!(packed.len(), 4 * 64);
        assert_eq!(packed[0], latents[0]);
        assert_eq!(packed[1], latents[1]);
        assert_eq!(packed[2], latents[4]);
        assert_eq!(packed[3], latents[5]);

        let unpacked = unpack_flux_latents_nchw(&packed, batch, h, w).unwrap();
        assert_eq!(unpacked, latents);
    }
}
