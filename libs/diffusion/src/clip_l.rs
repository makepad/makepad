use crate::backend::{
    compile_graph_session, new_runtime, runtime_available, try_attention_softmax_weighted_sum_f32,
    try_layer_norm_mul_add_f32, try_matmul_nn_f32, try_matmul_nt_f32, BufferStorageMode,
    GraphSession, GraphTensorWrite, Runtime,
};
use crate::clip::ClipTokenChunk;
use crate::flux::ClipLTextEncoderConfig;
use crate::{DiffusionError, Result};
use makepad_ggml::backend::{try_get_rows_ggml_bytes_cached, try_matmul_nt_ggml_bytes_cached};
use makepad_ggml::{
    bf16_to_f32, f16_to_f32, get_rows_ggml_bytes_cpu, ggml_pad, BufferUsage, Context, GluOp, Graph,
    InitParams, Op, Tensor, TensorDesc, TensorId, TensorLayout, TensorType, GGML_MEM_ALIGN,
};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CLIP_L_HEAD_DIM: u32 = 64;
const CLIP_L_LAYER_NORM_EPSILON: f32 = 1.0e-5;
const CLIP_L_QUICK_GELU_COEF: f32 = 1.702;
const DEFAULT_GRAPH_EXTRA_BYTES: usize = 256 << 20;
const MAX_GRAPH_GROWTH_ATTEMPTS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipLModelConfig {
    pub vocab_size: u32,
    pub max_position_embeddings: u32,
    pub hidden_size: u32,
    pub intermediate_size: u32,
    pub layer_count: u32,
    pub attention_head_count: u32,
    pub layer_norm_epsilon_bits: u32,
}

impl ClipLModelConfig {
    pub fn layer_norm_epsilon(&self) -> f32 {
        f32::from_bits(self.layer_norm_epsilon_bits)
    }
}

#[derive(Clone, Debug)]
pub struct LoadedClipLWeights {
    pub ctx: Context,
    pub tensor_ids: BTreeMap<String, TensorId>,
    pub config: ClipLModelConfig,
    pub path: PathBuf,
    graph_extra_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct ClipLGraph {
    pub graph: Graph,
    pub input_token_ids: TensorId,
    pub result_hidden_states: TensorId,
    pub result_pooled: TensorId,
    pub eos_index: usize,
}

pub struct CompiledClipL {
    inner: ClipLExecutor,
}

pub type CompiledClipLMetal = CompiledClipL;
pub type LazyClipLMetal = LazyClipL;

enum ClipLExecutor {
    Compiled(CompiledClipLGraph),
    Lazy(LazyClipL),
}

struct CompiledClipLGraph {
    graph: ClipLGraph,
    session: GraphSession,
}

#[derive(Clone, Debug)]
pub struct LazyClipL {
    token_count: usize,
    eos_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipLExecutionMode {
    Lazy,
    Compiled,
}

#[derive(Clone, Debug)]
pub struct ClipLRun {
    pub hidden_states: Vec<f32>,
    pub pooled: Vec<f32>,
    pub token_count: usize,
    pub hidden_size: usize,
    pub eos_index: usize,
}

#[derive(Clone, Debug)]
struct RowsTensor {
    rows: usize,
    cols: usize,
    data: Vec<f32>,
}

#[derive(Clone, Copy)]
struct ResidentMatrix<'a> {
    bytes: &'a [u8],
    ggml_type: u32,
    cols: usize,
    rows: usize,
}

impl LoadedClipLWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_extra(path, DEFAULT_GRAPH_EXTRA_BYTES)
    }

    pub fn load_with_extra(path: impl AsRef<Path>, extra_bytes: usize) -> Result<Self> {
        let header = MlxSafetensorsHeader::load(path.as_ref())?;
        let inspect = ClipLTextEncoderConfig::from_header(&header)?;
        let config = clip_model_config_from_inspection(&inspect)?;
        let total_bytes = clip_weight_total_bytes(&header, extra_bytes)?;
        let mut ctx = Context::new(InitParams {
            mem_size: total_bytes,
            mem_buffer: None,
            no_alloc: false,
        });
        let tensor_ids = allocate_clip_weight_tensors(&mut ctx, &header)?;
        load_clip_weight_bytes(&mut ctx, &header, &tensor_ids)?;

        Ok(Self {
            ctx,
            tensor_ids,
            config,
            path: header.path,
            graph_extra_bytes: extra_bytes,
        })
    }

    pub fn tensor_id(&self, name: &str) -> Result<TensorId> {
        self.tensor_ids
            .get(name)
            .copied()
            .ok_or_else(|| DiffusionError::model(format!("missing clip_l tensor '{}'", name)))
    }

    fn tensor_f32_values(&self, name: &str) -> Result<Vec<f32>> {
        let tensor_id = self.tensor_id(name)?;
        tensor_to_f32_vec(&self.ctx, tensor_id)
    }

    fn tensor_matrix(&self, name: &str) -> Result<ResidentMatrix<'_>> {
        resident_matrix(&self.ctx, self.tensor_id(name)?)
    }
}

impl CompiledClipL {
    pub fn compile(weights: &mut LoadedClipLWeights, chunk: &ClipTokenChunk) -> Result<Self> {
        Self::compile_for_mode(ClipLExecutionMode::from_env(), None, weights, chunk)
    }

    pub fn compile_with_runtime(
        runtime: Runtime,
        weights: &mut LoadedClipLWeights,
        chunk: &ClipTokenChunk,
    ) -> Result<Self> {
        Self::compile_for_mode(
            ClipLExecutionMode::from_env(),
            Some(runtime),
            weights,
            chunk,
        )
    }

    pub fn compile_for_mode(
        mode: ClipLExecutionMode,
        runtime: Option<Runtime>,
        weights: &mut LoadedClipLWeights,
        chunk: &ClipTokenChunk,
    ) -> Result<Self> {
        match mode {
            ClipLExecutionMode::Lazy => Ok(Self {
                inner: ClipLExecutor::Lazy(LazyClipL::compile(weights, chunk)?),
            }),
            ClipLExecutionMode::Compiled => {
                let runtime = match runtime {
                    Some(runtime) => runtime,
                    None => new_runtime()?,
                };
                Self::compile_graph(runtime, weights, chunk)
            }
        }
    }

    fn compile_graph(
        runtime: Runtime,
        weights: &mut LoadedClipLWeights,
        chunk: &ClipTokenChunk,
    ) -> Result<Self> {
        for attempt in 0..=MAX_GRAPH_GROWTH_ATTEMPTS {
            let graph = match build_clip_l_graph(weights, chunk) {
                Ok(graph) => graph,
                Err(err) if is_context_oom(&err) && attempt < MAX_GRAPH_GROWTH_ATTEMPTS => {
                    let next_extra = next_graph_reserve_bytes(weights)?;
                    *weights =
                        LoadedClipLWeights::load_with_extra(weights.path.clone(), next_extra)?;
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
                inner: ClipLExecutor::Compiled(CompiledClipLGraph { graph, session }),
            });
        }

        Err(DiffusionError::model(
            "clip_l graph compilation exhausted context growth attempts",
        ))
    }

    pub fn backend_name(&self) -> &'static str {
        match &self.inner {
            ClipLExecutor::Compiled(_) => ClipLExecutionMode::Compiled.as_str(),
            ClipLExecutor::Lazy(_) => ClipLExecutionMode::Lazy.as_str(),
        }
    }

    pub fn execute(&self, weights: &LoadedClipLWeights, token_ids: &[i32]) -> Result<ClipLRun> {
        match &self.inner {
            ClipLExecutor::Compiled(compiled) => compiled.execute(weights, token_ids),
            ClipLExecutor::Lazy(lazy) => lazy.execute(weights, token_ids),
        }
    }
}

impl CompiledClipLGraph {
    fn execute(&self, weights: &LoadedClipLWeights, token_ids: &[i32]) -> Result<ClipLRun> {
        let input_tensor = require_tensor(&weights.ctx, self.graph.input_token_ids)?;
        if input_tensor.ne[0] as usize != token_ids.len() {
            return Err(DiffusionError::workflow(format!(
                "clip_l token length mismatch: graph expects {}, got {}",
                input_tensor.ne[0],
                token_ids.len()
            )));
        }
        let input_bytes = i32s_to_le_bytes(token_ids);

        let execution = self
            .session
            .execute(
                &weights.ctx,
                &[GraphTensorWrite {
                    tensor_id: self.graph.input_token_ids,
                    bytes: &input_bytes,
                }],
                &[self.graph.result_hidden_states],
            )
            .map_err(DiffusionError::model)?;

        let hidden_bytes = execution
            .outputs
            .get(&self.graph.result_hidden_states)
            .ok_or_else(|| {
                DiffusionError::model("clip_l execution did not return hidden states")
            })?;

        let hidden_tensor = require_tensor(&weights.ctx, self.graph.result_hidden_states)?;
        let hidden_size = usize::try_from(hidden_tensor.ne[0])
            .map_err(|_| DiffusionError::model("clip_l hidden size exceeds usize"))?;
        let token_count = usize::try_from(hidden_tensor.ne[1])
            .map_err(|_| DiffusionError::model("clip_l token count exceeds usize"))?;
        let hidden_states = f32_bytes_to_vec(hidden_bytes)?;
        let pooled = pooled_from_hidden_states(
            &hidden_states,
            hidden_size,
            token_count,
            self.graph.eos_index,
        )?;

        Ok(ClipLRun {
            hidden_states,
            pooled,
            token_count,
            hidden_size,
            eos_index: self.graph.eos_index,
        })
    }
}

impl LazyClipL {
    pub fn compile(weights: &mut LoadedClipLWeights, chunk: &ClipTokenChunk) -> Result<Self> {
        Self::compile_internal(weights, chunk)
    }

    pub fn compile_with_runtime(
        _runtime: Runtime,
        weights: &mut LoadedClipLWeights,
        chunk: &ClipTokenChunk,
    ) -> Result<Self> {
        Self::compile_internal(weights, chunk)
    }

    fn compile_internal(_weights: &mut LoadedClipLWeights, chunk: &ClipTokenChunk) -> Result<Self> {
        if chunk.token_ids.is_empty() {
            return Err(DiffusionError::workflow(
                "clip_l lazy executor needs at least one token",
            ));
        }
        if chunk.eos_index >= chunk.token_ids.len() {
            return Err(DiffusionError::workflow(format!(
                "clip_l eos index {} is out of range for {} tokens",
                chunk.eos_index,
                chunk.token_ids.len()
            )));
        }
        Ok(Self {
            token_count: chunk.token_ids.len(),
            eos_index: chunk.eos_index,
        })
    }

    pub fn execute(&self, weights: &LoadedClipLWeights, token_ids: &[i32]) -> Result<ClipLRun> {
        if token_ids.len() != self.token_count {
            return Err(DiffusionError::workflow(format!(
                "clip_l token length mismatch: executor expects {}, got {}",
                self.token_count,
                token_ids.len()
            )));
        }

        let hidden_size = usize::try_from(weights.config.hidden_size)
            .map_err(|_| DiffusionError::model("clip_l hidden size exceeds usize"))?;
        let head_count = usize::try_from(weights.config.attention_head_count)
            .map_err(|_| DiffusionError::model("clip_l head count exceeds usize"))?;
        let head_dim = usize::try_from(CLIP_L_HEAD_DIM)
            .map_err(|_| DiffusionError::model("clip_l head dim exceeds usize"))?;
        let intermediate_size = usize::try_from(weights.config.intermediate_size)
            .map_err(|_| DiffusionError::model("clip_l intermediate size exceeds usize"))?;
        if hidden_size != head_count * head_dim {
            return Err(DiffusionError::model(format!(
                "clip_l hidden size {} is incompatible with {} heads of dim {}",
                hidden_size, head_count, head_dim
            )));
        }

        let mut hidden = embed_clip_tokens(weights, token_ids, hidden_size)?;
        for layer in 0..weights.config.layer_count as usize {
            let prefix = format!("text_model.encoder.layers.{layer}");
            let norm1 = layer_norm_rows_with_weight_bias(
                &hidden,
                weights
                    .tensor_f32_values(&format!("{prefix}.layer_norm1.weight"))?
                    .as_slice(),
                weights
                    .tensor_f32_values(&format!("{prefix}.layer_norm1.bias"))?
                    .as_slice(),
                weights.config.layer_norm_epsilon(),
            )?;
            let q = linear_rows_ggml(
                weights,
                &norm1,
                &format!("{prefix}.self_attn.q_proj.weight"),
                &format!("{prefix}.self_attn.q_proj.bias"),
            )?;
            let k = linear_rows_ggml(
                weights,
                &norm1,
                &format!("{prefix}.self_attn.k_proj.weight"),
                &format!("{prefix}.self_attn.k_proj.bias"),
            )?;
            let v = linear_rows_ggml(
                weights,
                &norm1,
                &format!("{prefix}.self_attn.v_proj.weight"),
                &format!("{prefix}.self_attn.v_proj.bias"),
            )?;
            let attn = clip_attention_rows(&q, &k, &v, self.token_count, head_count, head_dim)?;
            let attn_proj = linear_rows_ggml(
                weights,
                &attn,
                &format!("{prefix}.self_attn.out_proj.weight"),
                &format!("{prefix}.self_attn.out_proj.bias"),
            )?;
            hidden = add_rows(&hidden, &attn_proj)?;

            let norm2 = layer_norm_rows_with_weight_bias(
                &hidden,
                weights
                    .tensor_f32_values(&format!("{prefix}.layer_norm2.weight"))?
                    .as_slice(),
                weights
                    .tensor_f32_values(&format!("{prefix}.layer_norm2.bias"))?
                    .as_slice(),
                weights.config.layer_norm_epsilon(),
            )?;
            let mlp_fc1 = linear_rows_ggml(
                weights,
                &norm2,
                &format!("{prefix}.mlp.fc1.weight"),
                &format!("{prefix}.mlp.fc1.bias"),
            )?;
            if mlp_fc1.cols != intermediate_size {
                return Err(DiffusionError::model(format!(
                    "clip_l fc1 width mismatch: expected {} got {}",
                    intermediate_size, mlp_fc1.cols
                )));
            }
            let mlp_act = quick_gelu_rows(&mlp_fc1)?;
            let mlp_out = linear_rows_ggml(
                weights,
                &mlp_act,
                &format!("{prefix}.mlp.fc2.weight"),
                &format!("{prefix}.mlp.fc2.bias"),
            )?;
            hidden = add_rows(&hidden, &mlp_out)?;
        }

        let final_hidden = layer_norm_rows_with_weight_bias(
            &hidden,
            weights
                .tensor_f32_values("text_model.final_layer_norm.weight")?
                .as_slice(),
            weights
                .tensor_f32_values("text_model.final_layer_norm.bias")?
                .as_slice(),
            weights.config.layer_norm_epsilon(),
        )?;
        let pooled = pooled_from_hidden_states(
            &final_hidden.data,
            hidden_size,
            self.token_count,
            self.eos_index,
        )?;

        Ok(ClipLRun {
            hidden_states: final_hidden.data,
            pooled,
            token_count: self.token_count,
            hidden_size,
            eos_index: self.eos_index,
        })
    }
}

impl ClipLExecutionMode {
    pub fn from_env() -> Self {
        match std::env::var("FLUX_CLIP_L_MODE") {
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

impl LoadedClipLWeights {
    fn graph_reserve_bytes(&self) -> usize {
        self.graph_extra_bytes
    }
}

pub fn build_clip_l_graph(
    weights: &mut LoadedClipLWeights,
    chunk: &ClipTokenChunk,
) -> Result<ClipLGraph> {
    let n_tokens = chunk.token_ids.len();
    let hidden_size = i64::from(weights.config.hidden_size);
    let head_count = i64::from(weights.config.attention_head_count);
    let head_dim = i64::from(CLIP_L_HEAD_DIM);
    if head_dim * head_count != hidden_size {
        return Err(DiffusionError::model(format!(
            "clip_l hidden size {} is incompatible with head count {} and head dim {}",
            hidden_size, head_count, CLIP_L_HEAD_DIM
        )));
    }

    let input_token_ids = weights
        .ctx
        .new_named_tensor(
            "clip_l.input_token_ids",
            TensorType::I32,
            1,
            &[n_tokens as i64],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let position_ids = weights
        .ctx
        .new_named_tensor(
            "clip_l.position_ids",
            TensorType::I32,
            1,
            &[n_tokens as i64],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    weights
        .ctx
        .write_tensor_data(
            position_ids,
            &i32s_to_le_bytes(&(0..n_tokens as i32).collect::<Vec<_>>()),
        )
        .map_err(DiffusionError::model)?;
    let attention_mask = weights
        .ctx
        .new_named_tensor(
            "clip_l.attention_mask",
            TensorType::F32,
            4,
            &[n_tokens as i64, n_tokens as i64, 1, 1],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    weights
        .ctx
        .write_tensor_data(attention_mask, &causal_mask_f32_bytes(n_tokens))
        .map_err(DiffusionError::model)?;

    let token_embeddings = weights
        .ctx
        .get_rows(
            weights.tensor_id("text_model.embeddings.token_embedding.weight")?,
            input_token_ids,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let token_embeddings = weights
        .ctx
        .cont_2d(token_embeddings, hidden_size, n_tokens as i64)
        .map_err(DiffusionError::model)?;
    let position_embeddings = weights
        .ctx
        .get_rows(
            weights.tensor_id("text_model.embeddings.position_embedding.weight")?,
            position_ids,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let position_embeddings = weights
        .ctx
        .cont_2d(position_embeddings, hidden_size, n_tokens as i64)
        .map_err(DiffusionError::model)?;
    let mut hidden = weights
        .ctx
        .binary_like_a(
            Op::Add,
            token_embeddings,
            position_embeddings,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;

    for layer in 0..weights.config.layer_count as usize {
        let prefix = format!("text_model.encoder.layers.{layer}");

        let norm1 = apply_layer_norm(
            &mut weights.ctx,
            &weights.tensor_ids,
            hidden,
            &format!("{prefix}.layer_norm1.weight"),
            &format!("{prefix}.layer_norm1.bias"),
            weights.config.layer_norm_epsilon(),
        )?;
        let q = apply_linear(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm1,
            &format!("{prefix}.self_attn.q_proj.weight"),
            &format!("{prefix}.self_attn.q_proj.bias"),
        )?;
        let k = apply_linear(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm1,
            &format!("{prefix}.self_attn.k_proj.weight"),
            &format!("{prefix}.self_attn.k_proj.bias"),
        )?;
        let v = apply_linear(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm1,
            &format!("{prefix}.self_attn.v_proj.weight"),
            &format!("{prefix}.self_attn.v_proj.bias"),
        )?;

        let q = weights
            .ctx
            .reshape(q, &[head_dim, head_count, n_tokens as i64])
            .map_err(DiffusionError::model)?;
        let k = weights
            .ctx
            .reshape(k, &[head_dim, head_count, n_tokens as i64])
            .map_err(DiffusionError::model)?;
        let v = weights
            .ctx
            .reshape(v, &[head_dim, head_count, n_tokens as i64])
            .map_err(DiffusionError::model)?;

        let attn = build_attention_mha_output(
            &mut weights.ctx,
            q,
            k,
            v,
            attention_mask,
            weights.config.attention_head_count,
        )?;
        let attn_proj = apply_linear(
            &mut weights.ctx,
            &weights.tensor_ids,
            attn,
            &format!("{prefix}.self_attn.out_proj.weight"),
            &format!("{prefix}.self_attn.out_proj.bias"),
        )?;
        hidden = weights
            .ctx
            .binary_like_a(Op::Add, hidden, attn_proj, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;

        let norm2 = apply_layer_norm(
            &mut weights.ctx,
            &weights.tensor_ids,
            hidden,
            &format!("{prefix}.layer_norm2.weight"),
            &format!("{prefix}.layer_norm2.bias"),
            weights.config.layer_norm_epsilon(),
        )?;
        let mlp_fc1 = apply_linear(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm2,
            &format!("{prefix}.mlp.fc1.weight"),
            &format!("{prefix}.mlp.fc1.bias"),
        )?;
        let mlp_act = gelu_quick(&mut weights.ctx, mlp_fc1)?;
        let mlp_out = apply_linear(
            &mut weights.ctx,
            &weights.tensor_ids,
            mlp_act,
            &format!("{prefix}.mlp.fc2.weight"),
            &format!("{prefix}.mlp.fc2.bias"),
        )?;
        hidden = weights
            .ctx
            .binary_like_a(Op::Add, hidden, mlp_out, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
    }

    let result_hidden_states = apply_layer_norm(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        "text_model.final_layer_norm.weight",
        "text_model.final_layer_norm.bias",
        weights.config.layer_norm_epsilon(),
    )?;
    let result_pooled =
        view_token_column_contiguous(&mut weights.ctx, result_hidden_states, chunk.eos_index)?;

    let mut graph = Graph::new();
    graph
        .build_forward_expand(&weights.ctx, result_hidden_states)
        .map_err(DiffusionError::model)?;
    graph
        .build_forward_expand(&weights.ctx, result_pooled)
        .map_err(DiffusionError::model)?;

    Ok(ClipLGraph {
        graph,
        input_token_ids,
        result_hidden_states,
        result_pooled,
        eos_index: chunk.eos_index,
    })
}

fn build_attention_mha_output(
    ctx: &mut Context,
    q: TensorId,
    k: TensorId,
    v: TensorId,
    attention_mask: TensorId,
    head_count: u32,
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

    let q_tensor = require_tensor(ctx, q)?.clone();
    let n_tokens = usize::try_from(q_tensor.ne[1])
        .map_err(|_| DiffusionError::model("clip_l q token count exceeds usize"))?;
    let attention_scale = 1.0 / (CLIP_L_HEAD_DIM as f32).sqrt();

    if flash_attention_allowed(head_count, n_tokens) {
        let attn = ctx
            .flash_attn_ext(
                q,
                k,
                v,
                Some(attention_mask),
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
        .soft_max_ext(
            kq,
            Some(attention_mask),
            attention_scale,
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

fn apply_layer_norm(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight_name: &str,
    bias_name: &str,
    epsilon: f32,
) -> Result<TensorId> {
    let norm = ctx
        .norm_eps(input, epsilon, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let weight = repeat_weight(ctx, require_tensor_id(tensor_ids, weight_name)?, norm)?;
    let scaled = ctx
        .binary_like_a(Op::Mul, norm, weight, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let bias = repeat_weight(ctx, require_tensor_id(tensor_ids, bias_name)?, scaled)?;
    ctx.binary_like_a(Op::Add, scaled, bias, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn apply_linear(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight_name: &str,
    bias_name: &str,
) -> Result<TensorId> {
    let out = ctx
        .mul_mat(
            require_tensor_id(tensor_ids, weight_name)?,
            input,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let bias = repeat_weight(ctx, require_tensor_id(tensor_ids, bias_name)?, out)?;
    ctx.binary_like_a(Op::Add, out, bias, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn gelu_quick(ctx: &mut Context, input: TensorId) -> Result<TensorId> {
    let ones = repeat_scalar_one(ctx, input)?;
    ctx.glu_split(input, ones, GluOp::GegluQuick, BufferUsage::Activations)
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

fn repeat_weight(ctx: &mut Context, weight: TensorId, shape_of: TensorId) -> Result<TensorId> {
    ctx.repeat(weight, shape_of, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn view_token_column_contiguous(
    ctx: &mut Context,
    hidden: TensorId,
    token_index: usize,
) -> Result<TensorId> {
    let tensor = require_tensor(ctx, hidden)?.clone();
    let token_count = usize::try_from(tensor.ne[1])
        .map_err(|_| DiffusionError::model("clip_l token count exceeds usize"))?;
    if token_index >= token_count {
        return Err(DiffusionError::workflow(format!(
            "clip_l pooled token index {} out of range for {} tokens",
            token_index, token_count
        )));
    }
    let offset = tensor.nb[1]
        .checked_mul(token_index)
        .ok_or_else(|| DiffusionError::model("clip_l pooled token offset overflow"))?;
    let view = ctx
        .view_2d(hidden, tensor.ne[0], 1, tensor.nb[1], offset)
        .map_err(DiffusionError::model)?;
    ctx.cont_2d(view, tensor.ne[0], 1)
        .map_err(DiffusionError::model)
}

impl RowsTensor {
    fn new(rows: usize, cols: usize, data: Vec<f32>) -> Result<Self> {
        let expected = rows
            .checked_mul(cols)
            .ok_or_else(|| DiffusionError::model("clip_l rows tensor size overflow"))?;
        if data.len() != expected {
            return Err(DiffusionError::model(format!(
                "clip_l rows tensor len mismatch: expected {}, got {}",
                expected,
                data.len()
            )));
        }
        Ok(Self { rows, cols, data })
    }
}

fn embed_clip_tokens(
    weights: &LoadedClipLWeights,
    token_ids: &[i32],
    hidden_size: usize,
) -> Result<RowsTensor> {
    let token_embeddings = gather_rows_ggml(
        weights,
        "text_model.embeddings.token_embedding.weight",
        token_ids,
    )?;
    let position_ids = (0..token_ids.len())
        .map(|index| {
            i32::try_from(index)
                .map_err(|_| DiffusionError::model("clip_l position index exceeds i32"))
        })
        .collect::<Result<Vec<_>>>()?;
    let position_embeddings = gather_rows_ggml(
        weights,
        "text_model.embeddings.position_embedding.weight",
        &position_ids,
    )?;
    let token_embeddings = RowsTensor::new(token_ids.len(), hidden_size, token_embeddings)?;
    let position_embeddings = RowsTensor::new(token_ids.len(), hidden_size, position_embeddings)?;
    add_rows(&token_embeddings, &position_embeddings)
}

fn gather_rows_ggml(
    weights: &LoadedClipLWeights,
    name: &str,
    row_indices: &[i32],
) -> Result<Vec<f32>> {
    let matrix = weights.tensor_matrix(name)?;
    if let Some(result) = try_get_rows_ggml_bytes_cached(
        matrix.ggml_type,
        matrix.cols,
        matrix.rows,
        row_indices,
        &clip_cache_namespace(weights),
        name,
        || Ok(matrix.bytes.to_vec()),
    ) {
        match result {
            Ok(values) => return Ok(values),
            Err(err) if can_fallback_from_accel_error(&err) => {}
            Err(err) => return Err(DiffusionError::model(err)),
        }
    }
    get_rows_ggml_bytes_cpu(
        matrix.bytes,
        matrix.ggml_type,
        matrix.cols,
        matrix.rows,
        row_indices,
    )
    .ok_or_else(|| DiffusionError::model(format!("clip_l row gather fallback failed for {}", name)))
}

fn linear_rows_ggml(
    weights: &LoadedClipLWeights,
    input: &RowsTensor,
    weight_name: &str,
    bias_name: &str,
) -> Result<RowsTensor> {
    let weight = weights.tensor_matrix(weight_name)?;
    if input.cols != weight.cols {
        return Err(DiffusionError::model(format!(
            "clip_l linear input width mismatch: input={} weight={}",
            input.cols, weight.cols
        )));
    }
    if input.rows == 0 {
        return RowsTensor::new(0, weight.rows, Vec::new());
    }

    let mut output = if clip_force_cpu_math() {
        let decoded = decode_ggml_matrix_to_f32(weight)?;
        matmul_nt_f32_cpu(&input.data, &decoded, input.rows, input.cols, weight.rows)?
    } else if let Some(result) = try_matmul_nt_ggml_bytes_cached(
        &input.data,
        weight.ggml_type,
        input.rows,
        input.cols,
        weight.rows,
        &clip_cache_namespace(weights),
        weight_name,
        || Ok(weight.bytes.to_vec()),
    ) {
        match result {
            Ok(output) => output,
            Err(err) if can_fallback_from_accel_error(&err) => {
                let decoded = decode_ggml_matrix_to_f32(weight)?;
                if let Some(output) =
                    try_matmul_nt_f32(&input.data, &decoded, input.rows, input.cols, weight.rows)
                {
                    output
                } else {
                    matmul_nt_f32_cpu(&input.data, &decoded, input.rows, input.cols, weight.rows)?
                }
            }
            Err(err) => return Err(DiffusionError::model(err)),
        }
    } else {
        let decoded = decode_ggml_matrix_to_f32(weight)?;
        if let Some(output) =
            try_matmul_nt_f32(&input.data, &decoded, input.rows, input.cols, weight.rows)
        {
            output
        } else {
            matmul_nt_f32_cpu(&input.data, &decoded, input.rows, input.cols, weight.rows)?
        }
    };

    apply_row_bias_in_place(
        &mut output,
        weights.tensor_f32_values(bias_name)?.as_slice(),
        input.rows,
        weight.rows,
    )?;
    RowsTensor::new(input.rows, weight.rows, output)
}

fn layer_norm_rows_with_weight_bias(
    input: &RowsTensor,
    weight: &[f32],
    bias: &[f32],
    eps: f32,
) -> Result<RowsTensor> {
    if input.cols != weight.len() || input.cols != bias.len() {
        return Err(DiffusionError::model(format!(
            "clip_l layer norm weight/bias mismatch: cols={} weight={} bias={}",
            input.cols,
            weight.len(),
            bias.len()
        )));
    }
    if input.rows == 0 {
        return RowsTensor::new(0, input.cols, Vec::new());
    }
    if !clip_force_cpu_math() {
        if let Some(output) = try_layer_norm_mul_add_f32(
            &input.data,
            &[input.rows, input.cols],
            weight,
            &[input.cols],
            bias,
            &[input.cols],
            eps,
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
                let delta = *value - mean;
                delta * delta
            })
            .sum::<f32>()
            / input.cols as f32;
        let inv_std = 1.0 / (variance + eps).sqrt();
        for ((value, scale), bias_value) in row.iter().zip(weight.iter()).zip(bias.iter()) {
            output.push((value - mean) * inv_std * scale + bias_value);
        }
    }
    RowsTensor::new(input.rows, input.cols, output)
}

fn clip_attention_rows(
    q: &RowsTensor,
    k: &RowsTensor,
    v: &RowsTensor,
    token_count: usize,
    head_count: usize,
    head_dim: usize,
) -> Result<RowsTensor> {
    if q.rows != token_count || k.rows != token_count || v.rows != token_count {
        return Err(DiffusionError::model(
            "clip_l attention token count mismatch",
        ));
    }
    if q.cols != head_count * head_dim
        || k.cols != head_count * head_dim
        || v.cols != head_count * head_dim
    {
        return Err(DiffusionError::model(format!(
            "clip_l attention width mismatch: q={} k={} v={} expected {}",
            q.cols,
            k.cols,
            v.cols,
            head_count * head_dim
        )));
    }

    let scale = 1.0 / (head_dim as f32).sqrt();
    let mut output = vec![0.0f32; token_count * head_count * head_dim];
    for head_idx in 0..head_count {
        let q_head = extract_head_rows(q, head_idx, head_dim);
        let k_head = extract_head_rows(k, head_idx, head_dim);
        let v_head = extract_head_rows(v, head_idx, head_dim);
        let mut scores = if clip_force_cpu_math() {
            matmul_nt_f32_cpu(&q_head, &k_head, token_count, head_dim, token_count)?
        } else if let Some(scores) =
            try_matmul_nt_f32(&q_head, &k_head, token_count, head_dim, token_count)
        {
            scores
        } else {
            matmul_nt_f32_cpu(&q_head, &k_head, token_count, head_dim, token_count)?
        };
        apply_causal_scale_mask_in_place(&mut scores, token_count, scale)?;

        if !clip_force_cpu_math() {
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
        let head_output = if clip_force_cpu_math() {
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

fn quick_gelu_rows(input: &RowsTensor) -> Result<RowsTensor> {
    let output = input
        .data
        .iter()
        .copied()
        .map(quick_gelu)
        .collect::<Vec<_>>();
    RowsTensor::new(input.rows, input.cols, output)
}

fn add_rows(lhs: &RowsTensor, rhs: &RowsTensor) -> Result<RowsTensor> {
    if lhs.rows != rhs.rows || lhs.cols != rhs.cols {
        return Err(DiffusionError::model(format!(
            "clip_l add shape mismatch: lhs={}x{} rhs={}x{}",
            lhs.rows, lhs.cols, rhs.rows, rhs.cols
        )));
    }
    let output = lhs
        .data
        .iter()
        .zip(rhs.data.iter())
        .map(|(lhs_value, rhs_value)| lhs_value + rhs_value)
        .collect::<Vec<_>>();
    RowsTensor::new(lhs.rows, lhs.cols, output)
}

fn clip_cache_namespace(weights: &LoadedClipLWeights) -> String {
    format!("clip_l:{}", weights.path.display())
}

fn clip_force_cpu_math() -> bool {
    std::env::var_os("CLIP_L_FORCE_CPU_MATH").is_some()
}

fn can_fallback_from_accel_error(err: &str) -> bool {
    err.contains("only supports NVFP4 today") || err.contains("unsupported ggml type")
}

fn quick_gelu(x: f32) -> f32 {
    x * (1.0 / (1.0 + (-CLIP_L_QUICK_GELU_COEF * x).exp()))
}

fn resident_matrix<'a>(ctx: &'a Context, tensor_id: TensorId) -> Result<ResidentMatrix<'a>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    let cols = usize::try_from(tensor.ne[0]).map_err(|_| {
        DiffusionError::model(format!("clip_l tensor {} cols exceed usize", tensor_id))
    })?;
    let rows = usize::try_from(tensor.ne[1]).map_err(|_| {
        DiffusionError::model(format!("clip_l tensor {} rows exceed usize", tensor_id))
    })?;
    Ok(ResidentMatrix {
        bytes: ctx.tensor_data(tensor_id).map_err(DiffusionError::model)?,
        ggml_type: tensor.desc.ty.ggml_type(),
        cols,
        rows,
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
            "clip_l tensor {} cannot be decoded as f32 from {:?}",
            tensor_id, other
        ))),
    }
}

fn decode_ggml_matrix_to_f32(matrix: ResidentMatrix<'_>) -> Result<Vec<f32>> {
    let row_indices = (0..matrix.rows)
        .map(|row| {
            i32::try_from(row).map_err(|_| DiffusionError::model("clip_l row index exceeds i32"))
        })
        .collect::<Result<Vec<_>>>()?;
    get_rows_ggml_bytes_cpu(
        matrix.bytes,
        matrix.ggml_type,
        matrix.cols,
        matrix.rows,
        &row_indices,
    )
    .ok_or_else(|| DiffusionError::model("clip_l matrix decode fallback failed"))
}

fn matmul_nt_f32_cpu(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Result<Vec<f32>> {
    if a.len()
        != m.checked_mul(k)
            .ok_or_else(|| DiffusionError::model("clip_l matmul a overflow"))?
    {
        return Err(DiffusionError::model(
            "clip_l matmul_nt_f32_cpu a len mismatch",
        ));
    }
    if bt.len()
        != n.checked_mul(k)
            .ok_or_else(|| DiffusionError::model("clip_l matmul bt overflow"))?
    {
        return Err(DiffusionError::model(
            "clip_l matmul_nt_f32_cpu bt len mismatch",
        ));
    }
    let mut out = vec![
        0.0f32;
        m.checked_mul(n)
            .ok_or_else(|| DiffusionError::model("clip_l matmul out overflow"))?
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
            .ok_or_else(|| DiffusionError::model("clip_l matmul a overflow"))?
    {
        return Err(DiffusionError::model(
            "clip_l matmul_nn_f32_cpu a len mismatch",
        ));
    }
    if b.len()
        != k.checked_mul(n)
            .ok_or_else(|| DiffusionError::model("clip_l matmul b overflow"))?
    {
        return Err(DiffusionError::model(
            "clip_l matmul_nn_f32_cpu b len mismatch",
        ));
    }
    let mut out = vec![
        0.0f32;
        m.checked_mul(n)
            .ok_or_else(|| DiffusionError::model("clip_l matmul out overflow"))?
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

fn extract_head_rows(input: &RowsTensor, head_idx: usize, head_dim: usize) -> Vec<f32> {
    let start = head_idx * head_dim;
    let end = start + head_dim;
    let mut output = Vec::with_capacity(input.rows * head_dim);
    for row in input.data.chunks_exact(input.cols) {
        output.extend_from_slice(&row[start..end]);
    }
    output
}

fn write_head_rows(
    output: &mut [f32],
    token_count: usize,
    head_count: usize,
    head_dim: usize,
    head_idx: usize,
    head_output: &[f32],
) -> Result<()> {
    let expected_len = token_count
        .checked_mul(head_dim)
        .ok_or_else(|| DiffusionError::model("clip_l head output size overflow"))?;
    if head_output.len() != expected_len {
        return Err(DiffusionError::model(format!(
            "clip_l head output len mismatch: expected {} got {}",
            expected_len,
            head_output.len()
        )));
    }
    let model_dim = head_count
        .checked_mul(head_dim)
        .ok_or_else(|| DiffusionError::model("clip_l model dim overflow"))?;
    for token_idx in 0..token_count {
        let dst_start = token_idx * model_dim + head_idx * head_dim;
        let src_start = token_idx * head_dim;
        output[dst_start..dst_start + head_dim]
            .copy_from_slice(&head_output[src_start..src_start + head_dim]);
    }
    Ok(())
}

fn apply_causal_scale_mask_in_place(values: &mut [f32], width: usize, scale: f32) -> Result<()> {
    if width == 0 || values.len() % width != 0 {
        return Err(DiffusionError::model(format!(
            "clip_l causal mask width {} is invalid for {} values",
            width,
            values.len()
        )));
    }
    for (query_idx, row) in values.chunks_exact_mut(width).enumerate() {
        for (key_idx, value) in row.iter_mut().enumerate() {
            *value *= scale;
            if key_idx > query_idx {
                *value = f32::NEG_INFINITY;
            }
        }
    }
    Ok(())
}

fn apply_row_bias_in_place(
    values: &mut [f32],
    bias: &[f32],
    rows: usize,
    cols: usize,
) -> Result<()> {
    if bias.len() != cols {
        return Err(DiffusionError::model(format!(
            "clip_l bias width mismatch: bias={} cols={}",
            bias.len(),
            cols
        )));
    }
    if values.len()
        != rows
            .checked_mul(cols)
            .ok_or_else(|| DiffusionError::model("clip_l bias output overflow"))?
    {
        return Err(DiffusionError::model(format!(
            "clip_l bias output len mismatch: values={} rows={} cols={}",
            values.len(),
            rows,
            cols
        )));
    }
    for row in values.chunks_exact_mut(cols) {
        for (value, bias_value) in row.iter_mut().zip(bias.iter()) {
            *value += *bias_value;
        }
    }
    Ok(())
}

fn softmax_in_place(values: &mut [f32], width: usize) -> Result<()> {
    if width == 0 || values.len() % width != 0 {
        return Err(DiffusionError::model(format!(
            "clip_l softmax width {} is invalid for {} values",
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
                "clip_l softmax denominator became zero",
            ));
        }
        for value in row.iter_mut() {
            *value /= denom;
        }
    }
    Ok(())
}

fn allocate_clip_weight_tensors(
    ctx: &mut Context,
    header: &MlxSafetensorsHeader,
) -> Result<BTreeMap<String, TensorId>> {
    let mut tensor_ids = BTreeMap::new();
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let entry = header.tensor(&name).ok_or_else(|| {
            DiffusionError::model(format!(
                "clip_l header lost tensor '{}' while allocating",
                name
            ))
        })?;
        let ty = clip_target_tensor_type(entry)?;
        let extents = clip_target_extents(entry)?;
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

fn load_clip_weight_bytes(
    ctx: &mut Context,
    header: &MlxSafetensorsHeader,
    tensor_ids: &BTreeMap<String, TensorId>,
) -> Result<()> {
    for (name, tensor_id) in tensor_ids {
        let entry = header.tensor(name).ok_or_else(|| {
            DiffusionError::model(format!("clip_l header missing tensor '{}'", name))
        })?;
        let bytes = clip_target_bytes(header, name, entry)?;
        ctx.write_tensor_data(*tensor_id, &bytes)
            .map_err(DiffusionError::model)?;
    }
    Ok(())
}

fn clip_model_config_from_inspection(inspect: &ClipLTextEncoderConfig) -> Result<ClipLModelConfig> {
    if inspect.hidden_size % CLIP_L_HEAD_DIM != 0 {
        return Err(DiffusionError::model(format!(
            "clip_l hidden size {} is not divisible by head dim {}",
            inspect.hidden_size, CLIP_L_HEAD_DIM
        )));
    }
    Ok(ClipLModelConfig {
        vocab_size: inspect.vocab_size,
        max_position_embeddings: inspect.max_position_embeddings,
        hidden_size: inspect.hidden_size,
        intermediate_size: inspect.intermediate_size,
        layer_count: inspect.layer_count,
        attention_head_count: inspect.hidden_size / CLIP_L_HEAD_DIM,
        layer_norm_epsilon_bits: CLIP_L_LAYER_NORM_EPSILON.to_bits(),
    })
}

fn clip_weight_total_bytes(header: &MlxSafetensorsHeader, extra_bytes: usize) -> Result<usize> {
    let mut total = 0usize;
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let entry = header.tensor(&name).unwrap();
        total = ggml_pad(total, GGML_MEM_ALIGN);
        total = total
            .checked_add(clip_target_nbytes(&name, entry)?)
            .ok_or_else(|| {
                DiffusionError::model(format!("clip_l total bytes overflow at '{}'", name))
            })?;
    }
    total = ggml_pad(total, GGML_MEM_ALIGN);
    total
        .checked_add(extra_bytes)
        .ok_or_else(|| DiffusionError::model("clip_l context size overflow"))
}

fn clip_target_nbytes(_name: &str, entry: &MlxTensorEntry) -> Result<usize> {
    let ty = clip_target_tensor_type(entry)?;
    let extents = clip_target_extents(entry)?;
    let layout = TensorLayout::for_ggml(ty, &extents).map_err(DiffusionError::model)?;
    Ok(Tensor::from_desc(0, TensorDesc::new(ty, layout, BufferUsage::Weights)).nbytes())
}

fn clip_target_extents(entry: &MlxTensorEntry) -> Result<Vec<i64>> {
    match entry.shape.as_slice() {
        [dim] => Ok(vec![i64::try_from(*dim).map_err(|_| {
            DiffusionError::model(format!("clip_l extent {} exceeds i64", dim))
        })?]),
        [dim0, dim1] => Ok(vec![
            i64::try_from(*dim1).map_err(|_| {
                DiffusionError::model(format!("clip_l extent {} exceeds i64", dim1))
            })?,
            i64::try_from(*dim0).map_err(|_| {
                DiffusionError::model(format!("clip_l extent {} exceeds i64", dim0))
            })?,
        ]),
        other => Err(DiffusionError::model(format!(
            "clip_l only supports rank1/rank2 tensors today, got {:?}",
            other
        ))),
    }
}

fn clip_target_tensor_type(entry: &MlxTensorEntry) -> Result<TensorType> {
    match entry.dtype {
        MlxDType::F16 | MlxDType::BF16 if entry.shape.len() == 1 => Ok(TensorType::F32),
        MlxDType::F16 => Ok(TensorType::F16),
        MlxDType::BF16 => Ok(TensorType::BF16),
        MlxDType::F32 => Ok(TensorType::F32),
        other => Err(DiffusionError::model(format!(
            "clip_l unsupported tensor dtype {:?}",
            other
        ))),
    }
}

fn clip_target_bytes(
    header: &MlxSafetensorsHeader,
    name: &str,
    entry: &MlxTensorEntry,
) -> Result<Vec<u8>> {
    let bytes = header.read_tensor_bytes(name)?;
    match entry.dtype {
        MlxDType::F32 => Ok(bytes),
        MlxDType::F16 if entry.shape.len() == 1 => Ok(f16_bytes_to_f32_bytes(&bytes)?),
        MlxDType::BF16 if entry.shape.len() == 1 => Ok(bf16_bytes_to_f32_bytes(&bytes)?),
        MlxDType::F16 | MlxDType::BF16 => Ok(bytes),
        other => Err(DiffusionError::model(format!(
            "clip_l unsupported tensor dtype {:?}",
            other
        ))),
    }
}

fn f16_bytes_to_f32_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "clip_l F16 bytes length {} is not even",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for chunk in bytes.chunks_exact(2) {
        out.extend_from_slice(&f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])).to_le_bytes());
    }
    Ok(out)
}

fn f16_bytes_to_f32_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "clip_l F16 bytes length {} is not even",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
        .collect())
}

fn bf16_bytes_to_f32_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "clip_l BF16 bytes length {} is not even",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for chunk in bytes.chunks_exact(2) {
        out.extend_from_slice(&bf16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])).to_le_bytes());
    }
    Ok(out)
}

fn bf16_bytes_to_f32_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "clip_l BF16 bytes length {} is not even",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| bf16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
        .collect())
}

fn require_tensor_id(tensor_ids: &BTreeMap<String, TensorId>, name: &str) -> Result<TensorId> {
    tensor_ids
        .get(name)
        .copied()
        .ok_or_else(|| DiffusionError::model(format!("missing clip_l resident tensor '{}'", name)))
}

fn require_tensor<'a>(ctx: &'a Context, id: TensorId) -> Result<&'a Tensor> {
    ctx.tensor(id)
        .ok_or_else(|| DiffusionError::model(format!("invalid clip_l tensor id {}", id)))
}

fn flash_attention_allowed(head_count: u32, n_tokens: usize) -> bool {
    let _ = head_count;
    let _ = n_tokens;
    if std::env::var_os("CLIP_L_DISABLE_FLASH_ATTN").is_some() {
        return false;
    }
    matches!(
        CLIP_L_HEAD_DIM,
        32 | 40 | 48 | 64 | 72 | 80 | 96 | 112 | 128 | 192 | 256 | 576
    )
}

fn is_context_oom(err: &DiffusionError) -> bool {
    matches!(err, DiffusionError::Model(message) if message.starts_with("context out of memory allocating "))
}

fn next_graph_reserve_bytes(weights: &LoadedClipLWeights) -> Result<usize> {
    weights
        .graph_reserve_bytes()
        .checked_mul(2)
        .ok_or_else(|| DiffusionError::model("clip_l graph reserve overflow"))
}

fn causal_mask_f32_bytes(n_tokens: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n_tokens * n_tokens * std::mem::size_of::<f32>());
    for query in 0..n_tokens {
        for key in 0..n_tokens {
            let value = if key > query { f32::NEG_INFINITY } else { 0.0 };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
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
            "clip_l output byte length {} is not divisible by 4",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn pooled_from_hidden_states(
    hidden_states: &[f32],
    hidden_size: usize,
    token_count: usize,
    token_index: usize,
) -> Result<Vec<f32>> {
    if token_index >= token_count {
        return Err(DiffusionError::workflow(format!(
            "clip_l pooled token index {} out of range for {} tokens",
            token_index, token_count
        )));
    }
    let start = token_index
        .checked_mul(hidden_size)
        .ok_or_else(|| DiffusionError::model("clip_l pooled slice offset overflow"))?;
    let end = start
        .checked_add(hidden_size)
        .ok_or_else(|| DiffusionError::model("clip_l pooled slice end overflow"))?;
    let slice = hidden_states.get(start..end).ok_or_else(|| {
        DiffusionError::model("clip_l hidden state buffer is too small for pooled slice")
    })?;
    Ok(slice.to_vec())
}

#[cfg(test)]
mod tests {
    use super::{
        clip_model_config_from_inspection, clip_target_extents, clip_target_tensor_type,
        flash_attention_allowed,
    };
    use crate::flux::ClipLTextEncoderConfig;
    use makepad_ggml::TensorType;
    use makepad_mlx::{MlxDType, MlxTensorEntry};

    #[test]
    fn clip_layout_reverses_rank2_weights_for_ggml_matmul() {
        let entry = MlxTensorEntry {
            dtype: MlxDType::F16,
            shape: vec![49408, 768],
            data_offsets: [0, 0],
        };
        assert_eq!(clip_target_extents(&entry).unwrap(), vec![768, 49408]);
        assert_eq!(clip_target_tensor_type(&entry).unwrap(), TensorType::F16);
    }

    #[test]
    fn clip_rank1_f16_weights_promote_to_f32() {
        let entry = MlxTensorEntry {
            dtype: MlxDType::F16,
            shape: vec![768],
            data_offsets: [0, 0],
        };
        assert_eq!(clip_target_tensor_type(&entry).unwrap(), TensorType::F32);
    }

    #[test]
    fn clip_flash_attention_allows_standard_prompt_lengths() {
        assert!(flash_attention_allowed(12, 77));
    }

    #[test]
    fn clip_model_config_derives_head_count_from_hidden_size() {
        let config = clip_model_config_from_inspection(&ClipLTextEncoderConfig {
            vocab_size: 49408,
            max_position_embeddings: 77,
            hidden_size: 768,
            intermediate_size: 3072,
            layer_count: 12,
        })
        .unwrap();
        assert_eq!(config.attention_head_count, 12);
        assert_eq!(config.layer_norm_epsilon(), 1.0e-5);
    }
}
