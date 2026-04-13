use crate::clip::ClipTokenChunk;
use crate::flux::ClipLTextEncoderConfig;
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

const CLIP_L_HEAD_DIM: u32 = 64;
const CLIP_L_LAYER_NORM_EPSILON: f32 = 1.0e-5;
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

pub struct CompiledClipLMetal {
    graph: ClipLGraph,
    session: MetalGraphSession,
}

#[derive(Clone, Debug)]
pub struct ClipLRun {
    pub hidden_states: Vec<f32>,
    pub pooled: Vec<f32>,
    pub token_count: usize,
    pub hidden_size: usize,
    pub eos_index: usize,
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
}

impl CompiledClipLMetal {
    pub fn compile(weights: &mut LoadedClipLWeights, chunk: &ClipTokenChunk) -> Result<Self> {
        let runtime = MetalRuntime::new().map_err(DiffusionError::model)?;
        Self::compile_with_runtime(runtime, weights, chunk)
    }

    pub fn compile_with_runtime(
        runtime: MetalRuntime,
        weights: &mut LoadedClipLWeights,
        chunk: &ClipTokenChunk,
    ) -> Result<Self> {
        for attempt in 0..=MAX_GRAPH_GROWTH_ATTEMPTS {
            let graph = match build_clip_l_graph(weights, chunk) {
                Ok(graph) => graph,
                Err(err) if is_context_oom(&err) && attempt < MAX_GRAPH_GROWTH_ATTEMPTS => {
                    let next_extra = next_graph_reserve_bytes(weights)?;
                    *weights = LoadedClipLWeights::load_with_extra(weights.path.clone(), next_extra)?;
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
            "clip_l graph compilation exhausted context growth attempts",
        ))
    }

    pub fn execute(&self, weights: &LoadedClipLWeights, token_ids: &[i32]) -> Result<ClipLRun> {
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
                &[MetalGraphTensorWrite {
                    tensor_id: self.graph.input_token_ids,
                    bytes: &input_bytes,
                }],
                &[self.graph.result_hidden_states],
            )
            .map_err(DiffusionError::model)?;

        let hidden_bytes = execution
            .outputs
            .get(&self.graph.result_hidden_states)
            .ok_or_else(|| DiffusionError::model("clip_l execution did not return hidden states"))?;

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

impl LoadedClipLWeights {
    fn graph_reserve_bytes(&self) -> usize {
        self.graph_extra_bytes
    }
}

pub fn build_clip_l_graph(weights: &mut LoadedClipLWeights, chunk: &ClipTokenChunk) -> Result<ClipLGraph> {
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
        .write_tensor_data(position_ids, &i32s_to_le_bytes(&(0..n_tokens as i32).collect::<Vec<_>>()))
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
        .binary_like_a(Op::Add, token_embeddings, position_embeddings, BufferUsage::Activations)
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
    let result_pooled = view_token_column_contiguous(&mut weights.ctx, result_hidden_states, chunk.eos_index)?;

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
        .soft_max_ext(kq, Some(attention_mask), attention_scale, 0.0, BufferUsage::Activations)
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
        .mul_mat(require_tensor_id(tensor_ids, weight_name)?, input, BufferUsage::Activations)
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

fn view_token_column_contiguous(ctx: &mut Context, hidden: TensorId, token_index: usize) -> Result<TensorId> {
    let tensor = require_tensor(ctx, hidden)?.clone();
    let token_count = usize::try_from(tensor.ne[1])
        .map_err(|_| DiffusionError::model("clip_l token count exceeds usize"))?;
    if token_index >= token_count {
        return Err(DiffusionError::workflow(format!(
            "clip_l pooled token index {} out of range for {} tokens",
            token_index, token_count
        )));
    }
    let offset = tensor
        .nb[1]
        .checked_mul(token_index)
        .ok_or_else(|| DiffusionError::model("clip_l pooled token offset overflow"))?;
    let view = ctx
        .view_2d(hidden, tensor.ne[0], 1, tensor.nb[1], offset)
        .map_err(DiffusionError::model)?;
    ctx.cont_2d(view, tensor.ne[0], 1).map_err(DiffusionError::model)
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
        let ty = clip_target_tensor_type(entry.dtype)?;
        let extents = clip_target_extents(entry)?;
        let id = ctx
            .new_named_tensor(name.clone(), ty, extents.len(), &extents, BufferUsage::Weights)
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
            .ok_or_else(|| DiffusionError::model(format!("clip_l total bytes overflow at '{}'", name)))?;
    }
    total = ggml_pad(total, GGML_MEM_ALIGN);
    total
        .checked_add(extra_bytes)
        .ok_or_else(|| DiffusionError::model("clip_l context size overflow"))
}

fn clip_target_nbytes(_name: &str, entry: &MlxTensorEntry) -> Result<usize> {
    let ty = clip_target_tensor_type(entry.dtype)?;
    let extents = clip_target_extents(entry)?;
    let layout = TensorLayout::for_ggml(ty, &extents).map_err(DiffusionError::model)?;
    Ok(Tensor::from_desc(0, TensorDesc::new(ty, layout, BufferUsage::Weights)).nbytes())
}

fn clip_target_extents(entry: &MlxTensorEntry) -> Result<Vec<i64>> {
    match entry.shape.as_slice() {
        [dim] => Ok(vec![i64::try_from(*dim)
            .map_err(|_| DiffusionError::model(format!("clip_l extent {} exceeds i64", dim)))?]),
        [dim0, dim1] => Ok(vec![
            i64::try_from(*dim1)
                .map_err(|_| DiffusionError::model(format!("clip_l extent {} exceeds i64", dim1)))?,
            i64::try_from(*dim0)
                .map_err(|_| DiffusionError::model(format!("clip_l extent {} exceeds i64", dim0)))?,
        ]),
        other => Err(DiffusionError::model(format!(
            "clip_l only supports rank1/rank2 tensors today, got {:?}",
            other
        ))),
    }
}

fn clip_target_tensor_type(dtype: MlxDType) -> Result<TensorType> {
    match dtype {
        MlxDType::F16 | MlxDType::BF16 | MlxDType::F32 => Ok(TensorType::F32),
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
        MlxDType::F16 => Ok(f16_bytes_to_f32_bytes(&bytes)?),
        MlxDType::BF16 => Ok(bf16_bytes_to_f32_bytes(&bytes)?),
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
    matches!(CLIP_L_HEAD_DIM, 32 | 40 | 48 | 64 | 72 | 80 | 96 | 112 | 128 | 192 | 256 | 576)
        && n_tokens < 20
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
    use super::{clip_model_config_from_inspection, clip_target_extents, clip_target_tensor_type};
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
        assert_eq!(clip_target_tensor_type(entry.dtype).unwrap(), TensorType::F32);
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
