use crate::flux::{
    canonicalize_flux_diffusion_tensor_name, FluxLatentShape, FluxTransformerConfig,
    FluxTransformerInspection,
};
use crate::flux_text::FluxConditioning;
use crate::{DiffusionError, Result};
use makepad_ggml::backend::metal::{
    prepare_graph, BufferStorageMode, MetalGraphSession, MetalGraphTensorWrite, MetalRuntime,
};
use makepad_ggml::{
    bf16_to_f32, f16_to_f32, ggml_pad, BufferUsage, Context, GluOp, Graph, InitParams, Op,
    Tensor, TensorDesc, TensorId, TensorLayout, TensorType, GGML_MEM_ALIGN,
};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

pub struct CompiledFluxTransformerMetal {
    graph: FluxTransformerGraph,
    session: MetalGraphSession,
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
        self.tensor_ids
            .get(name)
            .copied()
            .ok_or_else(|| DiffusionError::model(format!("missing flux transformer tensor '{}'", name)))
    }

    fn graph_reserve_bytes(&self) -> usize {
        self.graph_extra_bytes
    }
}

impl CompiledFluxTransformerMetal {
    pub fn compile(
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        let runtime = MetalRuntime::new().map_err(DiffusionError::model)?;
        Self::compile_with_runtime(runtime, weights, conditioning, latent_shape)
    }

    pub fn compile_with_runtime(
        runtime: MetalRuntime,
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
    ) -> Result<Self> {
        Ok(
            Self::compile_with_runtime_profiled(runtime, weights, conditioning, latent_shape)?.0,
        )
    }

    pub fn compile_with_runtime_profiled(
        runtime: MetalRuntime,
        weights: &mut LoadedFluxTransformerWeights,
        conditioning: &FluxConditioning,
        latent_shape: FluxLatentShape,
    ) -> Result<(Self, FluxTransformerCompileTiming)> {
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
            let prepared = prepare_graph(&weights.ctx, &graph.graph, runtime.features())
                .map_err(DiffusionError::model)?;
            let graph_prepare_ms = prepare_start.elapsed().as_secs_f64() * 1000.0;
            let session_start = Instant::now();
            let session = MetalGraphSession::from_runtime(
                runtime.clone(),
                &weights.ctx,
                &prepared,
                BufferStorageMode::Shared,
                BufferStorageMode::Shared,
            )
            .map_err(DiffusionError::model)?;
            let session_create_ms = session_start.elapsed().as_secs_f64() * 1000.0;
            return Ok((
                Self { graph, session },
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

    pub fn execute(
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
            MetalGraphTensorWrite {
                tensor_id: self.graph.input_packed_latents,
                bytes: &packed_latents_bytes,
            },
            MetalGraphTensorWrite {
                tensor_id: self.graph.input_encoder_hidden_states,
                bytes: &encoder_hidden_bytes,
            },
            MetalGraphTensorWrite {
                tensor_id: self.graph.input_pooled_projections,
                bytes: &pooled_bytes,
            },
            MetalGraphTensorWrite {
                tensor_id: self.graph.input_timestep,
                bytes: &timestep_bytes,
            },
        ];
        if let Some(input_guidance) = self.graph.input_guidance {
            writes.push(MetalGraphTensorWrite {
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
            .ok_or_else(|| DiffusionError::model("flux transformer execution did not return output"))?;
        let output_tensor = require_tensor(&weights.ctx, self.graph.result_prediction)?;
        let channel_count = usize::try_from(output_tensor.ne[0])
            .map_err(|_| DiffusionError::model("flux transformer output channels exceed usize"))?;

        Ok(FluxTransformerRun {
            prediction: f32_bytes_to_vec(prediction_bytes)?,
            image_token_count: self.graph.image_token_count,
            channel_count,
        })
    }

    pub fn execute_with_debug(
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
            MetalGraphTensorWrite {
                tensor_id: self.graph.input_packed_latents,
                bytes: &packed_latents_bytes,
            },
            MetalGraphTensorWrite {
                tensor_id: self.graph.input_encoder_hidden_states,
                bytes: &encoder_hidden_bytes,
            },
            MetalGraphTensorWrite {
                tensor_id: self.graph.input_pooled_projections,
                bytes: &pooled_bytes,
            },
            MetalGraphTensorWrite {
                tensor_id: self.graph.input_timestep,
                bytes: &timestep_bytes,
            },
        ];
        if let Some(input_guidance) = self.graph.input_guidance {
            writes.push(MetalGraphTensorWrite {
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
            .ok_or_else(|| DiffusionError::model("flux transformer execution did not return output"))?;
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

pub fn build_flux_transformer_graph(
    weights: &mut LoadedFluxTransformerWeights,
    conditioning: &FluxConditioning,
    latent_shape: FluxLatentShape,
) -> Result<FluxTransformerGraph> {
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

    let text_token_count = conditioning.t5_token_count;
    let image_token_count = latent_shape.image_token_count as usize;
    let hidden_size = i64::from(weights.config.hidden_size);
    let head_count = i64::from(weights.config.num_heads);
    let head_dim = i64::from(weights.config.head_dim());
    if head_count * head_dim != hidden_size {
        return Err(DiffusionError::model(format!(
            "flux hidden size {} is incompatible with {} heads of {} dims",
            hidden_size, head_count, head_dim
        )));
    }

    let input_packed_latents = weights
        .ctx
        .new_named_tensor(
            "flux.input_packed_latents",
            TensorType::F32,
            2,
            &[i64::from(weights.config.in_channels), image_token_count as i64],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let input_encoder_hidden_states = weights
        .ctx
        .new_named_tensor(
            "flux.input_encoder_hidden_states",
            TensorType::F32,
            2,
            &[i64::from(weights.config.context_in_dim), text_token_count as i64],
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
    let (rope_cos, rope_sin) =
        build_flux_rope_tables(&mut weights.ctx, text_token_count, latent_shape, weights.config)?;
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
        let hidden_attn =
            slice_cols_2d(&mut weights.ctx, attn, text_token_count as i64, image_token_count as i64)?;
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
        hidden = gated_residual(
            &mut weights.ctx,
            hidden,
            hidden_attn,
            img_gate_msa,
        )?;
        encoder_hidden = gated_residual(
            &mut weights.ctx,
            encoder_hidden,
            encoder_attn,
            txt_gate_msa,
        )?;
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
        encoder_hidden = gated_residual(&mut weights.ctx, encoder_hidden, encoder_ff, txt_gate_mlp)?;
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
            push_debug_tensor(
                &mut weights.ctx,
                &mut debug_tensors,
                "single_blocks.0.v",
                v,
            )?;
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
        hidden = slice_cols_2d(&mut weights.ctx, joint, text_token_count as i64, image_token_count as i64)?;
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
    let scaled = ctx.scale(input, 1000.0, BufferUsage::Activations).map_err(DiffusionError::model)?;
    let embed = ctx
        .timestep_embedding(scaled, FLUX_TIMESTEP_EMBED_DIM, 10_000, BufferUsage::Activations)
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
        .reshape(tensor, &[2, head_dim / 2, tensor_ref.ne[1], tensor_ref.ne[2]])
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
        .reshape(tensor, &[head_dim / 2, 2, tensor_ref.ne[1], tensor_ref.ne[2]])
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

    let cos_tensor = ctx
        .new_named_tensor(
            "flux.rope_cos",
            TensorType::F32,
            3,
            &[half_dim as i64, 1, token_count as i64],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    ctx.write_tensor_data(cos_tensor, &f32s_to_le_bytes(&cos))
        .map_err(DiffusionError::model)?;

    let sin_tensor = ctx
        .new_named_tensor(
            "flux.rope_sin",
            TensorType::F32,
            3,
            &[half_dim as i64, 1, token_count as i64],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    ctx.write_tensor_data(sin_tensor, &f32s_to_le_bytes(&sin))
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
    let q = ctx.permute(q, [0, 2, 1, 3]).map_err(DiffusionError::model)?;
    let k = ctx.permute(k, [0, 2, 1, 3]).map_err(DiffusionError::model)?;
    let v = ctx.permute(v, [0, 2, 1, 3]).map_err(DiffusionError::model)?;
    let attention_scale = 1.0 / (head_dim as f32).sqrt();

    if flux_flash_attention_allowed(head_dim) {
        let attn = ctx
            .flash_attn_ext(q, k, v, None, attention_scale, 0.0, 0.0, BufferUsage::Activations)
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

    let mut kq = ctx.mul_mat(k, q, BufferUsage::Activations).map_err(DiffusionError::model)?;
    kq = ctx
        .soft_max_ext(
            kq,
            None,
            attention_scale,
            0.0,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let v = ctx.permute(v, [1, 0, 2, 3]).map_err(DiffusionError::model)?;
    let v = ctx.cont(v).map_err(DiffusionError::model)?;
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
    ctx.cont_2d(view, len, tensor.ne[1]).map_err(DiffusionError::model)
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
    ctx.cont_2d(view, tensor.ne[0], len).map_err(DiffusionError::model)
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
            .new_named_tensor(canonical.clone(), ty, extents.len(), &extents, BufferUsage::Weights)
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
            DiffusionError::model(format!(
                "flux transformer header missing tensor '{}'",
                name
            ))
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
            .ok_or_else(|| DiffusionError::model(format!("flux total bytes overflow at '{}'", name)))?;
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
        [dim] => Ok(vec![i64::try_from(*dim)
            .map_err(|_| DiffusionError::model(format!("flux extent {} exceeds i64", dim)))?]),
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

fn f32s_to_le_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn i32s_to_le_bytes(values: &[i32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<i32>());
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
