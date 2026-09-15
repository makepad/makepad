use crate::backend::{
    create_graph_session, gpu_add, gpu_attention_planar_single, gpu_conv2d_planar_cached,
    gpu_device_available, gpu_download, gpu_group_norm_planar, gpu_pool_cap_override,
    gpu_pool_clear, gpu_silu, gpu_upload, gpu_upsample_nearest2x, new_runtime, prepare_graph,
    runtime_available, try_attention_softmax_weighted_sum_f32, try_matmul_nn_f32,
    try_matmul_nt_f32, BufferStorageMode, GpuTensor, GraphSession, GraphTensorWrite, Runtime,
};
use crate::flux::FluxLatentShape;
use crate::{emit_progress, DiffusionError, ProgressHook, Result};
use makepad_ai_common::{
    bf16_to_f32, f16_to_f32, f32_to_f16, ggml_pad, BufferUsage, Context, Graph, InitParams, Op,
    ScaleMode, Tensor, TensorDesc, TensorId, TensorLayout, TensorType, UnaryOp, GGML_MEM_ALIGN,
};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const VAE_GROUP_NORM_EPSILON: f32 = 1.0e-6;
// Graph intermediates are no_alloc; this only covers input + metadata.
const DEFAULT_GRAPH_EXTRA_BYTES: usize = 64 * 1024 * 1024;
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
        Self::load_scoped_with_extra(path, None, extra_bytes)
    }

    /// Load the vae component of a combined checkpoint: the `prefix` scope
    /// (`vae.`) exposes exactly the standalone ae.safetensors naming
    /// (`decoder.*`/`encoder.*`); the weights keep the combined file's path
    /// as their device-cache identity. Only the decode path's tensors are
    /// ever uploaded — the device cache fills lazily per used tensor.
    pub fn load_component(path: impl AsRef<Path>, prefix: Option<&str>) -> Result<Self> {
        Self::load_scoped_with_extra(path, prefix, DEFAULT_GRAPH_EXTRA_BYTES)
    }

    fn load_scoped_with_extra(
        path: impl AsRef<Path>,
        prefix: Option<&str>,
        extra_bytes: usize,
    ) -> Result<Self> {
        let header = crate::flux::flux_component_header(path.as_ref(), prefix)?;
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
            let prepared = prepare_graph(&runtime, &weights.ctx, &graph.graph)?;
            eprintln!(
                "flux vae compiled main_buffer={} used={} arena={}",
                prepared.main_buffer_size,
                weights.ctx.used_mem(),
                weights.ctx.mem_buffer().len()
            );
            let session = create_graph_session(
                &runtime,
                &weights.ctx,
                &prepared,
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
        self.execute_hooked(weights, latents_whcb, None)
    }

    /// [`Self::execute`] with per-block progress ("block 5/17") from the
    /// lazy/device decode; the compiled (single-graph) executor has no
    /// interior boundary and emits nothing.
    pub fn execute_hooked(
        &self,
        weights: &LoadedFluxVaeWeights,
        latents_whcb: &[f32],
        mut progress: Option<ProgressHook>,
    ) -> Result<FluxVaeDecodeRun> {
        match &self.inner {
            FluxVaeExecutor::Compiled(compiled) => compiled.execute(weights, latents_whcb),
            FluxVaeExecutor::Lazy(lazy) => {
                lazy.execute_with_progress(weights, latents_whcb, &mut progress)
            }
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

        eprintln!(
            "flux vae input n={} {}",
            latents_whcb.len(),
            f32_stats(latents_whcb)
        );
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

        let image = f32_bytes_to_vec(image_bytes)?;
        eprintln!(
            "flux vae image n={} {}",
            image.len(),
            f32_stats(&image)
        );
        Ok(FluxVaeDecodeRun {
            image,
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
        self.execute_with_progress(weights, latents_whcb, &mut None)
    }

    fn execute_with_progress(
        &self,
        weights: &LoadedFluxVaeWeights,
        latents_whcb: &[f32],
        progress: &mut Option<ProgressHook>,
    ) -> Result<FluxVaeDecodeRun> {
        if gpu_device_available() {
            match self.execute_device(weights, latents_whcb, progress) {
                Ok(run) => return Ok(run),
                Err(DiffusionError::Cancelled) => {
                    // Mirror the success-path cleanup: the decode clamped the
                    // idle-pool cap, don't leak that into the next job.
                    gpu_pool_clear();
                    gpu_pool_cap_override(None);
                    return Err(DiffusionError::Cancelled);
                }
                Err(err) => {
                    eprintln!("flux vae device path failed ({err}); falling back to host math");
                }
            }
        }
        Ok(self.execute_with_debug(weights, latents_whcb)?.final_image)
    }

    /// Device-resident mirror of `execute_with_debug`: the whole decode runs
    /// on the GPU (planar [c][y][x] activations resident; conv weights
    /// device-cached across runs), only the final RGB planes come back.
    fn execute_device(
        &self,
        weights: &LoadedFluxVaeWeights,
        latents_whcb: &[f32],
        progress: &mut Option<ProgressHook>,
    ) -> Result<FluxVaeDecodeRun> {
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
        let namespace = flux_vae_cache_namespace(weights);
        // Phase boundary: drop the denoise loop's pooled activation buckets
        // so the transformer and VAE working sets never coexist in VRAM, and
        // clamp the idle-pool cap for the decode — its multi-GB planes on top
        // of the resident weights sit at the 32GB WDDM residency cliff where
        // every idle GB costs ~3x in paging stalls (1438ms -> 406ms @1024).
        gpu_pool_clear();
        let vae_cap_mb = std::env::var("FLUX_GPU_POOL_CAP_VAE_MB")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(2048);
        gpu_pool_cap_override(Some(vae_cap_mb * 1024 * 1024));

        let mut hidden = GpuSpatial {
            tensor: gpu_upload(
                latents_whcb,
                self.latent_shape.latent_channels as usize,
                (self.latent_shape.latent_width * self.latent_shape.latent_height) as usize,
            )
            .map_err(DiffusionError::model)?,
            width: self.latent_shape.latent_width as usize,
            height: self.latent_shape.latent_height as usize,
            channels: self.latent_shape.latent_channels as usize,
        };
        // Per-unit progress: mid section (3) + per-stage resnet blocks + the
        // output head. On warm runs the loop enqueues async kernels and the
        // labels tick ahead of execution; on cold runs each unit's conv
        // weights upload synchronously, so the labels track the stream-in.
        let total_units = 3 + (0..=3)
            .map(|stage| {
                3 + weights.has_tensor(&format!("decoder.up.{stage}.upsample.conv.weight"))
                    as usize
            })
            .sum::<usize>()
            + 1;
        let mut done_units = 0usize;
        let unit_progress = |done: &mut usize,
                             progress: &mut Option<ProgressHook>|
         -> Result<()> {
            if progress.is_some() {
                emit_progress(
                    progress,
                    &format!("block {}/{total_units}", *done + 1),
                    *done as f64 / total_units as f64,
                )?;
            }
            *done += 1;
            Ok(())
        };

        unit_progress(&mut done_units, progress)?;
        hidden = conv2d_device(
            weights,
            &namespace,
            &hidden,
            "decoder.conv_in.weight",
            "decoder.conv_in.bias",
            1,
            1,
        )?;
        hidden = resnet_block_device(weights, &namespace, &hidden, "decoder.mid.block_1")?;
        unit_progress(&mut done_units, progress)?;
        hidden = mid_attention_device(weights, &namespace, &hidden, "decoder.mid.attn_1")?;
        unit_progress(&mut done_units, progress)?;
        hidden = resnet_block_device(weights, &namespace, &hidden, "decoder.mid.block_2")?;

        for stage in (0..=3).rev() {
            let stage_prefix = format!("decoder.up.{stage}");
            for block in 0..=2 {
                unit_progress(&mut done_units, progress)?;
                hidden = resnet_block_device(
                    weights,
                    &namespace,
                    &hidden,
                    &format!("{stage_prefix}.block.{block}"),
                )?;
            }
            if weights.has_tensor(&format!("{stage_prefix}.upsample.conv.weight")) {
                unit_progress(&mut done_units, progress)?;
                hidden = upsample_nearest2x_device(&hidden)?;
                hidden = conv2d_device(
                    weights,
                    &namespace,
                    &hidden,
                    &format!("{stage_prefix}.upsample.conv.weight"),
                    &format!("{stage_prefix}.upsample.conv.bias"),
                    1,
                    1,
                )?;
            }
        }
        unit_progress(&mut done_units, progress)?;

        hidden = group_norm_device(
            weights,
            &namespace,
            &hidden,
            "decoder.norm_out.weight",
            "decoder.norm_out.bias",
            32,
        )?;
        hidden = silu_device(&hidden)?;
        let image = conv2d_device(
            weights,
            &namespace,
            &hidden,
            "decoder.conv_out.weight",
            "decoder.conv_out.bias",
            1,
            1,
        )?;

        let data = gpu_download(&image.tensor).map_err(DiffusionError::model)?;
        let run = FluxVaeDecodeRun {
            image: data,
            width: image.width,
            height: image.height,
        };
        drop(image);
        drop(hidden);
        // Free the VAE's pooled buckets before the next generation's denoise
        // and restore the roomier denoise-phase pool cap.
        gpu_pool_clear();
        gpu_pool_cap_override(None);
        Ok(run)
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
    // Intermediates stay metadata-only. The Metal binding planner assigns
    // (and reuses) GPU ranges; CPU-allocating them is what grew a 1GiB
    // extra arena for every VAE compile.
    weights.ctx.set_no_alloc(true);

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
    let out = conv_2d_im2col_mm(
        ctx,
        require_tensor_id(tensor_ids, weight_name)?,
        input,
        1,
        1,
        pad_x,
        pad_y,
        1,
        1,
    )?;
    let bias = broadcast_channel_vector(ctx, require_tensor_id(tensor_ids, bias_name)?, out)?;
    ctx.binary_like_a(Op::Add, out, bias, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

/// ggml_conv_2d as im2col + mul_mat (the official CPU/CUDA expansion).
/// Metal's kernel_conv_2d is a naive per-pixel IC*KH*KW loop; mul_mm is
/// the fast path already used by the Flux transformer.
///
/// Weights stay F16; im2col dest stays F32 so GEMM is `mul_mm_f16_f32`.
fn conv_2d_im2col_mm(
    ctx: &mut Context,
    kernel: TensorId,
    input: TensorId,
    s0: i32,
    s1: i32,
    p0: i32,
    p1: i32,
    d0: i32,
    d1: i32,
) -> Result<TensorId> {
    let (k_flat, oc) = {
        let kernel_t = require_tensor(ctx, kernel)?;
        let k_flat = kernel_t.ne[0]
            .checked_mul(kernel_t.ne[1])
            .and_then(|value| value.checked_mul(kernel_t.ne[2]))
            .ok_or_else(|| DiffusionError::model("flux vae conv k_flat overflow"))?;
        (k_flat, kernel_t.ne[3])
    };
    // Keep im2col activations F32. F16-act × F16-weight GEMM (K up to
    // 3*3*512) overflows half and the VAE decode goes solid black.
    // F16 weights × F32 cols select kernel_mul_mm_f16_f32.
    let cols = ctx
        .im2col(
            kernel,
            input,
            s0,
            s1,
            p0,
            p1,
            d0,
            d1,
            true,
            TensorType::F32,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let (ow, oh, n) = {
        let cols_t = require_tensor(ctx, cols)?;
        (cols_t.ne[1], cols_t.ne[2], cols_t.ne[3])
    };
    let plane = ow
        .checked_mul(oh)
        .and_then(|value| value.checked_mul(n))
        .ok_or_else(|| DiffusionError::model("flux vae conv plane overflow"))?;
    let cols_2d = ctx
        .reshape(cols, &[k_flat, plane])
        .map_err(DiffusionError::model)?;
    let kernel_2d = ctx
        .reshape(kernel, &[k_flat, oc])
        .map_err(DiffusionError::model)?;
    let gemm = ctx
        .mul_mat(kernel_2d, cols_2d, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    // mul_mat dest is [OC, OW*OH*N] == [OC, OW, OH, N] in memory.
    // ggml permute is dest.ne[axis[i]] = src.ne[i], so (2,0,1,3) yields
    // [OW, OH, OC, N] — the same layout as Op::Conv2d.
    let gemm_4d = ctx
        .reshape(gemm, &[oc, ow, oh, n])
        .map_err(DiffusionError::model)?;
    let whcn = ctx
        .permute(gemm_4d, [2, 0, 1, 3])
        .map_err(DiffusionError::model)?;
    ctx.cont(whcn).map_err(DiffusionError::model)
}

fn apply_silu(ctx: &mut Context, input: TensorId) -> Result<TensorId> {
    ctx.unary(input, UnaryOp::Silu, BufferUsage::Activations)
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

    if let Some(output) = crate::backend::try_conv2d_planar_f32(
        &input.data,
        input.width,
        input.height,
        input.channels,
        &kernel.data,
        &bias,
        kernel.out_channels,
        kernel.kw,
        kernel.kh,
        pad_x,
        pad_y,
    ) {
        return SpatialTensor::new(input.width, input.height, kernel.out_channels, output);
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
    if let Some(output) = crate::backend::try_group_norm_planar_f32(
        &input.data,
        input.width,
        input.height,
        input.channels,
        groups,
        &weight,
        &bias,
        VAE_GROUP_NORM_EPSILON,
    ) {
        return SpatialTensor::new(input.width, input.height, input.channels, output);
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
    if let Some(data) = crate::backend::try_silu_f32(&input.data) {
        return SpatialTensor::new(input.width, input.height, input.channels, data);
    }
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

// ---------------------------------------------------------------------------
// Host encoder path (image -> latent mean): used by FLUX.1-Fill's masked-
// image conditioning (`flux_fill_pipeline.rs`). Mirrors the *_spatial decode
// helpers above in reverse — the `down` stages use strided convs (no
// backend-accelerated stride>1 conv2d exists yet, see
// `downsample_conv2d_spatial`), 2 resnet blocks per stage (vs. the decoder's
// 3), and a `mid` block identical in shape to the decoder's. There is
// deliberately no device-resident fast path yet (unlike decode's
// `execute_device`) — this always runs the host f32 math below.
// ---------------------------------------------------------------------------

/// Encodes an RGB image (already in `[-1, 1]`, planar `[c][y][x]` layout,
/// `width`/`height` both a multiple of 8) through the FLUX VAE encoder down
/// to the raw (unscaled) latent distribution MEAN — 16 channels at
/// `width/8 x height/8`. Deliberately returns the distribution's mean, not a
/// sample from it: FLUX.1-Fill's `masked_image_latents` conditioning is
/// fixed for the whole denoise run, and a mean encode keeps the whole
/// request deterministic on the caller's `seed` alone (matches ComfyUI's
/// VAEEncode convention; diffusers' `FluxFillPipeline` instead samples the
/// posterior through a second RNG stream this port does not reproduce). The
/// result is in the VAE's own latent space — callers apply
/// `FLUX_VAE_SHIFT_FACTOR`/`FLUX_VAE_SCALING_FACTOR` (`flux_schedule.rs`)
/// before packing, the exact inverse of what decode does after unpacking.
pub fn encode_image_to_latent_mean(
    weights: &LoadedFluxVaeWeights,
    image_chw: &[f32],
    width: usize,
    height: usize,
) -> Result<Vec<f32>> {
    if width % 8 != 0 || height % 8 != 0 {
        return Err(DiffusionError::workflow(format!(
            "flux vae encode: width/height must be multiples of 8, got {}x{}",
            width, height
        )));
    }
    let expected = width
        .checked_mul(height)
        .and_then(|value| value.checked_mul(3))
        .ok_or_else(|| DiffusionError::model("flux vae encode input size overflow"))?;
    if image_chw.len() != expected {
        return Err(DiffusionError::workflow(format!(
            "flux vae encode expected {} values for a {}x{} RGB image, got {}",
            expected,
            width,
            height,
            image_chw.len()
        )));
    }

    let mut hidden = SpatialTensor::new(width, height, 3, image_chw.to_vec())?;
    hidden = apply_conv2d_spatial(
        weights,
        &hidden,
        "encoder.conv_in.weight",
        "encoder.conv_in.bias",
        1,
        1,
    )?;

    for stage in 0..=3usize {
        let stage_prefix = format!("encoder.down.{stage}");
        for block in 0..=1usize {
            hidden =
                resnet_block_spatial(weights, &hidden, &format!("{stage_prefix}.block.{block}"))?;
        }
        if weights.has_tensor(&format!("{stage_prefix}.downsample.conv.weight")) {
            hidden = downsample_conv2d_spatial(
                weights,
                &hidden,
                &format!("{stage_prefix}.downsample.conv.weight"),
                &format!("{stage_prefix}.downsample.conv.bias"),
            )?;
        }
    }

    hidden = resnet_block_spatial(weights, &hidden, "encoder.mid.block_1")?;
    hidden = mid_attention_spatial(weights, &hidden, "encoder.mid.attn_1")?;
    hidden = resnet_block_spatial(weights, &hidden, "encoder.mid.block_2")?;

    hidden = apply_group_norm_spatial(
        weights,
        &hidden,
        "encoder.norm_out.weight",
        "encoder.norm_out.bias",
        32,
    )?;
    hidden = silu_spatial(&hidden)?;
    let moments = apply_conv2d_spatial(
        weights,
        &hidden,
        "encoder.conv_out.weight",
        "encoder.conv_out.bias",
        1,
        1,
    )?;

    // `moments` is [2*z_channels][latent_h][latent_w] channel-major
    // (`encoder.conv_out` has no quant_conv after it in FLUX's autoencoder,
    // unlike vanilla SD): channels 0..16 are the mean, 16..32 the logvar we
    // deliberately drop (see the doc comment above).
    let z_channels = 16usize;
    if moments.channels != z_channels * 2 {
        return Err(DiffusionError::model(format!(
            "flux vae encode: encoder.conv_out produced {} channels, expected {}",
            moments.channels,
            z_channels * 2
        )));
    }
    let plane = moments.width * moments.height;
    Ok(moments.data[..z_channels * plane].to_vec())
}

/// The FLUX/SD `Downsample` module: zero-pad width/height by 1 on the
/// right/bottom only (`F.pad(x, (0,1,0,1))`), then a stride-2, padding-0,
/// 3x3 conv. Exact output size is `width/2 x height/2` for even input
/// (guaranteed by the caller: FLUX image sizes are always multiples of 16).
/// No backend-accelerated conv2d takes a stride today (`try_conv2d_planar_f32`
/// is stride-1 "same" only) — this is host f32 math, the same tier the
/// fallback loop inside `apply_conv2d_spatial` already runs.
fn downsample_conv2d_spatial(
    weights: &LoadedFluxVaeWeights,
    input: &SpatialTensor,
    weight_name: &str,
    bias_name: &str,
) -> Result<SpatialTensor> {
    let kernel = weights.tensor_kernel(weight_name)?;
    let bias = weights.tensor_f32_values(bias_name)?;
    if kernel.in_channels != input.channels {
        return Err(DiffusionError::model(format!(
            "flux vae downsample input channels mismatch: input={} kernel={}",
            input.channels, kernel.in_channels
        )));
    }
    if bias.len() != kernel.out_channels {
        return Err(DiffusionError::model(format!(
            "flux vae downsample bias len mismatch: bias={} out_channels={}",
            bias.len(),
            kernel.out_channels
        )));
    }
    if kernel.kw != 3 || kernel.kh != 3 {
        return Err(DiffusionError::model(format!(
            "flux vae downsample expects a 3x3 kernel, got {}x{}",
            kernel.kw, kernel.kh
        )));
    }
    if input.width % 2 != 0 || input.height % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "flux vae downsample expects even input size, got {}x{}",
            input.width, input.height
        )));
    }
    let out_width = input.width / 2;
    let out_height = input.height / 2;
    let mut output = vec![0.0f32; out_width * out_height * kernel.out_channels];
    for out_c in 0..kernel.out_channels {
        for oy in 0..out_height {
            for ox in 0..out_width {
                let mut acc = bias[out_c];
                for in_c in 0..kernel.in_channels {
                    for ky in 0..3usize {
                        // Padded space has zero padding only on the
                        // right/bottom, so a padded row/col index equal to
                        // the (unpadded) input size is the zero pad and
                        // contributes nothing.
                        let py = oy * 2 + ky;
                        if py >= input.height {
                            continue;
                        }
                        for kx in 0..3usize {
                            let px = ox * 2 + kx;
                            if px >= input.width {
                                continue;
                            }
                            let input_value = input.data[input.offset(px, py, in_c)];
                            let weight_index = kx
                                + kernel.kw
                                    * (ky + kernel.kh * (in_c + kernel.in_channels * out_c));
                            acc += input_value * kernel.data[weight_index];
                        }
                    }
                }
                output[ox + out_width * (oy + out_height * out_c)] = acc;
            }
        }
    }
    SpatialTensor::new(out_width, out_height, kernel.out_channels, output)
}

// ---------------------------------------------------------------------------
// Device-resident decode path (mirrors the *_spatial helpers above; planar
// [c][y][x] data lives in a GpuTensor with rows=channels, cols=width*height).
// ---------------------------------------------------------------------------

struct GpuSpatial {
    tensor: GpuTensor,
    width: usize,
    height: usize,
    channels: usize,
}

pub(crate) fn flux_vae_cache_namespace(weights: &LoadedFluxVaeWeights) -> String {
    format!("flux_vae:{}", weights.path.display())
}

/// Drops every device weight-cache entry belonging to these vae weights.
/// Called on a model switch — combined checkpoints key all components on
/// the outgoing checkpoint path.
pub(crate) fn evict_device_weight_cache(weights: &LoadedFluxVaeWeights) -> usize {
    crate::backend::gpu_weight_cache_evict_prefix(&flux_vae_cache_namespace(weights)).unwrap_or(0)
}

fn conv2d_device(
    weights: &LoadedFluxVaeWeights,
    namespace: &str,
    input: &GpuSpatial,
    weight_name: &str,
    bias_name: &str,
    pad_x: usize,
    pad_y: usize,
) -> Result<GpuSpatial> {
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
    let tensor = gpu_conv2d_planar_cached(
        &input.tensor,
        input.width,
        input.height,
        namespace,
        weight_name,
        &kernel.data,
        &bias,
        kernel.out_channels,
        kernel.kw,
        kernel.kh,
        pad_x,
        pad_y,
    )
    .map_err(DiffusionError::model)?;
    Ok(GpuSpatial {
        tensor,
        width: input.width,
        height: input.height,
        channels: kernel.out_channels,
    })
}

fn group_norm_device(
    weights: &LoadedFluxVaeWeights,
    namespace: &str,
    input: &GpuSpatial,
    weight_name: &str,
    bias_name: &str,
    groups: usize,
) -> Result<GpuSpatial> {
    let gamma = weights.tensor_f32_values(weight_name)?;
    let beta = weights.tensor_f32_values(bias_name)?;
    if gamma.len() != input.channels || beta.len() != input.channels {
        return Err(DiffusionError::model(format!(
            "flux vae group norm weight/bias mismatch: channels={} weight={} bias={}",
            input.channels,
            gamma.len(),
            beta.len()
        )));
    }
    let tensor = gpu_group_norm_planar(
        &input.tensor,
        input.width,
        input.height,
        groups,
        namespace,
        weight_name,
        &gamma,
        &beta,
        VAE_GROUP_NORM_EPSILON,
    )
    .map_err(DiffusionError::model)?;
    Ok(GpuSpatial {
        tensor,
        width: input.width,
        height: input.height,
        channels: input.channels,
    })
}

fn silu_device(input: &GpuSpatial) -> Result<GpuSpatial> {
    Ok(GpuSpatial {
        tensor: gpu_silu(&input.tensor).map_err(DiffusionError::model)?,
        width: input.width,
        height: input.height,
        channels: input.channels,
    })
}

fn add_device(lhs: &GpuSpatial, rhs: &GpuSpatial) -> Result<GpuSpatial> {
    if lhs.width != rhs.width || lhs.height != rhs.height || lhs.channels != rhs.channels {
        return Err(DiffusionError::model("flux vae device add shape mismatch"));
    }
    Ok(GpuSpatial {
        tensor: gpu_add(&lhs.tensor, &rhs.tensor).map_err(DiffusionError::model)?,
        width: lhs.width,
        height: lhs.height,
        channels: lhs.channels,
    })
}

fn upsample_nearest2x_device(input: &GpuSpatial) -> Result<GpuSpatial> {
    Ok(GpuSpatial {
        tensor: gpu_upsample_nearest2x(&input.tensor, input.width, input.height)
            .map_err(DiffusionError::model)?,
        width: input.width * 2,
        height: input.height * 2,
        channels: input.channels,
    })
}

fn resnet_block_device(
    weights: &LoadedFluxVaeWeights,
    namespace: &str,
    input: &GpuSpatial,
    prefix: &str,
) -> Result<GpuSpatial> {
    // Re-assign (not shadow): each intermediate plane returns to the pool as
    // soon as its consumer ran. Shadowing kept six planes live per block —
    // at 1024px that alone exceeded the idle-pool cap and turned every op
    // into a fresh WDDM cudaMalloc (see flux2_vae.rs::resnet).
    let mut hidden = group_norm_device(
        weights,
        namespace,
        input,
        &format!("{prefix}.norm1.weight"),
        &format!("{prefix}.norm1.bias"),
        32,
    )?;
    hidden = silu_device(&hidden)?;
    hidden = conv2d_device(
        weights,
        namespace,
        &hidden,
        &format!("{prefix}.conv1.weight"),
        &format!("{prefix}.conv1.bias"),
        1,
        1,
    )?;
    hidden = group_norm_device(
        weights,
        namespace,
        &hidden,
        &format!("{prefix}.norm2.weight"),
        &format!("{prefix}.norm2.bias"),
        32,
    )?;
    hidden = silu_device(&hidden)?;
    hidden = conv2d_device(
        weights,
        namespace,
        &hidden,
        &format!("{prefix}.conv2.weight"),
        &format!("{prefix}.conv2.bias"),
        1,
        1,
    )?;
    if weights.has_tensor(&format!("{prefix}.nin_shortcut.weight")) {
        let residual = conv2d_device(
            weights,
            namespace,
            input,
            &format!("{prefix}.nin_shortcut.weight"),
            &format!("{prefix}.nin_shortcut.bias"),
            0,
            0,
        )?;
        add_device(&residual, &hidden)
    } else {
        add_device(input, &hidden)
    }
}

fn mid_attention_device(
    weights: &LoadedFluxVaeWeights,
    namespace: &str,
    input: &GpuSpatial,
    prefix: &str,
) -> Result<GpuSpatial> {
    let hidden = group_norm_device(
        weights,
        namespace,
        input,
        &format!("{prefix}.norm.weight"),
        &format!("{prefix}.norm.bias"),
        32,
    )?;
    let q = conv2d_device(
        weights,
        namespace,
        &hidden,
        &format!("{prefix}.q.weight"),
        &format!("{prefix}.q.bias"),
        0,
        0,
    )?;
    let k = conv2d_device(
        weights,
        namespace,
        &hidden,
        &format!("{prefix}.k.weight"),
        &format!("{prefix}.k.bias"),
        0,
        0,
    )?;
    let v = conv2d_device(
        weights,
        namespace,
        &hidden,
        &format!("{prefix}.v.weight"),
        &format!("{prefix}.v.bias"),
        0,
        0,
    )?;
    let scale = 1.0 / (q.channels as f32).sqrt();
    drop(hidden);
    let attn = gpu_attention_planar_single(&q.tensor, &k.tensor, &v.tensor, scale)
        .map_err(DiffusionError::model)?;
    drop((q, k, v));
    let attn = GpuSpatial {
        tensor: attn,
        width: input.width,
        height: input.height,
        channels: input.channels,
    };
    let attn = conv2d_device(
        weights,
        namespace,
        &attn,
        &format!("{prefix}.proj_out.weight"),
        &format!("{prefix}.proj_out.bias"),
        0,
        0,
    )?;
    add_device(input, &attn)
}

fn tensor_to_f32_vec(ctx: &Context, tensor_id: TensorId) -> Result<Vec<f32>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    let bytes = ctx.tensor_data(tensor_id).map_err(DiffusionError::model)?;
    match tensor.desc.ty {
        TensorType::F32 => f32_bytes_to_vec(bytes),
        TensorType::F16 => f16_bytes_to_f32_vec(bytes),
        other => Err(DiffusionError::model(format!(
            "flux vae tensor {} cannot be decoded as f32 from {:?}",
            tensor_id, other
        ))),
    }
}

fn conv_kernel(ctx: &Context, tensor_id: TensorId) -> Result<ConvKernel> {
    let tensor = require_tensor(ctx, tensor_id)?;
    let data = match tensor.desc.ty {
        TensorType::F32 => {
            f32_bytes_to_vec(ctx.tensor_data(tensor_id).map_err(DiffusionError::model)?)?
        }
        TensorType::F16 => {
            f16_bytes_to_f32_vec(ctx.tensor_data(tensor_id).map_err(DiffusionError::model)?)?
        }
        other => {
            return Err(DiffusionError::model(format!(
                "flux vae conv kernel {} must be F16/F32, got {:?}",
                tensor_id, other
            )))
        }
    };
    Ok(ConvKernel {
        data,
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
        let ty = vae_stored_tensor_type(entry)?;
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
    let ty = vae_stored_tensor_type(entry)?;
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
        // BF16 source (e.g. the community flux-vae-bf16.safetensors repack
        // used by the flux1-fill-dev split bundle) is upconverted to F32
        // here; `vae_stored_tensor_type` below decides whether rank-4 conv
        // kernels then get narrowed back to F16 for the im2col GEMM path,
        // same as an F32 source would.
        MlxDType::F32 | MlxDType::BF16 => Ok(TensorType::F32),
        other => Err(DiffusionError::model(format!(
            "flux vae unsupported tensor dtype {:?}",
            other
        ))),
    }
}

/// Rank-4 conv kernels are stored F16 once so compiled GEMMs use
/// `kernel_mul_mm_f16_f16`. Rank-1 bias/norm stay F32.
fn vae_stored_tensor_type(entry: &MlxTensorEntry) -> Result<TensorType> {
    vae_target_tensor_type(entry.dtype)?;
    if entry.shape.len() == 4 {
        Ok(TensorType::F16)
    } else {
        Ok(TensorType::F32)
    }
}

fn vae_target_bytes(
    header: &MlxSafetensorsHeader,
    name: &str,
    entry: &MlxTensorEntry,
) -> Result<Vec<u8>> {
    match (entry.dtype, vae_stored_tensor_type(entry)?) {
        (MlxDType::F32, TensorType::F32) => Ok(header.read_tensor_bytes(name)?),
        (MlxDType::F32, TensorType::F16) => {
            let f32_bytes = header.read_tensor_bytes(name)?;
            f32_bytes_to_f16_bytes(&f32_bytes)
        }
        (MlxDType::BF16, TensorType::F32) => {
            let bf16_bytes = header.read_tensor_bytes(name)?;
            bf16_bytes_to_f32_bytes(&bf16_bytes)
        }
        (MlxDType::BF16, TensorType::F16) => {
            let bf16_bytes = header.read_tensor_bytes(name)?;
            let f32_bytes = bf16_bytes_to_f32_bytes(&bf16_bytes)?;
            f32_bytes_to_f16_bytes(&f32_bytes)
        }
        (other, _) => Err(DiffusionError::model(format!(
            "flux vae unsupported tensor dtype {:?}",
            other
        ))),
    }
}

fn f32_bytes_to_f16_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    let values = f32_bytes_to_vec(bytes)?;
    let mut out = Vec::with_capacity(values.len() * 2);
    for value in values {
        out.extend_from_slice(&f32_to_f16(value).to_le_bytes());
    }
    Ok(out)
}

fn bf16_bytes_to_f32_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "flux vae bf16 byte length {} is not even",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for chunk in bytes.chunks_exact(2) {
        let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
        out.extend_from_slice(&bf16_to_f32(bits).to_le_bytes());
    }
    Ok(out)
}

fn f16_bytes_to_f32_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "flux vae f16 byte length {} is not even",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
        .collect())
}

fn f32_stats(values: &[f32]) -> String {
    let mut finite = 0usize;
    let mut nan = 0usize;
    let mut inf = 0usize;
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for &value in values {
        if value.is_nan() {
            nan += 1;
            continue;
        }
        if value.is_infinite() {
            inf += 1;
            continue;
        }
        finite += 1;
        min = min.min(value);
        max = max.max(value);
    }
    format!("finite={finite} nan={nan} inf={inf} min={min} max={max}")
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

// ---------------------------------------------------------------------------
// Encoder (image -> raw VAE latent mean) — added for the `control` domain
// (FLUX.1-Depth-dev / FLUX.1-Canny-dev), whose `img_in` concatenates packed
// noisy latents with a packed VAE-encoded control-image latent every step
// (see `flux_pipeline::FluxPipeline::generate_control_with_hooks`). The
// standard FLUX.1 `ae.safetensors` is a plain AutoencoderKL: the `encoder.*`
// tensors mirror the `decoder.*` ones above (conv_in -> 4 resnet+downsample
// stages -> mid block/attn/block -> norm_out+silu -> conv_out), plus an
// optional top-level `quant_conv` 1x1 (present in the classic SD-style
// layout, absent from FLUX.1's — decode never applies `post_quant_conv`
// either, see `build_flux_vae_decoder_graph`, so this is a defensive check,
// not a load-bearing one for FLUX.1 itself).
//
// This returns the RAW (unscaled) posterior MEAN — never `sample()` — the
// same "encode is deterministic" convention `flux2_vae::flux2_vae_encode`
// already established in this codebase; the caller (flux_pipeline.rs)
// applies the `(mean - shift) * scale` conversion into the trained-latent
// domain, mirroring the decode side's `(latent / scale) + shift` which lives
// in flux_pipeline.rs too, not here.
//
// Unlike decode, there is no eager "device" (non-compiled-graph) path here:
// the downsample stride-2 conv has no Metal implementation in the shared
// backend today (`gpu_conv2d_planar_strided` is CUDA-only — see
// `libs/ai/models/common/src/gpu.rs`; every other caller of that primitive
// in this codebase, e.g. `flux2_vae::downsample`, has the same restriction).
// The compiled-graph path (`Op::Conv2d` with an explicit stride, executed
// through the same CUDA/Metal graph runtime decode uses) has no such gap —
// it is the real path whenever a runtime is available at all. Only the pure
// CPU "spatial" path (used when `FLUX_VAE_MODE=lazy` or no runtime exists)
// is left as the fallback, same as decode's Lazy mode.
#[derive(Clone, Debug)]
pub struct FluxVaeEncoderGraph {
    pub graph: Graph,
    pub input_image: TensorId,
    pub result_moments: TensorId,
    pub image_width: usize,
    pub image_height: usize,
    pub latent_width: usize,
    pub latent_height: usize,
}

/// Encodes a control image (interleaved-by-channel CHW, i.e. `[c][y][x]`,
/// values already in `[-1, 1]`) into the raw 16-channel VAE latent mean, in
/// the same `[c][y][x]` (NCHW, batch 1) layout `pack_flux_latents_nchw`
/// expects. `image_width`/`image_height` must be divisible by 8 (three
/// downsample stages) — in practice always true here since the control
/// pipeline only ever calls this with a `FluxLatentShape`-validated size
/// (divisible by 16).
///
/// Loads its OWN fresh `LoadedFluxVaeWeights` from `vae_path` rather than
/// touching a pipeline's resident decode-oriented weights/graph: the VAE
/// file is small (~335MB) and this keeps the encode path fully independent
/// of the persistent decode `CompiledFluxVae` — no risk of two graphs
/// sharing one `Context`'s no-alloc bookkeeping in ways this port hasn't
/// verified on real hardware (no GPU in this development environment).
pub fn encode_control_image_hooked(
    vae_path: &Path,
    vae_component_prefix: Option<&str>,
    pixel_chw_neg1_to_1: &[f32],
    image_width: usize,
    image_height: usize,
    mut progress: Option<ProgressHook>,
) -> Result<Vec<f32>> {
    if image_width == 0 || image_height == 0 || image_width % 8 != 0 || image_height % 8 != 0 {
        return Err(DiffusionError::workflow(format!(
            "flux vae encode requires a non-zero image size divisible by 8, got {}x{}",
            image_width, image_height
        )));
    }
    let expected = image_width
        .checked_mul(image_height)
        .and_then(|value| value.checked_mul(3))
        .ok_or_else(|| DiffusionError::model("flux vae encode input size overflow"))?;
    if pixel_chw_neg1_to_1.len() != expected {
        return Err(DiffusionError::workflow(format!(
            "flux vae encode expected {} CHW values for {}x{}x3, got {}",
            expected,
            image_width,
            image_height,
            pixel_chw_neg1_to_1.len()
        )));
    }

    emit_progress(&mut progress, "vae-encode load", 0.0)?;
    let mut weights = LoadedFluxVaeWeights::load_component(vae_path, vae_component_prefix)?;
    if !weights.has_tensor("encoder.conv_in.weight") {
        return Err(DiffusionError::model(format!(
            "{} has no encoder.* tensors (VAE encode needs the full AutoencoderKL, not a decode-only export)",
            vae_path.display()
        )));
    }

    emit_progress(&mut progress, "vae-encode", 0.05)?;
    let mode = FluxVaeExecutionMode::from_env();
    let mean = match mode {
        FluxVaeExecutionMode::Compiled => {
            let runtime = new_runtime()?;
            encode_compiled_once(runtime, &mut weights, pixel_chw_neg1_to_1, image_width, image_height)?
        }
        FluxVaeExecutionMode::Lazy => {
            encode_spatial(&weights, pixel_chw_neg1_to_1, image_width, image_height)?
        }
    };
    emit_progress(&mut progress, "vae-encode", 1.0)?;

    let expected_latents = 16 * (image_width / 8) * (image_height / 8);
    if mean.len() != expected_latents {
        return Err(DiffusionError::model(format!(
            "flux vae encode produced {} latent values, expected {}",
            mean.len(),
            expected_latents
        )));
    }
    Ok(mean)
}

pub fn encode_control_image(
    vae_path: &Path,
    vae_component_prefix: Option<&str>,
    pixel_chw_neg1_to_1: &[f32],
    image_width: usize,
    image_height: usize,
) -> Result<Vec<f32>> {
    encode_control_image_hooked(
        vae_path,
        vae_component_prefix,
        pixel_chw_neg1_to_1,
        image_width,
        image_height,
        None,
    )
}

/// Builds + prepares + runs the encoder graph exactly once (no OOM-retry
/// growth loop — unlike decode/transformer, this is a small one-shot ~335MB
/// checkpoint, not a resident multi-GB stack, so a fixed default reserve is
/// expected to be plenty; the [`DiffusionError`] still surfaces clearly if
/// not). The session and graph are dropped at the end of this call.
fn encode_compiled_once(
    runtime: Runtime,
    weights: &mut LoadedFluxVaeWeights,
    pixel_chw_neg1_to_1: &[f32],
    image_width: usize,
    image_height: usize,
) -> Result<Vec<f32>> {
    let graph = build_flux_vae_encoder_graph(weights, image_width, image_height)?;
    let prepared = prepare_graph(&runtime, &weights.ctx, &graph.graph)?;
    let session = create_graph_session(
        &runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let pixel_bytes = f32s_to_le_bytes(pixel_chw_neg1_to_1);
    let execution = session
        .execute(
            &weights.ctx,
            &[GraphTensorWrite {
                tensor_id: graph.input_image,
                bytes: &pixel_bytes,
            }],
            &[graph.result_moments],
        )
        .map_err(DiffusionError::model)?;
    let moments_bytes = execution.outputs.get(&graph.result_moments).ok_or_else(|| {
        DiffusionError::model("flux vae encode did not return the moments tensor")
    })?;
    let moments = f32_bytes_to_vec(moments_bytes)?;
    let z_channels = 16usize;
    let plane = graph.latent_width * graph.latent_height;
    if moments.len() != 2 * z_channels * plane {
        return Err(DiffusionError::model(format!(
            "flux vae encode moments length {} != 2*{}*{}",
            moments.len(),
            z_channels,
            plane
        )));
    }
    // Moments are [2*z_channels][h][w] channel-planar; the first half is the
    // posterior mean (see the module doc: mean, never sample()).
    Ok(moments[..z_channels * plane].to_vec())
}

/// Builds the one-shot encoder graph: `encoder.conv_in` -> 4 downsample
/// stages (2 resnet blocks each, downsample on stages 0..=2 only — mirrors
/// decode's `up.{0..=3}` where only 3 of 4 stages upsample) -> mid
/// block/attn/block -> norm_out+silu -> `conv_out` (-> optional top-level
/// `quant_conv` 1x1, see the module doc). Reuses every `resnet_block` /
/// `mid_attention` / `apply_group_norm` / `apply_silu` helper decode already
/// defines above — those are prefix-parameterized and channel-count
/// agnostic, so "encoder.*" prefixes work unchanged.
pub fn build_flux_vae_encoder_graph(
    weights: &mut LoadedFluxVaeWeights,
    image_width: usize,
    image_height: usize,
) -> Result<FluxVaeEncoderGraph> {
    let input_image = weights
        .ctx
        .new_named_tensor(
            "flux_vae.encoder.input_image",
            TensorType::F32,
            4,
            &[image_width as i64, image_height as i64, 3, 1],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    weights.ctx.set_no_alloc(true);

    let mut hidden = apply_conv2d(
        &mut weights.ctx,
        &weights.tensor_ids,
        input_image,
        "encoder.conv_in.weight",
        "encoder.conv_in.bias",
        1,
        1,
    )?;
    for level in 0..=3 {
        let stage_prefix = format!("encoder.down.{level}");
        for block in 0..=1 {
            hidden = resnet_block(
                &mut weights.ctx,
                &weights.tensor_ids,
                hidden,
                &format!("{stage_prefix}.block.{block}"),
            )?;
        }
        if weights.has_tensor(&format!("{stage_prefix}.downsample.conv.weight")) {
            hidden = apply_conv2d_downsample(
                &mut weights.ctx,
                &weights.tensor_ids,
                hidden,
                &format!("{stage_prefix}.downsample.conv.weight"),
                &format!("{stage_prefix}.downsample.conv.bias"),
            )?;
        }
    }
    hidden = resnet_block(&mut weights.ctx, &weights.tensor_ids, hidden, "encoder.mid.block_1")?;
    hidden = mid_attention(&mut weights.ctx, &weights.tensor_ids, hidden, "encoder.mid.attn_1")?;
    hidden = resnet_block(&mut weights.ctx, &weights.tensor_ids, hidden, "encoder.mid.block_2")?;
    hidden = apply_group_norm(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        "encoder.norm_out.weight",
        "encoder.norm_out.bias",
        32,
    )?;
    hidden = apply_silu(&mut weights.ctx, hidden)?;
    let mut moments = apply_conv2d(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        "encoder.conv_out.weight",
        "encoder.conv_out.bias",
        1,
        1,
    )?;
    if weights.has_tensor("quant_conv.weight") {
        moments = apply_conv2d(
            &mut weights.ctx,
            &weights.tensor_ids,
            moments,
            "quant_conv.weight",
            "quant_conv.bias",
            0,
            0,
        )?;
    }

    let mut graph = Graph::new();
    graph
        .build_forward_expand(&weights.ctx, moments)
        .map_err(DiffusionError::model)?;

    Ok(FluxVaeEncoderGraph {
        graph,
        input_image,
        result_moments: moments,
        image_width,
        image_height,
        latent_width: image_width / 8,
        latent_height: image_height / 8,
    })
}

/// Encoder-only downsample conv: BFL's asymmetric pad — one extra
/// column/row on the right/bottom only — then a 3x3 stride-2 conv with no
/// further padding (`nn.functional.pad(x, (0,1,0,1))` then
/// `Conv2d(k=3, stride=2, padding=0)` in the reference implementation).
/// Unlike `apply_conv2d` (always stride 1, "same" output size), this halves
/// both spatial dimensions.
fn apply_conv2d_downsample(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight_name: &str,
    bias_name: &str,
) -> Result<TensorId> {
    // pad_4d pads only the "far" side of each dim: width +1, height +1.
    let padded = ctx
        .pad_4d(input, 1, 1, 0, 0, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let out = conv_2d_im2col_mm(
        ctx,
        require_tensor_id(tensor_ids, weight_name)?,
        padded,
        2,
        2,
        0,
        0,
        1,
        1,
    )?;
    let bias = broadcast_channel_vector(ctx, require_tensor_id(tensor_ids, bias_name)?, out)?;
    ctx.binary_like_a(Op::Add, out, bias, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

/// CPU fallback encode (`FLUX_VAE_MODE=lazy`, or no compiled runtime
/// available): pure `SpatialTensor` math, same helpers decode's CPU fallback
/// (`execute_with_debug`) uses.
fn encode_spatial(
    weights: &LoadedFluxVaeWeights,
    pixel_chw_neg1_to_1: &[f32],
    image_width: usize,
    image_height: usize,
) -> Result<Vec<f32>> {
    let mut hidden = SpatialTensor::new(image_width, image_height, 3, pixel_chw_neg1_to_1.to_vec())?;
    hidden = apply_conv2d_spatial(
        weights,
        &hidden,
        "encoder.conv_in.weight",
        "encoder.conv_in.bias",
        1,
        1,
    )?;
    for level in 0..=3 {
        let stage_prefix = format!("encoder.down.{level}");
        for block in 0..=1 {
            hidden = resnet_block_spatial(weights, &hidden, &format!("{stage_prefix}.block.{block}"))?;
        }
        if weights.has_tensor(&format!("{stage_prefix}.downsample.conv.weight")) {
            hidden = apply_conv2d_downsample_spatial(
                weights,
                &hidden,
                &format!("{stage_prefix}.downsample.conv.weight"),
                &format!("{stage_prefix}.downsample.conv.bias"),
            )?;
        }
    }
    hidden = resnet_block_spatial(weights, &hidden, "encoder.mid.block_1")?;
    hidden = mid_attention_spatial(weights, &hidden, "encoder.mid.attn_1")?;
    hidden = resnet_block_spatial(weights, &hidden, "encoder.mid.block_2")?;
    hidden = apply_group_norm_spatial(
        weights,
        &hidden,
        "encoder.norm_out.weight",
        "encoder.norm_out.bias",
        32,
    )?;
    hidden = silu_spatial(&hidden)?;
    let mut moments = apply_conv2d_spatial(
        weights,
        &hidden,
        "encoder.conv_out.weight",
        "encoder.conv_out.bias",
        1,
        1,
    )?;
    if weights.has_tensor("quant_conv.weight") {
        moments = apply_conv2d_spatial(weights, &moments, "quant_conv.weight", "quant_conv.bias", 0, 0)?;
    }
    if moments.channels % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "flux vae encoder moments has odd channel count {}",
            moments.channels
        )));
    }
    let z_channels = moments.channels / 2;
    if z_channels != 16 {
        return Err(DiffusionError::model(format!(
            "flux vae encoder moments has {} latent channels, expected 16",
            z_channels
        )));
    }
    let plane = moments.width * moments.height;
    Ok(moments.data[..z_channels * plane].to_vec())
}

/// [`apply_conv2d_spatial`]'s stride-2 counterpart, used only by the
/// downsample stages above. Same (0,1,0,1) pad-then-stride-2 contract as
/// [`apply_conv2d_downsample`], expressed as the naive triple loop (this
/// path never goes through the accelerated `try_conv2d_planar_f32` fast
/// path — that primitive is stride-1-only, see its doc in
/// `libs/ai/metal/src/shim.rs`).
fn apply_conv2d_downsample_spatial(
    weights: &LoadedFluxVaeWeights,
    input: &SpatialTensor,
    weight_name: &str,
    bias_name: &str,
) -> Result<SpatialTensor> {
    let kernel = weights.tensor_kernel(weight_name)?;
    let bias = weights.tensor_f32_values(bias_name)?;
    if kernel.in_channels != input.channels {
        return Err(DiffusionError::model(format!(
            "flux vae downsample conv input channels mismatch: input={} kernel={}",
            input.channels, kernel.in_channels
        )));
    }
    if bias.len() != kernel.out_channels {
        return Err(DiffusionError::model(format!(
            "flux vae downsample conv bias len mismatch: bias={} out_channels={}",
            bias.len(),
            kernel.out_channels
        )));
    }
    let out_w = input.width / 2;
    let out_h = input.height / 2;
    let mut output = vec![0.0f32; out_w * out_h * kernel.out_channels];
    for out_c in 0..kernel.out_channels {
        for oy in 0..out_h {
            for ox in 0..out_w {
                let mut acc = bias[out_c];
                let base_x = ox * 2;
                let base_y = oy * 2;
                for in_c in 0..kernel.in_channels {
                    for ky in 0..kernel.kh {
                        let src_y = base_y + ky;
                        if src_y >= input.height {
                            continue; // the (0,1) bottom pad: zero contribution
                        }
                        for kx in 0..kernel.kw {
                            let src_x = base_x + kx;
                            if src_x >= input.width {
                                continue; // the (0,1) right pad: zero contribution
                            }
                            let input_value = input.data[input.offset(src_x, src_y, in_c)];
                            let weight_index = kx
                                + kernel.kw * (ky + kernel.kh * (in_c + kernel.in_channels * out_c));
                            acc += input_value * kernel.data[weight_index];
                        }
                    }
                }
                let offset = ox + out_w * (oy + out_h * out_c);
                output[offset] = acc;
            }
        }
    }
    SpatialTensor::new(out_w, out_h, kernel.out_channels, output)
}

#[cfg(test)]
mod encoder_tests {
    use super::*;

    /// Hand-rolled minimal weights: one conv weight/bias pair under `name`,
    /// nothing else — enough for the primitives that only look up specific
    /// tensor names (mirrors this file's other synthetic-weight tests).
    fn tiny_conv_weights(
        name: &str,
        weight: Vec<f32>,
        bias: Vec<f32>,
        kw: i64,
        kh: i64,
        in_c: i64,
        out_c: i64,
    ) -> LoadedFluxVaeWeights {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });
        let mut tensor_ids = BTreeMap::new();
        let weight_name = format!("{name}.weight");
        let w_id = ctx
            .new_named_tensor(
                weight_name.clone(),
                TensorType::F32,
                4,
                &[kw, kh, in_c, out_c],
                BufferUsage::Weights,
            )
            .unwrap();
        ctx.write_tensor_data(w_id, &f32s_to_le_bytes(&weight)).unwrap();
        tensor_ids.insert(weight_name, w_id);
        let bias_name = format!("{name}.bias");
        let b_id = ctx
            .new_named_tensor(bias_name.clone(), TensorType::F32, 1, &[out_c], BufferUsage::Weights)
            .unwrap();
        ctx.write_tensor_data(b_id, &f32s_to_le_bytes(&bias)).unwrap();
        tensor_ids.insert(bias_name, b_id);
        LoadedFluxVaeWeights {
            ctx,
            tensor_ids,
            path: PathBuf::from("<test>"),
            graph_extra_bytes: DEFAULT_GRAPH_EXTRA_BYTES,
        }
    }

    #[test]
    fn downsample_spatial_matches_hand_computed_pad_right_bottom_conv() {
        // 4x4 single-channel input, sequential values 0..16 (row-major).
        let input = SpatialTensor::new(4, 4, 1, (0..16).map(|v| v as f32).collect()).unwrap();
        // All-ones 3x3 kernel (sum filter) makes the (0,1,0,1)-pad + stride-2
        // window sums hand-computable.
        let weights = tiny_conv_weights("down", vec![1.0; 9], vec![0.0], 3, 3, 1, 1);
        let out = apply_conv2d_downsample_spatial(&weights, &input, "down.weight", "down.bias").unwrap();
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 2);
        assert_eq!(out.channels, 1);
        // (ox,oy)=(0,0): rows/cols 0..3 -> 0+1+2 + 4+5+6 + 8+9+10 = 45
        // (1,0): cols 2,3 only (col 4 is the zero pad) -> 2+3+6+7+10+11 = 39
        // (0,1): rows 2,3 only (row 4 is the zero pad) -> 8+9+10+12+13+14 = 66
        // (1,1): rows 2,3 x cols 2,3 -> 10+11+14+15 = 50
        assert_eq!(out.data, vec![45.0, 39.0, 66.0, 50.0]);
    }

    #[test]
    fn downsample_spatial_applies_bias_and_rejects_channel_mismatch() {
        let input = SpatialTensor::new(2, 2, 1, vec![1.0, 1.0, 1.0, 1.0]).unwrap();
        let weights = tiny_conv_weights("down", vec![0.0; 9], vec![7.0], 3, 3, 1, 1);
        let out = apply_conv2d_downsample_spatial(&weights, &input, "down.weight", "down.bias").unwrap();
        // Zero kernel: every output is exactly the bias.
        assert_eq!(out.data, vec![7.0]);

        let wrong_channels = SpatialTensor::new(2, 2, 2, vec![1.0; 8]).unwrap();
        assert!(apply_conv2d_downsample_spatial(&weights, &wrong_channels, "down.weight", "down.bias").is_err());
    }

    #[test]
    fn encode_control_image_rejects_size_not_divisible_by_8() {
        let err = encode_control_image(Path::new("<test>"), None, &vec![0.0f32; 3 * 10 * 10], 10, 10)
            .unwrap_err();
        assert!(matches!(err, DiffusionError::Workflow(_)), "got {err:?}");
    }

    #[test]
    fn encode_control_image_rejects_zero_size() {
        let err = encode_control_image(Path::new("<test>"), None, &[], 0, 8).unwrap_err();
        assert!(matches!(err, DiffusionError::Workflow(_)), "got {err:?}");
    }

    #[test]
    fn encode_control_image_rejects_wrong_pixel_count() {
        // 8x8x3 expected, only give it half.
        let err = encode_control_image(Path::new("<test>"), None, &vec![0.0f32; 8 * 8], 8, 8).unwrap_err();
        assert!(matches!(err, DiffusionError::Workflow(_)), "got {err:?}");
    }

    #[test]
    fn downsample_graph_conv_halves_both_spatial_dims() {
        // Compiled-graph counterpart of the spatial test above: build a bare
        // Context (no weight loading machinery), run apply_conv2d_downsample
        // and check the resulting tensor's shape — 8x8 -> 4x4, channels
        // unchanged 2 -> 3 (kernel out_channels).
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: true,
        });
        let mut tensor_ids = BTreeMap::new();
        let weight = ctx
            .new_tensor_4d(TensorType::F32, 3, 3, 2, 3, BufferUsage::Weights)
            .unwrap();
        tensor_ids.insert("down.weight".to_string(), weight);
        let bias = ctx.new_tensor_1d(TensorType::F32, 3, BufferUsage::Weights).unwrap();
        tensor_ids.insert("down.bias".to_string(), bias);
        let input = ctx
            .new_tensor_4d(TensorType::F32, 8, 8, 2, 1, BufferUsage::Activations)
            .unwrap();
        let out = apply_conv2d_downsample(&mut ctx, &tensor_ids, input, "down.weight", "down.bias").unwrap();
        let out_t = ctx.tensor(out).unwrap();
        assert_eq!(out_t.ne, [4, 4, 3, 1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{
        create_graph_session, new_runtime, prepare_graph, BufferStorageMode, GraphTensorWrite,
    };

    #[test]
    fn conv_2d_im2col_mm_matches_conv_2d_output_shape() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: true,
        });
        let kernel = ctx
            .new_tensor_4d(TensorType::F32, 3, 3, 4, 8, BufferUsage::Weights)
            .unwrap();
        let input = ctx
            .new_tensor_4d(TensorType::F32, 16, 16, 4, 1, BufferUsage::Activations)
            .unwrap();
        let naive = ctx
            .conv_2d(kernel, input, 1, 1, 1, 1, 1, 1, BufferUsage::Activations)
            .unwrap();
        let gemm = conv_2d_im2col_mm(&mut ctx, kernel, input, 1, 1, 1, 1, 1, 1).unwrap();
        let naive_t = ctx.tensor(naive).unwrap();
        let gemm_t = ctx.tensor(gemm).unwrap();
        assert_eq!(naive_t.ne, gemm_t.ne);
        assert_eq!(naive_t.ne, [16, 16, 8, 1]);
        assert_eq!(gemm_t.desc.ty, TensorType::F32);
        assert!(gemm_t.is_contiguous());
    }

    #[test]
    fn conv_2d_im2col_mm_matches_conv_2d_values_on_metal_when_available() {
        let runtime = match new_runtime() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let mut ctx = Context::new(InitParams {
            mem_size: 4 << 20,
            mem_buffer: None,
            no_alloc: false,
        });
        let kernel = ctx
            .new_tensor_4d(TensorType::F32, 3, 3, 2, 3, BufferUsage::Weights)
            .unwrap();
        let input = ctx
            .new_tensor_4d(TensorType::F32, 4, 4, 2, 1, BufferUsage::Activations)
            .unwrap();
        let kvals = patterned_f32s(3 * 3 * 2 * 3, -0.15, 0.04);
        let ivals = patterned_f32s(4 * 4 * 2, 0.2, -0.03);
        ctx.write_tensor_data(kernel, &f32s_to_le_bytes(&kvals))
            .unwrap();
        ctx.write_tensor_data(input, &f32s_to_le_bytes(&ivals))
            .unwrap();

        let naive = ctx
            .conv_2d(kernel, input, 1, 1, 1, 1, 1, 1, BufferUsage::Activations)
            .unwrap();
        let gemm = conv_2d_im2col_mm(&mut ctx, kernel, input, 1, 1, 1, 1, 1, 1).unwrap();

        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, naive).unwrap();
        graph.build_forward_expand(&ctx, gemm).unwrap();
        let prepared = prepare_graph(&runtime, &ctx, &graph).unwrap();
        let session = create_graph_session(
            &runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let execution = session.execute(&ctx, &[], &[naive, gemm]).unwrap();
        let naive_v = f32_bytes_to_vec(execution.outputs.get(&naive).unwrap()).unwrap();
        let gemm_v = f32_bytes_to_vec(execution.outputs.get(&gemm).unwrap()).unwrap();
        assert_eq!(naive_v.len(), gemm_v.len());
        let mut max_abs = 0.0f32;
        for (a, b) in naive_v.iter().zip(gemm_v.iter()) {
            max_abs = max_abs.max((a - b).abs());
        }
        assert!(
            max_abs < 1e-4,
            "im2col+mul_mm diverges from conv_2d: max_abs={max_abs}"
        );
    }

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

    fn make_test_vae_weights(entries: &[(&str, Vec<i64>, Vec<f32>)]) -> LoadedFluxVaeWeights {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: false,
        });
        let mut tensor_ids = BTreeMap::new();
        for (name, extents, values) in entries {
            let id = ctx
                .new_named_tensor(
                    name.to_string(),
                    TensorType::F32,
                    extents.len(),
                    extents,
                    BufferUsage::Weights,
                )
                .unwrap();
            ctx.write_tensor_data(id, &f32s_to_le_bytes(values)).unwrap();
            tensor_ids.insert(name.to_string(), id);
        }
        LoadedFluxVaeWeights {
            ctx,
            tensor_ids,
            path: PathBuf::from("<test>"),
            graph_extra_bytes: 0,
        }
    }

    /// The strided/asymmetric-pad downsample conv against a hand reference
    /// (Python `F.pad(x, (0,1,0,1))` then `conv2d(k=3, stride=2, pad=0)`,
    /// verified independently before porting): a 2-in/2-out 4x4 case with
    /// non-trivial weights/bias, cross-checked value-for-value.
    #[test]
    fn downsample_conv2d_spatial_matches_python_reference() {
        // extents are [kw, kh, in_channels, out_channels] (ggml ne order).
        let weight = vec![
            -0.4, -0.39, -0.38, -0.37, -0.36000000000000004, -0.35000000000000003, -0.34, -0.33,
            -0.32, -0.31000000000000005, -0.30000000000000004, -0.29000000000000004, -0.28,
            -0.27, -0.26, -0.25, -0.24000000000000002, -0.23, -0.22000000000000003,
            -0.21000000000000002, -0.2, -0.19000000000000003, -0.18000000000000002, -0.17,
            -0.16000000000000003, -0.15000000000000002, -0.14, -0.13, -0.12,
            -0.11000000000000004, -0.10000000000000003, -0.09000000000000002,
            -0.08000000000000002, -0.07, -0.06, -0.04999999999999999,
        ]
        .into_iter()
        .map(|v: f64| v as f32)
        .collect::<Vec<_>>();
        let bias = vec![0.5f32, -0.25f32];
        let weights = make_test_vae_weights(&[
            ("t.downsample.conv.weight", vec![3, 3, 2, 2], weight),
            ("t.downsample.conv.bias", vec![2], bias),
        ]);

        let x = vec![
            -1.0, -0.9, -0.8, -0.7, -0.6, -0.5, -0.3999999999999999, -0.29999999999999993,
            -0.19999999999999996, -0.09999999999999998, 0.0, 0.10000000000000009,
            0.20000000000000018, 0.30000000000000004, 0.40000000000000013, 0.5,
            0.6000000000000001, 0.7000000000000002, 0.8, 0.9000000000000001, 1.0, 1.1,
            1.2000000000000002, 1.3000000000000003, 1.4000000000000004, 1.5, 1.6,
            1.7000000000000002, 1.8000000000000003, 1.9000000000000004, 2.0, 2.1,
        ]
        .into_iter()
        .map(|v: f64| v as f32)
        .collect::<Vec<_>>();
        let input = SpatialTensor::new(4, 4, 2, x).unwrap();

        let output = downsample_conv2d_spatial(
            &weights,
            &input,
            "t.downsample.conv.weight",
            "t.downsample.conv.bias",
        )
        .unwrap();

        assert_eq!((output.width, output.height, output.channels), (2, 2, 2));
        let expected = [
            -0.39700000000000024,
            -0.6970000000000005,
            -2.5880000000000005,
            -2.0000000000000004,
            -0.1750000000000001,
            -0.4750000000000001,
            -1.3940000000000001,
            -1.2380000000000004,
        ];
        for (actual, expected) in output.data.iter().zip(expected.iter()) {
            assert!(
                (actual - *expected as f32).abs() < 1.0e-4,
                "downsample mismatch: actual={} expected={}",
                actual,
                expected
            );
        }
    }

    #[test]
    fn encode_image_to_latent_mean_rejects_non_multiple_of_8_size() {
        let weights = make_test_vae_weights(&[]);
        let image = vec![0.0f32; 7 * 8 * 3];
        let err = encode_image_to_latent_mean(&weights, &image, 7, 8).unwrap_err();
        assert!(matches!(err, DiffusionError::Workflow(_)));
    }

    #[test]
    fn encode_image_to_latent_mean_rejects_wrong_input_length() {
        let weights = make_test_vae_weights(&[]);
        let image = vec![0.0f32; 8 * 8 * 3 - 1];
        let err = encode_image_to_latent_mean(&weights, &image, 8, 8).unwrap_err();
        assert!(matches!(err, DiffusionError::Workflow(_)));
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
