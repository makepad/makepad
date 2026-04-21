use crate::backend::{
    compile_graph_session, new_runtime, runtime_available, try_attention_softmax_weighted_sum_f32,
    try_matmul_nn_f32, try_matmul_nt_f32, BufferStorageMode, GraphSession, GraphTensorWrite,
    Runtime,
};
use crate::flux::FluxLatentShape;
use crate::{DiffusionError, Result};
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

pub struct CompiledFluxVae {
    inner: FluxVaeExecutor,
}

pub type CompiledFluxVaeMetal = CompiledFluxVae;
pub type LazyFluxVaeMetal = LazyFluxVae;

enum FluxVaeExecutor {
    Compiled(CompiledFluxVaeGraph),
    Lazy(LazyFluxVae),
}

struct CompiledFluxVaeGraph {
    graph: FluxVaeDecoderGraph,
    session: GraphSession,
}

#[derive(Clone, Debug)]
pub struct LazyFluxVae {
    latent_shape: FluxLatentShape,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FluxVaeExecutionMode {
    Lazy,
    Compiled,
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

#[derive(Clone, Debug)]
struct SpatialTensor {
    width: usize,
    height: usize,
    channels: usize,
    data: Vec<f32>,
}

#[derive(Clone, Debug)]
struct RowsTensor {
    rows: usize,
    cols: usize,
    data: Vec<f32>,
}

#[derive(Clone, Debug)]
struct ConvKernel {
    data: Vec<f32>,
    kw: usize,
    kh: usize,
    in_channels: usize,
    out_channels: usize,
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

    fn tensor_f32_values(&self, name: &str) -> Result<Vec<f32>> {
        let tensor_id = self.tensor_id(name)?;
        tensor_to_f32_vec(&self.ctx, tensor_id)
    }

    fn tensor_kernel(&self, name: &str) -> Result<ConvKernel> {
        conv_kernel(&self.ctx, self.tensor_id(name)?)
    }

    fn has_tensor(&self, name: &str) -> bool {
        self.tensor_ids.contains_key(name)
    }

    fn graph_reserve_bytes(&self) -> usize {
        self.graph_extra_bytes
    }
}

impl CompiledFluxVae {
    pub fn compile(
        weights: &mut LoadedFluxVaeWeights,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        Self::compile_for_mode(
            FluxVaeExecutionMode::from_env(),
            None,
            weights,
            latent_shape,
        )
    }

    pub fn compile_with_runtime(
        runtime: Runtime,
        weights: &mut LoadedFluxVaeWeights,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        Self::compile_for_mode(
            FluxVaeExecutionMode::from_env(),
            Some(runtime),
            weights,
            latent_shape,
        )
    }

    pub fn compile_for_mode(
        mode: FluxVaeExecutionMode,
        runtime: Option<Runtime>,
        weights: &mut LoadedFluxVaeWeights,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        match mode {
            FluxVaeExecutionMode::Lazy => Ok(Self {
                inner: FluxVaeExecutor::Lazy(LazyFluxVae::compile(weights, latent_shape)?),
            }),
            FluxVaeExecutionMode::Compiled => {
                let runtime = match runtime {
                    Some(runtime) => runtime,
                    None => new_runtime()?,
                };
                Self::compile_graph(runtime, weights, latent_shape)
            }
        }
    }

    fn compile_graph(
        runtime: Runtime,
        weights: &mut LoadedFluxVaeWeights,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        for attempt in 0..=MAX_GRAPH_GROWTH_ATTEMPTS {
            let graph = match build_flux_vae_decoder_graph(weights, latent_shape) {
                Ok(graph) => graph,
                Err(err) if is_context_oom(&err) && attempt < MAX_GRAPH_GROWTH_ATTEMPTS => {
                    let next_extra = next_graph_reserve_bytes(weights)?;
                    *weights =
                        LoadedFluxVaeWeights::load_with_extra(weights.path.clone(), next_extra)?;
                    continue;
                }
                Err(err) => return Err(err),
            };
            let session = compile_graph_session(
                &runtime,
                &weights.ctx,
                &graph.graph,
                BufferStorageMode::Shared,
                BufferStorageMode::Shared,
            )?;
            return Ok(Self {
                inner: FluxVaeExecutor::Compiled(CompiledFluxVaeGraph { graph, session }),
            });
        }

        Err(DiffusionError::model(
            "flux vae graph compilation exhausted context growth attempts",
        ))
    }

    pub fn backend_name(&self) -> &'static str {
        match &self.inner {
            FluxVaeExecutor::Compiled(_) => FluxVaeExecutionMode::Compiled.as_str(),
            FluxVaeExecutor::Lazy(_) => FluxVaeExecutionMode::Lazy.as_str(),
        }
    }

    pub fn execute(
        &self,
        weights: &LoadedFluxVaeWeights,
        latents_whcb: &[f32],
    ) -> Result<FluxVaeDecodeRun> {
        match &self.inner {
            FluxVaeExecutor::Compiled(compiled) => compiled.execute(weights, latents_whcb),
            FluxVaeExecutor::Lazy(lazy) => lazy.execute(weights, latents_whcb),
        }
    }

    pub fn execute_with_debug(
        &self,
        weights: &LoadedFluxVaeWeights,
        latents_whcb: &[f32],
    ) -> Result<FluxVaeDebugRun> {
        match &self.inner {
            FluxVaeExecutor::Compiled(compiled) => {
                compiled.execute_with_debug(weights, latents_whcb)
            }
            FluxVaeExecutor::Lazy(lazy) => lazy.execute_with_debug(weights, latents_whcb),
        }
    }
}

impl CompiledFluxVaeGraph {
    fn execute(
        &self,
        weights: &LoadedFluxVaeWeights,
        latents_whcb: &[f32],
    ) -> Result<FluxVaeDecodeRun> {
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
                &[GraphTensorWrite {
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

    fn execute_with_debug(
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
                &[GraphTensorWrite {
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
                DiffusionError::model(format!(
                    "flux vae debug tensor '{}' missing output",
                    stage.name
                ))
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

        Ok(FluxVaeDebugRun {
            final_image,
            stages,
        })
    }
}

impl LazyFluxVae {
    pub fn compile(
        weights: &mut LoadedFluxVaeWeights,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        let _ = weights;
        Ok(Self { latent_shape })
    }

    pub fn compile_with_runtime(
        _runtime: Runtime,
        weights: &mut LoadedFluxVaeWeights,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        Self::compile(weights, latent_shape)
    }

    pub fn execute(
        &self,
        weights: &LoadedFluxVaeWeights,
        latents_whcb: &[f32],
    ) -> Result<FluxVaeDecodeRun> {
        Ok(self.execute_with_debug(weights, latents_whcb)?.final_image)
    }

    pub fn execute_with_debug(
        &self,
        weights: &LoadedFluxVaeWeights,
        latents_whcb: &[f32],
    ) -> Result<FluxVaeDebugRun> {
        let expected = self
            .latent_shape
            .latent_width
            .checked_mul(self.latent_shape.latent_height)
            .and_then(|value| value.checked_mul(self.latent_shape.latent_channels))
            .ok_or_else(|| DiffusionError::model("flux vae latent size overflow"))?
            as usize;
        if latents_whcb.len() != expected {
            return Err(DiffusionError::workflow(format!(
                "flux vae input expected {} values, got {}",
                expected,
                latents_whcb.len()
            )));
        }

        let mut stages = Vec::new();
        let mut hidden = SpatialTensor::new(
            self.latent_shape.latent_width as usize,
            self.latent_shape.latent_height as usize,
            self.latent_shape.latent_channels as usize,
            latents_whcb.to_vec(),
        )?;
        hidden = apply_conv2d_spatial(
            weights,
            &hidden,
            "decoder.conv_in.weight",
            "decoder.conv_in.bias",
            1,
            1,
        )?;
        hidden = resnet_block_spatial(weights, &hidden, "decoder.mid.block_1")?;
        hidden = mid_attention_spatial(weights, &hidden, "decoder.mid.attn_1")?;
        hidden = resnet_block_spatial(weights, &hidden, "decoder.mid.block_2")?;
        stages.push(stage_output("decoder.mid", &hidden));

        for stage in (0..=3).rev() {
            let stage_prefix = format!("decoder.up.{stage}");
            for block in 0..=2 {
                hidden = resnet_block_spatial(
                    weights,
                    &hidden,
                    &format!("{stage_prefix}.block.{block}"),
                )?;
            }
            if weights.has_tensor(&format!("{stage_prefix}.upsample.conv.weight")) {
                hidden = upscale_nearest(&hidden, 2)?;
                hidden = apply_conv2d_spatial(
                    weights,
                    &hidden,
                    &format!("{stage_prefix}.upsample.conv.weight"),
                    &format!("{stage_prefix}.upsample.conv.bias"),
                    1,
                    1,
                )?;
            }
            stages.push(stage_output(&format!("{stage_prefix}.out"), &hidden));
        }

        hidden = apply_group_norm_spatial(
            weights,
            &hidden,
            "decoder.norm_out.weight",
            "decoder.norm_out.bias",
            32,
        )?;
        hidden = silu_spatial(&hidden)?;
        stages.push(stage_output("decoder.pre_rgb", &hidden));
        let image = apply_conv2d_spatial(
            weights,
            &hidden,
            "decoder.conv_out.weight",
            "decoder.conv_out.bias",
            1,
            1,
        )?;

        Ok(FluxVaeDebugRun {
            final_image: FluxVaeDecodeRun {
                image: image.data.clone(),
                width: image.width,
                height: image.height,
            },
            stages,
        })
    }
}

impl FluxVaeExecutionMode {
    pub fn from_env() -> Self {
        match std::env::var("FLUX_VAE_MODE") {
            Ok(value) if value.eq_ignore_ascii_case("lazy") => Self::Lazy,
            Ok(value) if value.eq_ignore_ascii_case("compiled") => Self::Compiled,
            _ if runtime_available() => Self::Compiled,
            _ => Self::Lazy,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lazy => "lazy",
            Self::Compiled => "compiled",
        }
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
    hidden = resnet_block(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        "decoder.mid.block_1",
    )?;
    hidden = mid_attention(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        "decoder.mid.attn_1",
    )?;
    hidden = resnet_block(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        "decoder.mid.block_2",
    )?;
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
                .upscale(
                    hidden,
                    2,
                    ScaleMode::Nearest,
                    false,
                    false,
                    BufferUsage::Activations,
                )
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
    let q = ctx
        .reshape(q, &[channels, 1, tokens])
        .map_err(DiffusionError::model)?;
    let k = ctx
        .reshape(k, &[channels, 1, tokens])
        .map_err(DiffusionError::model)?;
    let v = ctx
        .reshape(v, &[channels, 1, tokens])
        .map_err(DiffusionError::model)?;
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
        .group_norm(
            input,
            groups,
            VAE_GROUP_NORM_EPSILON,
            BufferUsage::Activations,
        )
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
    let q = ctx
        .permute(q, [0, 2, 1, 3])
        .map_err(DiffusionError::model)?;
    let k = ctx
        .permute(k, [0, 2, 1, 3])
        .map_err(DiffusionError::model)?;
    let mut v = ctx
        .permute(v, [0, 2, 1, 3])
        .map_err(DiffusionError::model)?;
    let mut kq = ctx
        .mul_mat(k, q, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
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
    let attn = ctx
        .permute(kqv, [0, 2, 1, 3])
        .map_err(DiffusionError::model)?;
    let attn_tensor = require_tensor(ctx, attn)?.clone();
    ctx.cont_2d(
        attn,
        attn_tensor.ne[0] * attn_tensor.ne[1],
        attn_tensor.ne[2] * attn_tensor.ne[3],
    )
    .map_err(DiffusionError::model)
}

fn broadcast_channel_vector(
    ctx: &mut Context,
    vector: TensorId,
    shape_of: TensorId,
) -> Result<TensorId> {
    let reshaped = ctx
        .reshape(vector, &[1, 1, require_tensor(ctx, vector)?.ne[0], 1])
        .map_err(DiffusionError::model)?;
    ctx.repeat(reshaped, shape_of, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

impl SpatialTensor {
    fn new(width: usize, height: usize, channels: usize, data: Vec<f32>) -> Result<Self> {
        let expected = width
            .checked_mul(height)
            .and_then(|value| value.checked_mul(channels))
            .ok_or_else(|| DiffusionError::model("flux vae spatial tensor size overflow"))?;
        if data.len() != expected {
            return Err(DiffusionError::model(format!(
                "flux vae spatial tensor len mismatch: expected {}, got {}",
                expected,
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            channels,
            data,
        })
    }

    fn offset(&self, x: usize, y: usize, channel: usize) -> usize {
        x + self.width * (y + self.height * channel)
    }
}

impl RowsTensor {
    fn new(rows: usize, cols: usize, data: Vec<f32>) -> Result<Self> {
        let expected = rows
            .checked_mul(cols)
            .ok_or_else(|| DiffusionError::model("flux vae rows tensor size overflow"))?;
        if data.len() != expected {
            return Err(DiffusionError::model(format!(
                "flux vae rows tensor len mismatch: expected {}, got {}",
                expected,
                data.len()
            )));
        }
        Ok(Self { rows, cols, data })
    }
}

fn stage_output(name: &str, tensor: &SpatialTensor) -> FluxVaeStageOutput {
    FluxVaeStageOutput {
        name: name.to_string(),
        values: tensor.data.clone(),
        width: tensor.width,
        height: tensor.height,
        channels: tensor.channels,
    }
}

fn apply_conv2d_spatial(
    weights: &LoadedFluxVaeWeights,
    input: &SpatialTensor,
    weight_name: &str,
    bias_name: &str,
    pad_x: usize,
    pad_y: usize,
) -> Result<SpatialTensor> {
    let kernel = weights.tensor_kernel(weight_name)?;
    let bias = weights.tensor_f32_values(bias_name)?;
    if kernel.in_channels != input.channels {
        return Err(DiffusionError::model(format!(
            "flux vae conv input channels mismatch: input={} kernel={}",
            input.channels, kernel.in_channels
        )));
    }
    if bias.len() != kernel.out_channels {
        return Err(DiffusionError::model(format!(
            "flux vae conv bias len mismatch: bias={} out_channels={}",
            bias.len(),
            kernel.out_channels
        )));
    }

    let mut output = vec![0.0f32; input.width * input.height * kernel.out_channels];
    for out_c in 0..kernel.out_channels {
        for y in 0..input.height {
            for x in 0..input.width {
                let mut acc = bias[out_c];
                for in_c in 0..kernel.in_channels {
                    for ky in 0..kernel.kh {
                        let src_y = y + ky;
                        if src_y < pad_y || src_y - pad_y >= input.height {
                            continue;
                        }
                        let in_y = src_y - pad_y;
                        for kx in 0..kernel.kw {
                            let src_x = x + kx;
                            if src_x < pad_x || src_x - pad_x >= input.width {
                                continue;
                            }
                            let in_x = src_x - pad_x;
                            let input_value = input.data[input.offset(in_x, in_y, in_c)];
                            let weight_index = kx
                                + kernel.kw
                                    * (ky + kernel.kh * (in_c + kernel.in_channels * out_c));
                            acc += input_value * kernel.data[weight_index];
                        }
                    }
                }
                let offset = x + input.width * (y + input.height * out_c);
                output[offset] = acc;
            }
        }
    }
    SpatialTensor::new(input.width, input.height, kernel.out_channels, output)
}

fn apply_group_norm_spatial(
    weights: &LoadedFluxVaeWeights,
    input: &SpatialTensor,
    weight_name: &str,
    bias_name: &str,
    groups: usize,
) -> Result<SpatialTensor> {
    let weight = weights.tensor_f32_values(weight_name)?;
    let bias = weights.tensor_f32_values(bias_name)?;
    if weight.len() != input.channels || bias.len() != input.channels {
        return Err(DiffusionError::model(format!(
            "flux vae group norm weight/bias mismatch: channels={} weight={} bias={}",
            input.channels,
            weight.len(),
            bias.len()
        )));
    }
    if input.channels % groups != 0 {
        return Err(DiffusionError::model(format!(
            "flux vae group norm channels {} not divisible by groups {}",
            input.channels, groups
        )));
    }
    let channels_per_group = input.channels / groups;
    let mut output = vec![0.0f32; input.data.len()];
    for group in 0..groups {
        let channel_start = group * channels_per_group;
        let channel_end = channel_start + channels_per_group;
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        let mut count = 0usize;
        for channel in channel_start..channel_end {
            for y in 0..input.height {
                for x in 0..input.width {
                    let value = input.data[input.offset(x, y, channel)] as f64;
                    sum += value;
                    sum_sq += value * value;
                    count += 1;
                }
            }
        }
        let mean = (sum / count as f64) as f32;
        let variance = (sum_sq / count as f64) as f32 - mean * mean;
        let inv_std = 1.0 / (variance + VAE_GROUP_NORM_EPSILON).sqrt();
        for channel in channel_start..channel_end {
            for y in 0..input.height {
                for x in 0..input.width {
                    let offset = input.offset(x, y, channel);
                    output[offset] =
                        (input.data[offset] - mean) * inv_std * weight[channel] + bias[channel];
                }
            }
        }
    }
    SpatialTensor::new(input.width, input.height, input.channels, output)
}

fn silu_spatial(input: &SpatialTensor) -> Result<SpatialTensor> {
    let data = input
        .data
        .iter()
        .copied()
        .map(|value| value / (1.0 + (-value).exp()))
        .collect::<Vec<_>>();
    SpatialTensor::new(input.width, input.height, input.channels, data)
}

fn resnet_block_spatial(
    weights: &LoadedFluxVaeWeights,
    input: &SpatialTensor,
    prefix: &str,
) -> Result<SpatialTensor> {
    let hidden = apply_group_norm_spatial(
        weights,
        input,
        &format!("{prefix}.norm1.weight"),
        &format!("{prefix}.norm1.bias"),
        32,
    )?;
    let hidden = silu_spatial(&hidden)?;
    let hidden = apply_conv2d_spatial(
        weights,
        &hidden,
        &format!("{prefix}.conv1.weight"),
        &format!("{prefix}.conv1.bias"),
        1,
        1,
    )?;
    let hidden = apply_group_norm_spatial(
        weights,
        &hidden,
        &format!("{prefix}.norm2.weight"),
        &format!("{prefix}.norm2.bias"),
        32,
    )?;
    let hidden = silu_spatial(&hidden)?;
    let hidden = apply_conv2d_spatial(
        weights,
        &hidden,
        &format!("{prefix}.conv2.weight"),
        &format!("{prefix}.conv2.bias"),
        1,
        1,
    )?;
    let residual = if weights.has_tensor(&format!("{prefix}.nin_shortcut.weight")) {
        apply_conv2d_spatial(
            weights,
            input,
            &format!("{prefix}.nin_shortcut.weight"),
            &format!("{prefix}.nin_shortcut.bias"),
            0,
            0,
        )?
    } else {
        input.clone()
    };
    add_spatial(&residual, &hidden)
}

fn mid_attention_spatial(
    weights: &LoadedFluxVaeWeights,
    input: &SpatialTensor,
    prefix: &str,
) -> Result<SpatialTensor> {
    let hidden = apply_group_norm_spatial(
        weights,
        input,
        &format!("{prefix}.norm.weight"),
        &format!("{prefix}.norm.bias"),
        32,
    )?;
    let q = apply_conv2d_spatial(
        weights,
        &hidden,
        &format!("{prefix}.q.weight"),
        &format!("{prefix}.q.bias"),
        0,
        0,
    )?;
    let k = apply_conv2d_spatial(
        weights,
        &hidden,
        &format!("{prefix}.k.weight"),
        &format!("{prefix}.k.bias"),
        0,
        0,
    )?;
    let v = apply_conv2d_spatial(
        weights,
        &hidden,
        &format!("{prefix}.v.weight"),
        &format!("{prefix}.v.bias"),
        0,
        0,
    )?;
    let q = spatial_to_tokens_rows(&q)?;
    let k = spatial_to_tokens_rows(&k)?;
    let v = spatial_to_tokens_rows(&v)?;
    let attn = attention_rows(&q, &k, &v)?;
    let attn = tokens_to_spatial_rows(&attn, input.width, input.height, input.channels)?;
    let attn = apply_conv2d_spatial(
        weights,
        &attn,
        &format!("{prefix}.proj_out.weight"),
        &format!("{prefix}.proj_out.bias"),
        0,
        0,
    )?;
    add_spatial(input, &attn)
}

fn upscale_nearest(input: &SpatialTensor, factor: usize) -> Result<SpatialTensor> {
    let out_width = input
        .width
        .checked_mul(factor)
        .ok_or_else(|| DiffusionError::model("flux vae upscale width overflow"))?;
    let out_height = input
        .height
        .checked_mul(factor)
        .ok_or_else(|| DiffusionError::model("flux vae upscale height overflow"))?;
    let mut output = vec![0.0f32; out_width * out_height * input.channels];
    for channel in 0..input.channels {
        for y in 0..out_height {
            for x in 0..out_width {
                let src_x = x / factor;
                let src_y = y / factor;
                let dst = x + out_width * (y + out_height * channel);
                output[dst] = input.data[input.offset(src_x, src_y, channel)];
            }
        }
    }
    SpatialTensor::new(out_width, out_height, input.channels, output)
}

fn spatial_to_tokens_rows(input: &SpatialTensor) -> Result<RowsTensor> {
    let rows = input
        .width
        .checked_mul(input.height)
        .ok_or_else(|| DiffusionError::model("flux vae token count overflow"))?;
    let mut data = Vec::with_capacity(rows * input.channels);
    for y in 0..input.height {
        for x in 0..input.width {
            for channel in 0..input.channels {
                data.push(input.data[input.offset(x, y, channel)]);
            }
        }
    }
    RowsTensor::new(rows, input.channels, data)
}

fn tokens_to_spatial_rows(
    input: &RowsTensor,
    width: usize,
    height: usize,
    channels: usize,
) -> Result<SpatialTensor> {
    if input.rows
        != width
            .checked_mul(height)
            .ok_or_else(|| DiffusionError::model("flux vae spatial rows overflow"))?
        || input.cols != channels
    {
        return Err(DiffusionError::model(format!(
            "flux vae tokens_to_spatial mismatch: rows={} cols={} expected {}x{}",
            input.rows,
            input.cols,
            width * height,
            channels
        )));
    }
    let mut data = vec![0.0f32; width * height * channels];
    let mut row_index = 0usize;
    for y in 0..height {
        for x in 0..width {
            let row = &input.data[row_index * channels..(row_index + 1) * channels];
            for channel in 0..channels {
                data[x + width * (y + height * channel)] = row[channel];
            }
            row_index += 1;
        }
    }
    SpatialTensor::new(width, height, channels, data)
}

fn attention_rows(q: &RowsTensor, k: &RowsTensor, v: &RowsTensor) -> Result<RowsTensor> {
    if q.rows != k.rows || q.rows != v.rows || q.cols != k.cols || q.cols != v.cols {
        return Err(DiffusionError::model(format!(
            "flux vae attention shape mismatch: q={}x{} k={}x{} v={}x{}",
            q.rows, q.cols, k.rows, k.cols, v.rows, v.cols
        )));
    }
    let token_count = q.rows;
    let head_dim = q.cols;
    let mut scores = if let Some(scores) =
        try_matmul_nt_f32(&q.data, &k.data, token_count, head_dim, token_count)
    {
        scores
    } else {
        matmul_nt_f32_cpu(&q.data, &k.data, token_count, head_dim, token_count)?
    };
    let scale = 1.0 / (head_dim as f32).sqrt();
    for value in &mut scores {
        *value *= scale;
    }
    let output = if let Some(output) =
        try_attention_softmax_weighted_sum_f32(&scores, &v.data, token_count, token_count, head_dim)
    {
        output
    } else {
        softmax_in_place(&mut scores, token_count)?;
        if let Some(output) =
            try_matmul_nn_f32(&scores, &v.data, token_count, token_count, head_dim)
        {
            output
        } else {
            matmul_nn_f32_cpu(&scores, &v.data, token_count, token_count, head_dim)?
        }
    };
    RowsTensor::new(token_count, head_dim, output)
}

fn add_spatial(lhs: &SpatialTensor, rhs: &SpatialTensor) -> Result<SpatialTensor> {
    if lhs.width != rhs.width || lhs.height != rhs.height || lhs.channels != rhs.channels {
        return Err(DiffusionError::model(format!(
            "flux vae add shape mismatch: lhs={}x{}x{} rhs={}x{}x{}",
            lhs.width, lhs.height, lhs.channels, rhs.width, rhs.height, rhs.channels
        )));
    }
    let data = lhs
        .data
        .iter()
        .zip(rhs.data.iter())
        .map(|(lhs_value, rhs_value)| lhs_value + rhs_value)
        .collect::<Vec<_>>();
    SpatialTensor::new(lhs.width, lhs.height, lhs.channels, data)
}

fn tensor_to_f32_vec(ctx: &Context, tensor_id: TensorId) -> Result<Vec<f32>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    let bytes = ctx.tensor_data(tensor_id).map_err(DiffusionError::model)?;
    match tensor.desc.ty {
        TensorType::F32 => f32_bytes_to_vec(bytes),
        other => Err(DiffusionError::model(format!(
            "flux vae tensor {} cannot be decoded as f32 from {:?}",
            tensor_id, other
        ))),
    }
}

fn conv_kernel(ctx: &Context, tensor_id: TensorId) -> Result<ConvKernel> {
    let tensor = require_tensor(ctx, tensor_id)?;
    if tensor.desc.ty != TensorType::F32 {
        return Err(DiffusionError::model(format!(
            "flux vae conv kernel {} must be F32, got {:?}",
            tensor_id, tensor.desc.ty
        )));
    }
    Ok(ConvKernel {
        data: f32_bytes_to_vec(ctx.tensor_data(tensor_id).map_err(DiffusionError::model)?)?,
        kw: usize::try_from(tensor.ne[0])
            .map_err(|_| DiffusionError::model("flux vae kernel kw exceeds usize"))?,
        kh: usize::try_from(tensor.ne[1])
            .map_err(|_| DiffusionError::model("flux vae kernel kh exceeds usize"))?,
        in_channels: usize::try_from(tensor.ne[2])
            .map_err(|_| DiffusionError::model("flux vae kernel in_channels exceeds usize"))?,
        out_channels: usize::try_from(tensor.ne[3])
            .map_err(|_| DiffusionError::model("flux vae kernel out_channels exceeds usize"))?,
    })
}

fn matmul_nt_f32_cpu(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Result<Vec<f32>> {
    if a.len()
        != m.checked_mul(k)
            .ok_or_else(|| DiffusionError::model("flux vae matmul a overflow"))?
    {
        return Err(DiffusionError::model(
            "flux vae matmul_nt_f32_cpu a len mismatch",
        ));
    }
    if bt.len()
        != n.checked_mul(k)
            .ok_or_else(|| DiffusionError::model("flux vae matmul bt overflow"))?
    {
        return Err(DiffusionError::model(
            "flux vae matmul_nt_f32_cpu bt len mismatch",
        ));
    }
    let mut out = vec![
        0.0f32;
        m.checked_mul(n)
            .ok_or_else(|| DiffusionError::model("flux vae matmul out overflow"))?
    ];
    for row in 0..m {
        let a_row = &a[row * k..(row + 1) * k];
        let out_row = &mut out[row * n..(row + 1) * n];
        for col in 0..n {
            let bt_row = &bt[col * k..(col + 1) * k];
            let mut acc = 0.0f32;
            for idx in 0..k {
                acc += a_row[idx] * bt_row[idx];
            }
            out_row[col] = acc;
        }
    }
    Ok(out)
}

fn matmul_nn_f32_cpu(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Result<Vec<f32>> {
    if a.len()
        != m.checked_mul(k)
            .ok_or_else(|| DiffusionError::model("flux vae matmul a overflow"))?
    {
        return Err(DiffusionError::model(
            "flux vae matmul_nn_f32_cpu a len mismatch",
        ));
    }
    if b.len()
        != k.checked_mul(n)
            .ok_or_else(|| DiffusionError::model("flux vae matmul b overflow"))?
    {
        return Err(DiffusionError::model(
            "flux vae matmul_nn_f32_cpu b len mismatch",
        ));
    }
    let mut out = vec![
        0.0f32;
        m.checked_mul(n)
            .ok_or_else(|| DiffusionError::model("flux vae matmul out overflow"))?
    ];
    for row in 0..m {
        let a_row = &a[row * k..(row + 1) * k];
        let out_row = &mut out[row * n..(row + 1) * n];
        for inner in 0..k {
            let a_value = a_row[inner];
            let b_row = &b[inner * n..(inner + 1) * n];
            for col in 0..n {
                out_row[col] += a_value * b_row[col];
            }
        }
    }
    Ok(out)
}

fn softmax_in_place(values: &mut [f32], width: usize) -> Result<()> {
    if width == 0 || values.len() % width != 0 {
        return Err(DiffusionError::model(format!(
            "flux vae softmax width {} is invalid for {} values",
            width,
            values.len()
        )));
    }
    for row in values.chunks_exact_mut(width) {
        let mut max_value = f32::NEG_INFINITY;
        for &value in row.iter() {
            max_value = max_value.max(value);
        }
        let mut denom = 0.0f32;
        for value in row.iter_mut() {
            *value = (*value - max_value).exp();
            denom += *value;
        }
        if denom == 0.0 {
            return Err(DiffusionError::model(
                "flux vae softmax denominator became zero",
            ));
        }
        for value in row.iter_mut() {
            *value /= denom;
        }
    }
    Ok(())
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
            DiffusionError::model(format!(
                "flux vae header lost tensor '{}' while allocating",
                name
            ))
        })?;
        let ty = vae_target_tensor_type(entry.dtype)?;
        let extents = vae_target_extents(entry)?;
        let id = ctx
            .new_named_tensor(
                name.clone(),
                ty,
                extents.len(),
                &extents,
                BufferUsage::Weights,
            )
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
        let entry = header.tensor(name).ok_or_else(|| {
            DiffusionError::model(format!("flux vae header missing tensor '{}'", name))
        })?;
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
            .ok_or_else(|| {
                DiffusionError::model(format!("flux vae total bytes overflow at '{}'", name))
            })?;
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
        [dim] => Ok(vec![i64::try_from(*dim).map_err(|_| {
            DiffusionError::model(format!("flux vae extent {} exceeds i64", dim))
        })?]),
        [d0, d1, d2, d3] => Ok(vec![
            i64::try_from(*d3).map_err(|_| {
                DiffusionError::model(format!("flux vae extent {} exceeds i64", d3))
            })?,
            i64::try_from(*d2).map_err(|_| {
                DiffusionError::model(format!("flux vae extent {} exceeds i64", d2))
            })?,
            i64::try_from(*d1).map_err(|_| {
                DiffusionError::model(format!("flux vae extent {} exceeds i64", d1))
            })?,
            i64::try_from(*d0).map_err(|_| {
                DiffusionError::model(format!("flux vae extent {} exceeds i64", d0))
            })?,
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
    tensor_ids.get(name).copied().ok_or_else(|| {
        DiffusionError::model(format!("missing flux vae resident tensor '{}'", name))
    })
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
    use crate::backend::{
        create_graph_session, new_runtime, prepare_graph, BufferStorageMode, GraphTensorWrite,
    };

    #[test]
    fn spatial_token_roundtrip_matches_original_on_metal_when_available() {
        let runtime = match new_runtime() {
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
            .new_tensor_4d(
                TensorType::F32,
                width,
                height,
                channels,
                1,
                BufferUsage::Activations,
            )
            .unwrap();
        ctx.write_tensor_data(input, &f32s_to_le_bytes(&values))
            .unwrap();

        let tokens = spatial_to_tokens(&mut ctx, input, width, height, channels).unwrap();
        let restored = tokens_to_spatial(&mut ctx, tokens, width, height, channels).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, restored).unwrap();
        let prepared = prepare_graph(&runtime, &ctx, &graph).unwrap();
        let session = create_graph_session(
            &runtime,
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
        let runtime = match new_runtime() {
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
            .new_tensor_3d(
                TensorType::F32,
                head_dim,
                1,
                token_count,
                BufferUsage::Activations,
            )
            .unwrap();
        let k = ctx
            .new_tensor_3d(
                TensorType::F32,
                head_dim,
                1,
                token_count,
                BufferUsage::Activations,
            )
            .unwrap();
        let v = ctx
            .new_tensor_3d(
                TensorType::F32,
                head_dim,
                1,
                token_count,
                BufferUsage::Activations,
            )
            .unwrap();
        let attn = build_attention_output(&mut ctx, q, k, v, head_dim as u32).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, attn).unwrap();
        let prepared = prepare_graph(&runtime, &ctx, &graph).unwrap();
        let session = create_graph_session(
            &runtime,
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
                    GraphTensorWrite {
                        tensor_id: q,
                        bytes: &f32s_to_le_bytes(&q_values),
                    },
                    GraphTensorWrite {
                        tensor_id: k,
                        bytes: &f32s_to_le_bytes(&k_values),
                    },
                    GraphTensorWrite {
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
