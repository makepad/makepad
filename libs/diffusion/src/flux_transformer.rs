use crate::backend::{
    create_graph_session, gpu_act_f16_enabled, gpu_attention_packed, gpu_concat_cols,
    gpu_concat_rows,
    gpu_device_available, gpu_download, gpu_gated_residual_mod, gpu_gelu, gpu_gelu_bias_f16,
    gpu_graph_capture, gpu_graph_launch,
    gpu_layer_norm_mod, gpu_layer_norm_mod_f16,
    gpu_linear_nt_cached, gpu_linear_nt_cached_f16, gpu_rms_norm_mul, gpu_rope_interleaved,
    gpu_slice_cols, gpu_slice_rows, gpu_to_f16,
    gpu_upload, gpu_upload_into, new_runtime, prepare_graph, runtime_available, try_add_f32,
    try_attention_softmax_weighted_sum_f32, try_flash_attn_f32_packed, try_gelu_f32,
    try_layer_norm_mul_add_f32, try_matmul_nn_f32, try_matmul_nt_f32, try_mul_f32,
    try_rms_norm_mul_f32, BufferStorageMode, GpuLinearPart, GpuStepGraph, GpuTensor,
    GraphSession, GraphTensorWrite, Runtime,
};
use crate::flux::{
    canonicalize_flux_diffusion_tensor_name, FluxLatentShape, FluxTransformerConfig,
    FluxTransformerInspection,
};
use crate::flux_text::FluxConditioning;
use crate::{emit_byte_progress, emit_progress, DiffusionError, ProgressHook, Result};
use makepad_ggml::backend::{try_matmul_nt_ggml_bytes, try_matmul_nt_ggml_bytes_cached};
use makepad_ggml::{
    bf16_to_f32, f16_to_f32, f8_e4m3_to_f32, ggml_pad, ggml_type_size_for_type, BufferUsage,
    Context, Graph, InitParams, Op, Tensor, TensorDesc, TensorId, TensorLayout, TensorType,
    UnaryOp, GGML_MEM_ALIGN, GGML_ROPE_TYPE_NORMAL,
};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const FLUX_TIMESTEP_EMBED_DIM: i32 = 256;
const FLUX_LAYER_NORM_EPSILON: f32 = 1.0e-6;
const DEFAULT_GRAPH_EXTRA_BYTES: usize = 4usize * 1024 * 1024 * 1024;
const MAX_GRAPH_GROWTH_ATTEMPTS: usize = 3;

#[derive(Clone, Debug)]
pub struct LoadedFluxTransformerWeights {
    pub ctx: Context,
    pub tensor_ids: BTreeMap<String, TensorId>,
    pub config: FluxTransformerConfig,
    pub path: PathBuf,
    /// True when the resident matrices are raw F8_E4M3 (the combined-FP8
    /// checkpoints). The device path then keeps the f32 activation spine
    /// (the F8 dequant feeds the bf16 f32-accumulate gemms, which refuse
    /// f16 activations) and never captures step graphs (pinned pool buffers
    /// have no headroom next to an all-resident 24GB-tier stack).
    pub f8_weights: bool,
    /// True when any weight is a block-quant GGUF type (Q4_K, …).
    /// Compiled Metal graphs still assume dense F16/BF16/F8, so these
    /// stay on the lazy `try_matmul_nt_ggml_bytes` path.
    pub quantized: bool,
    graph_extra_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct FluxTransformerGraph {
    pub graph: Graph,
    pub input_packed_latents: TensorId,
    pub input_encoder_hidden_states: TensorId,
    pub input_pooled_projections: TensorId,
    pub input_timestep: TensorId,
    pub input_guidance: Option<TensorId>,
    pub result_prediction: TensorId,
    pub image_token_count: usize,
    input_hidden: TensorId,
    input_hidden_mm: TensorId,
    debug_tensors: Vec<FluxTransformerDebugTensor>,
}

pub struct CompiledFluxTransformer {
    inner: FluxTransformerExecutor,
}

pub type CompiledFluxTransformerMetal = CompiledFluxTransformer;
pub type LazyFluxTransformerMetal = LazyFluxTransformer;

enum FluxTransformerExecutor {
    Compiled(CompiledFluxTransformerGraph),
    Lazy(LazyFluxTransformer),
}

struct CompiledFluxTransformerGraph {
    graph: FluxTransformerGraph,
    session: GraphSession,
}

#[derive(Clone, Debug)]
pub struct LazyFluxTransformer {
    text_token_count: usize,
    image_token_count: usize,
    hidden_size: usize,
    head_count: usize,
    head_dim: usize,
    rope_tables: FluxRopeTables,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FluxTransformerExecutionMode {
    Lazy,
    Compiled,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FluxTransformerCompileTiming {
    pub graph_build_ms: f64,
    pub graph_prepare_ms: f64,
    pub session_create_ms: f64,
}

#[derive(Clone, Debug)]
pub struct FluxTransformerRun {
    pub prediction: Vec<f32>,
    pub image_token_count: usize,
    pub channel_count: usize,
}

#[derive(Clone, Debug)]
struct FluxTransformerShapeInfo {
    text_token_count: usize,
    image_token_count: usize,
    hidden_size: usize,
    head_count: usize,
    head_dim: usize,
}

#[derive(Clone, Debug)]
struct FluxTransformerDebugTensor {
    name: String,
    tensor_id: TensorId,
}

#[derive(Clone, Debug)]
pub struct FluxTransformerStageOutput {
    pub name: String,
    pub values: Vec<f32>,
    pub extents: [usize; 4],
}

#[derive(Clone, Debug)]
pub struct FluxTransformerDebugRun {
    pub run: FluxTransformerRun,
    pub stages: Vec<FluxTransformerStageOutput>,
}

#[derive(Clone, Debug)]
struct FluxRopeTables {
    token_count: usize,
    half_dim: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

#[derive(Clone, Debug)]
struct RowsTensor {
    rows: usize,
    cols: usize,
    data: Vec<f32>,
}

#[derive(Clone, Debug)]
struct HeadTensor {
    token_count: usize,
    head_count: usize,
    head_dim: usize,
    data: Vec<f32>,
}

#[derive(Clone, Debug)]
struct ResidentMatrix<'a> {
    bytes: &'a [u8],
    ggml_type: u32,
    cols: usize,
    rows: usize,
    cache_key: String,
}

impl LoadedFluxTransformerWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_extra(path, DEFAULT_GRAPH_EXTRA_BYTES)
    }

    /// [`Self::load`] with cumulative byte progress ("load unet 8.2/23.8GB")
    /// every ~256MB of streamed weight bytes.
    pub fn load_with_progress(
        path: impl AsRef<Path>,
        progress: Option<ProgressHook>,
    ) -> Result<Self> {
        Self::load_with_extra_progress(path, DEFAULT_GRAPH_EXTRA_BYTES, progress)
    }

    pub fn load_with_extra(path: impl AsRef<Path>, extra_bytes: usize) -> Result<Self> {
        Self::load_with_extra_progress(path, extra_bytes, None)
    }

    /// [`Self::load_with_progress`] over the diffusion component of a
    /// combined checkpoint (see [`Self::load_scoped_with_extra_progress`]).
    pub fn load_component_with_progress(
        path: impl AsRef<Path>,
        prefix: Option<&str>,
        progress: Option<ProgressHook>,
    ) -> Result<Self> {
        Self::load_scoped_with_extra_progress(path, prefix, DEFAULT_GRAPH_EXTRA_BYTES, progress)
    }

    pub fn load_with_extra_progress(
        path: impl AsRef<Path>,
        extra_bytes: usize,
        progress: Option<ProgressHook>,
    ) -> Result<Self> {
        Self::load_scoped_with_extra_progress(path, None, extra_bytes, progress)
    }

    /// [`Self::load_with_extra_progress`] over a component of a combined
    /// checkpoint: `prefix` (e.g. `model.diffusion_model.`) scopes the header
    /// so only the diffusion component's tensors are allocated and its byte
    /// ranges read; the weights keep the combined file's path as their
    /// device-cache identity.
    pub fn load_scoped_with_extra_progress(
        path: impl AsRef<Path>,
        prefix: Option<&str>,
        extra_bytes: usize,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        if crate::flux_gguf::is_gguf_path(&path) {
            if prefix.is_some() {
                return Err(DiffusionError::model(
                    "flux GGUF loader does not take a combined-checkpoint prefix",
                ));
            }
            return crate::flux_gguf::load_weights(path, extra_bytes, progress);
        }
        let header = crate::flux::flux_component_header(path.as_ref(), prefix)?;
        let inspect = FluxTransformerInspection::from_header(&header)?;
        let total_bytes = flux_weight_total_bytes(&header, extra_bytes)?;
        let mut ctx = Context::new(InitParams {
            mem_size: total_bytes,
            mem_buffer: None,
            no_alloc: false,
        });
        let tensor_ids = allocate_flux_weight_tensors(&mut ctx, &header)?;
        load_flux_weight_bytes(&mut ctx, &header, &tensor_ids, &mut progress)?;
        let f8_weights = header
            .tensors
            .values()
            .any(|entry| entry.dtype == MlxDType::F8E4M3 && entry.shape.len() == 2);

        Ok(Self {
            ctx,
            tensor_ids,
            config: inspect.config,
            path: header.path,
            f8_weights,
            quantized: false,
            graph_extra_bytes: extra_bytes,
        })
    }

    pub(crate) fn from_loaded(
        ctx: Context,
        tensor_ids: BTreeMap<String, TensorId>,
        config: FluxTransformerConfig,
        path: PathBuf,
        f8_weights: bool,
        quantized: bool,
        extra_bytes: usize,
    ) -> Self {
        Self {
            ctx,
            tensor_ids,
            config,
            path,
            f8_weights,
            quantized,
            graph_extra_bytes: extra_bytes,
        }
    }

    pub fn tensor_id(&self, name: &str) -> Result<TensorId> {
        self.tensor_ids.get(name).copied().ok_or_else(|| {
            DiffusionError::model(format!("missing flux transformer tensor '{}'", name))
        })
    }

    fn tensor_f32_values(&self, name: &str) -> Result<Vec<f32>> {
        let tensor_id = self.tensor_id(name)?;
        tensor_to_f32_vec(&self.ctx, tensor_id)
    }

    fn tensor_f32_values_concat(&self, base_name: &str) -> Result<Vec<f32>> {
        let mut values = Vec::new();
        for part_name in tensor_part_names(&self.tensor_ids, base_name)? {
            values.extend(self.tensor_f32_values(&part_name)?);
        }
        Ok(values)
    }

    fn tensor_matrix_parts(&self, base_name: &str) -> Result<Vec<ResidentMatrix<'_>>> {
        let mut parts = Vec::new();
        let namespace = flux_cache_namespace(self);
        for part_name in tensor_part_names(&self.tensor_ids, base_name)? {
            parts.push(resident_matrix(
                &self.ctx,
                self.tensor_id(&part_name)?,
                format!("{namespace}::{part_name}"),
            )?);
        }
        Ok(parts)
    }

    fn graph_reserve_bytes(&self) -> usize {
        self.graph_extra_bytes
    }
}

impl CompiledFluxTransformer {
    pub fn compile(
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        Ok(Self::compile_profiled(weights, conditioning, latent_shape)?.0)
    }

    pub fn compile_profiled(
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
    ) -> Result<(Self, FluxTransformerCompileTiming)> {
        Self::compile_for_mode(
            flux_execution_mode(weights),
            None,
            weights,
            conditioning,
            latent_shape,
            &mut None,
        )
    }

    /// Profiled compile with sub-stage progress ("compile transformer
    /// graph/prepare/session") — the compiled mode's three multi-second
    /// stages each get a label and a cancel boundary; lazy mode is instant
    /// and emits nothing.
    pub fn compile_hooked(
        runtime: Option<Runtime>,
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
        mut progress: Option<ProgressHook>,
    ) -> Result<(Self, FluxTransformerCompileTiming)> {
        Self::compile_for_mode(
            flux_execution_mode(weights),
            runtime,
            weights,
            conditioning,
            latent_shape,
            &mut progress,
        )
    }

    pub fn compile_with_runtime(
        runtime: Runtime,
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        Ok(Self::compile_with_runtime_profiled(runtime, weights, conditioning, latent_shape)?.0)
    }

    pub fn compile_with_runtime_profiled(
        runtime: Runtime,
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
    ) -> Result<(Self, FluxTransformerCompileTiming)> {
        Self::compile_for_mode(
            flux_execution_mode(weights),
            Some(runtime),
            weights,
            conditioning,
            latent_shape,
            &mut None,
        )
    }

    fn compile_for_mode(
        mode: FluxTransformerExecutionMode,
        runtime: Option<Runtime>,
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
        progress: &mut Option<ProgressHook>,
    ) -> Result<(Self, FluxTransformerCompileTiming)> {
        match mode {
            FluxTransformerExecutionMode::Lazy => Ok((
                Self {
                    inner: FluxTransformerExecutor::Lazy(LazyFluxTransformer::compile(
                        weights,
                        conditioning,
                        latent_shape,
                    )?),
                },
                FluxTransformerCompileTiming::default(),
            )),
            FluxTransformerExecutionMode::Compiled => {
                let runtime = match runtime {
                    Some(runtime) => runtime,
                    None => new_runtime()?,
                };
                let (compiled, timing) =
                    Self::compile_graph(runtime, weights, conditioning, latent_shape, progress)?;
                Ok((
                    Self {
                        inner: FluxTransformerExecutor::Compiled(compiled),
                    },
                    timing,
                ))
            }
        }
    }

    fn compile_graph(
        runtime: Runtime,
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
        progress: &mut Option<ProgressHook>,
    ) -> Result<(CompiledFluxTransformerGraph, FluxTransformerCompileTiming)> {
        for attempt in 0..=MAX_GRAPH_GROWTH_ATTEMPTS {
            let build_start = Instant::now();
            emit_progress(progress, "compile transformer graph", 0.0)?;
            let graph = match build_flux_transformer_graph(weights, conditioning, latent_shape) {
                Ok(graph) => graph,
                Err(err) if is_context_oom(&err) && attempt < MAX_GRAPH_GROWTH_ATTEMPTS => {
                    let next_extra = next_graph_reserve_bytes(weights)?;
                    *weights = LoadedFluxTransformerWeights::load_with_extra(
                        weights.path.clone(),
                        next_extra,
                    )?;
                    continue;
                }
                Err(err) => return Err(err),
            };
            let graph_build_ms = build_start.elapsed().as_secs_f64() * 1000.0;
            let prepare_start = Instant::now();
            emit_progress(progress, "compile transformer prepare", 0.35)?;
            let prepared = prepare_graph(&runtime, &weights.ctx, &graph.graph)?;
            let fused = prepared
                .nodes
                .iter()
                .filter(|node| !node.fuse_src_ids.is_empty())
                .count();
            eprintln!(
                "flux dit compiled nodes={} fused={} main_buffer={}",
                prepared.nodes.len(),
                fused,
                prepared.main_buffer_size
            );
            let graph_prepare_ms = prepare_start.elapsed().as_secs_f64() * 1000.0;
            let session_start = Instant::now();
            emit_progress(progress, "compile transformer session", 0.7)?;
            let session = create_graph_session(
                &runtime,
                &weights.ctx,
                &prepared,
                BufferStorageMode::Shared,
                BufferStorageMode::Shared,
            )?;
            let session_create_ms = session_start.elapsed().as_secs_f64() * 1000.0;
            return Ok((
                CompiledFluxTransformerGraph { graph, session },
                FluxTransformerCompileTiming {
                    graph_build_ms,
                    graph_prepare_ms,
                    session_create_ms,
                },
            ));
        }

        Err(DiffusionError::model(
            "flux transformer graph compilation exhausted context growth attempts",
        ))
    }

    pub fn backend_name(&self) -> &'static str {
        match &self.inner {
            FluxTransformerExecutor::Compiled(_) => FluxTransformerExecutionMode::Compiled.as_str(),
            FluxTransformerExecutor::Lazy(_) => FluxTransformerExecutionMode::Lazy.as_str(),
        }
    }

    pub fn execute(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
    ) -> Result<FluxTransformerRun> {
        self.execute_hooked(weights, conditioning, packed_latents, timestep, guidance, None)
    }

    /// [`Self::execute`] with per-block progress ("block 12/57") from the
    /// lazy/device step — on the cold first denoise step each block's weights
    /// stream to the device, so the hook is what makes that stream visible.
    /// The compiled (single-graph) executor has no interior boundary and
    /// emits nothing.
    pub fn execute_hooked(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
        mut progress: Option<ProgressHook>,
    ) -> Result<FluxTransformerRun> {
        match &self.inner {
            FluxTransformerExecutor::Compiled(compiled) => {
                compiled.execute(weights, conditioning, packed_latents, timestep, guidance)
            }
            FluxTransformerExecutor::Lazy(lazy) => Ok(lazy
                .execute_internal(
                    weights,
                    conditioning,
                    packed_latents,
                    timestep,
                    guidance,
                    false,
                    &mut progress,
                )?
                .run),
        }
    }

    pub fn execute_with_debug(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
    ) -> Result<FluxTransformerDebugRun> {
        match &self.inner {
            FluxTransformerExecutor::Compiled(compiled) => {
                compiled.execute_with_debug(weights, conditioning, packed_latents, timestep, guidance)
            }
            FluxTransformerExecutor::Lazy(lazy) => {
                lazy.execute_with_debug(weights, conditioning, packed_latents, timestep, guidance)
            }
        }
    }
}

impl CompiledFluxTransformerGraph {
    fn execute(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
    ) -> Result<FluxTransformerRun> {
        let latents_tensor = require_tensor(&weights.ctx, self.graph.input_packed_latents)?;
        let expected_latents = usize::try_from(latents_tensor.ne[0] * latents_tensor.ne[1])
            .map_err(|_| DiffusionError::model("flux packed latents shape exceeds usize"))?;
        if packed_latents.len() != expected_latents {
            return Err(DiffusionError::workflow(format!(
                "flux packed latents expected {} values, got {}",
                expected_latents,
                packed_latents.len()
            )));
        }

        let encoder_tensor = require_tensor(&weights.ctx, self.graph.input_encoder_hidden_states)?;
        let expected_encoder = usize::try_from(encoder_tensor.ne[0] * encoder_tensor.ne[1])
            .map_err(|_| DiffusionError::model("flux encoder hidden shape exceeds usize"))?;
        if conditioning.t5_hidden_states.len() != expected_encoder {
            return Err(DiffusionError::workflow(format!(
                "flux encoder hidden expected {} values, got {}",
                expected_encoder,
                conditioning.t5_hidden_states.len()
            )));
        }

        let pooled_tensor = require_tensor(&weights.ctx, self.graph.input_pooled_projections)?;
        let expected_pooled = usize::try_from(pooled_tensor.ne[0] * pooled_tensor.ne[1])
            .map_err(|_| DiffusionError::model("flux pooled shape exceeds usize"))?;
        if conditioning.clip_pooled.len() != expected_pooled {
            return Err(DiffusionError::workflow(format!(
                "flux pooled projection expected {} values, got {}",
                expected_pooled,
                conditioning.clip_pooled.len()
            )));
        }

        let packed_latents_bytes = f32s_to_le_bytes(packed_latents);
        let encoder_hidden_bytes = f32s_to_le_bytes(&conditioning.t5_hidden_states);
        let pooled_bytes = f32s_to_le_bytes(&conditioning.clip_pooled);
        let timestep_bytes = f32s_to_le_bytes(&[timestep]);
        let guidance_bytes = f32s_to_le_bytes(&[guidance]);

        let mut writes = vec![
            GraphTensorWrite {
                tensor_id: self.graph.input_packed_latents,
                bytes: &packed_latents_bytes,
            },
            GraphTensorWrite {
                tensor_id: self.graph.input_encoder_hidden_states,
                bytes: &encoder_hidden_bytes,
            },
            GraphTensorWrite {
                tensor_id: self.graph.input_pooled_projections,
                bytes: &pooled_bytes,
            },
            GraphTensorWrite {
                tensor_id: self.graph.input_timestep,
                bytes: &timestep_bytes,
            },
        ];
        if let Some(input_guidance) = self.graph.input_guidance {
            writes.push(GraphTensorWrite {
                tensor_id: input_guidance,
                bytes: &guidance_bytes,
            });
        }

        let execution = self
            .session
            .execute(&weights.ctx, &writes, &[self.graph.result_prediction])
            .map_err(DiffusionError::model)?;
        let prediction_bytes = execution
            .outputs
            .get(&self.graph.result_prediction)
            .ok_or_else(|| {
                DiffusionError::model("flux transformer execution did not return output")
            })?;
        let output_tensor = require_tensor(&weights.ctx, self.graph.result_prediction)?;
        let channel_count = usize::try_from(output_tensor.ne[0])
            .map_err(|_| DiffusionError::model("flux transformer output channels exceed usize"))?;
        let prediction = f32_bytes_to_vec(prediction_bytes)?;
        if std::env::var_os("FLUX_DIT_DOUBLE_EXEC").is_some() {
            let execution2 = self
                .session
                .execute(&weights.ctx, &writes, &[self.graph.result_prediction])
                .map_err(DiffusionError::model)?;
            let pred2 = f32_bytes_to_vec(
                execution2
                    .outputs
                    .get(&self.graph.result_prediction)
                    .ok_or_else(|| {
                        DiffusionError::model("flux transformer double-exec missing output")
                    })?,
            )?;
            eprintln!("flux dit pred2-same-in {}", f32_stats(&pred2));
        }

        Ok(FluxTransformerRun {
            prediction,
            image_token_count: self.graph.image_token_count,
            channel_count,
        })
    }

    fn execute_with_debug(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
    ) -> Result<FluxTransformerDebugRun> {
        let latents_tensor = require_tensor(&weights.ctx, self.graph.input_packed_latents)?;
        let expected_latents = usize::try_from(latents_tensor.ne[0] * latents_tensor.ne[1])
            .map_err(|_| DiffusionError::model("flux packed latents shape exceeds usize"))?;
        if packed_latents.len() != expected_latents {
            return Err(DiffusionError::workflow(format!(
                "flux packed latents expected {} values, got {}",
                expected_latents,
                packed_latents.len()
            )));
        }

        let encoder_tensor = require_tensor(&weights.ctx, self.graph.input_encoder_hidden_states)?;
        let expected_encoder = usize::try_from(encoder_tensor.ne[0] * encoder_tensor.ne[1])
            .map_err(|_| DiffusionError::model("flux encoder hidden shape exceeds usize"))?;
        if conditioning.t5_hidden_states.len() != expected_encoder {
            return Err(DiffusionError::workflow(format!(
                "flux encoder hidden expected {} values, got {}",
                expected_encoder,
                conditioning.t5_hidden_states.len()
            )));
        }

        let pooled_tensor = require_tensor(&weights.ctx, self.graph.input_pooled_projections)?;
        let expected_pooled = usize::try_from(pooled_tensor.ne[0] * pooled_tensor.ne[1])
            .map_err(|_| DiffusionError::model("flux pooled shape exceeds usize"))?;
        if conditioning.clip_pooled.len() != expected_pooled {
            return Err(DiffusionError::workflow(format!(
                "flux pooled projection expected {} values, got {}",
                expected_pooled,
                conditioning.clip_pooled.len()
            )));
        }

        let packed_latents_bytes = f32s_to_le_bytes(packed_latents);
        let encoder_hidden_bytes = f32s_to_le_bytes(&conditioning.t5_hidden_states);
        let pooled_bytes = f32s_to_le_bytes(&conditioning.clip_pooled);
        let timestep_bytes = f32s_to_le_bytes(&[timestep]);
        let guidance_bytes = f32s_to_le_bytes(&[guidance]);

        let mut writes = vec![
            GraphTensorWrite {
                tensor_id: self.graph.input_packed_latents,
                bytes: &packed_latents_bytes,
            },
            GraphTensorWrite {
                tensor_id: self.graph.input_encoder_hidden_states,
                bytes: &encoder_hidden_bytes,
            },
            GraphTensorWrite {
                tensor_id: self.graph.input_pooled_projections,
                bytes: &pooled_bytes,
            },
            GraphTensorWrite {
                tensor_id: self.graph.input_timestep,
                bytes: &timestep_bytes,
            },
        ];
        if let Some(input_guidance) = self.graph.input_guidance {
            writes.push(GraphTensorWrite {
                tensor_id: input_guidance,
                bytes: &guidance_bytes,
            });
        }

        let mut outputs = Vec::with_capacity(1 + self.graph.debug_tensors.len());
        outputs.push(self.graph.result_prediction);
        for stage in &self.graph.debug_tensors {
            outputs.push(stage.tensor_id);
        }

        let execution = self
            .session
            .execute(&weights.ctx, &writes, &outputs)
            .map_err(DiffusionError::model)?;
        let prediction_bytes = execution
            .outputs
            .get(&self.graph.result_prediction)
            .ok_or_else(|| {
                DiffusionError::model("flux transformer execution did not return output")
            })?;
        let output_tensor = require_tensor(&weights.ctx, self.graph.result_prediction)?;
        let channel_count = usize::try_from(output_tensor.ne[0])
            .map_err(|_| DiffusionError::model("flux transformer output channels exceed usize"))?;
        let run = FluxTransformerRun {
            prediction: f32_bytes_to_vec(prediction_bytes)?,
            image_token_count: self.graph.image_token_count,
            channel_count,
        };

        let mut stages = Vec::with_capacity(self.graph.debug_tensors.len());
        for stage in &self.graph.debug_tensors {
            let tensor = require_tensor(&weights.ctx, stage.tensor_id)?;
            let bytes = execution.outputs.get(&stage.tensor_id).ok_or_else(|| {
                DiffusionError::model(format!(
                    "flux transformer debug tensor '{}' missing output",
                    stage.name
                ))
            })?;
            stages.push(FluxTransformerStageOutput {
                name: stage.name.clone(),
                values: f32_bytes_to_vec(bytes)?,
                extents: tensor_extents_usize(tensor)?,
            });
        }

        Ok(FluxTransformerDebugRun { run, stages })
    }
}

fn flux_execution_mode(weights: &LoadedFluxTransformerWeights) -> FluxTransformerExecutionMode {
    // Q4 GGUF tensors are valid ggml Metal MUL_MAT sources (Qwen already
    // runs them). Keeping quantized Flux on the compiled graph avoids the
    // lazy host↔GPU activation bounce that dominated the 256² step time.
    let _ = weights;
    FluxTransformerExecutionMode::from_env()
}

impl FluxTransformerExecutionMode {
    pub fn from_env() -> Self {
        match std::env::var("FLUX_TRANSFORMER_MODE") {
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

impl LazyFluxTransformer {
    fn compile(
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        let shape = validate_flux_transformer_inputs(weights, conditioning, latent_shape)?;
        Ok(Self {
            text_token_count: shape.text_token_count,
            image_token_count: shape.image_token_count,
            hidden_size: shape.hidden_size,
            head_count: shape.head_count,
            head_dim: shape.head_dim,
            rope_tables: flux_rope_table_values(shape.text_token_count, latent_shape, weights.config)?,
        })
    }

    fn execute_with_debug(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
    ) -> Result<FluxTransformerDebugRun> {
        self.execute_internal(
            weights,
            conditioning,
            packed_latents,
            timestep,
            guidance,
            true,
            &mut None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_internal(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
        capture_debug: bool,
        progress: &mut Option<ProgressHook>,
    ) -> Result<FluxTransformerDebugRun> {
        if packed_latents.len() != self.image_token_count * weights.config.in_channels as usize {
            return Err(DiffusionError::workflow(format!(
                "flux packed latents expected {} values, got {}",
                self.image_token_count * weights.config.in_channels as usize,
                packed_latents.len()
            )));
        }
        if conditioning.t5_hidden_states.len()
            != self.text_token_count * weights.config.context_in_dim as usize
        {
            return Err(DiffusionError::workflow(format!(
                "flux encoder hidden expected {} values, got {}",
                self.text_token_count * weights.config.context_in_dim as usize,
                conditioning.t5_hidden_states.len()
            )));
        }
        if conditioning.clip_pooled.len() != weights.config.vec_in_dim as usize {
            return Err(DiffusionError::workflow(format!(
                "flux pooled projection expected {} values, got {}",
                weights.config.vec_in_dim,
                conditioning.clip_pooled.len()
            )));
        }

        // Device-resident fast path: run the whole step on the GPU (activations
        // never leave the device between ops; only tiny modulation vectors and
        // the final prediction cross the bus). Any failure falls back to the
        // host math below, which stays byte-for-byte what it was.
        if !capture_debug
            && !flux_force_cpu_math()
            && flux_device_path_enabled()
            && gpu_device_available()
        {
            match self.execute_device(
                weights,
                conditioning,
                packed_latents,
                timestep,
                guidance,
                progress,
            ) {
                Ok(run) => {
                    return Ok(FluxTransformerDebugRun {
                        run,
                        stages: Vec::new(),
                    });
                }
                Err(DiffusionError::Cancelled) => return Err(DiffusionError::Cancelled),
                Err(err) => {
                    eprintln!(
                        "flux transformer device path failed ({err}); falling back to host math"
                    );
                }
            }
        }

        let mut stages = Vec::new();

        let input_packed_latents = RowsTensor::new(
            self.image_token_count,
            weights.config.in_channels as usize,
            packed_latents.to_vec(),
        )?;
        let input_encoder_hidden_states = RowsTensor::new(
            self.text_token_count,
            weights.config.context_in_dim as usize,
            conditioning.t5_hidden_states.clone(),
        )?;
        let input_pooled_projections =
            RowsTensor::new(1, weights.config.vec_in_dim as usize, conditioning.clip_pooled.clone())?;

        let mut hidden = linear_rows(
            weights,
            &input_packed_latents,
            "img_in.weight",
            "img_in.bias",
        )?;
        push_debug_rows(&mut stages, capture_debug, "input.hidden", &hidden);
        let mut encoder_hidden = linear_rows(
            weights,
            &input_encoder_hidden_states,
            "txt_in.weight",
            "txt_in.bias",
        )?;
        push_debug_rows(
            &mut stages,
            capture_debug,
            "input.encoder_hidden",
            &encoder_hidden,
        );

        let mut temb = apply_timestep_projection_rows(weights, timestep, "time_in")?;
        let pooled = apply_silu_mlp_rows(weights, &input_pooled_projections, "vector_in")?;
        temb = add_rows(&temb, &pooled)?;
        if weights.config.guidance_embed {
            let guidance = apply_timestep_projection_rows(weights, guidance, "guidance_in")?;
            temb = add_rows(&temb, &guidance)?;
        }
        push_debug_rows(&mut stages, capture_debug, "input.temb", &temb);

        let total_blocks =
            weights.config.depth as usize + weights.config.depth_single_blocks as usize;
        for layer in 0..weights.config.depth as usize {
            if progress.is_some() {
                emit_progress(
                    progress,
                    &format!("block {}/{total_blocks}", layer + 1),
                    layer as f64 / total_blocks as f64,
                )?;
            }
            let prefix = format!("double_blocks.{layer}");

            let (
                img_shift_msa,
                img_scale_msa,
                img_gate_msa,
                img_shift_mlp,
                img_scale_mlp,
                img_gate_mlp,
            ) = modulation_chunks_rows(
                weights,
                &temb,
                &format!("{prefix}.img_mod.lin.weight"),
                &format!("{prefix}.img_mod.lin.bias"),
                self.hidden_size,
                6,
            )?;
            let (
                txt_shift_msa,
                txt_scale_msa,
                txt_gate_msa,
                txt_shift_mlp,
                txt_scale_mlp,
                txt_gate_mlp,
            ) = modulation_chunks_rows(
                weights,
                &temb,
                &format!("{prefix}.txt_mod.lin.weight"),
                &format!("{prefix}.txt_mod.lin.bias"),
                self.hidden_size,
                6,
            )?;

            let norm_hidden =
                apply_modulated_layer_norm_rows(&hidden, &img_scale_msa, &img_shift_msa)?;
            let norm_encoder_hidden = apply_modulated_layer_norm_rows(
                &encoder_hidden,
                &txt_scale_msa,
                &txt_shift_msa,
            )?;
            if layer == 0 {
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.norm_hidden",
                    &norm_hidden,
                );
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.norm_encoder_hidden",
                    &norm_encoder_hidden,
                );
            }

            let (img_q, img_k, img_v) = qkv_projections_rows(
                weights,
                &norm_hidden,
                &format!("{prefix}.img_attn.qkv.weight"),
                &format!("{prefix}.img_attn.qkv.bias"),
                self.hidden_size,
                self.head_count,
                self.head_dim,
            )?;
            let (txt_q, txt_k, txt_v) = qkv_projections_rows(
                weights,
                &norm_encoder_hidden,
                &format!("{prefix}.txt_attn.qkv.weight"),
                &format!("{prefix}.txt_attn.qkv.bias"),
                self.hidden_size,
                self.head_count,
                self.head_dim,
            )?;

            let img_q = apply_head_rms_norm_rows(
                weights,
                &img_q,
                &format!("{prefix}.img_attn.norm.query_norm.scale"),
            )?;
            let img_k = apply_head_rms_norm_rows(
                weights,
                &img_k,
                &format!("{prefix}.img_attn.norm.key_norm.scale"),
            )?;
            let txt_q = apply_head_rms_norm_rows(
                weights,
                &txt_q,
                &format!("{prefix}.txt_attn.norm.query_norm.scale"),
            )?;
            let txt_k = apply_head_rms_norm_rows(
                weights,
                &txt_k,
                &format!("{prefix}.txt_attn.norm.key_norm.scale"),
            )?;
            if layer == 0 {
                push_debug_heads(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.img_q_norm",
                    &img_q,
                );
                push_debug_heads(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.img_k_norm",
                    &img_k,
                );
                push_debug_heads(&mut stages, capture_debug, "double_blocks.0.img_v", &img_v);
                push_debug_heads(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.txt_q_norm",
                    &txt_q,
                );
                push_debug_heads(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.txt_k_norm",
                    &txt_k,
                );
                push_debug_heads(&mut stages, capture_debug, "double_blocks.0.txt_v", &txt_v);
            }

            let q = HeadTensor::concat_tokens(&txt_q, &img_q)?;
            let k = HeadTensor::concat_tokens(&txt_k, &img_k)?;
            let v = HeadTensor::concat_tokens(&txt_v, &img_v)?;
            let q = apply_flux_rope_heads(&q, &self.rope_tables)?;
            let k = apply_flux_rope_heads(&k, &self.rope_tables)?;
            if layer == 0 {
                push_debug_heads(&mut stages, capture_debug, "double_blocks.0.q_rope", &q);
                push_debug_heads(&mut stages, capture_debug, "double_blocks.0.k_rope", &k);
            }

            let attn = build_attention_output_rows(&q, &k, &v)?;
            let encoder_attn = attn.slice_rows(0, self.text_token_count)?;
            let hidden_attn = attn.slice_rows(self.text_token_count, self.image_token_count)?;
            if layer == 0 {
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.hidden_attn_input",
                    &hidden_attn,
                );
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.encoder_attn_input",
                    &encoder_attn,
                );
            }

            let hidden_attn = linear_rows(
                weights,
                &hidden_attn,
                &format!("{prefix}.img_attn.proj.weight"),
                &format!("{prefix}.img_attn.proj.bias"),
            )?;
            let encoder_attn = linear_rows(
                weights,
                &encoder_attn,
                &format!("{prefix}.txt_attn.proj.weight"),
                &format!("{prefix}.txt_attn.proj.bias"),
            )?;
            hidden = gated_residual_rows(&hidden, &hidden_attn, &img_gate_msa)?;
            encoder_hidden = gated_residual_rows(&encoder_hidden, &encoder_attn, &txt_gate_msa)?;
            if layer == 0 {
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.hidden_post_attn",
                    &hidden,
                );
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.encoder_hidden_post_attn",
                    &encoder_hidden,
                );
            }

            let hidden_ff_input =
                apply_modulated_layer_norm_rows(&hidden, &img_scale_mlp, &img_shift_mlp)?;
            let encoder_ff_input = apply_modulated_layer_norm_rows(
                &encoder_hidden,
                &txt_scale_mlp,
                &txt_shift_mlp,
            )?;
            if layer == 0 {
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.hidden_ff_input",
                    &hidden_ff_input,
                );
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    "double_blocks.0.encoder_ff_input",
                    &encoder_ff_input,
                );
            }

            let hidden_ff = feed_forward_rows(
                weights,
                &hidden_ff_input,
                &format!("{prefix}.img_mlp.0.weight"),
                &format!("{prefix}.img_mlp.0.bias"),
                &format!("{prefix}.img_mlp.2.weight"),
                &format!("{prefix}.img_mlp.2.bias"),
            )?;
            let encoder_ff = feed_forward_rows(
                weights,
                &encoder_ff_input,
                &format!("{prefix}.txt_mlp.0.weight"),
                &format!("{prefix}.txt_mlp.0.bias"),
                &format!("{prefix}.txt_mlp.2.weight"),
                &format!("{prefix}.txt_mlp.2.bias"),
            )?;
            hidden = gated_residual_rows(&hidden, &hidden_ff, &img_gate_mlp)?;
            encoder_hidden = gated_residual_rows(&encoder_hidden, &encoder_ff, &txt_gate_mlp)?;
            push_debug_rows(
                &mut stages,
                capture_debug,
                &format!("double_blocks.{layer}.hidden"),
                &hidden,
            );
            push_debug_rows(
                &mut stages,
                capture_debug,
                &format!("double_blocks.{layer}.encoder_hidden"),
                &encoder_hidden,
            );
        }

        for layer in 0..weights.config.depth_single_blocks as usize {
            if progress.is_some() {
                let done = weights.config.depth as usize + layer;
                emit_progress(
                    progress,
                    &format!("block {}/{total_blocks}", done + 1),
                    done as f64 / total_blocks as f64,
                )?;
            }
            let prefix = format!("single_blocks.{layer}");
            let joint = RowsTensor::concat_rows(&encoder_hidden, &hidden)?;
            let (shift, scale, gate, _, _, _) = modulation_chunks_rows(
                weights,
                &temb,
                &format!("{prefix}.modulation.lin.weight"),
                &format!("{prefix}.modulation.lin.bias"),
                self.hidden_size,
                3,
            )?;
            let norm_joint = apply_modulated_layer_norm_rows(&joint, &scale, &shift)?;
            if layer == 0 {
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    "single_blocks.0.norm_joint",
                    &norm_joint,
                );
            }

            let linear1 = linear_rows(
                weights,
                &norm_joint,
                &format!("{prefix}.linear1.weight"),
                &format!("{prefix}.linear1.bias"),
            )?;
            let q = HeadTensor::from_rows(
                &linear1.slice_cols(0, self.hidden_size)?,
                self.head_count,
                self.head_dim,
            )?;
            let k = HeadTensor::from_rows(
                &linear1.slice_cols(self.hidden_size, self.hidden_size)?,
                self.head_count,
                self.head_dim,
            )?;
            let v = HeadTensor::from_rows(
                &linear1.slice_cols(self.hidden_size * 2, self.hidden_size)?,
                self.head_count,
                self.head_dim,
            )?;
            let mlp = linear1.slice_cols(self.hidden_size * 3, self.hidden_size * 4)?;

            let q = apply_head_rms_norm_rows(
                weights,
                &q,
                &format!("{prefix}.norm.query_norm.scale"),
            )?;
            let k = apply_head_rms_norm_rows(
                weights,
                &k,
                &format!("{prefix}.norm.key_norm.scale"),
            )?;
            if layer == 0 {
                push_debug_heads(&mut stages, capture_debug, "single_blocks.0.q_norm", &q);
                push_debug_heads(&mut stages, capture_debug, "single_blocks.0.k_norm", &k);
                push_debug_heads(&mut stages, capture_debug, "single_blocks.0.v", &v);
            }

            let q = apply_flux_rope_heads(&q, &self.rope_tables)?;
            let k = apply_flux_rope_heads(&k, &self.rope_tables)?;
            let attn = build_attention_output_rows(&q, &k, &v)?;
            if layer == 0 {
                push_debug_rows(&mut stages, capture_debug, "single_blocks.0.attn", &attn);
            }
            let mlp = gelu_rows(&mlp)?;
            if layer == 0 {
                push_debug_rows(&mut stages, capture_debug, "single_blocks.0.mlp", &mlp);
            }
            let fused = RowsTensor::concat_cols(&attn, &mlp)?;
            let proj = linear_rows(
                weights,
                &fused,
                &format!("{prefix}.linear2.weight"),
                &format!("{prefix}.linear2.bias"),
            )?;
            if layer == 0 {
                push_debug_rows(&mut stages, capture_debug, "single_blocks.0.proj", &proj);
            }
            let joint = gated_residual_rows(&joint, &proj, &gate)?;
            if trace_single_block(layer, weights.config.depth_single_blocks as usize) {
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    &format!("single_blocks.{layer}.joint"),
                    &joint,
                );
            }
            encoder_hidden = joint.slice_rows(0, self.text_token_count)?;
            hidden = joint.slice_rows(self.text_token_count, self.image_token_count)?;
            if trace_single_block(layer, weights.config.depth_single_blocks as usize) {
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    &format!("single_blocks.{layer}.hidden"),
                    &hidden,
                );
                push_debug_rows(
                    &mut stages,
                    capture_debug,
                    &format!("single_blocks.{layer}.encoder_hidden"),
                    &encoder_hidden,
                );
            }
        }

        let final_mod_input = silu_rows(&temb)?;
        let final_mod = linear_rows(
            weights,
            &final_mod_input,
            "final_layer.adaLN_modulation.1.weight",
            "final_layer.adaLN_modulation.1.bias",
        )?;
        let final_shift = final_mod.slice_cols(0, self.hidden_size)?;
        let final_scale = final_mod.slice_cols(self.hidden_size, self.hidden_size)?;
        hidden = apply_modulated_layer_norm_rows(&hidden, &final_scale.data, &final_shift.data)?;
        let result_prediction = linear_rows(
            weights,
            &hidden,
            "final_layer.linear.weight",
            "final_layer.linear.bias",
        )?;
        push_debug_rows(&mut stages, capture_debug, "final.hidden", &hidden);
        push_debug_rows(
            &mut stages,
            capture_debug,
            "final.output",
            &result_prediction,
        );

        Ok(FluxTransformerDebugRun {
            run: FluxTransformerRun {
                prediction: result_prediction.data.clone(),
                image_token_count: self.image_token_count,
                channel_count: result_prediction.cols,
            },
            stages,
        })
    }

    /// The device-resident mirror of `execute_internal`: identical math, but
    /// every activation tensor lives on the GPU for the whole step. Host
    /// traffic per step = packed latents + t5 states + rope tables up, the
    /// per-block modulation vectors (tiny, computed host-side from temb), and
    /// the final prediction down.
    fn execute_device(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
        progress: &mut Option<ProgressHook>,
    ) -> Result<FluxTransformerRun> {
        // temb is 1 x hidden_size; the host path is exact and cheap. Its
        // silu'd row is the only step-varying host input besides the latents.
        let input_pooled_projections = RowsTensor::new(
            1,
            weights.config.vec_in_dim as usize,
            conditioning.clip_pooled.clone(),
        )?;
        let mut temb = apply_timestep_projection_rows(weights, timestep, "time_in")?;
        let pooled = apply_silu_mlp_rows(weights, &input_pooled_projections, "vector_in")?;
        temb = add_rows(&temb, &pooled)?;
        if weights.config.guidance_embed {
            let guidance = apply_timestep_projection_rows(weights, guidance, "guidance_in")?;
            temb = add_rows(&temb, &guidance)?;
        }
        let temb_silu = silu_rows(&temb)?;

        if !weights.f8_weights
            && flux_graph_enabled(self.image_token_count + self.text_token_count)
        {
            return self.execute_device_graph(
                weights,
                conditioning,
                packed_latents,
                &temb_silu.data,
                progress,
            );
        }

        let input_latents = gpu_upload(
            packed_latents,
            self.image_token_count,
            weights.config.in_channels as usize,
        )
        .map_err(DiffusionError::model)?;
        let input_encoder = gpu_upload(
            &conditioning.t5_hidden_states,
            self.text_token_count,
            weights.config.context_in_dim as usize,
        )
        .map_err(DiffusionError::model)?;
        let temb_gpu =
            gpu_upload(&temb_silu.data, 1, self.hidden_size).map_err(DiffusionError::model)?;
        let rope_cos = gpu_upload(
            &self.rope_tables.cos,
            self.rope_tables.token_count,
            self.rope_tables.half_dim,
        )
        .map_err(DiffusionError::model)?;
        let rope_sin = gpu_upload(
            &self.rope_tables.sin,
            self.rope_tables.token_count,
            self.rope_tables.half_dim,
        )
        .map_err(DiffusionError::model)?;
        let prediction = self.execute_device_core(
            weights,
            &input_latents,
            &input_encoder,
            &temb_gpu,
            &rope_cos,
            &rope_sin,
            progress,
        )?;
        let channel_count = prediction.cols();
        let values = gpu_download(&prediction).map_err(DiffusionError::model)?;
        Ok(FluxTransformerRun {
            prediction: values,
            image_token_count: self.image_token_count,
            channel_count,
        })
    }

    /// The graph-replay path: step inputs live in persistent device tensors,
    /// the whole device-resident step is captured as a CUDA graph after two
    /// warm runs, and later steps are one upload + one graph launch + one
    /// download. Default policy: 512-class shapes only (at 1024 the pinned
    /// buffers would sit against the 32GB WDDM residency cliff during VAE).
    fn execute_device_graph(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        temb_silu: &[f32],
        progress: &mut Option<ProgressHook>,
    ) -> Result<FluxTransformerRun> {
        FLUX_DEVICE_GRAPH.with(|cell| {
            let mut slot = cell.borrow_mut();
            let namespace = flux_cache_namespace(weights);
            let act16 = flux_act16_enabled(weights);
            let matches = slot.as_ref().is_some_and(|state| {
                state.namespace == namespace
                    && state.image_token_count == self.image_token_count
                    && state.text_token_count == self.text_token_count
                    && state.act16 == act16
            });
            if matches {
                let state = slot.as_ref().expect("matching flux graph state");
                gpu_upload_into(&state.latents, packed_latents).map_err(DiffusionError::model)?;
                gpu_upload_into(&state.encoder, &conditioning.t5_hidden_states)
                    .map_err(DiffusionError::model)?;
                gpu_upload_into(&state.temb, temb_silu).map_err(DiffusionError::model)?;
            } else {
                // Drop the old graph first: its Drop unpins the pool buffers.
                *slot = None;
                *slot = Some(FluxDeviceGraphState {
                    namespace,
                    image_token_count: self.image_token_count,
                    text_token_count: self.text_token_count,
                    act16,
                    latents: gpu_upload(
                        packed_latents,
                        self.image_token_count,
                        weights.config.in_channels as usize,
                    )
                    .map_err(DiffusionError::model)?,
                    encoder: gpu_upload(
                        &conditioning.t5_hidden_states,
                        self.text_token_count,
                        weights.config.context_in_dim as usize,
                    )
                    .map_err(DiffusionError::model)?,
                    temb: gpu_upload(temb_silu, 1, self.hidden_size)
                        .map_err(DiffusionError::model)?,
                    rope_cos: gpu_upload(
                        &self.rope_tables.cos,
                        self.rope_tables.token_count,
                        self.rope_tables.half_dim,
                    )
                    .map_err(DiffusionError::model)?,
                    rope_sin: gpu_upload(
                        &self.rope_tables.sin,
                        self.rope_tables.token_count,
                        self.rope_tables.half_dim,
                    )
                    .map_err(DiffusionError::model)?,
                    warm_runs: 0,
                    graph: None,
                });
            }
            let state = slot.as_mut().expect("flux graph state");
            let (values, channel_count) = if let Some((graph, prediction)) = &state.graph {
                gpu_graph_launch(graph).map_err(DiffusionError::model)?;
                (
                    gpu_download(prediction).map_err(DiffusionError::model)?,
                    prediction.cols(),
                )
            } else if state.warm_runs < 2 {
                // Warm runs fill the weight caches and settle the pool into
                // its steady state before addresses get baked into a graph.
                // These are the runs where the weight stream-in happens, so
                // they get the per-block progress.
                state.warm_runs += 1;
                let prediction = self.execute_device_core(
                    weights,
                    &state.latents,
                    &state.encoder,
                    &state.temb,
                    &state.rope_cos,
                    &state.rope_sin,
                    progress,
                )?;
                (
                    gpu_download(&prediction).map_err(DiffusionError::model)?,
                    prediction.cols(),
                )
            } else {
                let captured = gpu_graph_capture(|| {
                    // Capture records without executing — no progress inside.
                    self.execute_device_core(
                        weights,
                        &state.latents,
                        &state.encoder,
                        &state.temb,
                        &state.rope_cos,
                        &state.rope_sin,
                        &mut None,
                    )
                    .map_err(|err| err.to_string())
                });
                match captured {
                    Ok((graph, prediction)) => {
                        // Capture records without executing: launch for the
                        // real step result.
                        gpu_graph_launch(&graph).map_err(DiffusionError::model)?;
                        let values =
                            gpu_download(&prediction).map_err(DiffusionError::model)?;
                        let channel_count = prediction.cols();
                        state.graph = Some((graph, prediction));
                        (values, channel_count)
                    }
                    Err(err) => {
                        eprintln!(
                            "flux device graph capture failed ({err}); running uncaptured"
                        );
                        state.warm_runs = u32::MAX;
                        let prediction = self.execute_device_core(
                            weights,
                            &state.latents,
                            &state.encoder,
                            &state.temb,
                            &state.rope_cos,
                            &state.rope_sin,
                            &mut None,
                        )?;
                        (
                            gpu_download(&prediction).map_err(DiffusionError::model)?,
                            prediction.cols(),
                        )
                    }
                }
            };
            Ok(FluxTransformerRun {
                prediction: values,
                image_token_count: self.image_token_count,
                channel_count,
            })
        })
    }

    /// The device-resident denoise step from persistent inputs to the
    /// still-on-device prediction: pure async device work on the dense
    /// backend's stream (capturable as a CUDA graph).
    #[allow(clippy::too_many_arguments)]
    fn execute_device_core(
        &self,
        weights: &LoadedFluxTransformerWeights,
        input_latents: &GpuTensor,
        input_encoder: &GpuTensor,
        temb_gpu: &GpuTensor,
        rope_cos: &GpuTensor,
        rope_sin: &GpuTensor,
        progress: &mut Option<ProgressHook>,
    ) -> Result<GpuTensor> {
        let namespace = flux_cache_namespace(weights);
        let attention_scale = 1.0 / (self.head_dim as f32).sqrt();
        // The f16 activation spine: modulated-norm outputs, qkv/linear1/mlp
        // activations and the q/k/v pipeline stay f16 between the f16acc
        // gemms and the attention/gelu consumers (FLUX_ACT_F16=0 reverts;
        // F8 weights force it off — their dequant feeds f32-accumulate
        // gemms, which refuse f16 activations).
        let act16 = flux_act16_enabled(weights);

        let mut hidden = linear_device(
            weights,
            &namespace,
            input_latents,
            "img_in.weight",
            "img_in.bias",
        )?;
        let mut encoder_hidden = linear_device(
            weights,
            &namespace,
            input_encoder,
            "txt_in.weight",
            "txt_in.bias",
        )?;

        // Every per-layer modulation projection depends only on temb, so run
        // them all as ONE resident multi-part linear + one download per step
        // instead of 76 host round-trips (each of which drains the stream).
        let hidden_size = self.hidden_size;
        let depth = weights.config.depth as usize;
        let depth_single = weights.config.depth_single_blocks as usize;
        let mut mod_names = Vec::with_capacity(depth * 2 + depth_single);
        for layer in 0..depth {
            mod_names.push((
                format!("double_blocks.{layer}.img_mod.lin.weight"),
                format!("double_blocks.{layer}.img_mod.lin.bias"),
            ));
            mod_names.push((
                format!("double_blocks.{layer}.txt_mod.lin.weight"),
                format!("double_blocks.{layer}.txt_mod.lin.bias"),
            ));
        }
        for layer in 0..depth_single {
            mod_names.push((
                format!("single_blocks.{layer}.modulation.lin.weight"),
                format!("single_blocks.{layer}.modulation.lin.bias"),
            ));
        }
        let mut mod_part_lists = Vec::with_capacity(mod_names.len());
        let mut mod_bias = Vec::new();
        let mut mod_offsets = Vec::with_capacity(mod_names.len());
        let mut mod_total = 0usize;
        for (weight_name, bias_name) in &mod_names {
            mod_offsets.push(mod_total);
            let parts = weights.tensor_matrix_parts(weight_name)?;
            for part in &parts {
                if part.cols != hidden_size {
                    return Err(DiffusionError::model(format!(
                        "flux modulation weight '{}' width mismatch: {}",
                        weight_name, part.cols
                    )));
                }
                mod_total += part.rows;
            }
            mod_bias.extend(weights.tensor_f32_values_concat(bias_name)?);
            mod_part_lists.push(parts);
        }
        if mod_bias.len() != mod_total {
            return Err(DiffusionError::model(
                "flux modulation bias/weight length mismatch",
            ));
        }
        let mod_gpu_parts = mod_part_lists
            .iter()
            .flatten()
            .map(|part| GpuLinearPart {
                bt_ggml_type: part.ggml_type,
                n: part.rows,
                cache_key: &part.cache_key,
                bytes: part.bytes,
            })
            .collect::<Vec<_>>();
        // The modulation row STAYS on device: layer norms and gated residuals
        // read scale/shift/gate at element offsets into it (no per-step
        // download, no per-call host vector uploads).
        let mod_out = gpu_linear_nt_cached(temb_gpu, &namespace, &mod_gpu_parts, &mod_bias)
            .map_err(DiffusionError::model)?;
        let mod_off = |index: usize, chunk: usize| -> usize {
            mod_offsets[index] + chunk * hidden_size
        };

        // Progress rides the block loop: on cold steps each block's weights
        // stream through gpu_weight_cache_ensure (synchronous uploads), so
        // per-block labels are what make the stream-in visible. On warm
        // steps the loop just enqueues async kernels and races ahead — the
        // labels then merely tick, which is harmless.
        let total_blocks = depth + depth_single;
        for layer in 0..depth {
            if progress.is_some() {
                emit_progress(
                    progress,
                    &format!("block {}/{total_blocks}", layer + 1),
                    layer as f64 / total_blocks as f64,
                )?;
            }
            let prefix = format!("double_blocks.{layer}");

            let img_mod = layer * 2;
            let txt_mod = layer * 2 + 1;
            let img_shift_msa = mod_off(img_mod, 0);
            let img_scale_msa = mod_off(img_mod, 1);
            let img_gate_msa = mod_off(img_mod, 2);
            let img_shift_mlp = mod_off(img_mod, 3);
            let img_scale_mlp = mod_off(img_mod, 4);
            let img_gate_mlp = mod_off(img_mod, 5);
            let txt_shift_msa = mod_off(txt_mod, 0);
            let txt_scale_msa = mod_off(txt_mod, 1);
            let txt_gate_msa = mod_off(txt_mod, 2);
            let txt_shift_mlp = mod_off(txt_mod, 3);
            let txt_scale_mlp = mod_off(txt_mod, 4);
            let txt_gate_mlp = mod_off(txt_mod, 5);

            let norm_hidden = if act16 {
                gpu_layer_norm_mod_f16(
                    &hidden,
                    &mod_out,
                    img_scale_msa,
                    img_shift_msa,
                    FLUX_LAYER_NORM_EPSILON,
                )
            } else {
                gpu_layer_norm_mod(
                    &hidden,
                    &mod_out,
                    img_scale_msa,
                    img_shift_msa,
                    FLUX_LAYER_NORM_EPSILON,
                )
            }
            .map_err(DiffusionError::model)?;
            let norm_encoder_hidden = if act16 {
                gpu_layer_norm_mod_f16(
                    &encoder_hidden,
                    &mod_out,
                    txt_scale_msa,
                    txt_shift_msa,
                    FLUX_LAYER_NORM_EPSILON,
                )
            } else {
                gpu_layer_norm_mod(
                    &encoder_hidden,
                    &mod_out,
                    txt_scale_msa,
                    txt_shift_msa,
                    FLUX_LAYER_NORM_EPSILON,
                )
            }
            .map_err(DiffusionError::model)?;

            let img_qkv = if act16 {
                linear_device_f16(
                    weights,
                    &namespace,
                    &norm_hidden,
                    &format!("{prefix}.img_attn.qkv.weight"),
                    Some(&format!("{prefix}.img_attn.qkv.bias")),
                )?
            } else {
                linear_device(
                    weights,
                    &namespace,
                    &norm_hidden,
                    &format!("{prefix}.img_attn.qkv.weight"),
                    &format!("{prefix}.img_attn.qkv.bias"),
                )?
            };
            drop(norm_hidden);
            let img_q = gpu_slice_cols(&img_qkv, 0, self.hidden_size).map_err(DiffusionError::model)?;
            let img_k = gpu_slice_cols(&img_qkv, self.hidden_size, self.hidden_size)
                .map_err(DiffusionError::model)?;
            let img_v = gpu_slice_cols(&img_qkv, self.hidden_size * 2, self.hidden_size)
                .map_err(DiffusionError::model)?;
            drop(img_qkv);
            let txt_qkv = if act16 {
                linear_device_f16(
                    weights,
                    &namespace,
                    &norm_encoder_hidden,
                    &format!("{prefix}.txt_attn.qkv.weight"),
                    Some(&format!("{prefix}.txt_attn.qkv.bias")),
                )?
            } else {
                linear_device(
                    weights,
                    &namespace,
                    &norm_encoder_hidden,
                    &format!("{prefix}.txt_attn.qkv.weight"),
                    &format!("{prefix}.txt_attn.qkv.bias"),
                )?
            };
            drop(norm_encoder_hidden);
            let txt_q = gpu_slice_cols(&txt_qkv, 0, self.hidden_size).map_err(DiffusionError::model)?;
            let txt_k = gpu_slice_cols(&txt_qkv, self.hidden_size, self.hidden_size)
                .map_err(DiffusionError::model)?;
            let txt_v = gpu_slice_cols(&txt_qkv, self.hidden_size * 2, self.hidden_size)
                .map_err(DiffusionError::model)?;
            drop(txt_qkv);

            let img_q = head_rms_norm_device(
                weights,
                &img_q,
                self.head_dim,
                &format!("{prefix}.img_attn.norm.query_norm.scale"),
            )?;
            let img_k = head_rms_norm_device(
                weights,
                &img_k,
                self.head_dim,
                &format!("{prefix}.img_attn.norm.key_norm.scale"),
            )?;
            let txt_q = head_rms_norm_device(
                weights,
                &txt_q,
                self.head_dim,
                &format!("{prefix}.txt_attn.norm.query_norm.scale"),
            )?;
            let txt_k = head_rms_norm_device(
                weights,
                &txt_k,
                self.head_dim,
                &format!("{prefix}.txt_attn.norm.key_norm.scale"),
            )?;

            let q = gpu_concat_rows(&txt_q, &img_q).map_err(DiffusionError::model)?;
            let k = gpu_concat_rows(&txt_k, &img_k).map_err(DiffusionError::model)?;
            let v = gpu_concat_rows(&txt_v, &img_v).map_err(DiffusionError::model)?;
            drop((txt_q, img_q, txt_k, img_k, txt_v, img_v));
            let q = gpu_rope_interleaved(&q, self.head_count, &rope_cos, &rope_sin)
                .map_err(DiffusionError::model)?;
            let k = gpu_rope_interleaved(&k, self.head_count, &rope_cos, &rope_sin)
                .map_err(DiffusionError::model)?;

            let attn = gpu_attention_packed(&q, &k, &v, self.head_count, attention_scale)
                .map_err(DiffusionError::model)?;
            drop((q, k, v));
            let encoder_attn =
                gpu_slice_rows(&attn, 0, self.text_token_count).map_err(DiffusionError::model)?;
            let hidden_attn = gpu_slice_rows(&attn, self.text_token_count, self.image_token_count)
                .map_err(DiffusionError::model)?;
            drop(attn);

            let hidden_attn = linear_device(
                weights,
                &namespace,
                &hidden_attn,
                &format!("{prefix}.img_attn.proj.weight"),
                &format!("{prefix}.img_attn.proj.bias"),
            )?;
            let encoder_attn = linear_device(
                weights,
                &namespace,
                &encoder_attn,
                &format!("{prefix}.txt_attn.proj.weight"),
                &format!("{prefix}.txt_attn.proj.bias"),
            )?;
            hidden = gpu_gated_residual_mod(&hidden, &hidden_attn, &mod_out, img_gate_msa)
                .map_err(DiffusionError::model)?;
            encoder_hidden =
                gpu_gated_residual_mod(&encoder_hidden, &encoder_attn, &mod_out, txt_gate_msa)
                    .map_err(DiffusionError::model)?;
            drop((hidden_attn, encoder_attn));

            let hidden_ff_input = if act16 {
                gpu_layer_norm_mod_f16(
                    &hidden,
                    &mod_out,
                    img_scale_mlp,
                    img_shift_mlp,
                    FLUX_LAYER_NORM_EPSILON,
                )
            } else {
                gpu_layer_norm_mod(
                    &hidden,
                    &mod_out,
                    img_scale_mlp,
                    img_shift_mlp,
                    FLUX_LAYER_NORM_EPSILON,
                )
            }
            .map_err(DiffusionError::model)?;
            let encoder_ff_input = if act16 {
                gpu_layer_norm_mod_f16(
                    &encoder_hidden,
                    &mod_out,
                    txt_scale_mlp,
                    txt_shift_mlp,
                    FLUX_LAYER_NORM_EPSILON,
                )
            } else {
                gpu_layer_norm_mod(
                    &encoder_hidden,
                    &mod_out,
                    txt_scale_mlp,
                    txt_shift_mlp,
                    FLUX_LAYER_NORM_EPSILON,
                )
            }
            .map_err(DiffusionError::model)?;

            let hidden_ff = feed_forward_device(
                weights,
                &namespace,
                &hidden_ff_input,
                &format!("{prefix}.img_mlp.0.weight"),
                &format!("{prefix}.img_mlp.0.bias"),
                &format!("{prefix}.img_mlp.2.weight"),
                &format!("{prefix}.img_mlp.2.bias"),
            )?;
            drop(hidden_ff_input);
            let encoder_ff = feed_forward_device(
                weights,
                &namespace,
                &encoder_ff_input,
                &format!("{prefix}.txt_mlp.0.weight"),
                &format!("{prefix}.txt_mlp.0.bias"),
                &format!("{prefix}.txt_mlp.2.weight"),
                &format!("{prefix}.txt_mlp.2.bias"),
            )?;
            drop(encoder_ff_input);
            hidden = gpu_gated_residual_mod(&hidden, &hidden_ff, &mod_out, img_gate_mlp)
                .map_err(DiffusionError::model)?;
            encoder_hidden =
                gpu_gated_residual_mod(&encoder_hidden, &encoder_ff, &mod_out, txt_gate_mlp)
                    .map_err(DiffusionError::model)?;
        }

        for layer in 0..depth_single {
            if progress.is_some() {
                let done = depth + layer;
                emit_progress(
                    progress,
                    &format!("block {}/{total_blocks}", done + 1),
                    done as f64 / total_blocks as f64,
                )?;
            }
            let prefix = format!("single_blocks.{layer}");
            let joint = gpu_concat_rows(&encoder_hidden, &hidden).map_err(DiffusionError::model)?;
            let single_mod = depth * 2 + layer;
            let shift = mod_off(single_mod, 0);
            let scale = mod_off(single_mod, 1);
            let gate = mod_off(single_mod, 2);
            let norm_joint = if act16 {
                gpu_layer_norm_mod_f16(&joint, &mod_out, scale, shift, FLUX_LAYER_NORM_EPSILON)
            } else {
                gpu_layer_norm_mod(&joint, &mod_out, scale, shift, FLUX_LAYER_NORM_EPSILON)
            }
            .map_err(DiffusionError::model)?;
            let linear1 = if act16 {
                linear_device_f16(
                    weights,
                    &namespace,
                    &norm_joint,
                    &format!("{prefix}.linear1.weight"),
                    Some(&format!("{prefix}.linear1.bias")),
                )?
            } else {
                linear_device(
                    weights,
                    &namespace,
                    &norm_joint,
                    &format!("{prefix}.linear1.weight"),
                    &format!("{prefix}.linear1.bias"),
                )?
            };
            drop(norm_joint);
            let q = gpu_slice_cols(&linear1, 0, self.hidden_size).map_err(DiffusionError::model)?;
            let k = gpu_slice_cols(&linear1, self.hidden_size, self.hidden_size)
                .map_err(DiffusionError::model)?;
            let v = gpu_slice_cols(&linear1, self.hidden_size * 2, self.hidden_size)
                .map_err(DiffusionError::model)?;
            let mlp = gpu_slice_cols(&linear1, self.hidden_size * 3, self.hidden_size * 4)
                .map_err(DiffusionError::model)?;
            drop(linear1);

            let q = head_rms_norm_device(
                weights,
                &q,
                self.head_dim,
                &format!("{prefix}.norm.query_norm.scale"),
            )?;
            let k = head_rms_norm_device(
                weights,
                &k,
                self.head_dim,
                &format!("{prefix}.norm.key_norm.scale"),
            )?;
            let q = gpu_rope_interleaved(&q, self.head_count, &rope_cos, &rope_sin)
                .map_err(DiffusionError::model)?;
            let k = gpu_rope_interleaved(&k, self.head_count, &rope_cos, &rope_sin)
                .map_err(DiffusionError::model)?;
            let attn = gpu_attention_packed(&q, &k, &v, self.head_count, attention_scale)
                .map_err(DiffusionError::model)?;
            drop((q, k, v));
            let mlp = gpu_gelu(&mlp).map_err(DiffusionError::model)?;
            // On the f16 spine the mlp half is f16; bring the f32 attention
            // output over so linear2 consumes one f16 activation block.
            let fused = if act16 {
                let attn16 = gpu_to_f16(&attn).map_err(DiffusionError::model)?;
                gpu_concat_cols(&[&attn16, &mlp]).map_err(DiffusionError::model)?
            } else {
                gpu_concat_cols(&[&attn, &mlp]).map_err(DiffusionError::model)?
            };
            drop((attn, mlp));
            let proj = linear_device(
                weights,
                &namespace,
                &fused,
                &format!("{prefix}.linear2.weight"),
                &format!("{prefix}.linear2.bias"),
            )?;
            drop(fused);
            let joint =
                gpu_gated_residual_mod(&joint, &proj, &mod_out, gate).map_err(DiffusionError::model)?;
            drop(proj);
            encoder_hidden =
                gpu_slice_rows(&joint, 0, self.text_token_count).map_err(DiffusionError::model)?;
            hidden = gpu_slice_rows(&joint, self.text_token_count, self.image_token_count)
                .map_err(DiffusionError::model)?;
        }

        // Final adaLN on device from the same silu(temb) row the modulation
        // linear used (m=1, f32-accumulate): adaLN output is [shift, scale]
        // and gpu_layer_norm_mod applies the +1 on scale internally — the
        // step stays fully device-resident (graph-capturable).
        let final_mod = linear_device(
            weights,
            &namespace,
            temb_gpu,
            "final_layer.adaLN_modulation.1.weight",
            "final_layer.adaLN_modulation.1.bias",
        )?;
        let hidden = gpu_layer_norm_mod(
            &hidden,
            &final_mod,
            self.hidden_size,
            0,
            FLUX_LAYER_NORM_EPSILON,
        )
        .map_err(DiffusionError::model)?;
        linear_device(
            weights,
            &namespace,
            &hidden,
            "final_layer.linear.weight",
            "final_layer.linear.bias",
        )
    }

}

/// The captured denoise-step graph + its persistent step-input tensors,
/// keyed by (weights namespace, shape, spine mode). Thread-local because
/// GpuTensor is single-thread; the denoise loop runs on one thread.
struct FluxDeviceGraphState {
    namespace: String,
    image_token_count: usize,
    text_token_count: usize,
    act16: bool,
    latents: GpuTensor,
    encoder: GpuTensor,
    temb: GpuTensor,
    rope_cos: GpuTensor,
    rope_sin: GpuTensor,
    warm_runs: u32,
    graph: Option<(GpuStepGraph, GpuTensor)>,
}

thread_local! {
    static FLUX_DEVICE_GRAPH: std::cell::RefCell<Option<FluxDeviceGraphState>> =
        const { std::cell::RefCell::new(None) };
}

fn flux_device_path_enabled() -> bool {
    match std::env::var("FLUX_DEVICE") {
        Ok(value) => value != "0",
        Err(_) => true,
    }
}

/// CUDA-graph replay of the denoise step. Default: 512-class shapes only —
/// at 1024 the graph-pinned pool buffers cannot be freed at the VAE phase
/// boundary, which re-crowds the 32GB WDDM residency cliff the per-phase
/// pool cap exists to avoid. FLUX_GRAPH=1 forces all shapes, =0 disables.
/// Compare/profile modes bypass the graph (they sync inside the step).
fn flux_graph_enabled(total_tokens: usize) -> bool {
    if matches!(std::env::var("FLUX_ATTN_COMPARE"), Ok(value) if value == "1")
        || matches!(std::env::var("MAKEPAD_GPU_PROF"), Ok(value) if value == "1")
    {
        return false;
    }
    match std::env::var("FLUX_GRAPH") {
        Ok(value) if value == "0" => false,
        Ok(value) if value == "1" => true,
        _ => total_tokens < 2048,
    }
}

/// Device linear mirroring `linear_rows`: multi-part weights land in their
/// column ranges directly; the cache key layout ("{ns}::{ns}::{name}") matches
/// the host lazy path exactly so both paths share one weight cache.
fn linear_device(
    weights: &LoadedFluxTransformerWeights,
    namespace: &str,
    input: &GpuTensor,
    weight_name: &str,
    bias_name: &str,
) -> Result<GpuTensor> {
    let parts = weights.tensor_matrix_parts(weight_name)?;
    let bias = weights.tensor_f32_values_concat(bias_name)?;
    for part in &parts {
        if part.cols != input.cols() {
            return Err(DiffusionError::model(format!(
                "flux device linear '{}' width mismatch: input={} weight={}",
                weight_name,
                input.cols(),
                part.cols
            )));
        }
    }
    let gpu_parts = parts
        .iter()
        .map(|part| GpuLinearPart {
            bt_ggml_type: part.ggml_type,
            n: part.rows,
            cache_key: &part.cache_key,
            bytes: part.bytes,
        })
        .collect::<Vec<_>>();
    gpu_linear_nt_cached(input, namespace, &gpu_parts, &bias).map_err(DiffusionError::model)
}

/// linear_device with an f16 result (the f16 activation spine): the gemm's
/// f16 C is the output, bias broadcast in place. `bias_name` = None defers
/// the bias into the consumer (the fused gelu).
fn linear_device_f16(
    weights: &LoadedFluxTransformerWeights,
    namespace: &str,
    input: &GpuTensor,
    weight_name: &str,
    bias_name: Option<&str>,
) -> Result<GpuTensor> {
    let parts = weights.tensor_matrix_parts(weight_name)?;
    let bias = match bias_name {
        Some(name) => weights.tensor_f32_values_concat(name)?,
        None => Vec::new(),
    };
    for part in &parts {
        if part.cols != input.cols() {
            return Err(DiffusionError::model(format!(
                "flux device linear '{}' width mismatch: input={} weight={}",
                weight_name,
                input.cols(),
                part.cols
            )));
        }
    }
    let gpu_parts = parts
        .iter()
        .map(|part| GpuLinearPart {
            bt_ggml_type: part.ggml_type,
            n: part.rows,
            cache_key: &part.cache_key,
            bytes: part.bytes,
        })
        .collect::<Vec<_>>();
    gpu_linear_nt_cached_f16(input, namespace, &gpu_parts, &bias).map_err(DiffusionError::model)
}

/// Device mirror of `apply_head_rms_norm_rows`.
fn head_rms_norm_device(
    weights: &LoadedFluxTransformerWeights,
    input: &GpuTensor,
    head_dim: usize,
    scale_name: &str,
) -> Result<GpuTensor> {
    let scale = weights.tensor_f32_values(scale_name)?;
    if scale.len() != head_dim {
        return Err(DiffusionError::model(format!(
            "flux head rms scale mismatch: scale={} head_dim={}",
            scale.len(),
            head_dim
        )));
    }
    gpu_rms_norm_mul(
        input,
        head_dim,
        &flux_cache_namespace(weights),
        scale_name,
        &scale,
        FLUX_LAYER_NORM_EPSILON,
    )
    .map_err(DiffusionError::model)
}

/// Device mirror of `feed_forward_rows`. On the f16 spine the mlp.0 C stays
/// f16 with its bias deferred into the fused gelu, and mlp.2 consumes the
/// f16 activations directly.
fn feed_forward_device(
    weights: &LoadedFluxTransformerWeights,
    namespace: &str,
    input: &GpuTensor,
    weight0: &str,
    bias0: &str,
    weight2: &str,
    bias2: &str,
) -> Result<GpuTensor> {
    if flux_act16_enabled(weights) {
        let hidden = linear_device_f16(weights, namespace, input, weight0, None)?;
        let bias = weights.tensor_f32_values_concat(bias0)?;
        let hidden = gpu_gelu_bias_f16(&hidden, namespace, weight0, &bias)
            .map_err(DiffusionError::model)?;
        return linear_device(weights, namespace, &hidden, weight2, bias2);
    }
    let hidden = linear_device(weights, namespace, input, weight0, bias0)?;
    let hidden = gpu_gelu(&hidden).map_err(DiffusionError::model)?;
    linear_device(weights, namespace, &hidden, weight2, bias2)
}

impl RowsTensor {
    fn new(rows: usize, cols: usize, data: Vec<f32>) -> Result<Self> {
        let expected = rows
            .checked_mul(cols)
            .ok_or_else(|| DiffusionError::model("flux rows tensor size overflow"))?;
        if data.len() != expected {
            return Err(DiffusionError::model(format!(
                "flux rows tensor expected {} values for {}x{}, got {}",
                expected,
                rows,
                cols,
                data.len()
            )));
        }
        Ok(Self { rows, cols, data })
    }

    fn slice_cols(&self, start: usize, len: usize) -> Result<Self> {
        if start + len > self.cols {
            return Err(DiffusionError::model(format!(
                "flux rows col slice [{}..{}) exceeds {}",
                start,
                start + len,
                self.cols
            )));
        }
        let mut data = Vec::with_capacity(self.rows * len);
        for row in self.data.chunks_exact(self.cols) {
            data.extend_from_slice(&row[start..start + len]);
        }
        Self::new(self.rows, len, data)
    }

    fn slice_rows(&self, start: usize, len: usize) -> Result<Self> {
        if start + len > self.rows {
            return Err(DiffusionError::model(format!(
                "flux rows row slice [{}..{}) exceeds {}",
                start,
                start + len,
                self.rows
            )));
        }
        let start_idx = start
            .checked_mul(self.cols)
            .ok_or_else(|| DiffusionError::model("flux rows slice start overflow"))?;
        let end_idx = (start + len)
            .checked_mul(self.cols)
            .ok_or_else(|| DiffusionError::model("flux rows slice end overflow"))?;
        Self::new(len, self.cols, self.data[start_idx..end_idx].to_vec())
    }

    fn concat_rows(lhs: &Self, rhs: &Self) -> Result<Self> {
        if lhs.cols != rhs.cols {
            return Err(DiffusionError::model(format!(
                "flux row concat width mismatch: lhs={} rhs={}",
                lhs.cols, rhs.cols
            )));
        }
        let mut data = Vec::with_capacity(lhs.data.len() + rhs.data.len());
        data.extend_from_slice(&lhs.data);
        data.extend_from_slice(&rhs.data);
        Self::new(lhs.rows + rhs.rows, lhs.cols, data)
    }

    fn concat_cols(lhs: &Self, rhs: &Self) -> Result<Self> {
        if lhs.rows != rhs.rows {
            return Err(DiffusionError::model(format!(
                "flux col concat row mismatch: lhs={} rhs={}",
                lhs.rows, rhs.rows
            )));
        }
        let mut data = Vec::with_capacity(lhs.data.len() + rhs.data.len());
        for row in 0..lhs.rows {
            let lhs_row = &lhs.data[row * lhs.cols..(row + 1) * lhs.cols];
            let rhs_row = &rhs.data[row * rhs.cols..(row + 1) * rhs.cols];
            data.extend_from_slice(lhs_row);
            data.extend_from_slice(rhs_row);
        }
        Self::new(lhs.rows, lhs.cols + rhs.cols, data)
    }
}

impl HeadTensor {
    fn new(token_count: usize, head_count: usize, head_dim: usize, data: Vec<f32>) -> Result<Self> {
        let expected = token_count
            .checked_mul(head_count)
            .and_then(|value| value.checked_mul(head_dim))
            .ok_or_else(|| DiffusionError::model("flux head tensor size overflow"))?;
        if data.len() != expected {
            return Err(DiffusionError::model(format!(
                "flux head tensor expected {} values for {}x{}x{}, got {}",
                expected,
                token_count,
                head_count,
                head_dim,
                data.len()
            )));
        }
        Ok(Self {
            token_count,
            head_count,
            head_dim,
            data,
        })
    }

    fn from_rows(rows: &RowsTensor, head_count: usize, head_dim: usize) -> Result<Self> {
        if rows.cols != head_count * head_dim {
            return Err(DiffusionError::model(format!(
                "flux rows-to-heads width mismatch: rows={} expected {}",
                rows.cols,
                head_count * head_dim
            )));
        }
        Self::new(rows.rows, head_count, head_dim, rows.data.clone())
    }

    fn concat_tokens(lhs: &Self, rhs: &Self) -> Result<Self> {
        if lhs.head_count != rhs.head_count || lhs.head_dim != rhs.head_dim {
            return Err(DiffusionError::model("flux head concat shape mismatch"));
        }
        let mut data = Vec::with_capacity(lhs.data.len() + rhs.data.len());
        data.extend_from_slice(&lhs.data);
        data.extend_from_slice(&rhs.data);
        Self::new(
            lhs.token_count + rhs.token_count,
            lhs.head_count,
            lhs.head_dim,
            data,
        )
    }
}

fn validate_flux_transformer_inputs(
    weights: &LoadedFluxTransformerWeights,
    conditioning: &FluxConditioning,
    latent_shape: FluxLatentShape,
) -> Result<FluxTransformerShapeInfo> {
    if conditioning.clip_hidden_size != weights.config.vec_in_dim as usize {
        return Err(DiffusionError::workflow(format!(
            "flux clip pooled hidden size mismatch: expected {}, got {}",
            weights.config.vec_in_dim, conditioning.clip_hidden_size
        )));
    }
    if conditioning.t5_hidden_size != weights.config.context_in_dim as usize {
        return Err(DiffusionError::workflow(format!(
            "flux t5 hidden size mismatch: expected {}, got {}",
            weights.config.context_in_dim, conditioning.t5_hidden_size
        )));
    }
    if latent_shape.transformer_channels != weights.config.in_channels {
        return Err(DiffusionError::workflow(format!(
            "flux packed latent channels mismatch: expected {}, got {}",
            weights.config.in_channels, latent_shape.transformer_channels
        )));
    }

    let hidden_size = usize::try_from(weights.config.hidden_size)
        .map_err(|_| DiffusionError::model("flux hidden size exceeds usize"))?;
    let head_count = usize::try_from(weights.config.num_heads)
        .map_err(|_| DiffusionError::model("flux head count exceeds usize"))?;
    let head_dim = usize::try_from(weights.config.head_dim())
        .map_err(|_| DiffusionError::model("flux head dim exceeds usize"))?;
    if head_count * head_dim != hidden_size {
        return Err(DiffusionError::model(format!(
            "flux hidden size {} is incompatible with {} heads of {} dims",
            hidden_size, head_count, head_dim
        )));
    }
    Ok(FluxTransformerShapeInfo {
        text_token_count: conditioning.t5_token_count,
        image_token_count: latent_shape.image_token_count as usize,
        hidden_size,
        head_count,
        head_dim,
    })
}

fn apply_timestep_projection_rows(
    weights: &LoadedFluxTransformerWeights,
    timestep: f32,
    prefix: &str,
) -> Result<RowsTensor> {
    let embed = cpu_timestep_embedding(timestep * 1000.0, FLUX_TIMESTEP_EMBED_DIM as usize, 10_000);
    let embed = RowsTensor::new(1, FLUX_TIMESTEP_EMBED_DIM as usize, embed)?;
    apply_silu_mlp_rows(weights, &embed, prefix)
}

fn apply_silu_mlp_rows(
    weights: &LoadedFluxTransformerWeights,
    input: &RowsTensor,
    prefix: &str,
) -> Result<RowsTensor> {
    let hidden = linear_rows(
        weights,
        input,
        &format!("{prefix}.in_layer.weight"),
        &format!("{prefix}.in_layer.bias"),
    )?;
    let hidden = silu_rows(&hidden)?;
    linear_rows(
        weights,
        &hidden,
        &format!("{prefix}.out_layer.weight"),
        &format!("{prefix}.out_layer.bias"),
    )
}

fn feed_forward_rows(
    weights: &LoadedFluxTransformerWeights,
    input: &RowsTensor,
    weight0: &str,
    bias0: &str,
    weight2: &str,
    bias2: &str,
) -> Result<RowsTensor> {
    let hidden = linear_rows(weights, input, weight0, bias0)?;
    let hidden = gelu_rows(&hidden)?;
    linear_rows(weights, &hidden, weight2, bias2)
}

fn modulation_chunks_rows(
    weights: &LoadedFluxTransformerWeights,
    temb: &RowsTensor,
    weight_name: &str,
    bias_name: &str,
    chunk_size: usize,
    chunk_count: usize,
) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>)> {
    let temb = silu_rows(temb)?;
    let linear = linear_rows(weights, &temb, weight_name, bias_name)?;
    let mut chunks = Vec::with_capacity(chunk_count.max(6));
    for index in 0..chunk_count {
        chunks.push(linear.slice_cols(index * chunk_size, chunk_size)?.data);
    }
    while chunks.len() < 6 {
        chunks.push(chunks[chunks.len() - 1].clone());
    }
    Ok((
        chunks[0].clone(),
        chunks[1].clone(),
        chunks[2].clone(),
        chunks[3].clone(),
        chunks[4].clone(),
        chunks[5].clone(),
    ))
}

fn qkv_projections_rows(
    weights: &LoadedFluxTransformerWeights,
    input: &RowsTensor,
    weight_name: &str,
    bias_name: &str,
    hidden_size: usize,
    head_count: usize,
    head_dim: usize,
) -> Result<(HeadTensor, HeadTensor, HeadTensor)> {
    let qkv = linear_rows(weights, input, weight_name, bias_name)?;
    let q = HeadTensor::from_rows(&qkv.slice_cols(0, hidden_size)?, head_count, head_dim)?;
    let k = HeadTensor::from_rows(
        &qkv.slice_cols(hidden_size, hidden_size)?,
        head_count,
        head_dim,
    )?;
    let v = HeadTensor::from_rows(
        &qkv.slice_cols(hidden_size * 2, hidden_size)?,
        head_count,
        head_dim,
    )?;
    Ok((q, k, v))
}

fn linear_rows(
    weights: &LoadedFluxTransformerWeights,
    input: &RowsTensor,
    weight_name: &str,
    bias_name: &str,
) -> Result<RowsTensor> {
    let weight_parts = weights.tensor_matrix_parts(weight_name)?;
    let bias = weights.tensor_f32_values_concat(bias_name)?;
    let total_rows = weight_parts.iter().map(|part| part.rows).sum::<usize>();
    if total_rows != bias.len() {
        return Err(DiffusionError::model(format!(
            "flux linear '{}' bias len mismatch: weights={} bias={}",
            weight_name,
            total_rows,
            bias.len()
        )));
    }
    let mut output = None;
    let mut bias_offset = 0usize;
    for part in &weight_parts {
        if input.cols != part.cols {
            return Err(DiffusionError::model(format!(
                "flux linear input width mismatch: input={} weight={}",
                input.cols, part.cols
            )));
        }
        let mut part_output = if input.rows == 0 {
            Vec::new()
        } else if flux_force_cpu_math() {
            let decoded = decoded_matrix_f32_cached(part)?;
            matmul_nt_f32_cpu(&input.data, decoded.as_slice(), input.rows, input.cols, part.rows)?
        } else if let Some(values) = try_matmul_nt_ggml_bytes(
            &input.data,
            part.bytes,
            part.ggml_type,
            input.rows,
            input.cols,
            part.rows,
        ) {
            // Pass the resident mmap/arena slice so Metal's pointer-keyed
            // weight cache can reuse the GPU buffer. `to_vec()` here used to
            // allocate a new host copy every linear and bust that cache.
            values
        } else if let Some(result) = try_matmul_nt_ggml_bytes_cached(
            &input.data,
            part.ggml_type,
            input.rows,
            input.cols,
            part.rows,
            &flux_cache_namespace(weights),
            &part.cache_key,
            || Ok(part.bytes.to_vec()),
        ) {
            match result {
                Ok(values) => values,
                Err(err) if can_fallback_from_accel_error(&err) => {
                    let decoded = decoded_matrix_f32_cached(part)?;
                    if let Some(values) = try_matmul_nt_f32(
                        &input.data,
                        decoded.as_slice(),
                        input.rows,
                        input.cols,
                        part.rows,
                    )
                    {
                        values
                    } else {
                        matmul_nt_f32_cpu(
                            &input.data,
                            decoded.as_slice(),
                            input.rows,
                            input.cols,
                            part.rows,
                        )?
                    }
                }
                Err(err) => return Err(DiffusionError::model(err)),
            }
        } else {
            let decoded = decoded_matrix_f32_cached(part)?;
            if let Some(values) = try_matmul_nt_f32(
                &input.data,
                decoded.as_slice(),
                input.rows,
                input.cols,
                part.rows,
            )
            {
                values
            } else {
                matmul_nt_f32_cpu(
                    &input.data,
                    decoded.as_slice(),
                    input.rows,
                    input.cols,
                    part.rows,
                )?
            }
        };
        let part_bias = &bias[bias_offset..bias_offset + part.rows];
        apply_row_bias_in_place(&mut part_output, part_bias, input.rows, part.rows)?;
        let part_output = RowsTensor::new(input.rows, part.rows, part_output)?;
        output = Some(match output {
            Some(existing) => RowsTensor::concat_cols(&existing, &part_output)?,
            None => part_output,
        });
        bias_offset += part.rows;
    }
    output.ok_or_else(|| DiffusionError::model(format!("flux missing linear weights '{}'", weight_name)))
}

fn apply_modulated_layer_norm_rows(
    input: &RowsTensor,
    scale: &[f32],
    shift: &[f32],
) -> Result<RowsTensor> {
    if input.cols != scale.len() || input.cols != shift.len() {
        return Err(DiffusionError::model(format!(
            "flux layer norm modulation mismatch: cols={} scale={} shift={}",
            input.cols,
            scale.len(),
            shift.len()
        )));
    }
    let scale_plus_one = scale.iter().map(|value| value + 1.0).collect::<Vec<_>>();
    if input.rows == 0 {
        return RowsTensor::new(0, input.cols, Vec::new());
    }
    if !flux_force_cpu_math() {
        if let Some(output) = try_layer_norm_mul_add_f32(
            &input.data,
            &[input.rows, input.cols],
            &scale_plus_one,
            &[input.cols],
            shift,
            &[input.cols],
            FLUX_LAYER_NORM_EPSILON,
        ) {
            return RowsTensor::new(input.rows, input.cols, output);
        }
    }
    let mut output = Vec::with_capacity(input.data.len());
    for row in input.data.chunks_exact(input.cols) {
        let mean = row.iter().copied().sum::<f32>() / input.cols as f32;
        let variance = row
            .iter()
            .map(|value| {
                let centered = *value - mean;
                centered * centered
            })
            .sum::<f32>()
            / input.cols as f32;
        let inv_std = 1.0 / (variance + FLUX_LAYER_NORM_EPSILON).sqrt();
        for ((value, mul), add) in row.iter().zip(scale_plus_one.iter()).zip(shift.iter()) {
            output.push((value - mean) * inv_std * mul + add);
        }
    }
    RowsTensor::new(input.rows, input.cols, output)
}

fn apply_head_rms_norm_rows(
    weights: &LoadedFluxTransformerWeights,
    input: &HeadTensor,
    scale_name: &str,
) -> Result<HeadTensor> {
    let scale = weights.tensor_f32_values(scale_name)?;
    if scale.len() != input.head_dim {
        return Err(DiffusionError::model(format!(
            "flux head rms scale mismatch: scale={} head_dim={}",
            scale.len(),
            input.head_dim
        )));
    }
    if input.token_count == 0 {
        return HeadTensor::new(0, input.head_count, input.head_dim, Vec::new());
    }
    if !flux_force_cpu_math() {
        if let Some(output) = try_rms_norm_mul_f32(
            &input.data,
            &[input.token_count * input.head_count, input.head_dim],
            &scale,
            &[scale.len()],
            FLUX_LAYER_NORM_EPSILON,
        ) {
            return HeadTensor::new(input.token_count, input.head_count, input.head_dim, output);
        }
    }
    let mut output = Vec::with_capacity(input.data.len());
    for row in input.data.chunks_exact(input.head_dim) {
        let mean_square = row.iter().map(|value| value * value).sum::<f32>() / input.head_dim as f32;
        let inv_rms = 1.0 / (mean_square + FLUX_LAYER_NORM_EPSILON).sqrt();
        for (value, scale) in row.iter().zip(scale.iter()) {
            output.push(value * inv_rms * scale);
        }
    }
    HeadTensor::new(input.token_count, input.head_count, input.head_dim, output)
}

fn apply_flux_rope_heads(input: &HeadTensor, rope_tables: &FluxRopeTables) -> Result<HeadTensor> {
    if input.token_count != rope_tables.token_count {
        return Err(DiffusionError::model(format!(
            "flux rope token mismatch: heads={} rope={}",
            input.token_count,
            rope_tables.token_count
        )));
    }
    if input.head_dim != rope_tables.half_dim * 2 {
        return Err(DiffusionError::model(format!(
            "flux rope dim mismatch: head_dim={} expected {}",
            input.head_dim,
            rope_tables.half_dim * 2
        )));
    }
    let mut output = input.data.clone();
    let hidden_size = input.head_count * input.head_dim;
    for token in 0..input.token_count {
        for head in 0..input.head_count {
            let base = token * hidden_size + head * input.head_dim;
            for pair in 0..rope_tables.half_dim {
                let even = base + pair * 2;
                let odd = even + 1;
                let table_index = pair + rope_tables.half_dim * token;
                let cos = rope_tables.cos[table_index];
                let sin = rope_tables.sin[table_index];
                let x0 = output[even];
                let x1 = output[odd];
                output[even] = x0 * cos - x1 * sin;
                output[odd] = x0 * sin + x1 * cos;
            }
        }
    }
    HeadTensor::new(input.token_count, input.head_count, input.head_dim, output)
}

fn build_attention_output_rows(q: &HeadTensor, k: &HeadTensor, v: &HeadTensor) -> Result<RowsTensor> {
    if q.token_count != k.token_count
        || q.token_count != v.token_count
        || q.head_count != k.head_count
        || q.head_count != v.head_count
        || q.head_dim != k.head_dim
        || q.head_dim != v.head_dim
    {
        return Err(DiffusionError::model("flux attention shape mismatch"));
    }
    let token_count = q.token_count;
    let head_count = q.head_count;
    let head_dim = q.head_dim;
    let scale = 1.0 / (head_dim as f32).sqrt();

    if !flux_force_cpu_math() {
        if let Some(output) = try_flash_attn_f32_packed(
            &q.data,
            &k.data,
            &v.data,
            token_count,
            token_count,
            head_count,
            head_dim,
            scale,
        ) {
            return RowsTensor::new(token_count, head_count * head_dim, output);
        }
    }

    let mut output = vec![0.0f32; token_count * head_count * head_dim];
    for head_idx in 0..head_count {
        let q_head = extract_head_rows(&q.data, token_count, head_count, head_dim, head_idx);
        let k_head = extract_head_rows(&k.data, token_count, head_count, head_dim, head_idx);
        let v_head = extract_head_rows(&v.data, token_count, head_count, head_dim, head_idx);
        let mut scores = if flux_force_cpu_math() {
            matmul_nt_f32_cpu(&q_head, &k_head, token_count, head_dim, token_count)?
        } else if let Some(scores) =
            try_matmul_nt_f32(&q_head, &k_head, token_count, head_dim, token_count)
        {
            scores
        } else {
            matmul_nt_f32_cpu(&q_head, &k_head, token_count, head_dim, token_count)?
        };
        for score in &mut scores {
            *score *= scale;
        }

        if !flux_force_cpu_math() {
            if let Some(head_output) = try_attention_softmax_weighted_sum_f32(
                &scores,
                &v_head,
                token_count,
                token_count,
                head_dim,
            ) {
                write_head_rows(
                    &mut output,
                    token_count,
                    head_count,
                    head_dim,
                    head_idx,
                    &head_output,
                )?;
                continue;
            }
        }

        softmax_in_place(&mut scores, token_count)?;
        let head_output = if flux_force_cpu_math() {
            matmul_nn_f32_cpu(&scores, &v_head, token_count, token_count, head_dim)?
        } else if let Some(head_output) =
            try_matmul_nn_f32(&scores, &v_head, token_count, token_count, head_dim)
        {
            head_output
        } else {
            matmul_nn_f32_cpu(&scores, &v_head, token_count, token_count, head_dim)?
        };
        write_head_rows(
            &mut output,
            token_count,
            head_count,
            head_dim,
            head_idx,
            &head_output,
        )?;
    }
    RowsTensor::new(token_count, head_count * head_dim, output)
}

fn silu_rows(input: &RowsTensor) -> Result<RowsTensor> {
    RowsTensor::new(
        input.rows,
        input.cols,
        input
            .data
            .iter()
            .copied()
            .map(|value| value / (1.0 + (-value).exp()))
            .collect(),
    )
}

fn gelu_rows(input: &RowsTensor) -> Result<RowsTensor> {
    if input.rows == 0 {
        return RowsTensor::new(0, input.cols, Vec::new());
    }
    if !flux_force_cpu_math() {
        if let Some(output) = try_gelu_f32(&input.data, &[input.rows, input.cols]) {
            return RowsTensor::new(input.rows, input.cols, output);
        }
    }
    RowsTensor::new(
        input.rows,
        input.cols,
        input
            .data
            .iter()
            .copied()
            .map(gelu_scalar)
            .collect(),
    )
}

fn add_rows(lhs: &RowsTensor, rhs: &RowsTensor) -> Result<RowsTensor> {
    if lhs.rows != rhs.rows || lhs.cols != rhs.cols {
        return Err(DiffusionError::model(format!(
            "flux add shape mismatch: lhs={}x{} rhs={}x{}",
            lhs.rows, lhs.cols, rhs.rows, rhs.cols
        )));
    }
    if lhs.rows == 0 {
        return RowsTensor::new(0, lhs.cols, Vec::new());
    }
    if !flux_force_cpu_math() {
        if let Some(output) =
            try_add_f32(&lhs.data, &[lhs.rows, lhs.cols], &rhs.data, &[rhs.rows, rhs.cols])
        {
            return RowsTensor::new(lhs.rows, lhs.cols, output);
        }
    }
    RowsTensor::new(
        lhs.rows,
        lhs.cols,
        lhs.data
            .iter()
            .zip(rhs.data.iter())
            .map(|(lhs, rhs)| lhs + rhs)
            .collect(),
    )
}

fn gated_residual_rows(residual: &RowsTensor, update: &RowsTensor, gate: &[f32]) -> Result<RowsTensor> {
    if residual.rows != update.rows || residual.cols != update.cols {
        return Err(DiffusionError::model(format!(
            "flux gated residual shape mismatch: residual={}x{} update={}x{}",
            residual.rows,
            residual.cols,
            update.rows,
            update.cols
        )));
    }
    if gate.len() != update.cols {
        return Err(DiffusionError::model(format!(
            "flux gate width mismatch: gate={} cols={}",
            gate.len(),
            update.cols
        )));
    }
    if residual.rows == 0 {
        return RowsTensor::new(0, residual.cols, Vec::new());
    }
    if !flux_force_cpu_math() {
        if let Some(scaled_update) =
            try_mul_f32(&update.data, &[update.rows, update.cols], gate, &[gate.len()])
        {
            if let Some(output) = try_add_f32(
                &residual.data,
                &[residual.rows, residual.cols],
                &scaled_update,
                &[update.rows, update.cols],
            ) {
                return RowsTensor::new(residual.rows, residual.cols, output);
            }
        }
    }
    let mut output = residual.data.clone();
    for row in 0..update.rows {
        for col in 0..update.cols {
            let index = row * update.cols + col;
            output[index] += update.data[index] * gate[col];
        }
    }
    RowsTensor::new(residual.rows, residual.cols, output)
}

fn flux_rope_table_values(
    text_token_count: usize,
    latent_shape: FluxLatentShape,
    config: FluxTransformerConfig,
) -> Result<FluxRopeTables> {
    let token_count = text_token_count + latent_shape.image_token_count as usize;
    let half_dim = usize::try_from(config.axes_dim_sum() / 2)
        .map_err(|_| DiffusionError::model("flux rope half dim exceeds usize"))?;
    let mut cos = vec![1.0f32; half_dim * token_count];
    let mut sin = vec![0.0f32; half_dim * token_count];
    let theta = config.theta as f32;
    let packed_width = latent_shape.packed_width as usize;
    let packed_height = latent_shape.packed_height as usize;

    let mut token_index = text_token_count;
    for row in 0..packed_height {
        for col in 0..packed_width {
            let positions = [0.0f32, row as f32, col as f32];
            let mut pair_offset = 0usize;
            for (axis_index, axis_dim) in config.axes_dim.into_iter().enumerate() {
                let section_dim = axis_dim as usize;
                let section_pairs = section_dim / 2;
                for pair in 0..section_pairs {
                    let exponent = (2.0f32 * pair as f32) / section_dim as f32;
                    let angle = positions[axis_index] / theta.powf(exponent);
                    let index = pair_offset + pair + half_dim * token_index;
                    cos[index] = angle.cos();
                    sin[index] = angle.sin();
                }
                pair_offset += section_pairs;
            }
            token_index += 1;
        }
    }

    Ok(FluxRopeTables {
        token_count,
        half_dim,
        cos,
        sin,
    })
}

fn push_debug_rows(
    stages: &mut Vec<FluxTransformerStageOutput>,
    enabled: bool,
    name: &str,
    tensor: &RowsTensor,
) {
    if !enabled {
        return;
    }
    stages.push(FluxTransformerStageOutput {
        name: name.to_string(),
        values: tensor.data.clone(),
        extents: [tensor.cols, tensor.rows, 1, 1],
    });
}

fn push_debug_heads(
    stages: &mut Vec<FluxTransformerStageOutput>,
    enabled: bool,
    name: &str,
    tensor: &HeadTensor,
) {
    if !enabled {
        return;
    }
    stages.push(FluxTransformerStageOutput {
        name: name.to_string(),
        values: tensor.data.clone(),
        extents: [tensor.head_dim, tensor.head_count, tensor.token_count, 1],
    });
}

pub(crate) fn flux_cache_namespace(weights: &LoadedFluxTransformerWeights) -> String {
    format!("flux_transformer:{}", weights.path.display())
}

/// The f16 activation spine requires f16-accumulate gemms, which the F8
/// dequant path (bf16 scratch + f32 accumulate, the ComfyUI-default numerics
/// class) deliberately refuses — so F8 weights force the f32 spine.
fn flux_act16_enabled(weights: &LoadedFluxTransformerWeights) -> bool {
    !weights.f8_weights && gpu_act_f16_enabled()
}

/// Compiled-graph F16 spine (Metal). Off by default: at 256 the extra
/// F32↔F16 casts after ggml mul_mat (always F32) cost more than Q4×F16
/// saves. `FLUX_ACT_F16=1` enables; dirty drops ~150MB.
fn flux_compiled_act16() -> bool {
    matches!(std::env::var("FLUX_ACT_F16"), Ok(value) if value != "0")
}

fn tensor_type_of(ctx: &Context, id: TensorId) -> Result<TensorType> {
    Ok(require_tensor(ctx, id)?.desc.ty)
}

fn cast_activation(ctx: &mut Context, src: TensorId, ty: TensorType) -> Result<TensorId> {
    let tensor = require_tensor(ctx, src)?;
    if tensor.desc.ty == ty {
        return Ok(src);
    }
    let ne = tensor.ne;
    let dst = ctx
        .new_tensor_4d(ty, ne[0], ne[1], ne[2], ne[3], BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    ctx.cpy(src, dst, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn match_activation_type(ctx: &mut Context, src: TensorId, like: TensorId) -> Result<TensorId> {
    let ty = tensor_type_of(ctx, like)?;
    cast_activation(ctx, src, ty)
}

/// Drops every device weight-cache entry belonging to `weights` (the whole
/// unet, ~24GB in f16 on dev/schnell). The cache keys on the transformer
/// path and never evicts on its own — call this before replacing a resident
/// pipeline with a DIFFERENT model on the same thread, or a 32GB card ends
/// up asked to hold two flux unets. Returns the number of buffers freed.
pub(crate) fn evict_device_weight_cache(weights: &LoadedFluxTransformerWeights) -> usize {
    crate::backend::gpu_weight_cache_evict_prefix(&flux_cache_namespace(weights)).unwrap_or(0)
}

fn flux_force_cpu_math() -> bool {
    std::env::var_os("FLUX_TRANSFORMER_FORCE_CPU_MATH").is_some()
}

fn can_fallback_from_accel_error(err: &str) -> bool {
    err.contains("only supports NVFP4 today") || err.contains("unsupported ggml type")
}

fn resident_matrix<'a>(ctx: &'a Context, tensor_id: TensorId, cache_key: String) -> Result<ResidentMatrix<'a>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    let cols = usize::try_from(tensor.ne[0])
        .map_err(|_| DiffusionError::model(format!("flux tensor {} cols exceed usize", tensor_id)))?;
    let rows = usize::try_from(tensor.ne[1])
        .map_err(|_| DiffusionError::model(format!("flux tensor {} rows exceed usize", tensor_id)))?;
    Ok(ResidentMatrix {
        bytes: ctx.tensor_data(tensor_id).map_err(DiffusionError::model)?,
        ggml_type: tensor.desc.ty.ggml_type(),
        cols,
        rows,
        cache_key,
    })
}

fn tensor_to_f32_vec(ctx: &Context, tensor_id: TensorId) -> Result<Vec<f32>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    let bytes = ctx.tensor_data(tensor_id).map_err(DiffusionError::model)?;
    match tensor.desc.ty {
        TensorType::F32 => f32_bytes_to_vec(bytes),
        TensorType::F16 => f16_bytes_to_f32_vec(bytes),
        TensorType::BF16 => bf16_bytes_to_f32_vec(bytes),
        TensorType::F8E4M3 => Ok(bytes.iter().map(|&b| f8_e4m3_to_f32(b)).collect()),
        other => Err(DiffusionError::model(format!(
            "flux tensor {} cannot be decoded as f32 from {:?}",
            tensor_id, other
        ))),
    }
}

fn decode_ggml_matrix_to_f32(matrix: &ResidentMatrix<'_>) -> Result<Vec<f32>> {
    let mut out = Vec::with_capacity(matrix.rows * matrix.cols);
    match matrix.ggml_type {
        x if x == TensorType::F32.ggml_type() => {
            out.extend_from_slice(&f32_bytes_to_vec(matrix.bytes)?);
            Ok(out)
        }
        x if x == TensorType::F16.ggml_type() => {
            out.extend_from_slice(&f16_bytes_to_f32_vec(matrix.bytes)?);
            Ok(out)
        }
        x if x == TensorType::BF16.ggml_type() => {
            out.extend_from_slice(&bf16_bytes_to_f32_vec(matrix.bytes)?);
            Ok(out)
        }
        x if x == TensorType::F8E4M3.ggml_type() => {
            out.extend(matrix.bytes.iter().map(|&b| f8_e4m3_to_f32(b)));
            Ok(out)
        }
        other => Err(DiffusionError::model(format!(
            "flux transformer unsupported ggml matrix type {}",
            other
        ))),
    }
}

fn decoded_matrix_f32_cached(matrix: &ResidentMatrix<'_>) -> Result<Arc<Vec<f32>>> {
    thread_local! {
        static DECODED_F32_MATRIX_CACHE: RefCell<BTreeMap<String, Arc<Vec<f32>>>> =
            const { RefCell::new(BTreeMap::new()) };
    }

    DECODED_F32_MATRIX_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(decoded) = cache.get(&matrix.cache_key) {
            return Ok(decoded.clone());
        }
        let decoded = Arc::new(decode_ggml_matrix_to_f32(matrix)?);
        cache.insert(matrix.cache_key.clone(), decoded.clone());
        Ok(decoded)
    })
}

fn matmul_nt_f32_cpu(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Result<Vec<f32>> {
    if a.len()
        != m.checked_mul(k)
            .ok_or_else(|| DiffusionError::model("flux matmul a overflow"))?
    {
        return Err(DiffusionError::model("flux matmul_nt_f32_cpu a len mismatch"));
    }
    if bt.len()
        != n.checked_mul(k)
            .ok_or_else(|| DiffusionError::model("flux matmul bt overflow"))?
    {
        return Err(DiffusionError::model("flux matmul_nt_f32_cpu bt len mismatch"));
    }
    let mut out = vec![
        0.0f32;
        m.checked_mul(n)
            .ok_or_else(|| DiffusionError::model("flux matmul out overflow"))?
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
            .ok_or_else(|| DiffusionError::model("flux matmul a overflow"))?
    {
        return Err(DiffusionError::model("flux matmul_nn_f32_cpu a len mismatch"));
    }
    if b.len()
        != k.checked_mul(n)
            .ok_or_else(|| DiffusionError::model("flux matmul b overflow"))?
    {
        return Err(DiffusionError::model("flux matmul_nn_f32_cpu b len mismatch"));
    }
    let mut out = vec![
        0.0f32;
        m.checked_mul(n)
            .ok_or_else(|| DiffusionError::model("flux matmul out overflow"))?
    ];
    for row in 0..m {
        for col in 0..n {
            let mut acc = 0.0f32;
            for idx in 0..k {
                acc += a[row * k + idx] * b[idx * n + col];
            }
            out[row * n + col] = acc;
        }
    }
    Ok(out)
}

fn apply_row_bias_in_place(
    values: &mut [f32],
    bias: &[f32],
    row_count: usize,
    row_width: usize,
) -> Result<()> {
    if bias.len() != row_width {
        return Err(DiffusionError::model(format!(
            "flux bias width mismatch: bias={} row_width={}",
            bias.len(),
            row_width
        )));
    }
    if values.len() != row_count * row_width {
        return Err(DiffusionError::model(format!(
            "flux bias apply len mismatch: values={} expected {}",
            values.len(),
            row_count * row_width
        )));
    }
    for row in values.chunks_exact_mut(row_width) {
        for (value, bias_value) in row.iter_mut().zip(bias.iter()) {
            *value += bias_value;
        }
    }
    Ok(())
}

fn extract_head_rows(
    values: &[f32],
    token_count: usize,
    head_count: usize,
    head_dim: usize,
    head_idx: usize,
) -> Vec<f32> {
    let hidden = head_count * head_dim;
    let mut out = Vec::with_capacity(token_count * head_dim);
    for token in 0..token_count {
        let start = token * hidden + head_idx * head_dim;
        out.extend_from_slice(&values[start..start + head_dim]);
    }
    out
}

fn write_head_rows(
    dst: &mut [f32],
    token_count: usize,
    head_count: usize,
    head_dim: usize,
    head_idx: usize,
    src: &[f32],
) -> Result<()> {
    if src.len() != token_count * head_dim {
        return Err(DiffusionError::model(format!(
            "flux head write len mismatch: src={} expected {}",
            src.len(),
            token_count * head_dim
        )));
    }
    let hidden = head_count * head_dim;
    for token in 0..token_count {
        let dst_start = token * hidden + head_idx * head_dim;
        let src_start = token * head_dim;
        dst[dst_start..dst_start + head_dim].copy_from_slice(&src[src_start..src_start + head_dim]);
    }
    Ok(())
}

fn softmax_in_place(values: &mut [f32], width: usize) -> Result<()> {
    if width == 0 || values.len() % width != 0 {
        return Err(DiffusionError::model(format!(
            "flux softmax width {} is incompatible with len {}",
            width,
            values.len()
        )));
    }
    for row in values.chunks_exact_mut(width) {
        let max_value = row
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let mut sum = 0.0f32;
        for value in row.iter_mut() {
            *value = (*value - max_value).exp();
            sum += *value;
        }
        if sum == 0.0 {
            return Err(DiffusionError::model("flux softmax row sum is zero"));
        }
        for value in row.iter_mut() {
            *value /= sum;
        }
    }
    Ok(())
}

fn cpu_timestep_embedding(timestep: f32, dim: usize, max_period: i32) -> Vec<f32> {
    let half = dim / 2;
    let mut embed = vec![0.0f32; dim];
    for j in 0..half {
        let freq = (-((max_period as f32).ln()) * j as f32 / half as f32).exp();
        let arg = timestep * freq;
        embed[j] = arg.cos();
        embed[j + half] = arg.sin();
    }
    embed
}

fn gelu_scalar(x: f32) -> f32 {
    let inner = (2.0f32 / std::f32::consts::PI).sqrt() * (x + 0.044_715 * x * x * x);
    0.5 * x * (1.0 + inner.tanh())
}

fn tensor_part_names(
    tensor_ids: &BTreeMap<String, TensorId>,
    base_name: &str,
) -> Result<Vec<String>> {
    let mut names = Vec::new();
    if tensor_ids.contains_key(base_name) {
        names.push(base_name.to_string());
    }
    let mut suffix_index = 1usize;
    loop {
        let part_name = format!("{base_name}.{suffix_index}");
        if tensor_ids.contains_key(&part_name) {
            names.push(part_name);
            suffix_index += 1;
        } else {
            break;
        }
    }
    if names.is_empty() {
        return Err(DiffusionError::model(format!(
            "missing flux resident tensor '{}'",
            base_name
        )));
    }
    Ok(names)
}

pub fn build_flux_transformer_graph(
    weights: &mut LoadedFluxTransformerWeights,
    conditioning: &FluxConditioning,
    latent_shape: FluxLatentShape,
) -> Result<FluxTransformerGraph> {
    let shape = validate_flux_transformer_inputs(weights, conditioning, latent_shape)?;
    let text_token_count = shape.text_token_count;
    let image_token_count = shape.image_token_count;
    let hidden_size = i64::try_from(shape.hidden_size)
        .map_err(|_| DiffusionError::model("flux hidden size exceeds i64"))?;
    let head_count = i64::try_from(shape.head_count)
        .map_err(|_| DiffusionError::model("flux head count exceeds i64"))?;
    let head_dim = i64::try_from(shape.head_dim)
        .map_err(|_| DiffusionError::model("flux head dim exceeds i64"))?;

    let input_packed_latents = weights
        .ctx
        .new_named_tensor(
            "flux.input_packed_latents",
            TensorType::F32,
            2,
            &[
                i64::from(weights.config.in_channels),
                image_token_count as i64,
            ],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let input_encoder_hidden_states = weights
        .ctx
        .new_named_tensor(
            "flux.input_encoder_hidden_states",
            TensorType::F32,
            2,
            &[
                i64::from(weights.config.context_in_dim),
                text_token_count as i64,
            ],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let input_pooled_projections = weights
        .ctx
        .new_named_tensor(
            "flux.input_pooled_projections",
            TensorType::F32,
            2,
            &[i64::from(weights.config.vec_in_dim), 1],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let input_timestep = weights
        .ctx
        .new_named_tensor(
            "flux.input_timestep",
            TensorType::F32,
            1,
            &[1],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let input_guidance = if weights.config.guidance_embed {
        Some(
            weights
                .ctx
                .new_named_tensor(
                    "flux.input_guidance",
                    TensorType::F32,
                    1,
                    &[1],
                    BufferUsage::Activations,
                )
                .map_err(DiffusionError::model)?,
        )
    } else {
        None
    };
    let (rope_pos_h, rope_pos_w) = build_flux_rope_axis_positions(
        &mut weights.ctx,
        text_token_count,
        latent_shape,
    )?;
    bake_modulation_scale_plus_one(weights, shape.hidden_size)?;
    // Host-backed scalar used by silu/gelu `repeat`. Created before
    // no_alloc so write_tensor_data still has a dirty-arena slot.
    weights
        .ctx
        .new_named_tensor(
            "flux.scalar_one",
            TensorType::F32,
            1,
            &[1],
            BufferUsage::Weights,
        )
        .and_then(|id| {
            weights
                .ctx
                .write_tensor_data(id, &1.0f32.to_le_bytes())
                .map(|_| id)
        })
        .map_err(DiffusionError::model)?;
    // Graph intermediates stay metadata-only. The Metal binding planner
    // assigns (and reuses) GPU dirty ranges; CPU-allocating them is what
    // grew a 32GiB arena and then failed newBufferWithBytesNoCopy.
    weights.ctx.set_no_alloc(true);
    let mut debug_tensors = Vec::new();

    let img_in_weight = concat_linear_parts(
        &mut weights.ctx,
        &weights.tensor_ids,
        "img_in.weight",
    )?;
    let input_hidden_mm = weights
        .ctx
        .mul_mat(img_in_weight, input_packed_latents, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let img_in_bias = concat_linear_parts(
        &mut weights.ctx,
        &weights.tensor_ids,
        "img_in.bias",
    )?;
    let mut hidden = weights
        .ctx
        .binary_like_a(Op::Add, input_hidden_mm, img_in_bias, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let input_hidden = hidden;
    if flux_compiled_act16() {
        hidden = cast_activation(&mut weights.ctx, hidden, TensorType::F16)?;
    }
    push_debug_tensor(&mut weights.ctx, &mut debug_tensors, "input.hidden", hidden)?;
    let mut encoder_hidden = apply_linear(
        &mut weights.ctx,
        &weights.tensor_ids,
        input_encoder_hidden_states,
        "txt_in.weight",
        "txt_in.bias",
    )?;
    if flux_compiled_act16() {
        encoder_hidden = cast_activation(&mut weights.ctx, encoder_hidden, TensorType::F16)?;
    }
    push_debug_tensor(
        &mut weights.ctx,
        &mut debug_tensors,
        "input.encoder_hidden",
        encoder_hidden,
    )?;

    let mut temb = apply_timestep_projection(
        &mut weights.ctx,
        &weights.tensor_ids,
        input_timestep,
        "time_in",
    )?;
    let pooled = apply_silu_mlp(
        &mut weights.ctx,
        &weights.tensor_ids,
        input_pooled_projections,
        "vector_in",
    )?;
    temb = weights
        .ctx
        .binary_like_a(Op::Add, temb, pooled, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    if let Some(guidance) = input_guidance {
        let guidance = apply_timestep_projection(
            &mut weights.ctx,
            &weights.tensor_ids,
            guidance,
            "guidance_in",
        )?;
        temb = weights
            .ctx
            .binary_like_a(Op::Add, temb, guidance, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
    }
    push_debug_tensor(&mut weights.ctx, &mut debug_tensors, "input.temb", temb)?;
    // One SiLU(temb) for every AdaLN linear. The previous graph silu'd the
    // same 1x3072 row 76 times (19*2 + 38).
    let temb_silu = silu(&mut weights.ctx, temb)?;

    for layer in 0..weights.config.depth as usize {
        let prefix = format!("double_blocks.{layer}");

        let (
            img_shift_msa,
            img_scale_msa,
            img_gate_msa,
            img_shift_mlp,
            img_scale_mlp,
            img_gate_mlp,
        ) = modulation_chunks(
            &mut weights.ctx,
            &weights.tensor_ids,
            temb_silu,
            &format!("{prefix}.img_mod.lin.weight"),
            &format!("{prefix}.img_mod.lin.bias"),
            hidden_size as usize,
            6,
        )?;
        let (
            txt_shift_msa,
            txt_scale_msa,
            txt_gate_msa,
            txt_shift_mlp,
            txt_scale_mlp,
            txt_gate_mlp,
        ) = modulation_chunks(
            &mut weights.ctx,
            &weights.tensor_ids,
            temb_silu,
            &format!("{prefix}.txt_mod.lin.weight"),
            &format!("{prefix}.txt_mod.lin.bias"),
            hidden_size as usize,
            6,
        )?;

        let norm_hidden = apply_modulated_layer_norm(
            &mut weights.ctx,
            hidden,
            img_scale_msa,
            img_shift_msa,
        )?;
        let norm_encoder_hidden = apply_modulated_layer_norm(
            &mut weights.ctx,
            encoder_hidden,
            txt_scale_msa,
            txt_shift_msa,
        )?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.norm_hidden",
                norm_hidden,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.norm_encoder_hidden",
                norm_encoder_hidden,
            )?;
        }

        let (img_q, img_k, img_v) = qkv_projections(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm_hidden,
            &format!("{prefix}.img_attn.qkv.weight"),
            &format!("{prefix}.img_attn.qkv.bias"),
            hidden_size as usize,
            head_dim,
            head_count,
            image_token_count as i64,
        )?;
        let (txt_q, txt_k, txt_v) = qkv_projections(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm_encoder_hidden,
            &format!("{prefix}.txt_attn.qkv.weight"),
            &format!("{prefix}.txt_attn.qkv.bias"),
            hidden_size as usize,
            head_dim,
            head_count,
            text_token_count as i64,
        )?;

        let img_q = apply_head_rms_norm(
            &mut weights.ctx,
            &weights.tensor_ids,
            img_q,
            &format!("{prefix}.img_attn.norm.query_norm.scale"),
        )?;
        let img_k = apply_head_rms_norm(
            &mut weights.ctx,
            &weights.tensor_ids,
            img_k,
            &format!("{prefix}.img_attn.norm.key_norm.scale"),
        )?;
        let txt_q = apply_head_rms_norm(
            &mut weights.ctx,
            &weights.tensor_ids,
            txt_q,
            &format!("{prefix}.txt_attn.norm.query_norm.scale"),
        )?;
        let txt_k = apply_head_rms_norm(
            &mut weights.ctx,
            &weights.tensor_ids,
            txt_k,
            &format!("{prefix}.txt_attn.norm.key_norm.scale"),
        )?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.img_q_norm",
                img_q,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.img_k_norm",
                img_k,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.img_v",
                img_v,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.txt_q_norm",
                txt_q,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.txt_k_norm",
                txt_k,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.txt_v",
                txt_v,
            )?;
        }

        let q = weights
            .ctx
            .concat(txt_q, img_q, 2, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
        let k = weights
            .ctx
            .concat(txt_k, img_k, 2, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
        let v = weights
            .ctx
            .concat(txt_v, img_v, 2, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
        let (q, k) = apply_flux_rope_pair(
            &mut weights.ctx,
            q,
            k,
            rope_pos_h,
            rope_pos_w,
            weights.config,
        )?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.q_rope",
                q,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.k_rope",
                k,
            )?;
        }
        let attn = build_attention_output(&mut weights.ctx, q, k, v, weights.config.head_dim())?;
        let encoder_attn = slice_cols_2d(&mut weights.ctx, attn, 0, text_token_count as i64)?;
        let hidden_attn = slice_cols_2d(
            &mut weights.ctx,
            attn,
            text_token_count as i64,
            image_token_count as i64,
        )?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.hidden_attn_input",
                hidden_attn,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.encoder_attn_input",
                encoder_attn,
            )?;
        }

        let hidden_attn = apply_linear(
            &mut weights.ctx,
            &weights.tensor_ids,
            hidden_attn,
            &format!("{prefix}.img_attn.proj.weight"),
            &format!("{prefix}.img_attn.proj.bias"),
        )?;
        let encoder_attn = apply_linear(
            &mut weights.ctx,
            &weights.tensor_ids,
            encoder_attn,
            &format!("{prefix}.txt_attn.proj.weight"),
            &format!("{prefix}.txt_attn.proj.bias"),
        )?;
        hidden = gated_residual(&mut weights.ctx, hidden, hidden_attn, img_gate_msa)?;
        encoder_hidden =
            gated_residual(&mut weights.ctx, encoder_hidden, encoder_attn, txt_gate_msa)?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.hidden_post_attn",
                hidden,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.encoder_hidden_post_attn",
                encoder_hidden,
            )?;
        }

        let hidden_ff_input = apply_modulated_layer_norm(
            &mut weights.ctx,
            hidden,
            img_scale_mlp,
            img_shift_mlp,
        )?;
        let hidden_ff = feed_forward(
            &mut weights.ctx,
            &weights.tensor_ids,
            hidden_ff_input,
            &format!("{prefix}.img_mlp.0.weight"),
            &format!("{prefix}.img_mlp.0.bias"),
            &format!("{prefix}.img_mlp.2.weight"),
            &format!("{prefix}.img_mlp.2.bias"),
        )?;
        let encoder_ff_input = apply_modulated_layer_norm(
            &mut weights.ctx,
            encoder_hidden,
            txt_scale_mlp,
            txt_shift_mlp,
        )?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.hidden_ff_input",
                hidden_ff_input,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "double_blocks.0.encoder_ff_input",
                encoder_ff_input,
            )?;
        }
        let encoder_ff = feed_forward(
            &mut weights.ctx,
            &weights.tensor_ids,
            encoder_ff_input,
            &format!("{prefix}.txt_mlp.0.weight"),
            &format!("{prefix}.txt_mlp.0.bias"),
            &format!("{prefix}.txt_mlp.2.weight"),
            &format!("{prefix}.txt_mlp.2.bias"),
        )?;
        hidden = gated_residual(&mut weights.ctx, hidden, hidden_ff, img_gate_mlp)?;
        encoder_hidden =
            gated_residual(&mut weights.ctx, encoder_hidden, encoder_ff, txt_gate_mlp)?;
        push_debug_tensor(
            &mut weights.ctx,
            &mut debug_tensors,
            &format!("double_blocks.{layer}.hidden"),
            hidden,
        )?;
        push_debug_tensor(
            &mut weights.ctx,
            &mut debug_tensors,
            &format!("double_blocks.{layer}.encoder_hidden"),
            encoder_hidden,
        )?;
    }

    // Stay in the joint [text+image] layout for every single block.
    // The old loop concat/split each layer (~37 extra 6MB CPY at 256).
    let enc_f32 = cast_activation(&mut weights.ctx, encoder_hidden, TensorType::F32)?;
    let hid_f32 = cast_activation(&mut weights.ctx, hidden, TensorType::F32)?;
    let mut joint = weights
        .ctx
        .concat(enc_f32, hid_f32, 1, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    if flux_compiled_act16() {
        joint = cast_activation(&mut weights.ctx, joint, TensorType::F16)?;
    }

    for layer in 0..weights.config.depth_single_blocks as usize {
        let prefix = format!("single_blocks.{layer}");
        let (shift, scale, gate, _, _, _) = modulation_chunks(
            &mut weights.ctx,
            &weights.tensor_ids,
            temb_silu,
            &format!("{prefix}.modulation.lin.weight"),
            &format!("{prefix}.modulation.lin.bias"),
            hidden_size as usize,
            3,
        )?;
        let norm_joint =
            apply_modulated_layer_norm(&mut weights.ctx, joint, scale, shift)?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "single_blocks.0.norm_joint",
                norm_joint,
            )?;
        }
        let linear1 = apply_linear(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm_joint,
            &format!("{prefix}.linear1.weight"),
            &format!("{prefix}.linear1.bias"),
        )?;
        let total_token_count = text_token_count + image_token_count;
        let q = view_heads(
            &mut weights.ctx,
            linear1,
            0,
            hidden_size,
            head_dim,
            head_count,
            total_token_count as i64,
        )?;
        let k = view_heads(
            &mut weights.ctx,
            linear1,
            hidden_size,
            hidden_size,
            head_dim,
            head_count,
            total_token_count as i64,
        )?;
        let v = view_heads(
            &mut weights.ctx,
            linear1,
            hidden_size * 2,
            hidden_size,
            head_dim,
            head_count,
            total_token_count as i64,
        )?;
        let mlp = slice_rows_2d(&mut weights.ctx, linear1, hidden_size * 3, hidden_size * 4)?;
        let q = apply_head_rms_norm(
            &mut weights.ctx,
            &weights.tensor_ids,
            q,
            &format!("{prefix}.norm.query_norm.scale"),
        )?;
        let k = apply_head_rms_norm(
            &mut weights.ctx,
            &weights.tensor_ids,
            k,
            &format!("{prefix}.norm.key_norm.scale"),
        )?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "single_blocks.0.q_norm",
                q,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "single_blocks.0.k_norm",
                k,
            )?;
            push_debug_tensor(&mut weights.ctx, &mut debug_tensors, "single_blocks.0.v", v)?;
        }
        let (q, k) = apply_flux_rope_pair(
            &mut weights.ctx,
            q,
            k,
            rope_pos_h,
            rope_pos_w,
            weights.config,
        )?;
        let attn = build_attention_output(&mut weights.ctx, q, k, v, weights.config.head_dim())?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "single_blocks.0.attn",
                attn,
            )?;
        }
        let mlp = gelu(&mut weights.ctx, mlp)?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "single_blocks.0.mlp",
                mlp,
            )?;
        }
        let fused = weights
            .ctx
            .concat(attn, mlp, 0, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
        let proj = apply_linear(
            &mut weights.ctx,
            &weights.tensor_ids,
            fused,
            &format!("{prefix}.linear2.weight"),
            &format!("{prefix}.linear2.bias"),
        )?;
        if layer == 0 {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "single_blocks.0.proj",
                proj,
            )?;
        }
        joint = gated_residual(&mut weights.ctx, joint, proj, gate)?;
        if trace_single_block(layer, weights.config.depth_single_blocks as usize) {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                &format!("single_blocks.{layer}.joint"),
                joint,
            )?;
            let dbg_enc =
                slice_cols_2d(&mut weights.ctx, joint, 0, text_token_count as i64)?;
            let dbg_hid = slice_cols_2d(
                &mut weights.ctx,
                joint,
                text_token_count as i64,
                image_token_count as i64,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                &format!("single_blocks.{layer}.hidden"),
                dbg_hid,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                &format!("single_blocks.{layer}.encoder_hidden"),
                dbg_enc,
            )?;
        }
    }
    hidden = slice_cols_2d(
        &mut weights.ctx,
        joint,
        text_token_count as i64,
        image_token_count as i64,
    )?;

    let final_mod_input = silu(&mut weights.ctx, temb)?;
    let final_mod = apply_linear(
        &mut weights.ctx,
        &weights.tensor_ids,
        final_mod_input,
        "final_layer.adaLN_modulation.1.weight",
        "final_layer.adaLN_modulation.1.bias",
    )?;
    let final_shift = slice_rows_2d(&mut weights.ctx, final_mod, 0, hidden_size)?;
    let final_scale = slice_rows_2d(&mut weights.ctx, final_mod, hidden_size, hidden_size)?;
    hidden = apply_modulated_layer_norm(
        &mut weights.ctx,
        hidden,
        final_scale,
        final_shift,
    )?;
    let result_prediction = apply_linear(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        "final_layer.linear.weight",
        "final_layer.linear.bias",
    )?;
    push_debug_tensor(&mut weights.ctx, &mut debug_tensors, "final.hidden", hidden)?;
    push_debug_tensor(
        &mut weights.ctx,
        &mut debug_tensors,
        "final.output",
        result_prediction,
    )?;

    let mut graph = Graph::new();
    graph
        .build_forward_expand(&weights.ctx, result_prediction)
        .map_err(DiffusionError::model)?;
    for debug_tensor in &debug_tensors {
        graph
            .build_forward_expand(&weights.ctx, debug_tensor.tensor_id)
            .map_err(DiffusionError::model)?;
    }
    graph.add_leaf(input_hidden);
    graph.add_leaf(input_hidden_mm);

    Ok(FluxTransformerGraph {
        graph,
        input_packed_latents,
        input_encoder_hidden_states,
        input_pooled_projections,
        input_timestep,
        input_guidance,
        result_prediction,
        image_token_count,
        input_hidden,
        input_hidden_mm,
        debug_tensors,
    })
}

fn flux_debug_transformer_stages() -> bool {
    std::env::var_os("FLUX_DEBUG_TRANSFORMER_STAGES").is_some()
}

fn push_debug_tensor(
    ctx: &mut Context,
    debug_tensors: &mut Vec<FluxTransformerDebugTensor>,
    name: &str,
    tensor_id: TensorId,
) -> Result<()> {
    // Production denoise never reads these. Leaving them in the compiled
    // graph runs ~80 extra CPY kernels and pins activations in the dirty
    // buffer. Smoke/compare opt in with FLUX_DEBUG_TRANSFORMER_STAGES.
    if !flux_debug_transformer_stages() {
        return Ok(());
    }
    let captured = ctx.cont(tensor_id).map_err(DiffusionError::model)?;
    debug_tensors.push(FluxTransformerDebugTensor {
        name: name.to_string(),
        tensor_id: captured,
    });
    Ok(())
}

fn trace_single_block(layer: usize, total_layers: usize) -> bool {
    layer < 2 || layer + 1 == total_layers || (layer + 1) % 4 == 0
}

fn apply_timestep_projection(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    prefix: &str,
) -> Result<TensorId> {
    let scaled = ctx
        .scale(input, 1000.0, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let embed = ctx
        .timestep_embedding(
            scaled,
            FLUX_TIMESTEP_EMBED_DIM,
            10_000,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    apply_silu_mlp(ctx, tensor_ids, embed, prefix)
}

fn apply_silu_mlp(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    prefix: &str,
) -> Result<TensorId> {
    let hidden = apply_linear(
        ctx,
        tensor_ids,
        input,
        &format!("{prefix}.in_layer.weight"),
        &format!("{prefix}.in_layer.bias"),
    )?;
    let hidden = silu(ctx, hidden)?;
    apply_linear(
        ctx,
        tensor_ids,
        hidden,
        &format!("{prefix}.out_layer.weight"),
        &format!("{prefix}.out_layer.bias"),
    )
}

fn feed_forward(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight0: &str,
    bias0: &str,
    weight2: &str,
    bias2: &str,
) -> Result<TensorId> {
    let hidden = apply_linear(ctx, tensor_ids, input, weight0, bias0)?;
    let hidden = match_activation_type(ctx, hidden, input)?;
    let hidden = gelu(ctx, hidden)?;
    apply_linear(ctx, tensor_ids, hidden, weight2, bias2)
}

fn modulation_chunks(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    temb: TensorId,
    weight_name: &str,
    bias_name: &str,
    chunk_size: usize,
    chunk_count: usize,
) -> Result<(TensorId, TensorId, TensorId, TensorId, TensorId, TensorId)> {
    let linear = apply_linear(ctx, tensor_ids, temb, weight_name, bias_name)?;
    let mut chunks = Vec::with_capacity(chunk_count);
    for index in 0..chunk_count {
        chunks.push(slice_rows_2d(
            ctx,
            linear,
            (index * chunk_size) as i64,
            chunk_size as i64,
        )?);
    }
    while chunks.len() < 6 {
        chunks.push(chunks[chunks.len() - 1]);
    }
    Ok((
        chunks[0], chunks[1], chunks[2], chunks[3], chunks[4], chunks[5],
    ))
}

fn qkv_projections(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight_name: &str,
    bias_name: &str,
    hidden_size: usize,
    head_dim: i64,
    head_count: i64,
    token_count: i64,
) -> Result<(TensorId, TensorId, TensorId)> {
    let qkv = apply_linear(ctx, tensor_ids, input, weight_name, bias_name)?;
    let q = view_heads(
        ctx,
        qkv,
        0,
        hidden_size as i64,
        head_dim,
        head_count,
        token_count,
    )?;
    let k = view_heads(
        ctx,
        qkv,
        hidden_size as i64,
        hidden_size as i64,
        head_dim,
        head_count,
        token_count,
    )?;
    let v = view_heads(
        ctx,
        qkv,
        (hidden_size * 2) as i64,
        hidden_size as i64,
        head_dim,
        head_count,
        token_count,
    )?;
    Ok((q, k, v))
}

/// `[head_dim, heads, tokens]` window into a packed `[rows, tokens]` linear.
/// Official ggml view — no CPY. Dim0 stays contiguous so RMS/bin/flash work.
fn view_heads(
    ctx: &mut Context,
    input: TensorId,
    start_row: i64,
    row_count: i64,
    head_dim: i64,
    head_count: i64,
    token_count: i64,
) -> Result<TensorId> {
    if row_count != head_dim * head_count {
        return Err(DiffusionError::model(format!(
            "flux head view expected {} rows, got {}",
            head_dim * head_count,
            row_count
        )));
    }
    let tensor = require_tensor(ctx, input)?.clone();
    if tensor.ne[1] != token_count {
        return Err(DiffusionError::model(format!(
            "flux head view token mismatch: tensor={} expected={}",
            tensor.ne[1], token_count
        )));
    }
    let elem = ggml_type_size_for_type(tensor.desc.ty);
    let offset = usize::try_from(start_row)
        .map_err(|_| DiffusionError::model("flux head view start is negative"))?
        .checked_mul(elem)
        .ok_or_else(|| DiffusionError::model("flux head view offset overflow"))?;
    ctx.view_3d(
        input,
        head_dim,
        head_count,
        token_count,
        usize::try_from(head_dim)
            .map_err(|_| DiffusionError::model("flux head dim exceeds usize"))?
            .checked_mul(elem)
            .ok_or_else(|| DiffusionError::model("flux head view nb1 overflow"))?,
        tensor.nb[1],
        offset,
    )
    .map_err(DiffusionError::model)
}

fn flux_qk_stack() -> bool {
    match std::env::var("FLUX_QK_STACK") {
        Ok(value) if value == "0" || value.eq_ignore_ascii_case("off") => false,
        _ => true,
    }
}

fn apply_flux_rope_pair(
    ctx: &mut Context,
    q: TensorId,
    k: TensorId,
    pos_h: TensorId,
    pos_w: TensorId,
    config: FluxTransformerConfig,
) -> Result<(TensorId, TensorId)> {
    // Stacking Q/K on ne[3] halved rope dispatches but flash_attn then
    // NaN'd at mid-schedule sigmas (t=0.8448). Default is separate ropes;
    // FLUX_QK_STACK=1 restores the cut.
    if flux_qk_stack() {
        let q_t = require_tensor(ctx, q)?.clone();
        let k_t = require_tensor(ctx, k)?.clone();
        let q4 = ctx
            .reshape(q, &[q_t.ne[0], q_t.ne[1], q_t.ne[2], 1])
            .map_err(DiffusionError::model)?;
        let k4 = ctx
            .reshape(k, &[k_t.ne[0], k_t.ne[1], k_t.ne[2], 1])
            .map_err(DiffusionError::model)?;
        let qk = ctx
            .concat(q4, k4, 3, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
        let qk_rot = apply_flux_rope(ctx, qk, pos_h, pos_w, config)?;
        let rot = require_tensor(ctx, qk_rot)?.clone();
        let elem = ggml_type_size_for_type(rot.desc.ty);
        let plane = usize::try_from(rot.ne[0] * rot.ne[1] * rot.ne[2])
            .map_err(|_| DiffusionError::model("flux rope qk plane exceeds usize"))?
            .checked_mul(elem)
            .ok_or_else(|| DiffusionError::model("flux rope qk plane overflow"))?;
        let q_rot = ctx
            .view_4d(
                qk_rot,
                rot.ne[0],
                rot.ne[1],
                rot.ne[2],
                1,
                rot.nb[1],
                rot.nb[2],
                rot.nb[3],
                0,
            )
            .map_err(DiffusionError::model)?;
        let k_rot = ctx
            .view_4d(
                qk_rot,
                rot.ne[0],
                rot.ne[1],
                rot.ne[2],
                1,
                rot.nb[1],
                rot.nb[2],
                rot.nb[3],
                plane,
            )
            .map_err(DiffusionError::model)?;
        return Ok((q_rot, k_rot));
    }
    let q = apply_flux_rope(ctx, q, pos_h, pos_w, config)?;
    let k = apply_flux_rope(ctx, k, pos_h, pos_w, config)?;
    Ok((q, k))
}

fn apply_flux_rope(
    ctx: &mut Context,
    tensor: TensorId,
    pos_h: TensorId,
    pos_w: TensorId,
    config: FluxTransformerConfig,
) -> Result<TensorId> {
    // Official kernel_rope_norm on each spatial axis. Time (axes_dim[0]) is
    // always position 0 in Flux1 — identity, so we just keep that view.
    // Interleaved pairing matches rope_norm; no pack/unpack CPY.
    let mut cursor = 0i64;
    let mut out: Option<TensorId> = None;
    for (axis_i, &axis_dim) in config.axes_dim.iter().enumerate() {
        let dim = i64::from(axis_dim);
        let section = view_rope_section(ctx, tensor, cursor, dim)?;
        let piece = if axis_i == 0 {
            section
        } else {
            let positions = if axis_i == 1 { pos_h } else { pos_w };
            ctx.rope_ext(
                section,
                positions,
                None,
                axis_dim as i32,
                GGML_ROPE_TYPE_NORMAL,
                256,
                config.theta as f32,
                1.0,
                0.0,
                1.0,
                32.0,
                1.0,
                BufferUsage::Activations,
            )
            .map_err(DiffusionError::model)?
        };
        out = Some(match out {
            None => piece,
            Some(prev) => ctx
                .concat(prev, piece, 0, BufferUsage::Activations)
                .map_err(DiffusionError::model)?,
        });
        cursor += dim;
    }
    out.ok_or_else(|| DiffusionError::model("flux rope has no axes"))
}

fn view_rope_section(
    ctx: &mut Context,
    input: TensorId,
    start_dim: i64,
    section_dim: i64,
) -> Result<TensorId> {
    let tensor = require_tensor(ctx, input)?.clone();
    let elem = ggml_type_size_for_type(tensor.desc.ty);
    let offset = usize::try_from(start_dim)
        .map_err(|_| DiffusionError::model("flux rope section start is negative"))?
        .checked_mul(elem)
        .ok_or_else(|| DiffusionError::model("flux rope section offset overflow"))?;
    ctx.view_4d(
        input,
        section_dim,
        tensor.ne[1],
        tensor.ne[2],
        tensor.ne[3].max(1),
        tensor.nb[1],
        tensor.nb[2],
        tensor.nb[3],
        offset,
    )
    .map_err(DiffusionError::model)
}

#[allow(dead_code)]
fn pack_rope_interleaved_pairs(
    ctx: &mut Context,
    tensor: TensorId,
    head_dim: i64,
) -> Result<TensorId> {
    let tensor_ref = require_tensor(ctx, tensor)?.clone();
    let reshaped = ctx
        .reshape(
            tensor,
            &[2, head_dim / 2, tensor_ref.ne[1], tensor_ref.ne[2]],
        )
        .map_err(DiffusionError::model)?;
    let permuted = ctx
        .permute(reshaped, [1, 0, 2, 3])
        .map_err(DiffusionError::model)?;
    let packed = ctx.cont(permuted).map_err(DiffusionError::model)?;
    ctx.reshape(packed, &[head_dim, tensor_ref.ne[1], tensor_ref.ne[2]])
        .map_err(DiffusionError::model)
}

#[allow(dead_code)]
fn unpack_rope_interleaved_pairs(
    ctx: &mut Context,
    tensor: TensorId,
    head_dim: i64,
) -> Result<TensorId> {
    let tensor_ref = require_tensor(ctx, tensor)?.clone();
    let reshaped = ctx
        .reshape(
            tensor,
            &[head_dim / 2, 2, tensor_ref.ne[1], tensor_ref.ne[2]],
        )
        .map_err(DiffusionError::model)?;
    let permuted = ctx
        .permute(reshaped, [1, 0, 2, 3])
        .map_err(DiffusionError::model)?;
    let unpacked = ctx.cont(permuted).map_err(DiffusionError::model)?;
    ctx.reshape(unpacked, &[head_dim, tensor_ref.ne[1], tensor_ref.ne[2]])
        .map_err(DiffusionError::model)
}

fn build_flux_rope_axis_positions(
    ctx: &mut Context,
    text_token_count: usize,
    latent_shape: FluxLatentShape,
) -> Result<(TensorId, TensorId)> {
    let token_count = text_token_count + latent_shape.image_token_count as usize;
    let mut pos_h = vec![0i32; token_count];
    let mut pos_w = vec![0i32; token_count];
    let packed_width = latent_shape.packed_width as usize;
    let packed_height = latent_shape.packed_height as usize;
    let mut token_index = text_token_count;
    for row in 0..packed_height {
        for col in 0..packed_width {
            pos_h[token_index] = i32::try_from(row)
                .map_err(|_| DiffusionError::model("flux rope row exceeds i32"))?;
            pos_w[token_index] = i32::try_from(col)
                .map_err(|_| DiffusionError::model("flux rope col exceeds i32"))?;
            token_index += 1;
        }
    }
    let h_tensor = ctx
        .new_named_tensor(
            "flux.rope_pos_h",
            TensorType::I32,
            1,
            &[token_count as i64],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    ctx.write_tensor_data(h_tensor, &i32s_to_le_bytes(&pos_h))
        .map_err(DiffusionError::model)?;
    let w_tensor = ctx
        .new_named_tensor(
            "flux.rope_pos_w",
            TensorType::I32,
            1,
            &[token_count as i64],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    ctx.write_tensor_data(w_tensor, &i32s_to_le_bytes(&pos_w))
        .map_err(DiffusionError::model)?;
    Ok((h_tensor, w_tensor))
}

fn bake_modulation_scale_plus_one(
    weights: &mut LoadedFluxTransformerWeights,
    hidden: usize,
) -> Result<()> {
    let mut jobs: Vec<(String, usize, Vec<usize>)> = Vec::new();
    for layer in 0..weights.config.depth as usize {
        jobs.push((
            format!("double_blocks.{layer}.img_mod.lin.bias"),
            6,
            vec![1, 4],
        ));
        jobs.push((
            format!("double_blocks.{layer}.txt_mod.lin.bias"),
            6,
            vec![1, 4],
        ));
    }
    for layer in 0..weights.config.depth_single_blocks as usize {
        jobs.push((
            format!("single_blocks.{layer}.modulation.lin.bias"),
            3,
            vec![1],
        ));
    }
    jobs.push((
        "final_layer.adaLN_modulation.1.bias".to_string(),
        2,
        vec![1],
    ));
    for (name, chunks, scale_idxs) in jobs {
        let src = require_tensor_id(&weights.tensor_ids, &name)?;
        let baked = bias_with_scale_plus_one(&mut weights.ctx, src, hidden, chunks, &scale_idxs, &name)?;
        weights.tensor_ids.insert(name, baked);
    }
    Ok(())
}

fn bias_with_scale_plus_one(
    ctx: &mut Context,
    src: TensorId,
    hidden: usize,
    chunks: usize,
    scale_idxs: &[usize],
    name: &str,
) -> Result<TensorId> {
    let src_t = require_tensor(ctx, src)?.clone();
    if src_t.desc.ty != TensorType::F32 {
        return Err(DiffusionError::model(format!(
            "flux modulation bias '{}' must be f32 to bake scale+1, got {}",
            name,
            src_t.desc.ty.name()
        )));
    }
    let expected = hidden
        .checked_mul(chunks)
        .ok_or_else(|| DiffusionError::model("flux modulation bias size overflow"))?;
    let nbytes = expected * 4;
    let bytes = ctx.tensor_data(src).map_err(DiffusionError::model)?;
    if bytes.len() < nbytes {
        return Err(DiffusionError::model(format!(
            "flux modulation bias '{}' is {} bytes, expected at least {}",
            name,
            bytes.len(),
            nbytes
        )));
    }
    let mut baked = bytes[..nbytes].to_vec();
    for &idx in scale_idxs {
        let start = idx
            .checked_mul(hidden)
            .and_then(|v| v.checked_mul(4))
            .ok_or_else(|| DiffusionError::model("flux scale-bias offset overflow"))?;
        let end = start + hidden * 4;
        for chunk in baked[start..end].chunks_exact_mut(4) {
            let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) + 1.0;
            chunk.copy_from_slice(&value.to_le_bytes());
        }
    }
    let dst = ctx
        .new_named_tensor(
            &format!("{name}.scale_p1"),
            TensorType::F32,
            src_t.desc.layout.rank(),
            &src_t.ne[..src_t.desc.layout.rank()],
            BufferUsage::Weights,
        )
        .map_err(DiffusionError::model)?;
    ctx.write_tensor_data(dst, &baked)
        .map_err(DiffusionError::model)?;
    Ok(dst)
}

fn i32s_to_le_bytes(values: &[i32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
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
    let v = ctx
        .permute(v, [0, 2, 1, 3])
        .map_err(DiffusionError::model)?;
    // flash_attn_ext_tile assumes a contiguous D-major layout. A permute
    // view at mid-timestep AdaLN scales produced all-NaN preds (t=0.9);
    // t=0/1 happened to stay finite.
    let q = ctx.cont(q).map_err(DiffusionError::model)?;
    let k = ctx.cont(k).map_err(DiffusionError::model)?;
    let v = ctx.cont(v).map_err(DiffusionError::model)?;
    let attention_scale = 1.0 / (head_dim as f32).sqrt();

    if flux_flash_attention_allowed(head_dim) {
        let attn = ctx
            .flash_attn_ext(
                q,
                k,
                v,
                None,
                attention_scale,
                0.0,
                0.0,
                BufferUsage::Activations,
            )
            .map_err(DiffusionError::model)?;
        ctx.flash_attn_ext_set_prec(attn, makepad_ggml::Prec::F32)
            .map_err(DiffusionError::model)?;
        let attn_tensor = require_tensor(ctx, attn)?.clone();
        return ctx
            .reshape(
                attn,
                &[
                    attn_tensor.ne[0] * attn_tensor.ne[1],
                    attn_tensor.ne[2] * attn_tensor.ne[3],
                ],
            )
            .map_err(DiffusionError::model);
    }

    let mut kq = ctx
        .mul_mat(k, q, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    kq = ctx
        .soft_max_ext(kq, None, attention_scale, 0.0, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let v = ctx
        .permute(v, [1, 0, 2, 3])
        .map_err(DiffusionError::model)?;
    let v = ctx.cont(v).map_err(DiffusionError::model)?;
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

fn apply_head_rms_norm(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    scale_name: &str,
) -> Result<TensorId> {
    let scale = require_tensor_id(tensor_ids, scale_name)?;
    let scale = match_activation_type(ctx, scale, input)?;
    let norm = ctx
        .rms_norm_eps(input, FLUX_LAYER_NORM_EPSILON, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    // Vector scale: Metal bin/norm kernels broadcast. A Repeat here is a
    // full activation copy and blocks official RMS_NORM+MUL fusion.
    ctx.binary_like_a(Op::Mul, norm, scale, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn apply_modulated_layer_norm(
    ctx: &mut Context,
    input: TensorId,
    scale: TensorId,
    shift: TensorId,
) -> Result<TensorId> {
    // scale already has +1 baked into the modulation bias (see
    // bake_modulation_scale_plus_one). Stream is NORM, MUL, ADD —
    // official kernel_norm_mul_add.
    let scale = match_activation_type(ctx, scale, input)?;
    let shift = match_activation_type(ctx, shift, input)?;
    let norm = ctx
        .norm_eps(input, FLUX_LAYER_NORM_EPSILON, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let scaled = ctx
        .binary_like_a(Op::Mul, norm, scale, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    ctx.binary_like_a(Op::Add, scaled, shift, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn gated_residual(
    ctx: &mut Context,
    residual: TensorId,
    update: TensorId,
    gate: TensorId,
) -> Result<TensorId> {
    let update = match_activation_type(ctx, update, residual)?;
    let gate = match_activation_type(ctx, gate, residual)?;
    let update = ctx
        .binary_like_a(Op::Mul, update, gate, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    ctx.binary_like_a(Op::Add, residual, update, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn silu(ctx: &mut Context, input: TensorId) -> Result<TensorId> {
    ctx.unary(input, UnaryOp::Silu, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn gelu(ctx: &mut Context, input: TensorId) -> Result<TensorId> {
    ctx.unary(input, UnaryOp::Gelu, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn apply_linear(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight_name: &str,
    bias_name: &str,
) -> Result<TensorId> {
    let weight = concat_linear_parts(ctx, tensor_ids, weight_name)?;
    let out = ctx
        .mul_mat(weight, input, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let bias = concat_linear_parts(ctx, tensor_ids, bias_name)?;
    ctx.binary_like_a(Op::Add, out, bias, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn concat_linear_parts(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    base_name: &str,
) -> Result<TensorId> {
    let mut merged = require_tensor_id(tensor_ids, base_name)?;
    let concat_dim = match require_tensor(ctx, merged)?.desc.layout.rank() {
        1 => 0,
        2 => 1,
        rank => {
            return Err(DiffusionError::model(format!(
                "flux linear parameter '{}' has unsupported rank {}",
                base_name, rank
            )))
        }
    };

    let mut suffix_index = 1usize;
    loop {
        let part_name = format!("{base_name}.{suffix_index}");
        let Some(&part) = tensor_ids.get(&part_name) else {
            break;
        };
        merged = ctx
            .concat(merged, part, concat_dim, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
        suffix_index += 1;
    }

    Ok(merged)
}

fn slice_rows_2d(ctx: &mut Context, input: TensorId, start: i64, len: i64) -> Result<TensorId> {
    let tensor = require_tensor(ctx, input)?.clone();
    let offset = usize::try_from(start)
        .map_err(|_| DiffusionError::model("flux row slice start is negative"))?
        .checked_mul(tensor.nb[0])
        .ok_or_else(|| DiffusionError::model("flux row slice offset overflow"))?;
    let view = ctx
        .view_2d(input, len, tensor.ne[1], tensor.nb[1], offset)
        .map_err(DiffusionError::model)?;
    if require_tensor(ctx, view)?.is_contiguous() {
        return Ok(view);
    }
    ctx.cont_2d(view, len, tensor.ne[1])
        .map_err(DiffusionError::model)
}

fn slice_cols_2d(ctx: &mut Context, input: TensorId, start: i64, len: i64) -> Result<TensorId> {
    let tensor = require_tensor(ctx, input)?.clone();
    let offset = usize::try_from(start)
        .map_err(|_| DiffusionError::model("flux col slice start is negative"))?
        .checked_mul(tensor.nb[1])
        .ok_or_else(|| DiffusionError::model("flux col slice offset overflow"))?;
    let view = ctx
        .view_2d(input, tensor.ne[0], len, tensor.nb[1], offset)
        .map_err(DiffusionError::model)?;
    if require_tensor(ctx, view)?.is_contiguous() {
        return Ok(view);
    }
    ctx.cont_2d(view, tensor.ne[0], len)
        .map_err(DiffusionError::model)
}

fn allocate_flux_weight_tensors(
    ctx: &mut Context,
    header: &MlxSafetensorsHeader,
) -> Result<BTreeMap<String, TensorId>> {
    let mut tensor_ids = BTreeMap::new();
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let entry = header.tensor(&name).ok_or_else(|| {
            DiffusionError::model(format!(
                "flux transformer header lost tensor '{}' while allocating",
                name
            ))
        })?;
        let canonical = canonicalize_flux_diffusion_tensor_name(&name);
        let ty = flux_target_tensor_type(entry)?;
        let extents = flux_target_extents(entry)?;
        let id = ctx
            .new_named_tensor(
                canonical.clone(),
                ty,
                extents.len(),
                &extents,
                BufferUsage::Weights,
            )
            .map_err(DiffusionError::model)?;
        if tensor_ids.insert(canonical.clone(), id).is_some() {
            return Err(DiffusionError::model(format!(
                "duplicate canonical flux tensor name '{}'",
                canonical
            )));
        }
    }
    Ok(tensor_ids)
}

fn load_flux_weight_bytes(
    ctx: &mut Context,
    header: &MlxSafetensorsHeader,
    tensor_ids: &BTreeMap<String, TensorId>,
    progress: &mut Option<ProgressHook>,
) -> Result<()> {
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    let total_bytes = flux_weight_total_bytes(header, 0)?;
    let mut done_bytes = 0usize;
    let mut last_emit = 0usize;
    emit_byte_progress(progress, "load unet", 0, total_bytes)?;
    for name in names {
        let entry = header.tensor(&name).ok_or_else(|| {
            DiffusionError::model(format!("flux transformer header missing tensor '{}'", name))
        })?;
        let canonical = canonicalize_flux_diffusion_tensor_name(&name);
        let tensor_id = tensor_ids.get(&canonical).copied().ok_or_else(|| {
            DiffusionError::model(format!(
                "flux transformer missing canonical tensor '{}'",
                canonical
            ))
        })?;
        let bytes = flux_target_bytes(header, &name, entry)?;
        ctx.write_tensor_data(tensor_id, &bytes)
            .map_err(DiffusionError::model)?;
        done_bytes = done_bytes.saturating_add(flux_target_nbytes(entry)?);
        if done_bytes - last_emit >= crate::BYTE_PROGRESS_STEP {
            last_emit = done_bytes;
            emit_byte_progress(progress, "load unet", done_bytes, total_bytes)?;
        }
    }
    Ok(())
}

fn flux_weight_total_bytes(header: &MlxSafetensorsHeader, extra_bytes: usize) -> Result<usize> {
    let mut total = 0usize;
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let entry = header.tensor(&name).unwrap();
        total = ggml_pad(total, GGML_MEM_ALIGN);
        total = total
            .checked_add(flux_target_nbytes(entry)?)
            .ok_or_else(|| {
                DiffusionError::model(format!("flux total bytes overflow at '{}'", name))
            })?;
    }
    total = ggml_pad(total, GGML_MEM_ALIGN);
    total
        .checked_add(extra_bytes)
        .ok_or_else(|| DiffusionError::model("flux transformer context size overflow"))
}

fn flux_target_nbytes(entry: &MlxTensorEntry) -> Result<usize> {
    let ty = flux_target_tensor_type(entry)?;
    let extents = flux_target_extents(entry)?;
    let layout = TensorLayout::for_ggml(ty, &extents).map_err(DiffusionError::model)?;
    Ok(Tensor::from_desc(0, TensorDesc::new(ty, layout, BufferUsage::Weights)).nbytes())
}

fn flux_target_extents(entry: &MlxTensorEntry) -> Result<Vec<i64>> {
    match entry.shape.as_slice() {
        [dim] => Ok(vec![i64::try_from(*dim).map_err(|_| {
            DiffusionError::model(format!("flux extent {} exceeds i64", dim))
        })?]),
        [dim0, dim1] => Ok(vec![
            i64::try_from(*dim1)
                .map_err(|_| DiffusionError::model(format!("flux extent {} exceeds i64", dim1)))?,
            i64::try_from(*dim0)
                .map_err(|_| DiffusionError::model(format!("flux extent {} exceeds i64", dim0)))?,
        ]),
        other => Err(DiffusionError::model(format!(
            "flux transformer only supports rank1/rank2 tensors today, got {:?}",
            other
        ))),
    }
}

fn flux_target_tensor_type(entry: &MlxTensorEntry) -> Result<TensorType> {
    if entry.shape.len() == 1 {
        return Ok(TensorType::F32);
    }
    if flux_force_f32_weights() {
        return Ok(TensorType::F32);
    }
    match entry.dtype {
        MlxDType::BF16 => Ok(TensorType::BF16),
        MlxDType::F16 => Ok(TensorType::F16),
        MlxDType::F32 => Ok(TensorType::F32),
        // Combined-FP8 checkpoints: matrices stay raw 1-byte resident
        // (host and device); rank-1 tensors promoted to F32 above.
        MlxDType::F8E4M3 => Ok(TensorType::F8E4M3),
        other => Err(DiffusionError::model(format!(
            "flux transformer unsupported tensor dtype {:?}",
            other
        ))),
    }
}

fn flux_target_bytes(
    header: &MlxSafetensorsHeader,
    name: &str,
    entry: &MlxTensorEntry,
) -> Result<Vec<u8>> {
    let bytes = header.read_tensor_bytes(name)?;
    if entry.shape.len() == 1 {
        return match entry.dtype {
            MlxDType::F32 => Ok(bytes),
            MlxDType::F16 => f16_bytes_to_f32_bytes(&bytes),
            MlxDType::BF16 => bf16_bytes_to_f32_bytes(&bytes),
            MlxDType::F8E4M3 => f8_bytes_to_f32_bytes_checked(name, &bytes),
            other => Err(DiffusionError::model(format!(
                "flux transformer unsupported rank1 dtype {:?}",
                other
            ))),
        };
    }
    match entry.dtype {
        MlxDType::BF16 if flux_force_f32_weights() => bf16_bytes_to_f32_bytes(&bytes),
        MlxDType::F16 if flux_force_f32_weights() => f16_bytes_to_f32_bytes(&bytes),
        MlxDType::F8E4M3 if flux_force_f32_weights() => {
            f8_bytes_to_f32_bytes_checked(name, &bytes)
        }
        MlxDType::F8E4M3 => {
            // Raw resident payload: reject the two NaN encodings up front —
            // fail closed at load rather than propagate NaN activations.
            reject_f8_nan_bytes(name, &bytes)?;
            Ok(bytes)
        }
        MlxDType::F32 | MlxDType::F16 | MlxDType::BF16 => Ok(bytes),
        other => Err(DiffusionError::model(format!(
            "flux transformer unsupported tensor dtype {:?}",
            other
        ))),
    }
}

/// Fail-closed NaN screen for raw E4M3FN payloads: 0x7f/0xff are the only
/// NaN encodings (the format has no infinities); a checkpoint containing one
/// is corrupt and must never reach the device.
fn reject_f8_nan_bytes(name: &str, bytes: &[u8]) -> Result<()> {
    if let Some(position) = bytes
        .iter()
        .position(|&byte| byte == 0x7f || byte == 0xff)
    {
        return Err(DiffusionError::model(format!(
            "flux tensor '{}' contains E4M3FN NaN byte {:#04x} at offset {} — checkpoint rejected",
            name, bytes[position], position
        )));
    }
    Ok(())
}

fn f8_bytes_to_f32_bytes_checked(name: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    reject_f8_nan_bytes(name, bytes)?;
    let mut out = Vec::with_capacity(bytes.len() * 4);
    for &byte in bytes {
        out.extend_from_slice(&f8_e4m3_to_f32(byte).to_le_bytes());
    }
    Ok(out)
}

fn flux_force_f32_weights() -> bool {
    std::env::var_os("FLUX_FORCE_F32_WEIGHTS").is_some()
}

#[cfg(test)]
fn flux_position_ids(text_token_count: usize, latent_shape: FluxLatentShape) -> Result<Vec<i32>> {
    let token_count = text_token_count + latent_shape.image_token_count as usize;
    let mut ids = vec![0i32; token_count * 3];
    let packed_width = latent_shape.packed_width as usize;
    let packed_height = latent_shape.packed_height as usize;
    let axis_stride = token_count;
    let mut token_index = text_token_count;
    for row in 0..packed_height {
        for col in 0..packed_width {
            ids[token_index + axis_stride] = i32::try_from(row)
                .map_err(|_| DiffusionError::workflow("flux row position exceeds i32"))?;
            ids[token_index + axis_stride * 2] = i32::try_from(col)
                .map_err(|_| DiffusionError::workflow("flux col position exceeds i32"))?;
            token_index += 1;
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flux_position_ids_are_axis_major_for_mrope() {
        let latent_shape = FluxLatentShape::from_image_size(32, 16).unwrap();

        let ids = flux_position_ids(2, latent_shape).unwrap();

        assert_eq!(
            ids,
            vec![
                0, 0, 0, 0, // t axis
                0, 0, 0, 0, // h axis
                0, 0, 0, 1, // w axis
            ]
        );
    }
}

fn f16_bytes_to_f32_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "flux F16 bytes length {} is not even",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for chunk in bytes.chunks_exact(2) {
        out.extend_from_slice(&f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])).to_le_bytes());
    }
    Ok(out)
}

fn bf16_bytes_to_f32_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "flux BF16 bytes length {} is not even",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for chunk in bytes.chunks_exact(2) {
        out.extend_from_slice(&bf16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])).to_le_bytes());
    }
    Ok(out)
}

fn f16_bytes_to_f32_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "flux F16 bytes length {} is not even",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
        .collect())
}

fn bf16_bytes_to_f32_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "flux BF16 bytes length {} is not even",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| bf16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
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
    let sum: f64 = values
        .iter()
        .filter(|v| v.is_finite())
        .map(|v| f64::from(*v))
        .sum();
    let first = values.first().copied().unwrap_or(0.0);
    let last = values.last().copied().unwrap_or(0.0);
    format!(
        "n={} finite={finite} nan={nan} inf={inf} min={min} max={max} sum={sum:.6} first={first} last={last}",
        values.len()
    )
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
            "flux output byte length {} is not divisible by 4",
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
        .ok_or_else(|| DiffusionError::model(format!("missing flux resident tensor '{}'", name)))
}

fn require_tensor<'a>(ctx: &'a Context, id: TensorId) -> Result<&'a Tensor> {
    ctx.tensor(id)
        .ok_or_else(|| DiffusionError::model(format!("invalid flux tensor id {}", id)))
}

fn tensor_extents_usize(tensor: &Tensor) -> Result<[usize; 4]> {
    Ok([
        usize::try_from(tensor.ne[0])
            .map_err(|_| DiffusionError::model("flux tensor extent ne0 exceeds usize"))?,
        usize::try_from(tensor.ne[1])
            .map_err(|_| DiffusionError::model("flux tensor extent ne1 exceeds usize"))?,
        usize::try_from(tensor.ne[2])
            .map_err(|_| DiffusionError::model("flux tensor extent ne2 exceeds usize"))?,
        usize::try_from(tensor.ne[3])
            .map_err(|_| DiffusionError::model("flux tensor extent ne3 exceeds usize"))?,
    ])
}

fn flux_flash_attention_allowed(head_dim: u32) -> bool {
    if matches!(
        std::env::var("FLUX_FLASH").ok().as_deref(),
        Some("0") | Some("off") | Some("OFF")
    ) {
        return false;
    }
    // Official ggml Metal flash_attn_ext has f32 dk128/dv128. Naive
    // softmax attention was a temporary disable and dominated step time
    // once weights were Metal-resident.
    matches!(head_dim, 128)
}

fn is_context_oom(err: &DiffusionError) -> bool {
    matches!(err, DiffusionError::Model(message) if message.starts_with("context out of memory allocating "))
}

fn next_graph_reserve_bytes(weights: &LoadedFluxTransformerWeights) -> Result<usize> {
    weights
        .graph_reserve_bytes()
        .checked_mul(2)
        .ok_or_else(|| DiffusionError::model("flux graph reserve overflow"))
}
