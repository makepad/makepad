use crate::flux::T5TextEncoderConfig;
use crate::t5::T5TokenizedPrompt;
use crate::{DiffusionError, Result};
use makepad_ggml::backend::{try_get_rows_ggml_bytes, try_matmul_nt_ggml_bytes};
use makepad_ggml::backend::metal::{
    prepare_graph, try_add_f32, try_gelu_f32, try_matmul_nn_f32, try_matmul_nt_f32, try_mul_f32,
    try_rms_norm_mul_f32, BufferStorageMode, MetalGraphSession, MetalGraphTensorWrite,
    MetalRuntime,
};
use makepad_ggml::{
    bf16_to_f32, f16_to_f32, get_rows_ggml_bytes_cpu, ggml_pad, BufferUsage, Context, Graph,
    InitParams, Op, Tensor, TensorDesc, TensorId, TensorLayout, TensorType, UnaryOp,
    GGML_MEM_ALIGN,
};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

const T5_LAYER_NORM_EPSILON: f32 = 1.0e-6;
const T5_RELATIVE_MAX_DISTANCE: u32 = 128;
const T5_GATED_FF_OUTPUT_INPUT_SCALE: f32 = 1.0 / 32.0;
const DEFAULT_GRAPH_EXTRA_BYTES: usize = 2usize * 1024 * 1024 * 1024;
const MAX_GRAPH_GROWTH_ATTEMPTS: usize = 3;
const T5_FINAL_LAYER_NORM_NAMES: [&str; 2] = ["encoder.final_layer_norm.weight", "final_layer_norm.weight"];
const T5_RELATIVE_ATTENTION_BIAS_NAME: &str =
    "encoder.block.0.layer.0.SelfAttention.relative_attention_bias.weight";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct T5ModelConfig {
    pub vocab_size: u32,
    pub model_dim: u32,
    pub feedforward_dim: u32,
    pub layer_count: u32,
    pub attention_head_count: u32,
    pub relative_attention_bucket_count: u32,
    pub relative_attention_max_distance: u32,
    pub layer_norm_epsilon_bits: u32,
}

impl T5ModelConfig {
    pub fn layer_norm_epsilon(&self) -> f32 {
        f32::from_bits(self.layer_norm_epsilon_bits)
    }

    pub fn head_dim(&self) -> u32 {
        self.model_dim / self.attention_head_count
    }
}

#[derive(Clone, Debug)]
pub struct LoadedT5xxlWeights {
    pub ctx: Context,
    pub tensor_ids: BTreeMap<String, TensorId>,
    pub config: T5ModelConfig,
    pub path: PathBuf,
    relative_attention_bias: Vec<f32>,
    graph_extra_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct T5xxlGraph {
    pub graph: Graph,
    pub input_token_ids: TensorId,
    pub result_hidden_states: TensorId,
    pub eos_index: usize,
    pub debug_hidden_states: Vec<(String, TensorId)>,
}

pub struct CompiledT5xxlMetal {
    graph: T5xxlGraph,
    session: MetalGraphSession,
}

#[derive(Clone, Debug)]
pub struct LazyT5xxlMetal {
    token_count: usize,
    eos_index: usize,
    attention_bias: Vec<f32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum T5xxlExecutionMode {
    Lazy,
    Compiled,
}

impl T5xxlExecutionMode {
    pub fn from_env() -> Self {
        match std::env::var("FLUX_T5_MODE") {
            Ok(value) if value.eq_ignore_ascii_case("compiled") => Self::Compiled,
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

#[derive(Clone, Debug)]
pub struct T5xxlRun {
    pub hidden_states: Vec<f32>,
    pub token_count: usize,
    pub hidden_size: usize,
    pub eos_index: usize,
}

#[derive(Clone, Debug)]
struct T5AttentionGraphOutput {
    attn: TensorId,
    debug_tensors: Vec<(String, TensorId)>,
}

#[derive(Clone, Debug)]
struct T5AttentionRowsOutput {
    attn: RowsTensor,
    scores: Option<Vec<f32>>,
    probs: Option<Vec<f32>>,
}

impl LoadedT5xxlWeights {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_extra(path, DEFAULT_GRAPH_EXTRA_BYTES)
    }

    pub fn load_with_extra(path: impl AsRef<Path>, extra_bytes: usize) -> Result<Self> {
        let header = MlxSafetensorsHeader::load(path.as_ref())?;
        let inspect = T5TextEncoderConfig::from_header(&header)?;
        let config = t5_model_config_from_header(&header, &inspect)?;
        let relative_attention_bias = decode_relative_attention_bias(&header, &config)?;
        let total_bytes = t5_weight_total_bytes(&header, extra_bytes)?;
        let mut ctx = Context::new(InitParams {
            mem_size: total_bytes,
            mem_buffer: None,
            no_alloc: false,
        });
        let tensor_ids = allocate_t5_weight_tensors(&mut ctx, &header)?;
        load_t5_weight_bytes(&mut ctx, &header, &tensor_ids)?;

        Ok(Self {
            ctx,
            tensor_ids,
            config,
            path: header.path,
            relative_attention_bias,
            graph_extra_bytes: extra_bytes,
        })
    }

    pub fn tensor_id(&self, name: &str) -> Result<TensorId> {
        self.tensor_ids
            .get(name)
            .copied()
            .ok_or_else(|| DiffusionError::model(format!("missing t5xxl tensor '{}'", name)))
    }

    pub fn tensor_id_candidates(&self, names: &[&str]) -> Result<TensorId> {
        for name in names {
            if let Some(id) = self.tensor_ids.get(*name) {
                return Ok(*id);
            }
        }
        Err(DiffusionError::model(format!(
            "missing t5xxl tensor; tried {}",
            names.join(", ")
        )))
    }

    fn graph_reserve_bytes(&self) -> usize {
        self.graph_extra_bytes
    }

    pub fn relative_attention_bias(&self) -> &[f32] {
        &self.relative_attention_bias
    }

    fn tensor_bytes(&self, name: &str) -> Result<&[u8]> {
        let tensor_id = self.tensor_id(name)?;
        self.ctx
            .tensor_data(tensor_id)
            .map_err(DiffusionError::model)
    }

    fn tensor_bytes_candidates(&self, names: &[&str]) -> Result<&[u8]> {
        let tensor_id = self.tensor_id_candidates(names)?;
        self.ctx
            .tensor_data(tensor_id)
            .map_err(DiffusionError::model)
    }

    fn tensor_matrix(&self, name: &str) -> Result<ResidentMatrix<'_>> {
        resident_matrix(&self.ctx, self.tensor_id(name)?)
    }

    fn tensor_matrix_candidates(&self, names: &[&str]) -> Result<ResidentMatrix<'_>> {
        resident_matrix(&self.ctx, self.tensor_id_candidates(names)?)
    }

    fn tensor_f32_values(&self, name: &str) -> Result<Vec<f32>> {
        let tensor_id = self.tensor_id(name)?;
        tensor_to_f32_vec(&self.ctx, tensor_id)
    }

    fn tensor_f32_values_candidates(&self, names: &[&str]) -> Result<Vec<f32>> {
        let tensor_id = self.tensor_id_candidates(names)?;
        tensor_to_f32_vec(&self.ctx, tensor_id)
    }
}

impl CompiledT5xxlMetal {
    pub fn compile(weights: &mut LoadedT5xxlWeights, prompt: &T5TokenizedPrompt) -> Result<Self> {
        let runtime = MetalRuntime::new().map_err(DiffusionError::model)?;
        Self::compile_with_runtime(runtime, weights, prompt)
    }

    pub fn compile_with_runtime(
        runtime: MetalRuntime,
        weights: &mut LoadedT5xxlWeights,
        prompt: &T5TokenizedPrompt,
    ) -> Result<Self> {
        for attempt in 0..=MAX_GRAPH_GROWTH_ATTEMPTS {
            let graph = match build_t5xxl_graph(weights, prompt) {
                Ok(graph) => graph,
                Err(err) if is_context_oom(&err) && attempt < MAX_GRAPH_GROWTH_ATTEMPTS => {
                    let next_extra = next_graph_reserve_bytes(weights)?;
                    *weights = LoadedT5xxlWeights::load_with_extra(weights.path.clone(), next_extra)?;
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
            "t5xxl graph compilation exhausted context growth attempts",
        ))
    }

    pub fn execute(&self, weights: &LoadedT5xxlWeights, token_ids: &[i32]) -> Result<T5xxlRun> {
        let input_tensor = require_tensor(&weights.ctx, self.graph.input_token_ids)?;
        if input_tensor.ne[0] as usize != token_ids.len() {
            return Err(DiffusionError::workflow(format!(
                "t5xxl token length mismatch: graph expects {}, got {}",
                input_tensor.ne[0],
                token_ids.len()
            )));
        }
        let input_bytes = i32s_to_le_bytes(token_ids);

        let mut requested_outputs = vec![self.graph.result_hidden_states];
        if t5_debug_dir().is_some() {
            requested_outputs.extend(
                self.graph
                    .debug_hidden_states
                    .iter()
                    .map(|(_, tensor_id)| *tensor_id),
            );
        }

        let execution = self
            .session
            .execute(
                &weights.ctx,
                &[MetalGraphTensorWrite {
                    tensor_id: self.graph.input_token_ids,
                    bytes: &input_bytes,
                }],
                &requested_outputs,
            )
            .map_err(DiffusionError::model)?;

        let hidden_bytes = execution.outputs.get(&self.graph.result_hidden_states).ok_or_else(|| {
            DiffusionError::model("t5xxl execution did not return hidden states")
        })?;
        let hidden_tensor = require_tensor(&weights.ctx, self.graph.result_hidden_states)?;
        let hidden_size = usize::try_from(hidden_tensor.ne[0])
            .map_err(|_| DiffusionError::model("t5xxl hidden size exceeds usize"))?;
        let token_count = usize::try_from(hidden_tensor.ne[1])
            .map_err(|_| DiffusionError::model("t5xxl token count exceeds usize"))?;
        if let Some(debug_dir) = t5_debug_dir() {
            dump_t5_debug_outputs(
                &debug_dir,
                &execution.outputs,
                &self.graph.debug_hidden_states,
                hidden_size,
                token_count,
            )?;
        }

        Ok(T5xxlRun {
            hidden_states: f32_bytes_to_vec(hidden_bytes)?,
            token_count,
            hidden_size,
            eos_index: self.graph.eos_index,
        })
    }
}

impl LazyT5xxlMetal {
    pub fn compile(weights: &mut LoadedT5xxlWeights, prompt: &T5TokenizedPrompt) -> Result<Self> {
        Self::compile_internal(weights, prompt)
    }

    pub fn compile_with_runtime(
        _runtime: MetalRuntime,
        weights: &mut LoadedT5xxlWeights,
        prompt: &T5TokenizedPrompt,
    ) -> Result<Self> {
        Self::compile_internal(weights, prompt)
    }

    fn compile_internal(weights: &mut LoadedT5xxlWeights, prompt: &T5TokenizedPrompt) -> Result<Self> {
        let token_count = prompt.token_ids.len();
        if token_count == 0 {
            return Err(DiffusionError::workflow(
                "t5xxl lazy executor needs at least one token",
            ));
        }
        let attention_bias = attention_bias_values(weights, token_count)?;
        Ok(Self {
            token_count,
            eos_index: prompt.eos_index,
            attention_bias,
        })
    }

    pub fn execute(&self, weights: &LoadedT5xxlWeights, token_ids: &[i32]) -> Result<T5xxlRun> {
        if token_ids.len() != self.token_count {
            return Err(DiffusionError::workflow(format!(
                "t5xxl token length mismatch: executor expects {}, got {}",
                self.token_count,
                token_ids.len()
            )));
        }

        let model_dim = usize::try_from(weights.config.model_dim)
            .map_err(|_| DiffusionError::model("t5xxl model dim exceeds usize"))?;
        let head_count = usize::try_from(weights.config.attention_head_count)
            .map_err(|_| DiffusionError::model("t5xxl head count exceeds usize"))?;
        let head_dim = usize::try_from(weights.config.head_dim())
            .map_err(|_| DiffusionError::model("t5xxl head dim exceeds usize"))?;
        let feedforward_dim = usize::try_from(weights.config.feedforward_dim)
            .map_err(|_| DiffusionError::model("t5xxl feedforward dim exceeds usize"))?;
        let debug_dir = t5_debug_dir();
        let dump_t5_debug = debug_dir.is_some();
        let dump_t5_debug_stages = dump_t5_debug && t5_debug_stages_enabled();
        let debug_stage_layer = t5_debug_stage_layer().unwrap_or(0);
        let mut debug_hidden_states = Vec::new();

        let mut hidden = embed_t5_tokens(weights, token_ids, model_dim)?;
        if dump_t5_debug {
            debug_hidden_states.push(("t5_embed".to_string(), hidden.data.clone()));
        }
        for layer in 0..weights.config.layer_count as usize {
            let attn_prefix = format!("encoder.block.{layer}.layer.0");
            let ff_prefix = format!("encoder.block.{layer}.layer.1");

            let norm1 = rms_norm_rows_with_weight(
                &hidden,
                weights.tensor_f32_values(&format!("{attn_prefix}.layer_norm.weight"))?.as_slice(),
                weights.config.layer_norm_epsilon(),
            )?;
            let debug_stage_prefix = format!("t5_block_{layer:02}");
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_norm1"), norm1.data.clone()));
            }
            let q = linear_rows_ggml(
                weights,
                &norm1,
                &format!("{attn_prefix}.SelfAttention.q.weight"),
                1.0,
            )?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_q_linear"), q.data.clone()));
            }
            let k = linear_rows_ggml(
                weights,
                &norm1,
                &format!("{attn_prefix}.SelfAttention.k.weight"),
                1.0,
            )?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_k_linear"), k.data.clone()));
            }
            let v = linear_rows_ggml(
                weights,
                &norm1,
                &format!("{attn_prefix}.SelfAttention.v.weight"),
                1.0,
            )?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_v_linear"), v.data.clone()));
            }
            let attn = t5_attention_rows(
                &q,
                &k,
                &v,
                &self.attention_bias,
                self.token_count,
                head_count,
                head_dim,
                dump_t5_debug_stages && layer == debug_stage_layer,
            )?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                if let Some(scores) = attn.scores.as_ref() {
                    debug_hidden_states.push((format!("{debug_stage_prefix}_scores"), scores.clone()));
                }
                if let Some(probs) = attn.probs.as_ref() {
                    debug_hidden_states.push((format!("{debug_stage_prefix}_probs"), probs.clone()));
                }
                debug_hidden_states.push((format!("{debug_stage_prefix}_attn"), attn.attn.data.clone()));
            }
            let attn_proj = linear_rows_ggml(
                weights,
                &attn.attn,
                &format!("{attn_prefix}.SelfAttention.o.weight"),
                1.0,
            )?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_attn_proj"), attn_proj.data.clone()));
            }
            hidden = add_rows(&hidden, &attn_proj)?;

            let norm2 = rms_norm_rows_with_weight(
                &hidden,
                weights.tensor_f32_values(&format!("{ff_prefix}.layer_norm.weight"))?.as_slice(),
                weights.config.layer_norm_epsilon(),
            )?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_norm2"), norm2.data.clone()));
            }
            let wi0 = linear_rows_ggml(
                weights,
                &norm2,
                &format!("{ff_prefix}.DenseReluDense.wi_0.weight"),
                1.0,
            )?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_wi0_linear"), wi0.data.clone()));
            }
            let wi1 = linear_rows_ggml(
                weights,
                &norm2,
                &format!("{ff_prefix}.DenseReluDense.wi_1.weight"),
                1.0,
            )?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_wi1_linear"), wi1.data.clone()));
            }
            let wi0 = gelu_rows(&wi0)?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_wi0_gelu"), wi0.data.clone()));
            }
            let gated = mul_rows(&wi0, &wi1)?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_gated"), gated.data.clone()));
            }
            let ff_out = linear_rows_ggml(
                weights,
                &gated,
                &format!("{ff_prefix}.DenseReluDense.wo.weight"),
                T5_GATED_FF_OUTPUT_INPUT_SCALE,
            )?;
            if dump_t5_debug_stages && layer == debug_stage_layer {
                debug_hidden_states.push((format!("{debug_stage_prefix}_ff_out"), ff_out.data.clone()));
            }
            if ff_out.cols != model_dim || ff_out.rows != self.token_count {
                return Err(DiffusionError::model(format!(
                    "t5xxl ff_out shape mismatch: got {}x{}, expected {}x{}",
                    ff_out.rows, ff_out.cols, self.token_count, model_dim
                )));
            }
            if wi0.cols != feedforward_dim || wi1.cols != feedforward_dim {
                return Err(DiffusionError::model(format!(
                    "t5xxl feedforward shape mismatch: wi0={} wi1={} expected {}",
                    wi0.cols, wi1.cols, feedforward_dim
                )));
            }
            hidden = add_rows(&hidden, &ff_out)?;
            if dump_t5_debug {
                debug_hidden_states.push((format!("t5_block_{layer:02}"), hidden.data.clone()));
            }
        }

        let final_hidden = rms_norm_rows_with_weight(
            &hidden,
            weights.tensor_f32_values_candidates(&T5_FINAL_LAYER_NORM_NAMES)?.as_slice(),
            weights.config.layer_norm_epsilon(),
        )?;
        if dump_t5_debug {
            debug_hidden_states.push(("t5_final".to_string(), final_hidden.data.clone()));
        }
        if let Some(debug_dir) = debug_dir.as_ref() {
            dump_t5_debug_rows(debug_dir, &debug_hidden_states, model_dim, self.token_count)?;
        }

        Ok(T5xxlRun {
            hidden_states: final_hidden.data,
            token_count: self.token_count,
            hidden_size: model_dim,
            eos_index: self.eos_index,
        })
    }
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

impl RowsTensor {
    fn new(rows: usize, cols: usize, data: Vec<f32>) -> Result<Self> {
        let expected = rows
            .checked_mul(cols)
            .ok_or_else(|| DiffusionError::model("t5xxl rows tensor size overflow"))?;
        if data.len() != expected {
            return Err(DiffusionError::model(format!(
                "t5xxl rows tensor len mismatch: expected {}, got {}",
                expected,
                data.len()
            )));
        }
        Ok(Self { rows, cols, data })
    }

    fn row(&self, row_idx: usize) -> &[f32] {
        let start = row_idx * self.cols;
        &self.data[start..start + self.cols]
    }
}

fn embed_t5_tokens(weights: &LoadedT5xxlWeights, token_ids: &[i32], model_dim: usize) -> Result<RowsTensor> {
    let embedding = weights.tensor_matrix("shared.weight")?;
    if embedding.cols != model_dim {
        return Err(DiffusionError::model(format!(
            "t5xxl embedding width mismatch: expected {} got {}",
            model_dim, embedding.cols
        )));
    }
    let values = if let Some(values) = try_get_rows_ggml_bytes(
        embedding.bytes,
        embedding.ggml_type,
        embedding.cols,
        embedding.rows,
        token_ids,
    ) {
        values
    } else {
        get_rows_ggml_bytes_cpu(
            embedding.bytes,
            embedding.ggml_type,
            embedding.cols,
            embedding.rows,
            token_ids,
        )
        .ok_or_else(|| DiffusionError::model("t5xxl embedding gather fallback failed"))?
    };
    RowsTensor::new(token_ids.len(), model_dim, values)
}

fn linear_rows_ggml(
    weights: &LoadedT5xxlWeights,
    input: &RowsTensor,
    weight_name: &str,
    input_scale: f32,
) -> Result<RowsTensor> {
    let weight = weights.tensor_matrix(weight_name)?;
    linear_rows_ggml_matrix(input, weight, input_scale)
}

fn linear_rows_ggml_matrix(
    input: &RowsTensor,
    weight: ResidentMatrix<'_>,
    input_scale: f32,
) -> Result<RowsTensor> {
    if input.cols != weight.cols {
        return Err(DiffusionError::model(format!(
            "t5xxl linear input width mismatch: input={} weight={}",
            input.cols, weight.cols
        )));
    }
    if input.rows == 0 {
        return RowsTensor::new(0, weight.rows, Vec::new());
    }
    let scaled_input;
    let input_values = if input_scale == 1.0 {
        &input.data
    } else {
        scaled_input = input.data.iter().map(|value| value * input_scale).collect::<Vec<_>>();
        &scaled_input
    };
    let mut output = if t5_force_cpu_math() || t5_force_f32_linear() {
        let dequantized = decode_ggml_matrix_to_f32(weight)?;
        if t5_force_cpu_math() {
            matmul_nt_f32_cpu(input_values, &dequantized, input.rows, input.cols, weight.rows)?
        } else if let Some(output) =
            try_matmul_nt_f32(input_values, &dequantized, input.rows, input.cols, weight.rows)
        {
            output
        } else {
            matmul_nt_f32_cpu(input_values, &dequantized, input.rows, input.cols, weight.rows)?
        }
    } else if let Some(output) = try_matmul_nt_ggml_bytes(
        input_values,
        weight.bytes,
        weight.ggml_type,
        input.rows,
        input.cols,
        weight.rows,
    ) {
        output
    } else {
        let dequantized = decode_ggml_matrix_to_f32(weight)?;
        matmul_nt_f32_cpu(input_values, &dequantized, input.rows, input.cols, weight.rows)?
    };
    if input_scale != 1.0 {
        let inv_scale = 1.0 / input_scale;
        for value in &mut output {
            *value *= inv_scale;
        }
    }
    RowsTensor::new(input.rows, weight.rows, output)
}

fn rms_norm_rows_with_weight(input: &RowsTensor, weight: &[f32], eps: f32) -> Result<RowsTensor> {
    if input.cols != weight.len() {
        return Err(DiffusionError::model(format!(
            "t5xxl rms_norm weight mismatch: input cols={} weight len={}",
            input.cols,
            weight.len()
        )));
    }
    if input.rows == 0 {
        return RowsTensor::new(0, input.cols, Vec::new());
    }
    if !t5_force_cpu_math() {
        if let Some(output) = try_rms_norm_mul_f32(
            &input.data,
            &[input.rows, input.cols],
            weight,
            &[weight.len()],
            eps,
        ) {
            return RowsTensor::new(input.rows, input.cols, output);
        }
    }
    let mut output = Vec::with_capacity(input.data.len());
    for row in input.data.chunks_exact(input.cols) {
        let mean_square = row.iter().map(|value| value * value).sum::<f32>() / input.cols as f32;
        let inv_rms = 1.0 / (mean_square + eps).sqrt();
        for (value, scale) in row.iter().zip(weight.iter()) {
            output.push(value * inv_rms * scale);
        }
    }
    RowsTensor::new(input.rows, input.cols, output)
}

fn add_rows(lhs: &RowsTensor, rhs: &RowsTensor) -> Result<RowsTensor> {
    if lhs.rows != rhs.rows || lhs.cols != rhs.cols {
        return Err(DiffusionError::model(format!(
            "t5xxl add shape mismatch: lhs={}x{} rhs={}x{}",
            lhs.rows, lhs.cols, rhs.rows, rhs.cols
        )));
    }
    if lhs.rows == 0 {
        return RowsTensor::new(0, lhs.cols, Vec::new());
    }
    if !t5_force_cpu_math() {
        if let Some(output) = try_add_f32(
            &lhs.data,
            &[lhs.rows, lhs.cols],
            &rhs.data,
            &[rhs.rows, rhs.cols],
        ) {
            return RowsTensor::new(lhs.rows, lhs.cols, output);
        }
    }
    let output = lhs
        .data
        .iter()
        .zip(rhs.data.iter())
        .map(|(lhs_value, rhs_value)| lhs_value + rhs_value)
        .collect::<Vec<_>>();
    RowsTensor::new(lhs.rows, lhs.cols, output)
}

fn mul_rows(lhs: &RowsTensor, rhs: &RowsTensor) -> Result<RowsTensor> {
    if lhs.rows != rhs.rows || lhs.cols != rhs.cols {
        return Err(DiffusionError::model(format!(
            "t5xxl mul shape mismatch: lhs={}x{} rhs={}x{}",
            lhs.rows, lhs.cols, rhs.rows, rhs.cols
        )));
    }
    if lhs.rows == 0 {
        return RowsTensor::new(0, lhs.cols, Vec::new());
    }
    if !t5_force_cpu_math() {
        if let Some(output) = try_mul_f32(
            &lhs.data,
            &[lhs.rows, lhs.cols],
            &rhs.data,
            &[rhs.rows, rhs.cols],
        ) {
            return RowsTensor::new(lhs.rows, lhs.cols, output);
        }
    }
    let output = lhs
        .data
        .iter()
        .zip(rhs.data.iter())
        .map(|(lhs_value, rhs_value)| lhs_value * rhs_value)
        .collect::<Vec<_>>();
    RowsTensor::new(lhs.rows, lhs.cols, output)
}

fn gelu_rows(input: &RowsTensor) -> Result<RowsTensor> {
    if input.rows == 0 {
        return RowsTensor::new(0, input.cols, Vec::new());
    }
    if !t5_force_cpu_math() {
        if let Some(output) = try_gelu_f32(&input.data, &[input.rows, input.cols]) {
            return RowsTensor::new(input.rows, input.cols, output);
        }
    }
    let output = input
        .data
        .iter()
        .copied()
        .map(gelu_approx)
        .collect::<Vec<_>>();
    RowsTensor::new(input.rows, input.cols, output)
}

fn t5_attention_rows(
    q: &RowsTensor,
    k: &RowsTensor,
    v: &RowsTensor,
    attention_bias: &[f32],
    token_count: usize,
    head_count: usize,
    head_dim: usize,
    dump_debug_stages: bool,
) -> Result<T5AttentionRowsOutput> {
    if q.rows != token_count || k.rows != token_count || v.rows != token_count {
        return Err(DiffusionError::model("t5xxl attention token count mismatch"));
    }
    if q.cols != head_count * head_dim || k.cols != head_count * head_dim || v.cols != head_count * head_dim {
        return Err(DiffusionError::model(format!(
            "t5xxl attention width mismatch: q={} k={} v={} expected {}",
            q.cols,
            k.cols,
            v.cols,
            head_count * head_dim
        )));
    }
    let head_bias_len = token_count
        .checked_mul(token_count)
        .ok_or_else(|| DiffusionError::model("t5xxl attention head bias overflow"))?;
    if attention_bias.len()
        != head_bias_len
            .checked_mul(head_count)
            .ok_or_else(|| DiffusionError::model("t5xxl attention bias size overflow"))?
    {
        return Err(DiffusionError::model(format!(
            "t5xxl attention bias len mismatch: got {} expected {}",
            attention_bias.len(),
            head_bias_len * head_count
        )));
    }
    let mut output = vec![0.0f32; token_count * head_count * head_dim];
    let mut debug_scores = dump_debug_stages.then(|| Vec::with_capacity(attention_bias.len()));
    let mut debug_probs = dump_debug_stages.then(|| Vec::with_capacity(attention_bias.len()));
    for head_idx in 0..head_count {
        let q_head = extract_head_rows(q, head_idx, head_dim);
        let k_head = extract_head_rows(k, head_idx, head_dim);
        let v_head = extract_head_rows(v, head_idx, head_dim);
        let force_cpu_attention = t5_force_cpu_math() || t5_force_cpu_attention();
        let mut scores = if force_cpu_attention {
            matmul_nt_f32_cpu(&q_head, &k_head, token_count, head_dim, token_count)?
        } else if let Some(scores) =
            try_matmul_nt_f32(&q_head, &k_head, token_count, head_dim, token_count)
        {
            scores
        } else {
            matmul_nt_f32_cpu(&q_head, &k_head, token_count, head_dim, token_count)?
        };
        let head_bias =
            &attention_bias[head_idx * head_bias_len..(head_idx + 1) * head_bias_len];
        if dump_debug_stages {
            add_bias_in_place(&mut scores, head_bias)?;
            if let Some(debug_scores) = debug_scores.as_mut() {
                debug_scores.extend_from_slice(&scores);
            }
            softmax_in_place(&mut scores, token_count)?;
            if let Some(debug_probs) = debug_probs.as_mut() {
                debug_probs.extend_from_slice(&scores);
            }
        } else {
            apply_bias_softmax_in_place(&mut scores, head_bias, token_count)?;
        }
        let head_output = if force_cpu_attention {
            matmul_nn_f32_cpu(&scores, &v_head, token_count, token_count, head_dim)?
        } else if let Some(head_output) =
            try_matmul_nn_f32(&scores, &v_head, token_count, token_count, head_dim)
        {
            head_output
        } else {
            matmul_nn_f32_cpu(&scores, &v_head, token_count, token_count, head_dim)?
        };
        write_head_rows(&mut output, token_count, head_count, head_dim, head_idx, &head_output)?;
    }
    Ok(T5AttentionRowsOutput {
        attn: RowsTensor::new(token_count, head_count * head_dim, output)?,
        scores: debug_scores,
        probs: debug_probs,
    })
}

pub fn build_t5xxl_graph(weights: &mut LoadedT5xxlWeights, prompt: &T5TokenizedPrompt) -> Result<T5xxlGraph> {
    let n_tokens = prompt.token_ids.len();
    let model_dim = i64::from(weights.config.model_dim);
    let head_count = i64::from(weights.config.attention_head_count);
    let head_dim = i64::from(weights.config.head_dim());
    if head_dim * head_count != model_dim {
        return Err(DiffusionError::model(format!(
            "t5xxl model dim {} is incompatible with head count {}",
            model_dim, head_count
        )));
    }

    let input_token_ids = weights
        .ctx
        .new_named_tensor(
            "t5xxl.input_token_ids",
            TensorType::I32,
            1,
            &[n_tokens as i64],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;

    let attention_bias = weights
        .ctx
        .new_named_tensor(
            "t5xxl.attention_bias",
            TensorType::F32,
            4,
            &[
                n_tokens as i64,
                n_tokens as i64,
                i64::from(weights.config.attention_head_count),
                1,
            ],
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    let attention_bias_bytes = attention_bias_f32_bytes(weights, n_tokens)?;
    weights
        .ctx
        .write_tensor_data(attention_bias, &attention_bias_bytes)
        .map_err(DiffusionError::model)?;
    let dump_t5_debug = t5_debug_dir().is_some();
    let dump_t5_debug_stages = dump_t5_debug && t5_debug_stages_enabled();
    let mut debug_hidden_states = Vec::new();

    let mut hidden = weights
        .ctx
        .get_rows(weights.tensor_id("shared.weight")?, input_token_ids, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    hidden = weights
        .ctx
        .cont_2d(hidden, model_dim, n_tokens as i64)
        .map_err(DiffusionError::model)?;
    if dump_t5_debug {
        debug_hidden_states.push(("t5_embed".to_string(), hidden));
    }

    for layer in 0..weights.config.layer_count as usize {
        let attn_prefix = format!("encoder.block.{layer}.layer.0");
        let ff_prefix = format!("encoder.block.{layer}.layer.1");

        let norm1 = apply_rms_norm(
            &mut weights.ctx,
            &weights.tensor_ids,
            hidden,
            &format!("{attn_prefix}.layer_norm.weight"),
            weights.config.layer_norm_epsilon(),
        )?;
        let q = apply_linear_no_bias(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm1,
            &format!("{attn_prefix}.SelfAttention.q.weight"),
            1.0,
        )?;
        let k = apply_linear_no_bias(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm1,
            &format!("{attn_prefix}.SelfAttention.k.weight"),
            1.0,
        )?;
        let v = apply_linear_no_bias(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm1,
            &format!("{attn_prefix}.SelfAttention.v.weight"),
            1.0,
        )?;

        if dump_t5_debug_stages && layer == 0 {
            debug_hidden_states.push((
                "t5_block_00_norm1".to_string(),
                weights.ctx.cont(norm1).map_err(DiffusionError::model)?,
            ));
            debug_hidden_states.push((
                "t5_block_00_q_linear".to_string(),
                weights.ctx.cont(q).map_err(DiffusionError::model)?,
            ));
            debug_hidden_states.push((
                "t5_block_00_k_linear".to_string(),
                weights.ctx.cont(k).map_err(DiffusionError::model)?,
            ));
            debug_hidden_states.push((
                "t5_block_00_v_linear".to_string(),
                weights.ctx.cont(v).map_err(DiffusionError::model)?,
            ));
        }

        let attn = build_attention_mha_output(
            &mut weights.ctx,
            q,
            k,
            v,
            attention_bias,
            head_dim,
            head_count,
            n_tokens as i64,
            if dump_t5_debug_stages && layer == 0 {
                Some("t5_block_00")
            } else {
                None
            },
        )?;
        debug_hidden_states.extend(attn.debug_tensors);
        let attn_proj = apply_linear_no_bias(
            &mut weights.ctx,
            &weights.tensor_ids,
            attn.attn,
            &format!("{attn_prefix}.SelfAttention.o.weight"),
            1.0,
        )?;
        if dump_t5_debug_stages && layer == 0 {
            debug_hidden_states.push((
                "t5_block_00_attn_proj".to_string(),
                weights.ctx.cont(attn_proj).map_err(DiffusionError::model)?,
            ));
        }
        hidden = weights
            .ctx
            .binary_like_a(Op::Add, hidden, attn_proj, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;

        let norm2 = apply_rms_norm(
            &mut weights.ctx,
            &weights.tensor_ids,
            hidden,
            &format!("{ff_prefix}.layer_norm.weight"),
            weights.config.layer_norm_epsilon(),
        )?;
        if dump_t5_debug_stages && layer == 0 {
            debug_hidden_states.push((
                "t5_block_00_norm2".to_string(),
                weights.ctx.cont(norm2).map_err(DiffusionError::model)?,
            ));
        }
        let wi0 = apply_linear_no_bias(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm2,
            &format!("{ff_prefix}.DenseReluDense.wi_0.weight"),
            1.0,
        )?;
        let wi1 = apply_linear_no_bias(
            &mut weights.ctx,
            &weights.tensor_ids,
            norm2,
            &format!("{ff_prefix}.DenseReluDense.wi_1.weight"),
            1.0,
        )?;
        if dump_t5_debug_stages && layer == 0 {
            debug_hidden_states.push((
                "t5_block_00_wi0_linear".to_string(),
                weights.ctx.cont(wi0).map_err(DiffusionError::model)?,
            ));
            debug_hidden_states.push((
                "t5_block_00_wi1_linear".to_string(),
                weights.ctx.cont(wi1).map_err(DiffusionError::model)?,
            ));
        }
        let wi0 = gelu(&mut weights.ctx, wi0)?;
        if dump_t5_debug_stages && layer == 0 {
            debug_hidden_states.push((
                "t5_block_00_wi0_gelu".to_string(),
                weights.ctx.cont(wi0).map_err(DiffusionError::model)?,
            ));
        }
        let gated = weights
            .ctx
            .binary_like_a(Op::Mul, wi0, wi1, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
        if dump_t5_debug_stages && layer == 0 {
            debug_hidden_states.push((
                "t5_block_00_gated".to_string(),
                weights.ctx.cont(gated).map_err(DiffusionError::model)?,
            ));
        }
        let ff_out = apply_linear_no_bias(
            &mut weights.ctx,
            &weights.tensor_ids,
            gated,
            &format!("{ff_prefix}.DenseReluDense.wo.weight"),
            T5_GATED_FF_OUTPUT_INPUT_SCALE,
        )?;
        if dump_t5_debug_stages && layer == 0 {
            debug_hidden_states.push((
                "t5_block_00_ff_out".to_string(),
                weights.ctx.cont(ff_out).map_err(DiffusionError::model)?,
            ));
        }
        hidden = weights
            .ctx
            .binary_like_a(Op::Add, hidden, ff_out, BufferUsage::Activations)
            .map_err(DiffusionError::model)?;
        if dump_t5_debug {
            let debug_hidden = weights
                .ctx
                .cont(hidden)
                .map_err(DiffusionError::model)?;
            debug_hidden_states.push((format!("t5_block_{layer:02}"), debug_hidden));
        }
    }

    let result_hidden_states = apply_rms_norm_candidates(
        &mut weights.ctx,
        &weights.tensor_ids,
        hidden,
        &T5_FINAL_LAYER_NORM_NAMES,
        weights.config.layer_norm_epsilon(),
    )?;
    if dump_t5_debug {
        let debug_hidden = weights
            .ctx
            .cont(result_hidden_states)
            .map_err(DiffusionError::model)?;
        debug_hidden_states.push(("t5_final".to_string(), debug_hidden));
    }

    let mut graph = Graph::new();
    graph
        .build_forward_expand(&weights.ctx, result_hidden_states)
        .map_err(DiffusionError::model)?;
    for (_, tensor_id) in &debug_hidden_states {
        graph
            .build_forward_expand(&weights.ctx, *tensor_id)
            .map_err(DiffusionError::model)?;
    }

    Ok(T5xxlGraph {
        graph,
        input_token_ids,
        result_hidden_states,
        eos_index: prompt.eos_index,
        debug_hidden_states,
    })
}

fn build_attention_mha_output(
    ctx: &mut Context,
    q: TensorId,
    k: TensorId,
    v: TensorId,
    attention_bias: TensorId,
    head_dim: i64,
    head_count: i64,
    token_count: i64,
    debug_prefix: Option<&str>,
) -> Result<T5AttentionGraphOutput> {
    let mut debug_tensors = Vec::new();

    let q = ctx
        .reshape(q, &[head_dim, head_count, token_count])
        .map_err(DiffusionError::model)?;
    let q = ctx.permute(q, [0, 2, 1, 3]).map_err(DiffusionError::model)?;
    let q = ctx.cont(q).map_err(DiffusionError::model)?;
    let q = ctx
        .reshape(q, &[head_dim, token_count, head_count])
        .map_err(DiffusionError::model)?;

    let k = ctx
        .reshape(k, &[head_dim, head_count, token_count])
        .map_err(DiffusionError::model)?;
    let k = ctx.permute(k, [0, 2, 1, 3]).map_err(DiffusionError::model)?;
    let k = ctx.cont(k).map_err(DiffusionError::model)?;
    let k = ctx
        .reshape(k, &[head_dim, token_count, head_count])
        .map_err(DiffusionError::model)?;

    let v = ctx
        .reshape(v, &[head_dim, head_count, token_count])
        .map_err(DiffusionError::model)?;
    let v = ctx.permute(v, [1, 2, 0, 3]).map_err(DiffusionError::model)?;
    let v = ctx.cont(v).map_err(DiffusionError::model)?;
    let v = ctx
        .reshape(v, &[token_count, head_dim, head_count])
        .map_err(DiffusionError::model)?;

    let mut kq = ctx.mul_mat(k, q, BufferUsage::Activations).map_err(DiffusionError::model)?;
    kq = ctx
        .binary_like_a(Op::Add, kq, attention_bias, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    if let Some(prefix) = debug_prefix {
        debug_tensors.push((
            format!("{prefix}_scores"),
            ctx.cont(kq).map_err(DiffusionError::model)?,
        ));
    }
    kq = ctx
        .soft_max(kq, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    if let Some(prefix) = debug_prefix {
        debug_tensors.push((
            format!("{prefix}_probs"),
            ctx.cont(kq).map_err(DiffusionError::model)?,
        ));
    }

    let kqv = ctx
        .mul_mat(v, kq, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let kqv = ctx
        .reshape(kqv, &[head_dim, token_count, head_count])
        .map_err(DiffusionError::model)?;
    let attn = ctx.permute(kqv, [0, 2, 1, 3]).map_err(DiffusionError::model)?;
    let attn = ctx.cont(attn).map_err(DiffusionError::model)?;
    let attn = ctx
        .reshape(attn, &[head_dim * head_count, token_count])
        .map_err(DiffusionError::model)?;
    if let Some(prefix) = debug_prefix {
        debug_tensors.push((
            format!("{prefix}_attn"),
            ctx.cont(attn).map_err(DiffusionError::model)?,
        ));
    }
    Ok(T5AttentionGraphOutput { attn, debug_tensors })
}

fn apply_rms_norm(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight_name: &str,
    epsilon: f32,
) -> Result<TensorId> {
    let norm = ctx
        .rms_norm_eps(input, epsilon, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let weight = repeat_weight(ctx, require_tensor_id(tensor_ids, weight_name)?, norm)?;
    ctx.binary_like_a(Op::Mul, norm, weight, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn apply_rms_norm_candidates(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight_names: &[&str],
    epsilon: f32,
) -> Result<TensorId> {
    let weight = require_tensor_id_candidates(tensor_ids, weight_names)?;
    let norm = ctx
        .rms_norm_eps(input, epsilon, BufferUsage::Activations)
        .map_err(DiffusionError::model)?;
    let weight = repeat_weight(ctx, weight, norm)?;
    ctx.binary_like_a(Op::Mul, norm, weight, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn apply_linear_no_bias(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    weight_name: &str,
    input_scale: f32,
) -> Result<TensorId> {
    let input = if input_scale == 1.0 {
        input
    } else {
        ctx.scale(input, input_scale, BufferUsage::Activations)
            .map_err(DiffusionError::model)?
    };
    let output = ctx
        .mul_mat(
            require_tensor_id(tensor_ids, weight_name)?,
            input,
            BufferUsage::Activations,
        )
        .map_err(DiffusionError::model)?;
    if input_scale == 1.0 {
        Ok(output)
    } else {
        ctx.scale(output, 1.0 / input_scale, BufferUsage::Activations)
            .map_err(DiffusionError::model)
    }
}

fn gelu(ctx: &mut Context, input: TensorId) -> Result<TensorId> {
    let input = ctx.cont(input).map_err(DiffusionError::model)?;
    ctx.unary(input, UnaryOp::Gelu, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn repeat_weight(ctx: &mut Context, weight: TensorId, shape_of: TensorId) -> Result<TensorId> {
    ctx.repeat(weight, shape_of, BufferUsage::Activations)
        .map_err(DiffusionError::model)
}

fn allocate_t5_weight_tensors(
    ctx: &mut Context,
    header: &MlxSafetensorsHeader,
) -> Result<BTreeMap<String, TensorId>> {
    let mut tensor_ids = BTreeMap::new();
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let entry = header.tensor(&name).ok_or_else(|| {
            DiffusionError::model(format!(
                "t5xxl header lost tensor '{}' while allocating",
                name
            ))
        })?;
        let ty = t5_target_tensor_type(entry)?;
        let extents = t5_target_extents(entry)?;
        let id = ctx
            .new_named_tensor(name.clone(), ty, extents.len(), &extents, BufferUsage::Weights)
            .map_err(DiffusionError::model)?;
        tensor_ids.insert(name, id);
    }
    Ok(tensor_ids)
}

fn load_t5_weight_bytes(
    ctx: &mut Context,
    header: &MlxSafetensorsHeader,
    tensor_ids: &BTreeMap<String, TensorId>,
) -> Result<()> {
    for (name, tensor_id) in tensor_ids {
        let entry = header
            .tensor(name)
            .ok_or_else(|| DiffusionError::model(format!("t5xxl header missing tensor '{}'", name)))?;
        let bytes = t5_target_bytes(header, entry, name)?;
        ctx.write_tensor_data(*tensor_id, &bytes)
            .map_err(DiffusionError::model)?;
    }
    Ok(())
}

fn t5_model_config_from_header(
    header: &MlxSafetensorsHeader,
    inspect: &T5TextEncoderConfig,
) -> Result<T5ModelConfig> {
    t5_model_config_from_tensors(&header.tensors, &header.path, inspect)
}

fn t5_model_config_from_tensors(
    tensors: &HashMap<String, MlxTensorEntry>,
    path: &Path,
    inspect: &T5TextEncoderConfig,
) -> Result<T5ModelConfig> {
    let relative_attention_bias = tensors.get(T5_RELATIVE_ATTENTION_BIAS_NAME).ok_or_else(|| {
        DiffusionError::model(format!("t5xxl relative attention bias missing in {}", path.display()))
    })?;
    let attention_head_count = shape_dim(relative_attention_bias, 1).ok_or_else(|| {
        DiffusionError::model("t5xxl relative attention bias missing head dimension")
    })?;
    let relative_attention_bucket_count = shape_dim(relative_attention_bias, 0).ok_or_else(|| {
        DiffusionError::model("t5xxl relative attention bias missing bucket dimension")
    })?;

    if inspect.model_dim % attention_head_count != 0 {
        return Err(DiffusionError::model(format!(
            "t5xxl model dim {} is not divisible by attention head count {}",
            inspect.model_dim, attention_head_count
        )));
    }

    Ok(T5ModelConfig {
        vocab_size: inspect.vocab_size,
        model_dim: inspect.model_dim,
        feedforward_dim: inspect.feedforward_dim,
        layer_count: inspect.layer_count,
        attention_head_count,
        relative_attention_bucket_count,
        relative_attention_max_distance: T5_RELATIVE_MAX_DISTANCE,
        layer_norm_epsilon_bits: T5_LAYER_NORM_EPSILON.to_bits(),
    })
}

fn t5_weight_total_bytes(header: &MlxSafetensorsHeader, extra_bytes: usize) -> Result<usize> {
    let mut total = 0usize;
    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let entry = header.tensor(&name).unwrap();
        total = ggml_pad(total, GGML_MEM_ALIGN);
        total = total
            .checked_add(t5_target_nbytes(entry)?)
            .ok_or_else(|| DiffusionError::model(format!("t5xxl total bytes overflow at '{}'", name)))?;
    }
    total = ggml_pad(total, GGML_MEM_ALIGN);
    total
        .checked_add(extra_bytes)
        .ok_or_else(|| DiffusionError::model("t5xxl context size overflow"))
}

fn t5_target_nbytes(entry: &MlxTensorEntry) -> Result<usize> {
    let ty = t5_target_tensor_type(entry)?;
    let extents = t5_target_extents(entry)?;
    let layout = TensorLayout::for_ggml(ty, &extents).map_err(DiffusionError::model)?;
    Ok(Tensor::from_desc(0, TensorDesc::new(ty, layout, BufferUsage::Weights)).nbytes())
}

fn t5_target_extents(entry: &MlxTensorEntry) -> Result<Vec<i64>> {
    match entry.shape.as_slice() {
        [dim] => Ok(vec![i64::try_from(*dim)
            .map_err(|_| DiffusionError::model(format!("t5xxl extent {} exceeds i64", dim)))?]),
        [dim0, dim1] => Ok(vec![
            i64::try_from(*dim1)
                .map_err(|_| DiffusionError::model(format!("t5xxl extent {} exceeds i64", dim1)))?,
            i64::try_from(*dim0)
                .map_err(|_| DiffusionError::model(format!("t5xxl extent {} exceeds i64", dim0)))?,
        ]),
        other => Err(DiffusionError::model(format!(
            "t5xxl only supports rank1/rank2 tensors today, got {:?}",
            other
        ))),
    }
}

fn t5_target_tensor_type(entry: &MlxTensorEntry) -> Result<TensorType> {
    match entry.dtype {
        MlxDType::F16 | MlxDType::BF16 if entry.shape.len() == 1 => Ok(TensorType::F32),
        MlxDType::F16 => Ok(TensorType::F16),
        MlxDType::BF16 => Ok(TensorType::BF16),
        MlxDType::F32 => Ok(TensorType::F32),
        other => Err(DiffusionError::model(format!(
            "t5xxl unsupported tensor dtype {:?}",
            other
        ))),
    }
}

fn t5_target_bytes(
    header: &MlxSafetensorsHeader,
    entry: &MlxTensorEntry,
    name: &str,
) -> Result<Vec<u8>> {
    match entry.dtype {
        MlxDType::F32 => header.read_tensor_bytes(name).map_err(Into::into),
        MlxDType::F16 if entry.shape.len() == 1 => {
            let bytes = header.read_tensor_bytes(name)?;
            let mut out = Vec::with_capacity(bytes.len() * 2);
            for value in f16_bytes_to_f32_vec(&bytes)? {
                out.extend_from_slice(&value.to_le_bytes());
            }
            Ok(out)
        }
        MlxDType::BF16 if entry.shape.len() == 1 => {
            let bytes = header.read_tensor_bytes(name)?;
            let mut out = Vec::with_capacity(bytes.len() * 2);
            for value in bf16_bytes_to_f32_vec(&bytes)? {
                out.extend_from_slice(&value.to_le_bytes());
            }
            Ok(out)
        }
        MlxDType::F16 | MlxDType::BF16 => header.read_tensor_bytes(name).map_err(Into::into),
        other => Err(DiffusionError::model(format!(
            "t5xxl unsupported tensor dtype {:?}",
            other
        ))),
    }
}

fn decode_relative_attention_bias(
    header: &MlxSafetensorsHeader,
    config: &T5ModelConfig,
) -> Result<Vec<f32>> {
    let entry = header.tensor(T5_RELATIVE_ATTENTION_BIAS_NAME).ok_or_else(|| {
        DiffusionError::model(format!(
            "t5xxl relative attention bias missing in {}",
            header.path.display()
        ))
    })?;
    let expected = usize::try_from(config.relative_attention_bucket_count)
        .ok()
        .and_then(|buckets| {
            usize::try_from(config.attention_head_count)
                .ok()
                .and_then(|heads| buckets.checked_mul(heads))
        })
        .ok_or_else(|| DiffusionError::model("t5xxl relative attention bias size overflow"))?;
    let bytes = header.read_tensor_bytes(T5_RELATIVE_ATTENTION_BIAS_NAME)?;
    let values = match entry.dtype {
        MlxDType::F32 => f32_bytes_to_vec(&bytes)?,
        MlxDType::F16 => f16_bytes_to_f32_vec(&bytes)?,
        MlxDType::BF16 => bf16_bytes_to_f32_vec(&bytes)?,
        other => {
            return Err(DiffusionError::model(format!(
                "t5xxl relative attention bias has unsupported dtype {:?}",
                other
            )))
        }
    };
    if values.len() != expected {
        return Err(DiffusionError::model(format!(
            "t5xxl relative attention bias expected {} values, got {}",
            expected,
            values.len()
        )));
    }
    Ok(values)
}

fn attention_bias_f32_bytes(weights: &LoadedT5xxlWeights, token_count: usize) -> Result<Vec<u8>> {
    let head_count = usize::try_from(weights.config.attention_head_count)
        .map_err(|_| DiffusionError::model("t5xxl head count exceeds usize"))?;
    let bucket_count = usize::try_from(weights.config.relative_attention_bucket_count)
        .map_err(|_| DiffusionError::model("t5xxl bucket count exceeds usize"))?;
    let expected_bias_len = bucket_count
        .checked_mul(head_count)
        .ok_or_else(|| DiffusionError::model("t5xxl relative attention bias size overflow"))?;
    if weights.relative_attention_bias.len() != expected_bias_len {
        return Err(DiffusionError::model(format!(
            "t5xxl relative attention bias length mismatch: expected {}, got {}",
            expected_bias_len,
            weights.relative_attention_bias.len()
        )));
    }

    let total_values = token_count
        .checked_mul(token_count)
        .and_then(|value| value.checked_mul(head_count))
        .ok_or_else(|| DiffusionError::model("t5xxl attention bias tensor size overflow"))?;
    let mut bytes = Vec::with_capacity(total_values * std::mem::size_of::<f32>());

    for head in 0..head_count {
        for query in 0..token_count {
            for key in 0..token_count {
                let bucket = relative_position_bucket(
                    query,
                    key,
                    weights.config.relative_attention_bucket_count,
                    weights.config.relative_attention_max_distance,
                )?;
                let value = weights.relative_attention_bias[bucket * head_count + head];
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
    }

    Ok(bytes)
}

fn attention_bias_values(weights: &LoadedT5xxlWeights, token_count: usize) -> Result<Vec<f32>> {
    f32_bytes_to_vec(&attention_bias_f32_bytes(weights, token_count)?)
}

fn relative_position_bucket(
    query_position: usize,
    key_position: usize,
    bucket_count: u32,
    max_distance: u32,
) -> Result<usize> {
    if bucket_count == 0 {
        return Err(DiffusionError::model(
            "t5xxl relative attention bucket count must be positive",
        ));
    }
    if max_distance == 0 {
        return Err(DiffusionError::model(
            "t5xxl relative attention max distance must be positive",
        ));
    }

    let half_bucket_count = i32::try_from(bucket_count / 2)
        .map_err(|_| DiffusionError::model("t5xxl relative bucket count exceeds i32"))?;
    if half_bucket_count == 0 {
        return Err(DiffusionError::model(
            "t5xxl bidirectional relative attention needs at least 2 buckets",
        ));
    }

    let relative_position = i64::try_from(key_position)
        .and_then(|key| i64::try_from(query_position).map(|query| key - query))
        .map_err(|_| DiffusionError::model("t5xxl relative position exceeds i64"))?;
    let positive_bucket_base = if relative_position > 0 {
        usize::try_from(half_bucket_count)
            .map_err(|_| DiffusionError::model("t5xxl positive bucket base exceeds usize"))?
    } else {
        0
    };
    let relative_position = relative_position.unsigned_abs() as i64;

    let max_exact = half_bucket_count / 2;
    let bucket_in_half = if relative_position < i64::from(max_exact) {
        relative_position as i32
    } else {
        let relative_position = relative_position as f32;
        let max_exact_f = max_exact as f32;
        let half_bucket_count_f = half_bucket_count as f32;
        let max_distance_f = max_distance as f32;
        let scaled = max_exact_f
            + (relative_position / max_exact_f).ln()
                / (max_distance_f / max_exact_f).ln()
                * (half_bucket_count_f - max_exact_f);
        scaled.floor().min((half_bucket_count - 1) as f32) as i32
    };

    usize::try_from(bucket_in_half)
        .map(|bucket| positive_bucket_base + bucket)
        .map_err(|_| DiffusionError::model("t5xxl relative bucket index exceeds usize"))
}

fn f16_bytes_to_f32_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 2 != 0 {
        return Err(DiffusionError::model(format!(
            "t5xxl F16 bytes length {} is not even",
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
            "t5xxl BF16 bytes length {} is not even",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| bf16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])))
        .collect())
}

fn f32_bytes_to_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 4 != 0 {
        return Err(DiffusionError::model(format!(
            "t5xxl byte length {} is not divisible by 4",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn tensor_to_f32_vec(ctx: &Context, tensor_id: TensorId) -> Result<Vec<f32>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    let bytes = ctx.tensor_data(tensor_id).map_err(DiffusionError::model)?;
    match tensor.desc.ty {
        TensorType::F32 => f32_bytes_to_vec(bytes),
        TensorType::F16 => f16_bytes_to_f32_vec(bytes),
        TensorType::BF16 => bf16_bytes_to_f32_vec(bytes),
        other => Err(DiffusionError::model(format!(
            "t5xxl tensor {} cannot be decoded as f32 from {:?}",
            tensor_id, other
        ))),
    }
}

fn resident_matrix<'a>(ctx: &'a Context, tensor_id: TensorId) -> Result<ResidentMatrix<'a>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    let cols = usize::try_from(tensor.ne[0])
        .map_err(|_| DiffusionError::model(format!("t5xxl tensor {} cols exceed usize", tensor_id)))?;
    let rows = usize::try_from(tensor.ne[1])
        .map_err(|_| DiffusionError::model(format!("t5xxl tensor {} rows exceed usize", tensor_id)))?;
    Ok(ResidentMatrix {
        bytes: ctx.tensor_data(tensor_id).map_err(DiffusionError::model)?,
        ggml_type: tensor.desc.ty.ggml_type(),
        cols,
        rows,
    })
}

fn decode_ggml_matrix_to_f32(matrix: ResidentMatrix<'_>) -> Result<Vec<f32>> {
    let row_indices = (0..matrix.rows)
        .map(|row| i32::try_from(row).map_err(|_| DiffusionError::model("t5xxl row index exceeds i32")))
        .collect::<Result<Vec<_>>>()?;
    get_rows_ggml_bytes_cpu(
        matrix.bytes,
        matrix.ggml_type,
        matrix.cols,
        matrix.rows,
        &row_indices,
    )
    .ok_or_else(|| DiffusionError::model("t5xxl matrix decode fallback failed"))
}

fn matmul_nt_f32_cpu(a: &[f32], bt: &[f32], m: usize, k: usize, n: usize) -> Result<Vec<f32>> {
    if a.len() != m.checked_mul(k).ok_or_else(|| DiffusionError::model("t5xxl matmul a overflow"))? {
        return Err(DiffusionError::model("t5xxl matmul_nt_f32_cpu a len mismatch"));
    }
    if bt.len() != n.checked_mul(k).ok_or_else(|| DiffusionError::model("t5xxl matmul bt overflow"))? {
        return Err(DiffusionError::model("t5xxl matmul_nt_f32_cpu bt len mismatch"));
    }
    let mut out = vec![0.0f32; m.checked_mul(n).ok_or_else(|| DiffusionError::model("t5xxl matmul out overflow"))?];
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
    if a.len() != m.checked_mul(k).ok_or_else(|| DiffusionError::model("t5xxl matmul a overflow"))? {
        return Err(DiffusionError::model("t5xxl matmul_nn_f32_cpu a len mismatch"));
    }
    if b.len() != k.checked_mul(n).ok_or_else(|| DiffusionError::model("t5xxl matmul b overflow"))? {
        return Err(DiffusionError::model("t5xxl matmul_nn_f32_cpu b len mismatch"));
    }
    let mut out = vec![0.0f32; m.checked_mul(n).ok_or_else(|| DiffusionError::model("t5xxl matmul out overflow"))?];
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
        .ok_or_else(|| DiffusionError::model("t5xxl head output size overflow"))?;
    if head_output.len() != expected_len {
        return Err(DiffusionError::model(format!(
            "t5xxl head output len mismatch: expected {} got {}",
            expected_len,
            head_output.len()
        )));
    }
    let model_dim = head_count
        .checked_mul(head_dim)
        .ok_or_else(|| DiffusionError::model("t5xxl model dim overflow"))?;
    for token_idx in 0..token_count {
        let dst_start = token_idx * model_dim + head_idx * head_dim;
        let src_start = token_idx * head_dim;
        output[dst_start..dst_start + head_dim]
            .copy_from_slice(&head_output[src_start..src_start + head_dim]);
    }
    Ok(())
}

fn apply_bias_softmax_in_place(values: &mut [f32], bias: &[f32], width: usize) -> Result<()> {
    if values.len() != bias.len() {
        return Err(DiffusionError::model(format!(
            "t5xxl softmax bias len mismatch: values={} bias={}",
            values.len(),
            bias.len()
        )));
    }
    add_bias_in_place(values, bias)?;
    softmax_in_place(values, width)
}

fn add_bias_in_place(values: &mut [f32], bias: &[f32]) -> Result<()> {
    if values.len() != bias.len() {
        return Err(DiffusionError::model(format!(
            "t5xxl bias len mismatch: values={} bias={}",
            values.len(),
            bias.len()
        )));
    }
    for (value, bias_value) in values.iter_mut().zip(bias.iter()) {
        *value += *bias_value;
    }
    Ok(())
}

fn softmax_in_place(values: &mut [f32], width: usize) -> Result<()> {
    if width == 0 || values.len() % width != 0 {
        return Err(DiffusionError::model(format!(
            "t5xxl softmax width {} is invalid for {} values",
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
            return Err(DiffusionError::model("t5xxl softmax denominator became zero"));
        }
        for value in row.iter_mut() {
            *value /= denom;
        }
    }
    Ok(())
}

fn gelu_approx(x: f32) -> f32 {
    let inner = 0.797_884_6 * (x + 0.044_715 * x * x * x);
    0.5 * x * (1.0 + inner.tanh())
}

fn require_tensor_id(tensor_ids: &BTreeMap<String, TensorId>, name: &str) -> Result<TensorId> {
    tensor_ids
        .get(name)
        .copied()
        .ok_or_else(|| DiffusionError::model(format!("missing t5xxl resident tensor '{}'", name)))
}

fn require_tensor_id_candidates(tensor_ids: &BTreeMap<String, TensorId>, names: &[&str]) -> Result<TensorId> {
    for name in names {
        if let Some(id) = tensor_ids.get(*name) {
            return Ok(*id);
        }
    }
    Err(DiffusionError::model(format!(
        "missing t5xxl resident tensor; tried {}",
        names.join(", ")
    )))
}

fn require_tensor<'a>(ctx: &'a Context, id: TensorId) -> Result<&'a Tensor> {
    ctx.tensor(id)
        .ok_or_else(|| DiffusionError::model(format!("invalid t5xxl tensor id {}", id)))
}

fn i32s_to_le_bytes(values: &[i32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<i32>());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn shape_dim(entry: &MlxTensorEntry, index: usize) -> Option<u32> {
    entry.shape.get(index).and_then(|&dim| u32::try_from(dim).ok())
}

fn is_context_oom(err: &DiffusionError) -> bool {
    matches!(err, DiffusionError::Model(message) if message.starts_with("context out of memory allocating "))
}

fn next_graph_reserve_bytes(weights: &LoadedT5xxlWeights) -> Result<usize> {
    weights
        .graph_reserve_bytes()
        .checked_mul(2)
        .ok_or_else(|| DiffusionError::model("t5xxl graph reserve overflow"))
}

fn t5_debug_dir() -> Option<PathBuf> {
    std::env::var_os("FLUX_T5_DEBUG_DIR").map(PathBuf::from)
}

fn t5_debug_stages_enabled() -> bool {
    std::env::var_os("FLUX_T5_DEBUG_STAGES")
        .map(|value| value != "0")
        .unwrap_or(false)
}

fn t5_debug_stage_layer() -> Option<usize> {
    std::env::var("FLUX_T5_DEBUG_LAYER")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
}

fn t5_force_cpu_math() -> bool {
    std::env::var_os("FLUX_T5_FORCE_CPU_MATH")
        .map(|value| value != "0")
        .unwrap_or(false)
}

fn t5_force_cpu_attention() -> bool {
    std::env::var_os("FLUX_T5_FORCE_CPU_ATTN")
        .map(|value| value != "0")
        .unwrap_or(false)
}

fn t5_force_f32_linear() -> bool {
    std::env::var_os("FLUX_T5_FORCE_F32_LINEAR")
        .map(|value| value != "0")
        .unwrap_or(false)
}

fn dump_t5_debug_outputs(
    dir: &Path,
    outputs: &BTreeMap<TensorId, Vec<u8>>,
    debug_hidden_states: &[(String, TensorId)],
    hidden_size: usize,
    token_count: usize,
) -> Result<()> {
    fs::create_dir_all(dir).map_err(|err| {
        DiffusionError::model(format!(
            "failed to create t5 debug dir {}: {}",
            dir.display(),
            err
        ))
    })?;
    let meta_path = dir.join("t5_meta.txt");
    fs::write(
        &meta_path,
        format!("hidden_size={hidden_size}\ntoken_count={token_count}\n"),
    )
    .map_err(|err| {
        DiffusionError::model(format!(
            "failed to write t5 debug meta {}: {}",
            meta_path.display(),
            err
        ))
    })?;
    for (name, tensor_id) in debug_hidden_states {
        let bytes = outputs.get(tensor_id).ok_or_else(|| {
            DiffusionError::model(format!(
                "missing t5 debug output '{}' for tensor {}",
                name, tensor_id
            ))
        })?;
        let path = dir.join(format!("{name}.bin"));
        fs::write(&path, bytes).map_err(|err| {
            DiffusionError::model(format!(
                "failed to write t5 debug tensor {}: {}",
                path.display(),
                err
            ))
        })?;
    }
    Ok(())
}

fn dump_t5_debug_rows(
    dir: &Path,
    debug_hidden_states: &[(String, Vec<f32>)],
    hidden_size: usize,
    token_count: usize,
) -> Result<()> {
    fs::create_dir_all(dir).map_err(|err| {
        DiffusionError::model(format!(
            "failed to create t5 debug dir {}: {}",
            dir.display(),
            err
        ))
    })?;
    let meta_path = dir.join("t5_meta.txt");
    fs::write(
        &meta_path,
        format!("hidden_size={hidden_size}\ntoken_count={token_count}\n"),
    )
    .map_err(|err| {
        DiffusionError::model(format!(
            "failed to write t5 debug meta {}: {}",
            meta_path.display(),
            err
        ))
    })?;
    for (name, values) in debug_hidden_states {
        let path = dir.join(format!("{name}.bin"));
        fs::write(&path, f32s_to_le_bytes(values)).map_err(|err| {
            DiffusionError::model(format!(
                "failed to write t5 debug tensor {}: {}",
                path.display(),
                err
            ))
        })?;
    }
    Ok(())
}

fn f32s_to_le_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::{
        attention_bias_f32_bytes, relative_position_bucket, t5_model_config_from_tensors,
        t5_target_extents, t5_target_tensor_type, LoadedT5xxlWeights, T5ModelConfig,
        T5_RELATIVE_ATTENTION_BIAS_NAME,
    };
    use crate::flux::T5TextEncoderConfig;
    use makepad_ggml::TensorType;
    use makepad_mlx::{MlxDType, MlxTensorEntry};
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;

    #[test]
    fn t5_layout_reverses_rank2_weights_for_ggml_matmul() {
        let entry = MlxTensorEntry {
            dtype: MlxDType::F16,
            shape: vec![10240, 4096],
            data_offsets: [0, 0],
        };
        assert_eq!(t5_target_extents(&entry).unwrap(), vec![4096, 10240]);
        assert_eq!(t5_target_tensor_type(&entry).unwrap(), TensorType::F16);
    }

    #[test]
    fn t5_rank1_norm_weights_promote_to_f32() {
        let entry = MlxTensorEntry {
            dtype: MlxDType::F16,
            shape: vec![4096],
            data_offsets: [0, 0],
        };
        assert_eq!(t5_target_extents(&entry).unwrap(), vec![4096]);
        assert_eq!(t5_target_tensor_type(&entry).unwrap(), TensorType::F32);
    }

    #[test]
    fn t5_model_config_derives_heads_and_buckets_from_relative_bias() {
        let mut tensors = HashMap::new();
        tensors.insert(
            "shared.weight".to_string(),
            MlxTensorEntry {
                dtype: MlxDType::F16,
                shape: vec![32128, 4096],
                data_offsets: [0, 0],
            },
        );
        tensors.insert(
            "encoder.block.0.layer.1.DenseReluDense.wi_0.weight".to_string(),
            MlxTensorEntry {
                dtype: MlxDType::F16,
                shape: vec![10240, 4096],
                data_offsets: [0, 0],
            },
        );
        tensors.insert(
            T5_RELATIVE_ATTENTION_BIAS_NAME.to_string(),
            MlxTensorEntry {
                dtype: MlxDType::F16,
                shape: vec![32, 64],
                data_offsets: [0, 0],
            },
        );
        let config = t5_model_config_from_tensors(
            &tensors,
            PathBuf::from("unit-test.safetensors").as_path(),
            &T5TextEncoderConfig {
                vocab_size: 32128,
                model_dim: 4096,
                feedforward_dim: 10240,
                layer_count: 24,
            },
        )
        .unwrap();
        assert_eq!(config.attention_head_count, 64);
        assert_eq!(config.relative_attention_bucket_count, 32);
        assert_eq!(config.head_dim(), 64);
        assert_eq!(config.layer_norm_epsilon(), 1.0e-6);
    }

    #[test]
    fn relative_position_bucket_uses_bidirectional_halves() {
        assert_eq!(relative_position_bucket(0, 0, 32, 128).unwrap(), 0);
        assert_eq!(relative_position_bucket(1, 0, 32, 128).unwrap(), 1);
        assert_eq!(relative_position_bucket(0, 1, 32, 128).unwrap(), 17);
    }

    #[test]
    fn attention_bias_uses_relative_bias_without_padding_mask() {
        let weights = LoadedT5xxlWeights {
            ctx: makepad_ggml::Context::new(makepad_ggml::InitParams {
                mem_size: 1024,
                mem_buffer: None,
                no_alloc: false,
            }),
            tensor_ids: BTreeMap::new(),
            config: T5ModelConfig {
                vocab_size: 8,
                model_dim: 4,
                feedforward_dim: 16,
                layer_count: 1,
                attention_head_count: 2,
                relative_attention_bucket_count: 4,
                relative_attention_max_distance: 8,
                layer_norm_epsilon_bits: (1.0e-6f32).to_bits(),
            },
            path: PathBuf::from("unit-test.safetensors"),
            relative_attention_bias: vec![
                0.0, 10.0, // bucket 0
                1.0, 11.0, // bucket 1
                2.0, 12.0, // bucket 2
                3.0, 13.0, // bucket 3
            ],
            graph_extra_bytes: 0,
        };
        let bytes = attention_bias_f32_bytes(&weights, 2).unwrap();
        let values = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert!(values.iter().all(|value| value.is_finite()));
        assert_eq!(values.len(), 8);
        assert_eq!(values[0], 0.0);
        assert_eq!(values[1], 3.0);
        assert_eq!(values[2], 1.0);
        assert_eq!(values[3], 0.0);
        assert_eq!(values[4], 10.0);
        assert_eq!(values[5], 13.0);
        assert_eq!(values[6], 11.0);
        assert_eq!(values[7], 10.0);
    }
}
