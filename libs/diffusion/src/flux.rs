use crate::clip::{ClipTokenizedPrompt, ClipTokenizer};
use crate::comfy::{
    FluxGenerationConfig, FluxPrompts, FluxWorkflow, FluxWorkflowFiles, FluxWorkflowKind,
};
use crate::t5::{T5TokenizedPrompt, T5Tokenizer};
use crate::{DiffusionError, Result};
use makepad_mlx::{MlxSafetensorsHeader, MlxTensorEntry};
use std::path::{Path, PathBuf};

pub const FLUX_CLIP_L_MAX_LENGTH: usize = 77;
pub const FLUX_T5XXL_MAX_LENGTH: usize = 256;

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
    pub fn from_workflow(workflow: &FluxWorkflow, roots: &ComfyModelRoots) -> Result<Self> {
        let bundle =
            FluxResolvedBundle::from_workflow_files(workflow.kind, &workflow.files, roots)?;
        let latent_shape = FluxLatentShape::from_image_size(
            workflow.generation.width,
            workflow.generation.height,
        )?;

        Ok(Self {
            workflow_path: workflow.path.clone(),
            kind: workflow.kind,
            bundle,
            prompts: workflow.prompts.clone(),
            generation: workflow.generation.clone(),
            latent_shape,
            transformer: FluxTransformerConfig::flux1_dev(),
        })
    }
}

pub fn tokenize_flux_clip_l_prompt(prompt: &str) -> Result<ClipTokenizedPrompt> {
    ClipTokenizer::new()?.tokenize_chunks(prompt, FLUX_CLIP_L_MAX_LENGTH, true)
}

pub fn tokenize_flux_t5xxl_prompt(prompt: &str) -> Result<T5TokenizedPrompt> {
    T5Tokenizer::new()?.tokenize(prompt, FLUX_T5XXL_MAX_LENGTH, true)
}

impl FluxResolvedBundle {
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
            FluxWorkflowKind::Checkpoint => Ok(Self {
                kind,
                diffusion_model_path: require_file(
                    roots
                        .checkpoints_dir
                        .join(require_name(&files.checkpoint_name, "ckpt_name")?),
                    "checkpoint",
                )?,
                vae_path: None,
                clip_l_path: None,
                t5xxl_path: None,
            }),
        }
    }

    pub fn inspect_headers(&self) -> Result<FluxBundleHeaders> {
        Ok(FluxBundleHeaders {
            diffusion_model: MlxSafetensorsHeader::load(&self.diffusion_model_path)?,
            vae: self
                .vae_path
                .as_ref()
                .map(MlxSafetensorsHeader::load)
                .transpose()?,
            clip_l: self
                .clip_l_path
                .as_ref()
                .map(MlxSafetensorsHeader::load)
                .transpose()?,
            t5xxl: self
                .t5xxl_path
                .as_ref()
                .map(MlxSafetensorsHeader::load)
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

fn canonical_name_recognized(name: &str) -> bool {
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
