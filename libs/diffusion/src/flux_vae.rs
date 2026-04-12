use crate::flux::FluxLatentShape;
use crate::{DiffusionError, Result};
use makepad_ggml::backend::metal::{
    prepare_graph, BufferStorageMode, MetalGraphSession, MetalGraphTensorWrite, MetalRuntime,
};
use makepad_ggml::{
    ggml_pad, BufferUsage, Context, GluOp, Graph, InitParams, Op, ScaleMode, Tensor, TensorDesc,
    TensorId, TensorLayout, TensorType, GGML_MEM_ALIGN,
};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const VAE_GROUP_NORM_EPSILON: f32 = 1.0e-6;
const DEFAULT_GRAPH_EXTRA_BYTES: usize = 1usize * 1024 * 1024 * 1024;
const MAX_GRAPH_GROWTH_ATTEMPTS: usize = 3;

#[derive(Clone, Debug)]
pub struct LoadedFluxVaeWeights {
    pub ctx: Context,
    pub tensor_ids: BTreeMap<String, TensorId>,
    pub path: PathBuf,
    graph_extra_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct FluxVaeDecoderGraph {
    pub graph: Graph,
    pub input_latents: TensorId,
    pub result_image: TensorId,
    pub image_width: usize,
    pub image_height: usize,
    pub debug_stages: Vec<FluxVaeDebugStage>,
}

pub struct CompiledFluxVaeMetal {
    graph: FluxVaeDecoderGraph,
    session: MetalGraphSession,
}

#[derive(Clone, Debug)]
pub struct FluxVaeDecodeRun {
    pub image: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Debug)]
pub struct FluxVaeDebugStage {
    pub name: String,
    pub tensor_id: TensorId,
}

#[derive(Clone, Debug)]
pub struct FluxVaeStageOutput {
    pub name: String,
    pub values: Vec<f32>,
    pub width: usize,
    pub height: usize,
    pub channels: usize,
}

#[derive(Clone, Debug)]
pub struct FluxVaeDebugRun {
    pub final_image: FluxVaeDecodeRun,
    pub stages: Vec<FluxVaeStageOutput>,
}

impl LoadedFluxVaeWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_extra(path, DEFAULT_GRAPH_EXTRA_BYTES)
    }

    pub fn load_with_extra(path: impl AsRef<Path>, extra_bytes: usize) -> Result<Self> {
        let header = MlxSafetensorsHeader::load(path.as_ref())?;
        let total_bytes = vae_weight_total_bytes(&header, extra_bytes)?;
        let mut ctx = Context::new(InitParams {
            mem_size: total_bytes,
            mem_buffer: None,
            no_alloc: false,
        });
        let tensor_ids = allocate_vae_weight_tensors(&mut ctx, &header)?;
        load_vae_weight_bytes(&mut ctx, &header, &tensor_ids)?;

        Ok(Self {
            ctx,
            tensor_ids,
            path: header.path,
            graph_extra_bytes: extra_bytes,
        })
    }

    pub fn tensor_id(&self, name: &str) -> Result<TensorId> {
        self.tensor_ids
            .get(name)
            .copied()
            .ok_or_else(|| DiffusionError::model(format!("missing flux vae tensor '{}'", name)))
    }

    fn has_tensor(&self, name: &str) -> bool {
        self.tensor_ids.contains_key(name)
    }

    fn graph_reserve_bytes(&self) -> usize {
        self.graph_extra_bytes
    }
}

impl CompiledFluxVaeMetal {
    pub fn compile(weights: &mut LoadedFluxVaeWeights, latent_shape: FluxLatentShape) -> Result<Self> {
        let runtime = MetalRuntime::new().map_err(DiffusionError::model)?;
        Self::compile_with_runtime(runtime, weights, latent_shape)
    }

    pub fn compile_with_runtime(
        runtime: MetalRuntime,
        weights: &mut LoadedFluxVaeWeights,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        for attempt in 0..=MAX_GRAPH_GROWTH_ATTEMPTS {
            let graph = match build_flux_vae_decoder_graph(weights, latent_shape) {
                Ok(graph) => graph,
                Err(err) if is_context_oom(&err) && attempt < MAX_GRAPH_GROWTH_ATTEMPTS => {
                    let next_extra = next_graph_reserve_bytes(weights)?;
                    *weights = LoadedFluxVaeWeights::load_with_extra(weights.path.clone(), next_extra)?;
                    continue;
                }
                Err(err) => return Err(err),
            };
            let prepared = prepare_graph(&weights.ctx, &graph.graph, runtime.features())
                .map_err(DiffusionError::model)?;
            let session = MetalGraphSession::from_runtime(
                runtime.clone(),
                &weights.ctx,
                &prepared,
                BufferStorageMode::Shared,
                BufferStorageMode::Shared,
            )
            .map_err(DiffusionError::model)?;
            return Ok(Self { graph, session });
        }

        Err(DiffusionError::model(
            "flux vae graph compilation exhausted context growth attempts",
        ))
    }

    pub fn execute(&self, weights: &LoadedFluxVaeWeights, latents_whcb: &[f32]) -> Result<FluxVaeDecodeRun> {
        let input_tensor = require_tensor(&weights.ctx, self.graph.input_latents)?;
        let expected = usize::try_from(
            input_tensor.ne[0] * input_tensor.ne[1] * input_tensor.ne[2] * input_tensor.ne[3],
        )
        .map_err(|_| DiffusionError::model("flux vae input size exceeds usize"))?;
        if latents_whcb.len() != expected {
            return Err(DiffusionError::workflow(format!(
                "flux vae input expected {} values, got {}",
                expected,
                latents_whcb.len()
            )));
        }

        let latent_bytes = f32s_to_le_bytes(latents_whcb);
        let execution = self
            .session
            .execute(
                &weights.ctx,
                &[MetalGraphTensorWrite {
                    tensor_id: self.graph.input_latents,
                    bytes: &latent_bytes,
                }],
                &[self.graph.result_image],
            )
            .map_err(DiffusionError::model)?;
        let image_bytes = execution
            .outputs
            .get(&self.graph.result_image)
            .ok_or_else(|| DiffusionError::model("flux vae decode did not return image tensor"))?;

        Ok(FluxVaeDecodeRun {
            image: f32_bytes_to_vec(image_bytes)?,
            width: self.graph.image_width,
            height: self.graph.image_height,
        })
    }

    pub fn execute_with_debug(
        &self,
        weights: &LoadedFluxVaeWeights,
        latents_whcb: &[f32],
    ) -> Result<FluxVaeDebugRun> {
        let input_tensor = require_tensor(&weights.ctx, self.graph.input_latents)?;
        let expected = usize::try_from(
            input_tensor.ne[0] * input_tensor.ne[1] * input_tensor.ne[2] * input_tensor.ne[3],
        )
        .map_err(|_| DiffusionError::model("flux vae input size exceeds usize"))?;
        if latents_whcb.len() != expected {
            return Err(DiffusionError::workflow(format!(
                "flux vae input expected {} values, got {}",
                expected,
                latents_whcb.len()
            )));
        }

        let mut output_ids = Vec::with_capacity(1 + self.graph.debug_stages.len());
        output_ids.push(self.graph.result_image);
        for stage in &self.graph.debug_stages {
            output_ids.push(stage.tensor_id);
        }

        let latent_bytes = f32s_to_le_bytes(latents_whcb);
        let execution = self
            .session
            .execute(
                &weights.ctx,
                &[MetalGraphTensorWrite {
                    tensor_id: self.graph.input_latents,
                    bytes: &latent_bytes,
                }],
                &output_ids,
            )
            .map_err(DiffusionError::model)?;
        let image_bytes = execution
            .outputs
            .get(&self.graph.result_image)
            .ok_or_else(|| DiffusionError::model("flux vae decode did not return image tensor"))?;
        let final_image = FluxVaeDecodeRun {
            image: f32_bytes_to_vec(image_bytes)?,
            width: self.graph.image_width,
            height: self.graph.image_height,
        };

        let mut stages = Vec::with_capacity(self.graph.debug_stages.len());
        for stage in &self.graph.debug_stages {
            let tensor = require_tensor(&weights.ctx, stage.tensor_id)?;
            let bytes = execution.outputs.get(&stage.tensor_id).ok_or_else(|| {
                DiffusionError::model(format!("flux vae debug tensor '{}' missing output", stage.name))
            })?;
            stages.push(FluxVaeStageOutput {
                name: stage.name.clone(),
                values: f32_bytes_to_vec(bytes)?,
                width: usize::try_from(tensor.ne[0])
                    .map_err(|_| DiffusionError::model("flux vae debug width exceeds usize"))?,
                height: usize::try_from(tensor.ne[1])
                    .map_err(|_| DiffusionError::model("flux vae debug height exceeds usize"))?,
                channels: usize::try_from(tensor.ne[2])
                    .map_err(|_| DiffusionError::model("flux vae debug channels exceeds usize"))?,
            });
        }

        Ok(FluxVaeDebugRun { final_image, stages })
    }
}

pub fn build_flux_vae_decoder_graph(
    weights: &mut LoadedFluxVaeWeights,
    latent_shape: FluxLatentShape,
) -> Result<FluxVaeDecoderGraph> {
    let mut debug_stages = Vec::new();
    let input_latents = weights
        .ctx
        .new_named_tensor(
            "flux_vae.input_latents",
            TensorType::F32,
            4,
            &[
                latent_shape.latent_width as i64,
                latent_shape.latent_height as i64,
                latent_shape.latent_channels as i64,
                1,
            ],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;

    let mut hidden = apply_conv2d(
        &mut weights.ctx,
        &weights.tensor_ids,
        input_latents,
        "decoder.conv_in.weight",
        "decoder.conv_in.bias",
        1,
        1,
    )?;
    hidden = resnet_block(&mut weights.ctx, &weights.tensor_ids, hidden, "decoder.mid.block_1")?;
    hidden = mid_attention(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        "decoder.mid.attn_1",
    )?;
    hidden = resnet_block(&mut weights.ctx, &weights.tensor_ids, hidden, "decoder.mid.block_2")?;
    debug_stages.push(FluxVaeDebugStage {
        name: "decoder.mid".to_string(),
        tensor_id: hidden,
    });

    for stage in (0..=3).rev() {
        let stage_prefix = format!("decoder.up.{stage}");
        for block in 0..=2 {
            hidden = resnet_block(
                &mut weights.ctx,
                &weights.tensor_ids,
                hidden,
                &format!("{stage_prefix}.block.{block}"),
            )?;
        }
        if weights.has_tensor(&format!("{stage_prefix}.upsample.conv.weight")) {
            hidden = weights
                .ctx
                .upscale(hidden, 2, ScaleMode::Nearest, false, false, BufferUsage::Activations)
                .map_err(DiffusionError::model)?;
            hidden = apply_conv2d(
                &mut weights.ctx,
                &weights.tensor_ids,
                hidden,
                &format!("{stage_prefix}.upsample.conv.weight"),
                &format!("{stage_prefix}.upsample.conv.bias"),
                1,
                1,
            )?;
        }
        debug_stages.push(FluxVaeDebugStage {
            name: format!("{stage_prefix}.out"),
            tensor_id: hidden,
        });
    }

    hidden = apply_group_norm(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        "decoder.norm_out.weight",
        "decoder.norm_out.bias",
        32,
    )?;
    hidden = apply_silu(&mut weights.ctx, hidden)?;
    debug_stages.push(FluxVaeDebugStage {
        name: "decoder.pre_rgb".to_string(),
        tensor_id: hidden,
    });
    let result_image = apply_conv2d(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        "decoder.conv_out.weight",
        "decoder.conv_out.bias",
        1,
        1,
    )?;

    let mut graph = Graph::new();
    graph
        .build_forward_expand(&weights.ctx, result_image)
        .map_err(DiffusionError::model)?;

    Ok(FluxVaeDecoderGraph {
        graph,
        input_latents,
        result_image,
        image_width: latent_shape.image_width as usize,
        image_height: latent_shape.image_height as usize,
        debug_stages,
    })
}

fn resnet_block(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    prefix: &str,
) -> Result<TensorId> {
    let hidden = apply_group_norm(
        ctx,
        tensor_ids,
        input,
        &format!("{prefix}.norm1.weight"),
        &format!("{prefix}.norm1.bias"),
        32,
    )?;
    let hidden = apply_silu(ctx, hidden)?;
    let hidden = apply_conv2d(
        ctx,
        tensor_ids,
        hidden,
        &format!("{prefix}.conv1.weight"),
        &format!("{prefix}.conv1.bias"),
        1,
        1,
    )?;
    let hidden = apply_group_norm(
        ctx,
        tensor_ids,
        hidden,
        &format!("{prefix}.norm2.weight"),
        &format!("{prefix}.norm2.bias"),
        32,
    )?;
    let hidden = apply_silu(ctx, hidden)?;
    let hidden = apply_conv2d(
        ctx,
        tensor_ids,
        hidden,
        &format!("{prefix}.conv2.weight"),
        &format!("{prefix}.conv2.bias"),
        1,
        1,
    )?;
    let residual = if tensor_ids.contains_key(&format!("{prefix}.nin_shortcut.weight")) {
        apply_conv2d(
            ctx,
            tensor_ids,
            input,
            &format!("{prefix}.nin_shortcut.weight"),
            &format!("{prefix}.nin_shortcut.bias"),
            0,
            0,
        )?
    } else {
        input
    };
    ctx.binary_like_a(Op::Add, residual, hidden, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn mid_attention(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    prefix: &str,
) -> Result<TensorId> {
    let hidden = apply_group_norm(
        ctx,
        tensor_ids,
        input,
        &format!("{prefix}.norm.weight"),
        &format!("{prefix}.norm.bias"),
        32,
    )?;
    let q = apply_conv2d(
        ctx,
        tensor_ids,
        hidden,
        &format!("{prefix}.q.weight"),
        &format!("{prefix}.q.bias"),
        0,
        0,
    )?;
    let k = apply_conv2d(
        ctx,
        tensor_ids,
        hidden,
        &format!("{prefix}.k.weight"),
        &format!("{prefix}.k.bias"),
        0,
        0,
    )?;
    let v = apply_conv2d(
        ctx,
        tensor_ids,
        hidden,
        &format!("{prefix}.v.weight"),
        &format!("{prefix}.v.bias"),
        0,
        0,
    )?;
    let spatial = require_tensor(ctx, q)?.clone();
    let width = spatial.ne[0];
    let height = spatial.ne[1];
    let channels = spatial.ne[2];
    let tokens = width
        .checked_mul(height)
        .ok_or_else(|| DiffusionError::model("flux vae attention token count overflow"))?;
    let q = spatial_to_tokens(ctx, q, width, height, channels)?;
    let k = spatial_to_tokens(ctx, k, width, height, channels)?;
    let v = spatial_to_tokens(ctx, v, width, height, channels)?;
    let q = ctx.reshape(q, &[channels, 1, tokens]).map_err(DiffusionError::model)?;
    let k = ctx.reshape(k, &[channels, 1, tokens]).map_err(DiffusionError::model)?;
    let v = ctx.reshape(v, &[channels, 1, tokens]).map_err(DiffusionError::model)?;
    let attn = build_attention_output(ctx, q, k, v, channels as u32)?;
    let attn = tokens_to_spatial(ctx, attn, width, height, channels)?;
    let attn = apply_conv2d(
        ctx,
        tensor_ids,
        attn,
        &format!("{prefix}.proj_out.weight"),
        &format!("{prefix}.proj_out.bias"),
        0,
        0,
    )?;
    ctx.binary_like_a(Op::Add, input, attn, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn apply_group_norm(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight_name: &str,
    bias_name: &str,
    groups: i32,
) -> Result<TensorId> {
    let norm = ctx
        .group_norm(input, groups, VAE_GROUP_NORM_EPSILON, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let weight = broadcast_channel_vector(ctx, require_tensor_id(tensor_ids, weight_name)?, norm)?;
    let bias = broadcast_channel_vector(ctx, require_tensor_id(tensor_ids, bias_name)?, norm)?;
    let scaled = ctx
        .binary_like_a(Op::Mul, norm, weight, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    ctx.binary_like_a(Op::Add, scaled, bias, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn apply_conv2d(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight_name: &str,
    bias_name: &str,
    pad_x: i32,
    pad_y: i32,
) -> Result<TensorId> {
    let out = ctx
        .conv_2d(
            require_tensor_id(tensor_ids, weight_name)?,
            input,
            1,
            1,
            pad_x,
            pad_y,
            1,
            1,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let bias = broadcast_channel_vector(ctx, require_tensor_id(tensor_ids, bias_name)?, out)?;
    ctx.binary_like_a(Op::Add, out, bias, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn apply_silu(ctx: &mut Context, input: TensorId) -> Result<TensorId> {
    let ones = repeat_scalar_one(ctx, input)?;
    ctx.glu_split(input, ones, GluOp::Swiglu, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn repeat_scalar_one(ctx: &mut Context, shape_of: TensorId) -> Result<TensorId> {
    let one = ctx
        .new_tensor_1d(TensorType::F32, 1, BufferUsage::Weights)
        .map_err(DiffusionError::model)?;
    ctx.write_tensor_data(one, &1.0f32.to_le_bytes())
        .map_err(DiffusionError::model)?;
    ctx.repeat(one, shape_of, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn spatial_to_tokens(
    ctx: &mut Context,
    input: TensorId,
    width: i64,
    height: i64,
    channels: i64,
) -> Result<TensorId> {
    let flattened = ctx
        .reshape(input, &[width * height, channels])
        .map_err(DiffusionError::model)?;
    let transposed = ctx.transpose(flattened).map_err(DiffusionError::model)?;
    ctx.cont_2d(transposed, channels, width * height)
        .map_err(DiffusionError::model)
}

fn tokens_to_spatial(
    ctx: &mut Context,
    input: TensorId,
    width: i64,
    height: i64,
    channels: i64,
) -> Result<TensorId> {
    let transposed = ctx.transpose(input).map_err(DiffusionError::model)?;
    let contiguous = ctx
        .cont_2d(transposed, width * height, channels)
        .map_err(DiffusionError::model)?;
    ctx.reshape(contiguous, &[width, height, channels, 1])
        .map_err(DiffusionError::model)
}

fn build_attention_output(
    ctx: &mut Context,
    q: TensorId,
    k: TensorId,
    v: TensorId,
    head_dim: u32,
) -> Result<TensorId> {
    let q = ctx.permute(q, [0, 2, 1, 3]).map_err(DiffusionError::model)?;
    let k = ctx.permute(k, [0, 2, 1, 3]).map_err(DiffusionError::model)?;
    let mut v = ctx.permute(v, [0, 2, 1, 3]).map_err(DiffusionError::model)?;
    let mut kq = ctx.mul_mat(k, q, BufferUsage::Activations).map_err(DiffusionError::model)?;
    kq = ctx
        .soft_max_ext(
            kq,
            None,
            1.0 / (head_dim as f32).sqrt(),
            0.0,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    v = ctx.transpose(v).map_err(DiffusionError::model)?;
    v = ctx.cont(v).map_err(DiffusionError::model)?;
    let kqv = ctx
        .mul_mat(v, kq, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let attn = ctx.permute(kqv, [0, 2, 1, 3]).map_err(DiffusionError::model)?;
    let attn_tensor = require_tensor(ctx, attn)?.clone();
    ctx.cont_2d(
        attn,
        attn_tensor.ne[0] * attn_tensor.ne[1],
        attn_tensor.ne[2] * attn_tensor.ne[3],
    )
    .map_err(DiffusionError::model)
}

fn broadcast_channel_vector(ctx: &mut Context, vector: TensorId, shape_of: TensorId) -> Result<TensorId> {
    let reshaped = ctx
        .reshape(vector, &[1, 1, require_tensor(ctx, vector)?.ne[0], 1])
        .map_err(DiffusionError::model)?;
    ctx.repeat(reshaped, shape_of, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn allocate_vae_weight_tensors(
    ctx: &mut Context,
    header: &MlxSafetensorsHeader,
) -> Result<BTreeMap<String, TensorId>> {
    let mut tensor_ids = BTreeMap::new();
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let entry = header.tensor(&name).ok_or_else(|| {
            DiffusionError::model(format!("flux vae header lost tensor '{}' while allocating", name))
        })?;
        let ty = vae_target_tensor_type(entry.dtype)?;
        let extents = vae_target_extents(entry)?;
        let id = ctx
            .new_named_tensor(name.clone(), ty, extents.len(), &extents, BufferUsage::Weights)
            .map_err(DiffusionError::model)?;
        tensor_ids.insert(name, id);
    }
    Ok(tensor_ids)
}

fn load_vae_weight_bytes(
    ctx: &mut Context,
    header: &MlxSafetensorsHeader,
    tensor_ids: &BTreeMap<String, TensorId>,
) -> Result<()> {
    for (name, tensor_id) in tensor_ids {
        let entry = header
            .tensor(name)
            .ok_or_else(|| DiffusionError::model(format!("flux vae header missing tensor '{}'", name)))?;
        let bytes = vae_target_bytes(header, name, entry)?;
        ctx.write_tensor_data(*tensor_id, &bytes)
            .map_err(DiffusionError::model)?;
    }
    Ok(())
}

fn vae_weight_total_bytes(header: &MlxSafetensorsHeader, extra_bytes: usize) -> Result<usize> {
    let mut total = 0usize;
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let entry = header.tensor(&name).unwrap();
        total = ggml_pad(total, GGML_MEM_ALIGN);
        total = total
            .checked_add(vae_target_nbytes(entry)?)
            .ok_or_else(|| DiffusionError::model(format!("flux vae total bytes overflow at '{}'", name)))?;
    }
    total = ggml_pad(total, GGML_MEM_ALIGN);
    total
        .checked_add(extra_bytes)
        .ok_or_else(|| DiffusionError::model("flux vae context size overflow"))
}

fn vae_target_nbytes(entry: &MlxTensorEntry) -> Result<usize> {
    let ty = vae_target_tensor_type(entry.dtype)?;
    let extents = vae_target_extents(entry)?;
    let layout = TensorLayout::for_ggml(ty, &extents).map_err(DiffusionError::model)?;
    Ok(Tensor::from_desc(0, TensorDesc::new(ty, layout, BufferUsage::Weights)).nbytes())
}

fn vae_target_extents(entry: &MlxTensorEntry) -> Result<Vec<i64>> {
    match entry.shape.as_slice() {
        [dim] => Ok(vec![i64::try_from(*dim)
            .map_err(|_| DiffusionError::model(format!("flux vae extent {} exceeds i64", dim)))?]),
        [d0, d1, d2, d3] => Ok(vec![
            i64::try_from(*d3)
                .map_err(|_| DiffusionError::model(format!("flux vae extent {} exceeds i64", d3)))?,
            i64::try_from(*d2)
                .map_err(|_| DiffusionError::model(format!("flux vae extent {} exceeds i64", d2)))?,
            i64::try_from(*d1)
                .map_err(|_| DiffusionError::model(format!("flux vae extent {} exceeds i64", d1)))?,
            i64::try_from(*d0)
                .map_err(|_| DiffusionError::model(format!("flux vae extent {} exceeds i64", d0)))?,
        ]),
        other => Err(DiffusionError::model(format!(
            "flux vae only supports rank1/rank4 tensors today, got {:?}",
            other
        ))),
    }
}

fn vae_target_tensor_type(dtype: MlxDType) -> Result<TensorType> {
    match dtype {
        MlxDType::F32 => Ok(TensorType::F32),
        other => Err(DiffusionError::model(format!(
            "flux vae unsupported tensor dtype {:?}",
            other
        ))),
    }
}

fn vae_target_bytes(
    header: &MlxSafetensorsHeader,
    name: &str,
    entry: &MlxTensorEntry,
) -> Result<Vec<u8>> {
    match entry.dtype {
        MlxDType::F32 => Ok(header.read_tensor_bytes(name)?),
        other => Err(DiffusionError::model(format!(
            "flux vae unsupported tensor dtype {:?}",
            other
        ))),
    }
}

fn f32s_to_le_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn f32_bytes_to_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 4 != 0 {
        return Err(DiffusionError::model(format!(
            "flux vae output byte length {} is not divisible by 4",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn require_tensor_id(tensor_ids: &BTreeMap<String, TensorId>, name: &str) -> Result<TensorId> {
    tensor_ids
        .get(name)
        .copied()
        .ok_or_else(|| DiffusionError::model(format!("missing flux vae resident tensor '{}'", name)))
}

fn require_tensor<'a>(ctx: &'a Context, id: TensorId) -> Result<&'a Tensor> {
    ctx.tensor(id)
        .ok_or_else(|| DiffusionError::model(format!("invalid flux vae tensor id {}", id)))
}

fn is_context_oom(err: &DiffusionError) -> bool {
    matches!(err, DiffusionError::Model(message) if message.starts_with("context out of memory allocating "))
}

fn next_graph_reserve_bytes(weights: &LoadedFluxVaeWeights) -> Result<usize> {
    weights
        .graph_reserve_bytes()
        .checked_mul(2)
        .ok_or_else(|| DiffusionError::model("flux vae graph reserve overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ggml::backend::metal::{
        prepare_graph, BufferStorageMode, MetalGraphSession, MetalGraphTensorWrite, MetalRuntime,
    };

    #[test]
    fn spatial_token_roundtrip_matches_original_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let width = 3_i64;
        let height = 2_i64;
        let channels = 4_i64;
        let values = patterned_f32s((width * height * channels) as usize, -0.25, 0.07);

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });
        let input = ctx
            .new_tensor_4d(TensorType::F32, width, height, channels, 1, BufferUsage::Activations)
            .unwrap();
        ctx.write_tensor_data(input, &f32s_to_le_bytes(&values)).unwrap();

        let tokens = spatial_to_tokens(&mut ctx, input, width, height, channels).unwrap();
        let restored = tokens_to_spatial(&mut ctx, tokens, width, height, channels).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, restored).unwrap();
        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let execution = session.execute(&ctx, &[], &[restored]).unwrap();
        let actual = f32_bytes_to_vec(execution.outputs.get(&restored).unwrap()).unwrap();

        assert_eq!(actual.len(), values.len());
        for (a, e) in actual.iter().zip(values.iter()) {
            assert!(
                (a - e).abs() < 1.0e-6,
                "vae spatial/token roundtrip mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    #[test]
    fn vae_attention_output_matches_cpu_reference_on_metal_when_available() {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let head_dim = 8_i64;
        let token_count = 6_i64;
        let q_values = patterned_f32s((head_dim * token_count) as usize, -0.2, 0.03);
        let k_values = patterned_f32s((head_dim * token_count) as usize, 0.15, -0.025);
        let v_values = patterned_f32s((head_dim * token_count) as usize, 0.05, 0.04);

        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });
        let q = ctx
            .new_tensor_3d(TensorType::F32, head_dim, 1, token_count, BufferUsage::Activations)
            .unwrap();
        let k = ctx
            .new_tensor_3d(TensorType::F32, head_dim, 1, token_count, BufferUsage::Activations)
            .unwrap();
        let v = ctx
            .new_tensor_3d(TensorType::F32, head_dim, 1, token_count, BufferUsage::Activations)
            .unwrap();
        let attn = build_attention_output(&mut ctx, q, k, v, head_dim as u32).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, attn).unwrap();
        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let execution = session
            .execute(
                &ctx,
                &[
                    MetalGraphTensorWrite {
                        tensor_id: q,
                        bytes: &f32s_to_le_bytes(&q_values),
                    },
                    MetalGraphTensorWrite {
                        tensor_id: k,
                        bytes: &f32s_to_le_bytes(&k_values),
                    },
                    MetalGraphTensorWrite {
                        tensor_id: v,
                        bytes: &f32s_to_le_bytes(&v_values),
                    },
                ],
                &[attn],
            )
            .unwrap();
        let actual = f32_bytes_to_vec(execution.outputs.get(&attn).unwrap()).unwrap();
        let expected = cpu_attention_f32(
            &q_values,
            &k_values,
            &v_values,
            head_dim as usize,
            token_count as usize,
        );

        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert!(
                (a - e).abs() < 1.0e-5,
                "vae attention mismatch: actual={} expected={}",
                a,
                e
            );
        }
    }

    fn patterned_f32s(len: usize, start: f32, step: f32) -> Vec<f32> {
        (0..len).map(|index| start + index as f32 * step).collect()
    }

    fn cpu_attention_f32(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        head_dim: usize,
        token_count: usize,
    ) -> Vec<f32> {
        let mut out = vec![0.0f32; head_dim * token_count];
        for query_token in 0..token_count {
            let q_row = &q[query_token * head_dim..(query_token + 1) * head_dim];
            let mut scores = vec![0.0f32; token_count];
            for key_token in 0..token_count {
                let k_row = &k[key_token * head_dim..(key_token + 1) * head_dim];
                let mut dot = 0.0f32;
                for dim in 0..head_dim {
                    dot += q_row[dim] * k_row[dim];
                }
                scores[key_token] = dot / (head_dim as f32).sqrt();
            }
            let max_score = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let mut denom = 0.0f32;
            for score in &mut scores {
                *score = (*score - max_score).exp();
                denom += *score;
            }
            denom = denom.max(f32::MIN_POSITIVE);
            for score in &mut scores {
                *score /= denom;
            }
            for key_token in 0..token_count {
                let v_row = &v[key_token * head_dim..(key_token + 1) * head_dim];
                let weight = scores[key_token];
                for dim in 0..head_dim {
                    out[query_token * head_dim + dim] += weight * v_row[dim];
                }
            }
        }
        out
    }
}
