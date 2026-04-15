use crate::backend::{
    create_graph_session, new_runtime, prepare_graph, runtime_available, try_add_f32,
    try_attention_softmax_weighted_sum_f32, try_flash_attn_f32_packed, try_gelu_f32,
    try_layer_norm_mul_add_f32, try_matmul_nn_f32, try_matmul_nt_f32, try_mul_f32,
    try_rms_norm_mul_f32,
    BufferStorageMode, GraphSession, GraphTensorWrite, Runtime,
};
use crate::flux::{
    canonicalize_flux_diffusion_tensor_name, FluxLatentShape, FluxTransformerConfig,
    FluxTransformerInspection,
};
use crate::flux_text::FluxConditioning;
use crate::{DiffusionError, Result};
use makepad_ggml::backend::try_matmul_nt_ggml_bytes_cached;
use makepad_ggml::{
    bf16_to_f32, f16_to_f32, ggml_pad, BufferUsage, Context, GluOp, Graph, InitParams, Op, Tensor,
    TensorDesc, TensorId, TensorLayout, TensorType, GGML_MEM_ALIGN,
};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry};
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

    pub fn load_with_extra(path: impl AsRef<Path>, extra_bytes: usize) -> Result<Self> {
        let header = MlxSafetensorsHeader::load(path.as_ref())?;
        let inspect = FluxTransformerInspection::from_header(&header)?;
        let total_bytes = flux_weight_total_bytes(&header, extra_bytes)?;
        let mut ctx = Context::new(InitParams {
            mem_size: total_bytes,
            mem_buffer: None,
            no_alloc: false,
        });
        let tensor_ids = allocate_flux_weight_tensors(&mut ctx, &header)?;
        load_flux_weight_bytes(&mut ctx, &header, &tensor_ids)?;

        Ok(Self {
            ctx,
            tensor_ids,
            config: inspect.config,
            path: header.path,
            graph_extra_bytes: extra_bytes,
        })
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
            FluxTransformerExecutionMode::from_env(),
            None,
            weights,
            conditioning,
            latent_shape,
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
            FluxTransformerExecutionMode::from_env(),
            Some(runtime),
            weights,
            conditioning,
            latent_shape,
        )
    }

    fn compile_for_mode(
        mode: FluxTransformerExecutionMode,
        runtime: Option<Runtime>,
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
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
                    Self::compile_graph(runtime, weights, conditioning, latent_shape)?;
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
    ) -> Result<(CompiledFluxTransformerGraph, FluxTransformerCompileTiming)> {
        for attempt in 0..=MAX_GRAPH_GROWTH_ATTEMPTS {
            let build_start = Instant::now();
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
            let prepared = prepare_graph(&runtime, &weights.ctx, &graph.graph)?;
            let graph_prepare_ms = prepare_start.elapsed().as_secs_f64() * 1000.0;
            let session_start = Instant::now();
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
        match &self.inner {
            FluxTransformerExecutor::Compiled(compiled) => {
                compiled.execute(weights, conditioning, packed_latents, timestep, guidance)
            }
            FluxTransformerExecutor::Lazy(lazy) => {
                lazy.execute(weights, conditioning, packed_latents, timestep, guidance)
            }
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

        Ok(FluxTransformerRun {
            prediction: f32_bytes_to_vec(prediction_bytes)?,
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

    fn execute(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
    ) -> Result<FluxTransformerRun> {
        Ok(self
            .execute_internal(weights, conditioning, packed_latents, timestep, guidance, false)?
            .run)
    }

    fn execute_with_debug(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
    ) -> Result<FluxTransformerDebugRun> {
        self.execute_internal(weights, conditioning, packed_latents, timestep, guidance, true)
    }

    fn execute_internal(
        &self,
        weights: &LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        packed_latents: &[f32],
        timestep: f32,
        guidance: f32,
        capture_debug: bool,
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

        for layer in 0..weights.config.depth as usize {
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

fn flux_cache_namespace(weights: &LoadedFluxTransformerWeights) -> String {
    format!("flux_transformer:{}", weights.path.display())
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
    let (rope_cos, rope_sin) = build_flux_rope_tables(
        &mut weights.ctx,
        text_token_count,
        latent_shape,
        weights.config,
    )?;
    let ones_hidden = weights
        .ctx
        .new_named_tensor(
            "flux.ones_hidden",
            TensorType::F32,
            2,
            &[hidden_size, 1],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    weights
        .ctx
        .write_tensor_data(
            ones_hidden,
            &f32s_to_le_bytes(&vec![1.0f32; hidden_size as usize]),
        )
        .map_err(DiffusionError::model)?;
    let mut debug_tensors = Vec::new();

    let mut hidden = apply_linear(
        &mut weights.ctx,
        &weights.tensor_ids,
        input_packed_latents,
        "img_in.weight",
        "img_in.bias",
    )?;
    push_debug_tensor(&mut weights.ctx, &mut debug_tensors, "input.hidden", hidden)?;
    let mut encoder_hidden = apply_linear(
        &mut weights.ctx,
        &weights.tensor_ids,
        input_encoder_hidden_states,
        "txt_in.weight",
        "txt_in.bias",
    )?;
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
            temb,
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
            temb,
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
            ones_hidden,
        )?;
        let norm_encoder_hidden = apply_modulated_layer_norm(
            &mut weights.ctx,
            encoder_hidden,
            txt_scale_msa,
            txt_shift_msa,
            ones_hidden,
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
        let q = apply_flux_rope(&mut weights.ctx, q, rope_cos, rope_sin, weights.config)?;
        let k = apply_flux_rope(&mut weights.ctx, k, rope_cos, rope_sin, weights.config)?;
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
            ones_hidden,
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
            ones_hidden,
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

    for layer in 0..weights.config.depth_single_blocks as usize {
        let prefix = format!("single_blocks.{layer}");
        let joint = weights
            .ctx
            .concat(encoder_hidden, hidden, 1, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
        let (shift, scale, gate, _, _, _) = modulation_chunks(
            &mut weights.ctx,
            &weights.tensor_ids,
            temb,
            &format!("{prefix}.modulation.lin.weight"),
            &format!("{prefix}.modulation.lin.bias"),
            hidden_size as usize,
            3,
        )?;
        let norm_joint =
            apply_modulated_layer_norm(&mut weights.ctx, joint, scale, shift, ones_hidden)?;
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
        let q = slice_rows_2d(&mut weights.ctx, linear1, 0, hidden_size)?;
        let k = slice_rows_2d(&mut weights.ctx, linear1, hidden_size, hidden_size)?;
        let v = slice_rows_2d(&mut weights.ctx, linear1, hidden_size * 2, hidden_size)?;
        let mlp = slice_rows_2d(&mut weights.ctx, linear1, hidden_size * 3, hidden_size * 4)?;

        let total_token_count = text_token_count + image_token_count;
        let q = weights
            .ctx
            .reshape(q, &[head_dim, head_count, total_token_count as i64])
            .map_err(DiffusionError::model)?;
        let k = weights
            .ctx
            .reshape(k, &[head_dim, head_count, total_token_count as i64])
            .map_err(DiffusionError::model)?;
        let v = weights
            .ctx
            .reshape(v, &[head_dim, head_count, total_token_count as i64])
            .map_err(DiffusionError::model)?;
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
        let q = apply_flux_rope(&mut weights.ctx, q, rope_cos, rope_sin, weights.config)?;
        let k = apply_flux_rope(&mut weights.ctx, k, rope_cos, rope_sin, weights.config)?;
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
        let joint = gated_residual(&mut weights.ctx, joint, proj, gate)?;
        if trace_single_block(layer, weights.config.depth_single_blocks as usize) {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                &format!("single_blocks.{layer}.joint"),
                joint,
            )?;
        }
        encoder_hidden = slice_cols_2d(&mut weights.ctx, joint, 0, text_token_count as i64)?;
        hidden = slice_cols_2d(
            &mut weights.ctx,
            joint,
            text_token_count as i64,
            image_token_count as i64,
        )?;
        if trace_single_block(layer, weights.config.depth_single_blocks as usize) {
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                &format!("single_blocks.{layer}.hidden"),
                hidden,
            )?;
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                &format!("single_blocks.{layer}.encoder_hidden"),
                encoder_hidden,
            )?;
        }
    }

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
        ones_hidden,
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

    Ok(FluxTransformerGraph {
        graph,
        input_packed_latents,
        input_encoder_hidden_states,
        input_pooled_projections,
        input_timestep,
        input_guidance,
        result_prediction,
        image_token_count,
        debug_tensors,
    })
}

fn push_debug_tensor(
    ctx: &mut Context,
    debug_tensors: &mut Vec<FluxTransformerDebugTensor>,
    name: &str,
    tensor_id: TensorId,
) -> Result<()> {
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
    let temb = silu(ctx, temb)?;
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
    let q = slice_rows_2d(ctx, qkv, 0, hidden_size as i64)?;
    let k = slice_rows_2d(ctx, qkv, hidden_size as i64, hidden_size as i64)?;
    let v = slice_rows_2d(ctx, qkv, (hidden_size * 2) as i64, hidden_size as i64)?;
    let q = ctx
        .reshape(q, &[head_dim, head_count, token_count])
        .map_err(DiffusionError::model)?;
    let k = ctx
        .reshape(k, &[head_dim, head_count, token_count])
        .map_err(DiffusionError::model)?;
    let v = ctx
        .reshape(v, &[head_dim, head_count, token_count])
        .map_err(DiffusionError::model)?;
    Ok((q, k, v))
}

fn apply_flux_rope(
    ctx: &mut Context,
    tensor: TensorId,
    rope_cos: TensorId,
    rope_sin: TensorId,
    config: FluxTransformerConfig,
) -> Result<TensorId> {
    let head_dim = i64::from(config.head_dim());
    let packed = pack_rope_interleaved_pairs(ctx, tensor, head_dim)?;
    let packed_ref = require_tensor(ctx, packed)?.clone();
    let half_dim = head_dim / 2;
    let x0 = ctx
        .view_3d(
            packed,
            half_dim,
            packed_ref.ne[1],
            packed_ref.ne[2],
            packed_ref.nb[1],
            packed_ref.nb[2],
            0,
        )
        .map_err(DiffusionError::model)?;
    let x0 = ctx
        .cont_3d(x0, half_dim, packed_ref.ne[1], packed_ref.ne[2])
        .map_err(DiffusionError::model)?;
    let x1 = ctx
        .view_3d(
            packed,
            half_dim,
            packed_ref.ne[1],
            packed_ref.ne[2],
            packed_ref.nb[1],
            packed_ref.nb[2],
            usize::try_from(half_dim)
                .map_err(|_| DiffusionError::model("flux rope half dim exceeds usize"))?
                .checked_mul(packed_ref.nb[0])
                .ok_or_else(|| DiffusionError::model("flux rope split offset overflow"))?,
        )
        .map_err(DiffusionError::model)?;
    let x1 = ctx
        .cont_3d(x1, half_dim, packed_ref.ne[1], packed_ref.ne[2])
        .map_err(DiffusionError::model)?;
    let rope_cos = ctx
        .repeat_4d(
            rope_cos,
            half_dim,
            packed_ref.ne[1],
            packed_ref.ne[2],
            1,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let rope_sin = ctx
        .repeat_4d(
            rope_sin,
            half_dim,
            packed_ref.ne[1],
            packed_ref.ne[2],
            1,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let x0_cos = ctx
        .binary_like_a(Op::Mul, x0, rope_cos, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let x0_sin = ctx
        .binary_like_a(Op::Mul, x0, rope_sin, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let x1_cos = ctx
        .binary_like_a(Op::Mul, x1, rope_cos, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let x1_sin = ctx
        .binary_like_a(Op::Mul, x1, rope_sin, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let out0 = ctx
        .binary_like_a(Op::Sub, x0_cos, x1_sin, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let out1 = ctx
        .binary_like_a(Op::Add, x0_sin, x1_cos, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let rotated = ctx
        .concat(out0, out1, 0, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    unpack_rope_interleaved_pairs(ctx, rotated, head_dim)
}

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

fn build_flux_rope_tables(
    ctx: &mut Context,
    text_token_count: usize,
    latent_shape: FluxLatentShape,
    config: FluxTransformerConfig,
) -> Result<(TensorId, TensorId)> {
    let tables = flux_rope_table_values(text_token_count, latent_shape, config)?;

    let cos_tensor = ctx
        .new_named_tensor(
            "flux.rope_cos",
            TensorType::F32,
            3,
            &[tables.half_dim as i64, 1, tables.token_count as i64],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    ctx.write_tensor_data(cos_tensor, &f32s_to_le_bytes(&tables.cos))
        .map_err(DiffusionError::model)?;

    let sin_tensor = ctx
        .new_named_tensor(
            "flux.rope_sin",
            TensorType::F32,
            3,
            &[tables.half_dim as i64, 1, tables.token_count as i64],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    ctx.write_tensor_data(sin_tensor, &f32s_to_le_bytes(&tables.sin))
        .map_err(DiffusionError::model)?;

    Ok((cos_tensor, sin_tensor))
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
    let norm = ctx
        .rms_norm_eps(input, FLUX_LAYER_NORM_EPSILON, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let scale = repeat_weight(ctx, require_tensor_id(tensor_ids, scale_name)?, norm)?;
    ctx.binary_like_a(Op::Mul, norm, scale, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn apply_modulated_layer_norm(
    ctx: &mut Context,
    input: TensorId,
    scale: TensorId,
    shift: TensorId,
    ones_hidden: TensorId,
) -> Result<TensorId> {
    let norm = ctx
        .norm_eps(input, FLUX_LAYER_NORM_EPSILON, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let scale_plus_one = ctx
        .binary_like_a(Op::Add, scale, ones_hidden, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let scale = repeat_weight(ctx, scale_plus_one, norm)?;
    let shift = repeat_weight(ctx, shift, norm)?;
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
    let gate = repeat_weight(ctx, gate, update)?;
    let update = ctx
        .binary_like_a(Op::Mul, update, gate, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    ctx.binary_like_a(Op::Add, residual, update, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn silu(ctx: &mut Context, input: TensorId) -> Result<TensorId> {
    apply_glu_activation(ctx, input, GluOp::Swiglu)
}

fn gelu(ctx: &mut Context, input: TensorId) -> Result<TensorId> {
    apply_glu_activation(ctx, input, GluOp::Geglu)
}

fn apply_glu_activation(ctx: &mut Context, input: TensorId, glu: GluOp) -> Result<TensorId> {
    let ones = repeat_scalar_one(ctx, input)?;
    ctx.glu_split(input, ones, glu, BufferUsage::Activations)
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
    let bias_tensor = concat_linear_parts(ctx, tensor_ids, bias_name)?;
    let bias = repeat_weight(ctx, bias_tensor, out)?;
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
    ctx.cont_2d(view, tensor.ne[0], len)
        .map_err(DiffusionError::model)
}

fn repeat_weight(ctx: &mut Context, weight: TensorId, shape_of: TensorId) -> Result<TensorId> {
    ctx.repeat(weight, shape_of, BufferUsage::Activations)
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
) -> Result<()> {
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
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
            other => Err(DiffusionError::model(format!(
                "flux transformer unsupported rank1 dtype {:?}",
                other
            ))),
        };
    }
    match entry.dtype {
        MlxDType::BF16 if flux_force_f32_weights() => bf16_bytes_to_f32_bytes(&bytes),
        MlxDType::F16 if flux_force_f32_weights() => f16_bytes_to_f32_bytes(&bytes),
        MlxDType::F32 | MlxDType::F16 | MlxDType::BF16 => Ok(bytes),
        other => Err(DiffusionError::model(format!(
            "flux transformer unsupported tensor dtype {:?}",
            other
        ))),
    }
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
    let _ = head_dim;
    false
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
