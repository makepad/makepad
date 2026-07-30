use makepad_ggml::backend::metal::{
    create_context_main_buffer, execute_compiled_graph, execute_compiled_graph_in_active_batch,
    execute_compiled_graph_with_buffer_inputs, try_matmul_nt_ggml_bytes_multi, BufferStorageMode,
    MatmulNtGgmlBytesMatrix, MetalBuffer, MetalBufferBindingRef, MetalGraphSession,
    MetalGraphTensorBufferCopy, MetalGraphTensorWrite, MetalPipeline, MetalPipelineDescriptor,
    MetalRuntime, MetalRuntimeCounters, MetalSize,
};
use makepad_ggml::backend::{
    try_affine_quantized_matmul_bf16, try_affine_quantized_matmul_bf16_rows,
    try_affine_quantized_matmul_bf16_top1, try_matmul_nt_ggml_bytes, AffineQuantizedMatmulRowsSpec,
    AffineQuantizedMatmulSpec,
};
use makepad_ggml::quant::{f32_to_f16, GGML_TYPE_BF16, GGML_TYPE_F32};
use makepad_ggml::{
    BufferUsage, Context, InitParams, TensorId, TensorType, UnaryOp, GGML_ROPE_TYPE_IMROPE,
};
use makepad_llama::{
    execute_prepared_attention_decode_metal_no_readback_buffer_input,
    execute_prepared_attention_decode_metal_no_readback_prepared_input,
    execute_prepared_attention_decode_metal_no_readback_prepared_input_in_active_batch,
    prepare_attention_decode_graph_with_key_count, prepare_delta_net_recurrent_decode_graph,
    AttentionBlockSpec, AttentionDecodeGraph, AttentionDecodeSpec, AttentionKvCacheSpec,
    AttentionQueryLayout, AttentionRopeSpec, DeltaNetRecurrentBlockSpec,
    DeltaNetRecurrentDecodeGraph, DeltaNetRecurrentDecodeSpec, DeltaNetRecurrentStateSpec,
    ProbeInputKind,
};
use makepad_micro_serde::{DeJson, JsonValue};
use makepad_mlx::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry, MlxWeightIndex};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, String>;

const DEFAULT_HF_CACHE_MODEL_DIR: &str = "/Users/admin/.cache/huggingface/hub/models--unsloth--Qwen3.6-27B-UD-MLX-4bit/snapshots/7baaa18ea869b8f8bd229b82bffd0d00ce5042f3";
const EMBED_TOKENS_BASE: &str = "language_model.model.embed_tokens";
const OUTPUT_NORM_WEIGHT: &str = "language_model.model.norm.weight";
const LM_HEAD_BASE: &str = "language_model.lm_head";
const ENABLE_EXPERIMENTAL_ATTENTION_METAL: bool = true;
const ENABLE_METAL_DECODE_TAIL: bool = true;
const ENABLE_METAL_DECODE_CHAIN: bool = true;

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxRmsNormRowArgs {
    n: u32,
    eps: f32,
    threadgroup_width: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxAffineQprojRowArgs {
    n_in: u32,
    weight_words_per_row: u32,
    qparams_per_row: u32,
    out_rows: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxAddRowArgs {
    n: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxGegluRowArgs {
    n: u32,
}

#[derive(Clone, Copy, Debug)]
struct MlxAffineQmvLayout {
    bits: u32,
    weight_words_per_row: usize,
    qparams_per_row: usize,
    out_rows: usize,
}

impl MlxAffineQmvLayout {
    fn row_args(self, n_in: usize) -> Result<MlxAffineQprojRowArgs> {
        Ok(MlxAffineQprojRowArgs {
            n_in: u32::try_from(n_in).map_err(|_| "qmv n_in does not fit in u32".to_string())?,
            weight_words_per_row: u32::try_from(self.weight_words_per_row)
                .map_err(|_| "qmv weight_words_per_row does not fit in u32".to_string())?,
            qparams_per_row: u32::try_from(self.qparams_per_row)
                .map_err(|_| "qmv qparams_per_row does not fit in u32".to_string())?,
            out_rows: u32::try_from(self.out_rows)
                .map_err(|_| "qmv out_rows does not fit in u32".to_string())?,
        })
    }
}

#[derive(Clone, Debug)]
struct QuantParams {
    bits: u32,
    group_size: u32,
}

#[derive(Clone, Debug)]
struct QwenTextConfig {
    _hidden_size: usize,
    _intermediate_size: usize,
    num_hidden_layers: usize,
    num_attention_heads: usize,
    num_key_value_heads: usize,
    head_dim: usize,
    linear_key_head_dim: usize,
    linear_num_key_heads: usize,
    linear_num_value_heads: usize,
    linear_value_head_dim: usize,
    linear_conv_kernel_dim: usize,
    full_attention_interval: usize,
    layer_types: Vec<String>,
    rms_norm_eps: f32,
    partial_rotary_factor: f32,
    rope_theta: f32,
    _vocab_size: usize,
    bos_token_id: u32,
    _eos_token_ids: Vec<u32>,
}

#[derive(Clone, Debug)]
struct QwenConfig {
    model_type: String,
    quantization_mode: String,
    default_quant: QuantParams,
    quant_overrides: HashMap<String, QuantParams>,
    text: QwenTextConfig,
}

#[derive(Clone, Debug)]
struct QwenMlxModel {
    root_dir: PathBuf,
    config: QwenConfig,
    weight_index: MlxWeightIndex,
}

impl QwenMlxModel {
    fn load(root_dir: impl AsRef<Path>) -> Result<Self> {
        let root_dir = root_dir.as_ref().to_path_buf();
        if !root_dir.is_dir() {
            return Err(format!("model dir does not exist: {}", root_dir.display()));
        }
        let config_path = root_dir.join("config.json");
        let config_text = fs::read_to_string(&config_path)
            .map_err(|err| format!("read {} failed: {err}", config_path.display()))?;
        let config_json = HashMap::<String, JsonValue>::deserialize_json(&config_text)
            .map_err(|err| format!("parse {} failed: {err:?}", config_path.display()))?;
        let config = parse_qwen_config(&config_path, &config_json)?;
        let index_path = root_dir.join("model.safetensors.index.json");
        let index_text = fs::read_to_string(&index_path)
            .map_err(|err| format!("read {} failed: {err}", index_path.display()))?;
        let weight_index = MlxWeightIndex::deserialize_json(&index_text)
            .map_err(|err| format!("parse {} failed: {err:?}", index_path.display()))?;
        Ok(Self {
            root_dir,
            config,
            weight_index,
        })
    }

    fn unique_weight_shards(&self) -> Vec<String> {
        let mut shards = BTreeSet::new();
        for shard in self.weight_index.weight_map.values() {
            shards.insert(shard.clone());
        }
        shards.into_iter().collect()
    }
}

#[derive(Clone, Debug)]
struct QwenMlxWeights {
    model: Arc<QwenMlxModel>,
    shard_headers: HashMap<String, MlxSafetensorsHeader>,
    bf16_cache: Arc<Mutex<HashMap<String, Arc<Vec<u16>>>>>,
    dequant_bf16_cache: Arc<Mutex<HashMap<String, Arc<Vec<u16>>>>>,
    concat_affine_cache: Arc<Mutex<HashMap<String, Arc<ConcatAffineTensor>>>>,
}

#[derive(Clone, Debug)]
struct ConcatAffineTensor {
    weight_bytes: Arc<Vec<u8>>,
    scales_bytes: Arc<Vec<u8>>,
    biases_bytes: Arc<Vec<u8>>,
    total_rows: usize,
    weight_words_per_row: usize,
    qparams_per_row: usize,
    bits: u32,
    group_size: u64,
    row_counts: Vec<usize>,
}

impl QwenMlxWeights {
    fn load(root_dir: impl AsRef<Path>) -> Result<Self> {
        let model = Arc::new(QwenMlxModel::load(root_dir)?);
        let mut shard_headers = HashMap::new();
        for shard_name in model.unique_weight_shards() {
            let shard_path = model.root_dir.join(&shard_name);
            let header = MlxSafetensorsHeader::load(&shard_path)
                .map_err(|err| format!("load {} failed: {err}", shard_path.display()))?;
            shard_headers.insert(shard_name, header);
        }
        Ok(Self {
            model,
            shard_headers,
            bf16_cache: Arc::new(Mutex::new(HashMap::new())),
            dequant_bf16_cache: Arc::new(Mutex::new(HashMap::new())),
            concat_affine_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn config(&self) -> &QwenConfig {
        &self.model.config
    }

    fn shard_name_for_tensor(&self, name: &str) -> Result<&str> {
        self.model
            .weight_index
            .weight_map
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("tensor {} missing from weight index", name))
    }

    fn header_for_tensor(&self, name: &str) -> Result<&MlxSafetensorsHeader> {
        let shard_name = self.shard_name_for_tensor(name)?;
        self.shard_headers
            .get(shard_name)
            .ok_or_else(|| format!("missing shard header for tensor {} in {}", name, shard_name))
    }

    fn tensor(&self, name: &str) -> Result<&MlxTensorEntry> {
        self.header_for_tensor(name)?
            .tensor(name)
            .ok_or_else(|| format!("tensor {} missing from shard header", name))
    }

    fn read_tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        self.header_for_tensor(name)?
            .read_tensor_bytes(name)
            .map_err(|err| err.to_string())
    }

    fn read_bf16_tensor_words(&self, name: &str) -> Result<Vec<u16>> {
        self.header_for_tensor(name)?
            .read_bf16_tensor_words(name)
            .map_err(|err| err.to_string())
    }

    fn read_bf16_tensor_words_cached(&self, name: &str) -> Result<Arc<Vec<u16>>> {
        {
            let cache = self
                .bf16_cache
                .lock()
                .map_err(|_| "bf16 cache mutex poisoned".to_string())?;
            if let Some(words) = cache.get(name) {
                return Ok(words.clone());
            }
        }
        let words = Arc::new(self.read_bf16_tensor_words(name)?);
        let mut cache = self
            .bf16_cache
            .lock()
            .map_err(|_| "bf16 cache mutex poisoned".to_string())?;
        Ok(cache
            .entry(name.to_owned())
            .or_insert_with(|| words.clone())
            .clone())
    }

    fn read_rank2_row_u32_words(&self, name: &str, row: u64) -> Result<Vec<u32>> {
        self.header_for_tensor(name)?
            .read_rank2_row_u32_words(name, row)
            .map_err(|err| err.to_string())
    }

    fn read_rank2_row_bf16_words(&self, name: &str, row: u64) -> Result<Vec<u16>> {
        self.header_for_tensor(name)?
            .read_rank2_row_bf16_words(name, row)
            .map_err(|err| err.to_string())
    }

    fn read_tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        let entry = self.tensor(name)?;
        let bytes = self.read_tensor_bytes(name)?;
        decode_tensor_bytes_to_f32(&bytes, entry.dtype)
    }

    fn read_rank2_row_f32(&self, name: &str, row: u64) -> Result<Vec<f32>> {
        let entry = self.tensor(name)?;
        if entry.shape.len() != 2 {
            return Err(format!(
                "tensor {} expected rank 2, got {:?}",
                name, entry.shape
            ));
        }
        let bytes = self
            .header_for_tensor(name)?
            .read_rank2_row_bytes(name, row)
            .map_err(|err| err.to_string())?;
        decode_tensor_bytes_to_f32(&bytes, entry.dtype)
    }

    fn dequantized_bf16_tensor_words_cached(
        &self,
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
        bits: u32,
        group_size: u32,
    ) -> Result<Arc<Vec<u16>>> {
        {
            let cache = self
                .dequant_bf16_cache
                .lock()
                .map_err(|_| "dequant bf16 cache mutex poisoned".to_string())?;
            if let Some(words) = cache.get(weight_name) {
                return Ok(words.clone());
            }
        }

        let weight_entry = self.tensor(weight_name)?;
        let scales_entry = self.tensor(scales_name)?;
        if weight_entry.shape.len() != 2 || scales_entry.shape.len() != 2 {
            return Err(format!(
                "dequant cache expects rank-2 tensors for {}, {:?} / {:?}",
                weight_name, weight_entry.shape, scales_entry.shape
            ));
        }
        let rows = weight_entry.shape[0] as usize;
        let groups_per_row = scales_entry.shape[1] as usize;
        let inner_dim = groups_per_row
            .checked_mul(group_size as usize)
            .ok_or_else(|| format!("inner dim overflow for {}", weight_name))?;
        let packed_bits_per_group = u64::from(group_size)
            .checked_mul(u64::from(bits))
            .ok_or_else(|| format!("packed bit count overflow for {}", weight_name))?;
        let words_per_group = packed_bits_per_group.div_ceil(32) as usize;
        let packed = self
            .header_for_tensor(weight_name)?
            .read_u32_tensor_words(weight_name)
            .map_err(|err| err.to_string())?;
        let scales = self.read_bf16_tensor_words(scales_name)?;
        let biases = self.read_bf16_tensor_words(biases_name)?;

        let mut out = Vec::with_capacity(
            rows.checked_mul(inner_dim)
                .ok_or_else(|| format!("dequant buffer size overflow for {}", weight_name))?,
        );
        for row in 0..rows {
            let packed_row_start = row
                .checked_mul(weight_entry.shape[1] as usize)
                .ok_or_else(|| format!("packed row offset overflow for {}", weight_name))?;
            let qparam_row_start = row
                .checked_mul(groups_per_row)
                .ok_or_else(|| format!("qparam row offset overflow for {}", weight_name))?;
            for group in 0..groups_per_row {
                let scale = bf16_word_to_f32(scales[qparam_row_start + group]);
                let bias = bf16_word_to_f32(biases[qparam_row_start + group]);
                let group_start = packed_row_start + group * words_per_group;
                let group_end = group_start + words_per_group;
                for group_index in 0..group_size as usize {
                    let q = unpack_affine_value(
                        &packed[group_start..group_end],
                        bits,
                        (1u32 << bits) - 1,
                        group_index,
                    ) as f32;
                    out.push(f32_to_bf16_word(scale * q + bias));
                }
            }
        }

        let words = Arc::new(out);
        let mut cache = self
            .dequant_bf16_cache
            .lock()
            .map_err(|_| "dequant bf16 cache mutex poisoned".to_string())?;
        Ok(cache
            .entry(weight_name.to_owned())
            .or_insert_with(|| words.clone())
            .clone())
    }

    fn rank2_weight_as_bf16_words_cached(&self, weight_name: &str) -> Result<Arc<Vec<u16>>> {
        let entry = self.tensor(weight_name)?;
        if entry.shape.len() != 2 {
            return Err(format!(
                "expected rank-2 tensor for {}, got {:?}",
                weight_name, entry.shape
            ));
        }
        match entry.dtype {
            MlxDType::BF16 => self.read_bf16_tensor_words_cached(weight_name),
            MlxDType::F32 => {
                {
                    let cache = self
                        .dequant_bf16_cache
                        .lock()
                        .map_err(|_| "dequant bf16 cache mutex poisoned".to_string())?;
                    if let Some(words) = cache.get(weight_name) {
                        return Ok(words.clone());
                    }
                }
                let values = self.read_tensor_f32(weight_name)?;
                let words = Arc::new(f32s_to_bf16_words(&values));
                let mut cache = self
                    .dequant_bf16_cache
                    .lock()
                    .map_err(|_| "dequant bf16 cache mutex poisoned".to_string())?;
                Ok(cache
                    .entry(weight_name.to_owned())
                    .or_insert_with(|| words.clone())
                    .clone())
            }
            MlxDType::U32 => {
                let base = strip_quant_suffix(weight_name);
                let params = self.quant_params_for_weight(weight_name);
                self.dequantized_bf16_tensor_words_cached(
                    weight_name,
                    &format!("{base}.scales"),
                    &format!("{base}.biases"),
                    params.bits,
                    params.group_size,
                )
            }
            other => Err(format!(
                "unsupported rank-2 weight dtype {:?} for {}",
                other, weight_name
            )),
        }
    }

    fn concat_affine_tensor_cached(
        &self,
        weight_names: &[&str],
    ) -> Result<Arc<ConcatAffineTensor>> {
        let cache_key = weight_names.join("|");
        {
            let cache = self
                .concat_affine_cache
                .lock()
                .map_err(|_| "concat affine cache mutex poisoned".to_string())?;
            if let Some(tensor) = cache.get(&cache_key) {
                return Ok(tensor.clone());
            }
        }

        let mut weight_bytes = Vec::new();
        let mut scales_bytes = Vec::new();
        let mut biases_bytes = Vec::new();
        let mut total_rows = 0usize;
        let mut weight_words_per_row = None::<usize>;
        let mut qparams_per_row = None::<usize>;
        let mut bits = None::<u32>;
        let mut group_size = None::<u64>;
        let mut row_counts = Vec::with_capacity(weight_names.len());

        for &weight_name in weight_names {
            let weight_entry = self.tensor(weight_name)?;
            if weight_entry.shape.len() != 2 || weight_entry.dtype != MlxDType::U32 {
                return Err(format!(
                    "concat affine expects rank-2 U32 tensor, got {:?} {:?} for {}",
                    weight_entry.shape, weight_entry.dtype, weight_name
                ));
            }

            let base = strip_quant_suffix(weight_name);
            let scales_name = format!("{base}.scales");
            let biases_name = format!("{base}.biases");
            let scales_entry = self.tensor(&scales_name)?;
            let biases_entry = self.tensor(&biases_name)?;
            if scales_entry.shape != biases_entry.shape {
                return Err(format!(
                    "concat affine scale/bias mismatch for {}: {:?} vs {:?}",
                    weight_name, scales_entry.shape, biases_entry.shape
                ));
            }

            let params = self.quant_params_for_weight(weight_name);
            let this_group_size = params.group_size as u64;
            let this_weight_words_per_row = weight_entry.shape[1] as usize;
            let this_qparams_per_row = scales_entry.shape[1] as usize;
            if let Some(expected) = weight_words_per_row {
                if this_weight_words_per_row != expected {
                    return Err(format!(
                        "concat affine weight stride mismatch for {}: got {} expected {}",
                        weight_name, this_weight_words_per_row, expected
                    ));
                }
            } else {
                weight_words_per_row = Some(this_weight_words_per_row);
            }
            if let Some(expected) = qparams_per_row {
                if this_qparams_per_row != expected {
                    return Err(format!(
                        "concat affine qparam stride mismatch for {}: got {} expected {}",
                        weight_name, this_qparams_per_row, expected
                    ));
                }
            } else {
                qparams_per_row = Some(this_qparams_per_row);
            }
            if let Some(expected) = bits {
                if params.bits != expected {
                    return Err(format!(
                        "concat affine bits mismatch for {}: got {} expected {}",
                        weight_name, params.bits, expected
                    ));
                }
            } else {
                bits = Some(params.bits);
            }
            if let Some(expected) = group_size {
                if this_group_size != expected {
                    return Err(format!(
                        "concat affine group_size mismatch for {}: got {} expected {}",
                        weight_name, this_group_size, expected
                    ));
                }
            } else {
                group_size = Some(this_group_size);
            }

            weight_bytes.extend_from_slice(&self.read_tensor_bytes(weight_name)?);
            scales_bytes.extend_from_slice(&self.read_tensor_bytes(&scales_name)?);
            biases_bytes.extend_from_slice(&self.read_tensor_bytes(&biases_name)?);
            let rows = weight_entry.shape[0] as usize;
            total_rows = total_rows
                .checked_add(rows)
                .ok_or_else(|| "concat affine row count overflow".to_string())?;
            row_counts.push(rows);
        }

        let tensor = Arc::new(ConcatAffineTensor {
            weight_bytes: Arc::new(weight_bytes),
            scales_bytes: Arc::new(scales_bytes),
            biases_bytes: Arc::new(biases_bytes),
            total_rows,
            weight_words_per_row: weight_words_per_row.unwrap_or(0),
            qparams_per_row: qparams_per_row.unwrap_or(0),
            bits: bits.unwrap_or(0),
            group_size: group_size.unwrap_or(0),
            row_counts,
        });
        let mut cache = self
            .concat_affine_cache
            .lock()
            .map_err(|_| "concat affine cache mutex poisoned".to_string())?;
        Ok(cache
            .entry(cache_key)
            .or_insert_with(|| tensor.clone())
            .clone())
    }

    fn quant_params_for_weight(&self, weight_name: &str) -> QuantParams {
        let base = strip_quant_suffix(weight_name);
        self.model
            .config
            .quant_overrides
            .get(base)
            .cloned()
            .unwrap_or_else(|| self.model.config.default_quant.clone())
    }

    fn embed_token_words(&self, token_id: u32) -> Result<Vec<u16>> {
        let row = project_embedding_row(self, EMBED_TOKENS_BASE, token_id)?;
        Ok(f32s_to_bf16_words(&row))
    }
}

#[derive(Clone, Debug)]
struct QwenLayerNames {
    input_norm_weight: String,
    post_attention_norm_weight: String,
    ffn_gate_base: String,
    ffn_up_base: String,
    ffn_down_base: String,
    kind: QwenLayerKindNames,
}

#[derive(Clone, Debug)]
enum QwenLayerKindNames {
    Attention {
        q_proj_base: String,
        q_norm_weight: String,
        k_proj_base: String,
        k_norm_weight: String,
        v_proj_base: String,
        o_proj_base: String,
    },
    Recurrent {
        qkv_proj_weight: String,
        z_proj_weight: String,
        beta_proj_weight: String,
        alpha_proj_weight: String,
        a_log_name: String,
        dt_bias_name: String,
        conv1d_weight: String,
        norm_weight: String,
        out_proj_weight: String,
    },
}

impl QwenLayerNames {
    fn for_layer(config: &QwenConfig, layer_idx: usize) -> Result<Self> {
        let base = format!("language_model.model.layers.{layer_idx}");
        let kind = match config
            .text
            .layer_types
            .get(layer_idx)
            .map(String::as_str)
            .ok_or_else(|| format!("missing layer type for layer {}", layer_idx))?
        {
            "full_attention" => QwenLayerKindNames::Attention {
                q_proj_base: format!("{base}.self_attn.q_proj"),
                q_norm_weight: format!("{base}.self_attn.q_norm.weight"),
                k_proj_base: format!("{base}.self_attn.k_proj"),
                k_norm_weight: format!("{base}.self_attn.k_norm.weight"),
                v_proj_base: format!("{base}.self_attn.v_proj"),
                o_proj_base: format!("{base}.self_attn.o_proj"),
            },
            "linear_attention" => QwenLayerKindNames::Recurrent {
                qkv_proj_weight: format!("{base}.linear_attn.in_proj_qkv.weight"),
                z_proj_weight: format!("{base}.linear_attn.in_proj_z.weight"),
                beta_proj_weight: format!("{base}.linear_attn.in_proj_b.weight"),
                alpha_proj_weight: format!("{base}.linear_attn.in_proj_a.weight"),
                a_log_name: format!("{base}.linear_attn.A_log"),
                dt_bias_name: format!("{base}.linear_attn.dt_bias"),
                conv1d_weight: format!("{base}.linear_attn.conv1d.weight"),
                norm_weight: format!("{base}.linear_attn.norm.weight"),
                out_proj_weight: format!("{base}.linear_attn.out_proj.weight"),
            },
            other => {
                return Err(format!(
                    "unsupported layer type {} at layer {}",
                    other, layer_idx
                ))
            }
        };
        Ok(Self {
            input_norm_weight: format!("{base}.input_layernorm.weight"),
            post_attention_norm_weight: format!("{base}.post_attention_layernorm.weight"),
            ffn_gate_base: format!("{base}.mlp.gate_proj"),
            ffn_up_base: format!("{base}.mlp.up_proj"),
            ffn_down_base: format!("{base}.mlp.down_proj"),
            kind,
        })
    }
}

#[derive(Clone, Debug)]
struct AttentionCache {
    keys: Vec<Vec<f32>>,
    values: Vec<Vec<f32>>,
    head_dim: usize,
    seq_len: usize,
}

impl AttentionCache {
    fn new(head_count: usize, head_dim: usize) -> Self {
        Self {
            keys: vec![Vec::new(); head_count],
            values: vec![Vec::new(); head_count],
            head_dim,
            seq_len: 0,
        }
    }

    fn append(&mut self, k_rows: &[f32], v_rows: &[f32]) -> Result<()> {
        if k_rows.len() != self.keys.len() * self.head_dim {
            return Err(format!(
                "attention key append length mismatch: got {} expected {}",
                k_rows.len(),
                self.keys.len() * self.head_dim
            ));
        }
        if v_rows.len() != self.values.len() * self.head_dim {
            return Err(format!(
                "attention value append length mismatch: got {} expected {}",
                v_rows.len(),
                self.values.len() * self.head_dim
            ));
        }
        for head in 0..self.keys.len() {
            let start = head * self.head_dim;
            let end = start + self.head_dim;
            self.keys[head].extend_from_slice(&k_rows[start..end]);
            self.values[head].extend_from_slice(&v_rows[start..end]);
        }
        self.seq_len += 1;
        Ok(())
    }

    fn reset(&mut self) {
        for keys in &mut self.keys {
            keys.clear();
        }
        for values in &mut self.values {
            values.clear();
        }
        self.seq_len = 0;
    }
}

fn qwen_attention_rope_spec(cfg: &QwenTextConfig, max_context: usize) -> Result<AttentionRopeSpec> {
    let rotary_dim = (cfg.partial_rotary_factor * cfg.head_dim as f32).round() as usize;
    let rotary_dim = rotary_dim.clamp(0, cfg.head_dim);
    Ok(AttentionRopeSpec {
        n_dims: i32::try_from(rotary_dim)
            .map_err(|_| "attention rotary_dim does not fit in i32".to_string())?,
        sections: [
            0,
            0,
            0,
            i32::try_from(rotary_dim)
                .map_err(|_| "attention rotary_dim does not fit in i32".to_string())?,
        ],
        mode: GGML_ROPE_TYPE_IMROPE,
        n_ctx_orig: i32::try_from(max_context)
            .map_err(|_| "attention max_context does not fit in i32".to_string())?,
        freq_base: cfg.rope_theta,
        freq_scale: 1.0,
        ext_factor: 0.0,
        attn_factor: 1.0,
        beta_fast: 0.0,
        beta_slow: 0.0,
    })
}

struct MlxAttentionMetalExecution {
    decode: AttentionDecodeGraph,
    session: MetalGraphSession,
}

impl MlxAttentionMetalExecution {
    fn eval(
        &self,
        ctx: &mut Context,
        spec: &AttentionDecodeSpec,
        input_words: &[u16],
        positions: &[i32],
        cache_tokens: usize,
    ) -> Result<Vec<f32>> {
        let input_storage;
        let input_primary = match spec.block.input {
            ProbeInputKind::Embeddings {
                hidden_size,
                input_type,
            } => {
                let expected = (hidden_size as usize)
                    .checked_mul(positions.len())
                    .ok_or_else(|| "attention input size overflow".to_string())?;
                if input_words.len() != expected {
                    return Err(format!(
                        "attention input length mismatch: got {} expected {}",
                        input_words.len(),
                        expected
                    ));
                }
                match input_type {
                    TensorType::BF16 => bf16_words_as_bytes(input_words),
                    TensorType::F32 => {
                        input_storage = input_words
                            .iter()
                            .copied()
                            .map(bf16_word_to_f32)
                            .collect::<Vec<_>>();
                        f32s_as_bytes(&input_storage)
                    }
                    other => {
                        return Err(format!(
                            "unsupported attention metal embedding input type {}",
                            other.name()
                        ))
                    }
                }
            }
            ProbeInputKind::TokenIds { .. } => {
                return Err("attention metal layer expects embedding input".to_string())
            }
        };
        let input_binding = self
            .session
            .compiled()
            .bindings
            .get(&self.decode.input_primary)
            .ok_or_else(|| "missing attention input_primary binding".to_string())?;
        self.session
            .runtime()
            .write_buffer(
                &self.session.compiled().main_buffer,
                input_binding.offset_bytes,
                input_primary,
            )
            .map_err(|err| err.to_string())?;
        execute_prepared_attention_decode_metal_no_readback_prepared_input(
            self.session.runtime(),
            ctx,
            spec,
            &self.decode,
            self.session.compiled(),
            positions,
            cache_tokens,
        )
        .map_err(|err| err.to_string())?;
        let output = ctx
            .tensor(self.decode.result_output)
            .ok_or_else(|| "attention result tensor is invalid".to_string())?;
        let hidden_size = usize::try_from(output.ne[0]).map_err(|_| {
            format!(
                "attention output dim 0 does not fit usize: {}",
                output.ne[0]
            )
        })?;
        let run_tokens = usize::try_from(output.ne[1]).map_err(|_| {
            format!(
                "attention output dim 1 does not fit usize: {}",
                output.ne[1]
            )
        })?;
        if run_tokens != positions.len() {
            return Err(format!(
                "attention metal decode returned {} tokens, expected {}",
                run_tokens,
                positions.len()
            ));
        }
        let binding = self
            .session
            .compiled()
            .bindings
            .get(&self.decode.result_output)
            .ok_or_else(|| "missing attention result_output binding".to_string())?;
        let output_type = output.desc.ty;
        self.session
            .runtime()
            .with_readable_buffer_range(
                &self.session.compiled().main_buffer,
                binding.offset_bytes,
                binding.size_bytes,
                |bytes| {
                    let expected_floats = hidden_size
                        .checked_mul(run_tokens)
                        .ok_or_else(|| "attention output size overflow".to_string())?;
                    let expected_bytes = match output_type {
                        TensorType::F32 => expected_floats * std::mem::size_of::<f32>(),
                        TensorType::BF16 => expected_floats * std::mem::size_of::<u16>(),
                        other => {
                            return Err(format!(
                                "unsupported attention result output type {}",
                                other.name()
                            ))
                        }
                    };
                    if bytes.len() != expected_bytes {
                        return Err(format!(
                            "attention output byte size mismatch: got {} expected {}",
                            bytes.len(),
                            expected_bytes
                        ));
                    }
                    let mut hidden = Vec::with_capacity(expected_floats);
                    match output_type {
                        TensorType::F32 => {
                            for chunk in bytes.chunks_exact(std::mem::size_of::<f32>()) {
                                hidden.push(f32::from_le_bytes([
                                    chunk[0], chunk[1], chunk[2], chunk[3],
                                ]));
                            }
                        }
                        TensorType::BF16 => {
                            for chunk in bytes.chunks_exact(std::mem::size_of::<u16>()) {
                                hidden.push(bf16_word_to_f32(u16::from_le_bytes([
                                    chunk[0], chunk[1],
                                ])));
                            }
                        }
                        _ => unreachable!(),
                    }
                    Ok(hidden)
                },
            )
            .map_err(|err| err.to_string())
    }
}

struct MlxAttentionMetalLayer {
    spec: AttentionDecodeSpec,
    tensor_ids: BTreeMap<String, TensorId>,
    ctx: Context,
    runtime: MetalRuntime,
    main_buffer: MetalBuffer,
    step_execution: MlxAttentionMetalExecution,
    prefill_execution: Option<(usize, MlxAttentionMetalExecution)>,
    k_cache_zeroes: Vec<u8>,
    v_cache_zeroes: Vec<u8>,
}

impl MlxAttentionMetalLayer {
    fn new(
        weights: &QwenMlxWeights,
        layer_idx: usize,
        layer: &QwenLayerNames,
        max_context: usize,
    ) -> Result<Self> {
        Self::new_with_runtime(weights, layer_idx, layer, max_context, MetalRuntime::new()?)
    }

    fn new_with_runtime(
        weights: &QwenMlxWeights,
        layer_idx: usize,
        layer: &QwenLayerNames,
        max_context: usize,
        runtime: MetalRuntime,
    ) -> Result<Self> {
        let QwenLayerKindNames::Attention {
            q_proj_base,
            q_norm_weight,
            k_proj_base,
            k_norm_weight,
            v_proj_base,
            o_proj_base,
        } = &layer.kind
        else {
            return Err("attention metal layer requires attention tensor names".to_string());
        };

        let cfg = &weights.config().text;
        let input_norm_values = weights.read_tensor_f32(&layer.input_norm_weight)?;
        let q_norm_values = weights.read_tensor_f32(q_norm_weight)?;
        let k_norm_values = weights.read_tensor_f32(k_norm_weight)?;

        let q_proj_weight = format!("{q_proj_base}.weight");
        let k_proj_weight = format!("{k_proj_base}.weight");
        let v_proj_weight = format!("{v_proj_base}.weight");
        let o_proj_weight = format!("{o_proj_base}.weight");

        let q_proj_entry = weights.tensor(&q_proj_weight)?;
        let k_proj_entry = weights.tensor(&k_proj_weight)?;
        let v_proj_entry = weights.tensor(&v_proj_weight)?;
        let o_proj_entry = weights.tensor(&o_proj_weight)?;

        let q_proj_words = weights.rank2_weight_as_bf16_words_cached(&q_proj_weight)?;
        let k_proj_words = weights.rank2_weight_as_bf16_words_cached(&k_proj_weight)?;
        let v_proj_words = weights.rank2_weight_as_bf16_words_cached(&v_proj_weight)?;
        let o_proj_words = weights.rank2_weight_as_bf16_words_cached(&o_proj_weight)?;

        let q_inner_dim = q_proj_words
            .len()
            .checked_div(q_proj_entry.shape[0] as usize)
            .ok_or_else(|| format!("q_proj row count is zero for {}", q_proj_weight))?;
        let k_inner_dim = k_proj_words
            .len()
            .checked_div(k_proj_entry.shape[0] as usize)
            .ok_or_else(|| format!("k_proj row count is zero for {}", k_proj_weight))?;
        let v_inner_dim = v_proj_words
            .len()
            .checked_div(v_proj_entry.shape[0] as usize)
            .ok_or_else(|| format!("v_proj row count is zero for {}", v_proj_weight))?;
        let o_inner_dim = o_proj_words
            .len()
            .checked_div(o_proj_entry.shape[0] as usize)
            .ok_or_else(|| format!("o_proj row count is zero for {}", o_proj_weight))?;
        for (name, entry, words, inner_dim) in [
            (
                &q_proj_weight,
                q_proj_entry,
                q_proj_words.as_slice(),
                q_inner_dim,
            ),
            (
                &k_proj_weight,
                k_proj_entry,
                k_proj_words.as_slice(),
                k_inner_dim,
            ),
            (
                &v_proj_weight,
                v_proj_entry,
                v_proj_words.as_slice(),
                v_inner_dim,
            ),
            (
                &o_proj_weight,
                o_proj_entry,
                o_proj_words.as_slice(),
                o_inner_dim,
            ),
        ] {
            let row_count = entry.shape[0] as usize;
            if row_count == 0 || words.len() != row_count * inner_dim {
                return Err(format!(
                    "attention weight {} length mismatch: got {} rows {} inner {}",
                    name,
                    words.len(),
                    row_count,
                    inner_dim
                ));
            }
        }

        let mut mem_size = 32usize << 20;
        for len in [
            input_norm_values.len() * std::mem::size_of::<f32>(),
            q_norm_values.len() * std::mem::size_of::<f32>(),
            k_norm_values.len() * std::mem::size_of::<f32>(),
            q_proj_words.len() * std::mem::size_of::<u16>(),
            k_proj_words.len() * std::mem::size_of::<u16>(),
            v_proj_words.len() * std::mem::size_of::<u16>(),
            o_proj_words.len() * std::mem::size_of::<u16>(),
        ] {
            mem_size = mem_size
                .checked_add(len)
                .ok_or_else(|| "attention metal context size overflow".to_string())?;
        }

        let mut ctx = Context::new(InitParams {
            mem_size,
            mem_buffer: None,
            no_alloc: false,
        });
        let mut tensor_ids = BTreeMap::<String, TensorId>::new();

        load_f32_tensor_1d(
            &mut ctx,
            &mut tensor_ids,
            &layer.input_norm_weight,
            &input_norm_values,
            BufferUsage::Weights,
        )?;
        load_f32_tensor_1d(
            &mut ctx,
            &mut tensor_ids,
            q_norm_weight,
            &q_norm_values,
            BufferUsage::Weights,
        )?;
        load_f32_tensor_1d(
            &mut ctx,
            &mut tensor_ids,
            k_norm_weight,
            &k_norm_values,
            BufferUsage::Weights,
        )?;
        load_bf16_tensor_rank2_words(
            &mut ctx,
            &mut tensor_ids,
            &q_proj_weight,
            q_proj_entry.shape[0] as usize,
            q_inner_dim,
            q_proj_words.as_slice(),
            BufferUsage::Weights,
        )?;
        load_bf16_tensor_rank2_words(
            &mut ctx,
            &mut tensor_ids,
            &k_proj_weight,
            k_proj_entry.shape[0] as usize,
            k_inner_dim,
            k_proj_words.as_slice(),
            BufferUsage::Weights,
        )?;
        load_bf16_tensor_rank2_words(
            &mut ctx,
            &mut tensor_ids,
            &v_proj_weight,
            v_proj_entry.shape[0] as usize,
            v_inner_dim,
            v_proj_words.as_slice(),
            BufferUsage::Weights,
        )?;
        load_bf16_tensor_rank2_words(
            &mut ctx,
            &mut tensor_ids,
            &o_proj_weight,
            o_proj_entry.shape[0] as usize,
            o_inner_dim,
            o_proj_words.as_slice(),
            BufferUsage::Weights,
        )?;

        let hidden_size = u32::try_from(cfg._hidden_size)
            .map_err(|_| "hidden_size does not fit in u32".to_string())?;
        let spec = AttentionDecodeSpec {
            block: AttentionBlockSpec {
                input: ProbeInputKind::Embeddings {
                    hidden_size,
                    input_type: TensorType::F32,
                },
                input_norm_name: layer.input_norm_weight.clone(),
                q_proj_name: q_proj_weight.clone(),
                q_proj_scale_name: None,
                q_layout: AttentionQueryLayout::InterleavedQueryGate {
                    gate_activation: UnaryOp::Sigmoid,
                },
                k_proj_name: k_proj_weight.clone(),
                k_proj_scale_name: None,
                v_proj_name: Some(v_proj_weight.clone()),
                v_proj_scale_name: None,
                output_proj_name: o_proj_weight.clone(),
                output_proj_scale_name: None,
                q_norm_name: Some(q_norm_weight.clone()),
                k_norm_name: Some(k_norm_weight.clone()),
                v_norm_epsilon: None,
                q_head_dim: u32::try_from(cfg.head_dim)
                    .map_err(|_| "head_dim does not fit in u32".to_string())?,
                q_head_count: u32::try_from(cfg.num_attention_heads)
                    .map_err(|_| "num_attention_heads does not fit in u32".to_string())?,
                k_head_dim: u32::try_from(cfg.head_dim)
                    .map_err(|_| "head_dim does not fit in u32".to_string())?,
                kv_head_count: u32::try_from(cfg.num_key_value_heads)
                    .map_err(|_| "num_key_value_heads does not fit in u32".to_string())?,
                v_head_dim: u32::try_from(cfg.head_dim)
                    .map_err(|_| "head_dim does not fit in u32".to_string())?,
                rms_epsilon: cfg.rms_norm_eps,
                rope: Some(qwen_attention_rope_spec(cfg, max_context)?),
                rope_factors_name: None,
                attention_scale: 1.0f32 / (cfg.head_dim as f32).sqrt(),
                causal: true,
                causal_window: None,
                residual: false,
            },
            cache: AttentionKvCacheSpec {
                max_context: u32::try_from(max_context)
                    .map_err(|_| "attention max_context does not fit in u32".to_string())?,
                max_sequences: 1,
                k_type: TensorType::F16,
                v_type: TensorType::F16,
            },
            cache_layer_index: u32::try_from(layer_idx)
                .map_err(|_| "layer_idx does not fit in u32".to_string())?,
            write_kv: true,
        };

        let main_buffer = create_context_main_buffer(&runtime, &ctx, BufferStorageMode::Shared)?;
        let step_execution = Self::compile_execution(
            &runtime,
            &mut ctx,
            &tensor_ids,
            &spec,
            1,
            max_context,
            &main_buffer,
        )?;

        let k_cache_zeroes = vec![
            0u8;
            step_execution
                .session
                .compiled()
                .bindings
                .get(&step_execution.decode.k_cache)
                .ok_or_else(|| "missing attention k_cache binding".to_string())?
                .size_bytes
        ];
        let v_cache_zeroes = vec![
            0u8;
            step_execution
                .session
                .compiled()
                .bindings
                .get(&step_execution.decode.v_cache)
                .ok_or_else(|| "missing attention v_cache binding".to_string())?
                .size_bytes
        ];

        let mut layer = Self {
            spec,
            tensor_ids,
            ctx,
            runtime,
            main_buffer,
            step_execution,
            prefill_execution: None,
            k_cache_zeroes,
            v_cache_zeroes,
        };
        layer.reset()?;
        Ok(layer)
    }

    fn compile_execution(
        runtime: &MetalRuntime,
        ctx: &mut Context,
        tensor_ids: &BTreeMap<String, TensorId>,
        spec: &AttentionDecodeSpec,
        n_tokens: usize,
        attention_key_count: usize,
        main_buffer: &MetalBuffer,
    ) -> Result<MlxAttentionMetalExecution> {
        let (decode, prepared) = prepare_attention_decode_graph_with_key_count(
            ctx,
            tensor_ids,
            spec,
            n_tokens,
            attention_key_count,
            runtime.features(),
        )
        .map_err(|err| err.to_string())?;
        let session = MetalGraphSession::from_runtime_with_main_buffer(
            runtime.clone(),
            &prepared,
            main_buffer,
            BufferStorageMode::Shared,
        )?;
        Ok(MlxAttentionMetalExecution { decode, session })
    }

    fn reset(&mut self) -> Result<()> {
        let compiled = self.step_execution.session.compiled();
        let runtime = self.step_execution.session.runtime();
        let k_binding = compiled
            .bindings
            .get(&self.step_execution.decode.k_cache)
            .ok_or_else(|| "missing attention k_cache binding".to_string())?;
        runtime.write_buffer(
            &compiled.main_buffer,
            k_binding.offset_bytes,
            &self.k_cache_zeroes,
        )?;
        let v_binding = compiled
            .bindings
            .get(&self.step_execution.decode.v_cache)
            .ok_or_else(|| "missing attention v_cache binding".to_string())?;
        runtime.write_buffer(
            &compiled.main_buffer,
            v_binding.offset_bytes,
            &self.v_cache_zeroes,
        )?;
        Ok(())
    }

    fn sync_from_cpu_cache(&mut self, cache: &AttentionCache) -> Result<()> {
        let q_head_dim = self.spec.block.k_head_dim as usize;
        let kv_head_count = self.spec.block.kv_head_count as usize;
        if cache.head_dim != q_head_dim || cache.keys.len() != kv_head_count {
            return Err(format!(
                "attention cache layout mismatch: cpu heads={} dim={} metal heads={} dim={}",
                cache.keys.len(),
                cache.head_dim,
                kv_head_count,
                q_head_dim
            ));
        }
        let max_context = self.spec.cache.max_context as usize;
        if cache.seq_len > max_context {
            return Err(format!(
                "attention cache length {} exceeds max_context {}",
                cache.seq_len, max_context
            ));
        }

        let k_width = q_head_dim
            .checked_mul(kv_head_count)
            .ok_or_else(|| "attention cache key width overflow".to_string())?;
        let v_head_dim = self.spec.block.v_head_dim as usize;
        let v_width = v_head_dim
            .checked_mul(kv_head_count)
            .ok_or_else(|| "attention cache value width overflow".to_string())?;
        let mut k_words = Vec::with_capacity(
            cache
                .seq_len
                .checked_mul(k_width)
                .ok_or_else(|| "attention cache key word count overflow".to_string())?,
        );
        let mut v_words = Vec::with_capacity(
            cache
                .seq_len
                .checked_mul(v_width)
                .ok_or_else(|| "attention cache value word count overflow".to_string())?,
        );
        for token_idx in 0..cache.seq_len {
            for head in 0..kv_head_count {
                let k_base = token_idx
                    .checked_mul(q_head_dim)
                    .ok_or_else(|| "attention cache key base overflow".to_string())?;
                let k_row = &cache.keys[head][k_base..k_base + q_head_dim];
                k_words.extend(k_row.iter().copied().map(f32_to_f16));

                let v_base = token_idx
                    .checked_mul(v_head_dim)
                    .ok_or_else(|| "attention cache value base overflow".to_string())?;
                let v_row = &cache.values[head][v_base..v_base + v_head_dim];
                v_words.extend(v_row.iter().copied().map(f32_to_f16));
            }
        }

        let compiled = self.step_execution.session.compiled();
        let runtime = self.step_execution.session.runtime();
        let k_binding = compiled
            .bindings
            .get(&self.step_execution.decode.k_cache)
            .ok_or_else(|| "missing attention k_cache binding".to_string())?;
        runtime.write_buffer(
            &compiled.main_buffer,
            k_binding.offset_bytes,
            bf16_words_as_bytes(&k_words),
        )?;
        let v_binding = compiled
            .bindings
            .get(&self.step_execution.decode.v_cache)
            .ok_or_else(|| "missing attention v_cache binding".to_string())?;
        runtime.write_buffer(
            &compiled.main_buffer,
            v_binding.offset_bytes,
            bf16_words_as_bytes(&v_words),
        )?;
        Ok(())
    }

    fn eval(&mut self, input_words: &[u16], position: usize) -> Result<Vec<f32>> {
        let positions = [i32::try_from(position)
            .map_err(|_| format!("attention position {} does not fit in i32", position))?];
        let cache_tokens = position
            .checked_add(1)
            .ok_or_else(|| "attention cache token count overflow".to_string())?;
        self.step_execution.eval(
            &mut self.ctx,
            &self.spec,
            input_words,
            &positions,
            cache_tokens,
        )
    }

    fn eval_rows(
        &mut self,
        input_words: &[u16],
        start_position: usize,
        n_tokens: usize,
    ) -> Result<Vec<f32>> {
        if n_tokens == 0 {
            return Ok(Vec::new());
        }
        let hidden_size = match self.spec.block.input {
            ProbeInputKind::Embeddings { hidden_size, .. } => hidden_size as usize,
            ProbeInputKind::TokenIds { .. } => {
                return Err("attention metal layer expects embedding input".to_string())
            }
        };
        if input_words.len()
            != hidden_size
                .checked_mul(n_tokens)
                .ok_or_else(|| "attention metal input size overflow".to_string())?
        {
            return Err(format!(
                "attention metal input length mismatch: got {} expected {}",
                input_words.len(),
                hidden_size * n_tokens
            ));
        }
        if n_tokens == 1 {
            return self.eval(input_words, start_position);
        }
        let needs_compile = self
            .prefill_execution
            .as_ref()
            .map(|(compiled_tokens, _)| *compiled_tokens != n_tokens)
            .unwrap_or(true);
        if needs_compile {
            let attention_key_count = start_position
                .checked_add(n_tokens)
                .ok_or_else(|| "attention key count overflow".to_string())?;
            let execution = Self::compile_execution(
                &self.runtime,
                &mut self.ctx,
                &self.tensor_ids,
                &self.spec,
                n_tokens,
                attention_key_count,
                &self.main_buffer,
            )?;
            self.prefill_execution = Some((n_tokens, execution));
        }
        let positions = (0..n_tokens)
            .map(|offset| {
                let position = start_position
                    .checked_add(offset)
                    .ok_or_else(|| "attention position overflow".to_string())?;
                i32::try_from(position)
                    .map_err(|_| format!("attention position {} does not fit in i32", position))
            })
            .collect::<Result<Vec<_>>>()?;
        let cache_tokens = start_position
            .checked_add(n_tokens)
            .ok_or_else(|| "attention cache token count overflow".to_string())?;
        let (_, execution) = self
            .prefill_execution
            .as_ref()
            .ok_or_else(|| "missing compiled attention prefill execution".to_string())?;
        let output = execution.eval(
            &mut self.ctx,
            &self.spec,
            input_words,
            &positions,
            cache_tokens,
        )?;
        self.copy_prefill_cache_to_step(execution)?;
        Ok(output)
    }

    fn copy_prefill_cache_to_step(&self, execution: &MlxAttentionMetalExecution) -> Result<()> {
        let prefill_compiled = execution.session.compiled();
        let step_compiled = self.step_execution.session.compiled();
        for (src_id, dst_id, label) in [
            (
                execution.decode.k_cache,
                self.step_execution.decode.k_cache,
                "k_cache",
            ),
            (
                execution.decode.v_cache,
                self.step_execution.decode.v_cache,
                "v_cache",
            ),
        ] {
            let src = prefill_compiled
                .bindings
                .get(&src_id)
                .ok_or_else(|| format!("missing attention prefill {label} binding"))?;
            let dst = step_compiled
                .bindings
                .get(&dst_id)
                .ok_or_else(|| format!("missing attention step {label} binding"))?;
            if src.size_bytes > dst.size_bytes {
                return Err(format!(
                    "attention prefill {label} cache is larger than step cache: {} > {}",
                    src.size_bytes, dst.size_bytes
                ));
            }
            if src.offset_bytes != dst.offset_bytes {
                execution
                    .session
                    .runtime()
                    .copy_buffer_range(
                        &prefill_compiled.main_buffer,
                        src.offset_bytes,
                        &step_compiled.main_buffer,
                        dst.offset_bytes,
                        src.size_bytes,
                    )
                    .map_err(|err| err.to_string())?;
            }
        }
        Ok(())
    }

    fn eval_with_decode_tail(
        &mut self,
        weights: &QwenMlxWeights,
        tail: &mut MlxQwenMetalDecodeTailBackend,
        spec: &MlxQwenDecodeTailSpec,
        input_words: &[u16],
        position: usize,
    ) -> Result<Vec<u16>> {
        let positions = [i32::try_from(position)
            .map_err(|_| format!("attention position {} does not fit in i32", position))?];
        let cache_tokens = position
            .checked_add(1)
            .ok_or_else(|| "attention cache token count overflow".to_string())?;
        let input_storage;
        let input_primary = match self.spec.block.input {
            ProbeInputKind::Embeddings {
                hidden_size,
                input_type,
            } => {
                if input_words.len() != hidden_size as usize {
                    return Err(format!(
                        "attention decode input length mismatch: got {} expected {}",
                        input_words.len(),
                        hidden_size
                    ));
                }
                match input_type {
                    TensorType::BF16 => bf16_words_as_bytes(input_words),
                    TensorType::F32 => {
                        input_storage = input_words
                            .iter()
                            .copied()
                            .map(bf16_word_to_f32)
                            .collect::<Vec<_>>();
                        f32s_as_bytes(&input_storage)
                    }
                    other => {
                        return Err(format!(
                            "unsupported attention metal embedding input type {}",
                            other.name()
                        ))
                    }
                }
            }
            ProbeInputKind::TokenIds { .. } => {
                return Err("attention metal layer expects embedding input".to_string())
            }
        };
        let input_binding = self
            .step_execution
            .session
            .compiled()
            .bindings
            .get(&self.step_execution.decode.input_primary)
            .ok_or_else(|| "missing attention input_primary binding".to_string())?;
        self.step_execution
            .session
            .runtime()
            .write_buffer(
                &self.step_execution.session.compiled().main_buffer,
                input_binding.offset_bytes,
                input_primary,
            )
            .map_err(|err| err.to_string())?;
        execute_prepared_attention_decode_metal_no_readback_prepared_input(
            self.step_execution.session.runtime(),
            &mut self.ctx,
            &self.spec,
            &self.step_execution.decode,
            self.step_execution.session.compiled(),
            &positions,
            cache_tokens,
        )
        .map_err(|err| err.to_string())?;
        let output = self
            .ctx
            .tensor(self.step_execution.decode.result_output)
            .ok_or_else(|| "attention decode result tensor is invalid".to_string())?;
        let binding = self
            .step_execution
            .session
            .compiled()
            .bindings
            .get(&self.step_execution.decode.result_output)
            .ok_or_else(|| "missing attention result_output binding".to_string())?;
        tail.run_from_graph_output(
            weights,
            spec,
            input_words,
            &self.step_execution.session.compiled().main_buffer,
            binding.offset_bytes,
            output.desc.ty,
        )
    }

    fn eval_with_decode_tail_from_buffer(
        &mut self,
        weights: &QwenMlxWeights,
        tail: &mut MlxQwenMetalDecodeTailBackend,
        spec: &MlxQwenDecodeTailSpec,
        input_hidden: &MetalBuffer,
        output_hidden: &MetalBuffer,
        position: usize,
    ) -> Result<()> {
        let positions = [i32::try_from(position)
            .map_err(|_| format!("attention position {} does not fit in i32", position))?];
        let cache_tokens = position
            .checked_add(1)
            .ok_or_else(|| "attention cache token count overflow".to_string())?;
        let compiled = self.step_execution.session.compiled();
        let input_binding = compiled
            .bindings
            .get(&self.step_execution.decode.input_primary)
            .ok_or_else(|| "missing attention input_primary binding".to_string())?;
        let main_buffer = compiled.main_buffer.clone();
        match self.spec.block.input {
            ProbeInputKind::Embeddings { input_type, .. } => match input_type {
                TensorType::BF16 => {
                    let expected_input_bytes = spec
                        .hidden_size
                        .checked_mul(std::mem::size_of::<u16>())
                        .ok_or_else(|| "attention input byte count overflow".to_string())?;
                    if input_binding.size_bytes != expected_input_bytes {
                        return Err(format!(
                            "attention input binding byte mismatch: got {} expected {}",
                            input_binding.size_bytes, expected_input_bytes
                        ));
                    }
                    execute_prepared_attention_decode_metal_no_readback_buffer_input(
                        self.step_execution.session.runtime(),
                        &mut self.ctx,
                        &self.spec,
                        &self.step_execution.decode,
                        self.step_execution.session.compiled(),
                        input_hidden,
                        0,
                        &positions,
                        cache_tokens,
                    )
                    .map_err(|err| err.to_string())?;
                }
                TensorType::F32 => {
                    let expected_input_bytes = spec
                        .hidden_size
                        .checked_mul(std::mem::size_of::<f32>())
                        .ok_or_else(|| "attention input byte count overflow".to_string())?;
                    if input_binding.size_bytes != expected_input_bytes {
                        return Err(format!(
                            "attention input binding byte mismatch: got {} expected {}",
                            input_binding.size_bytes, expected_input_bytes
                        ));
                    }
                    let output = self
                        .ctx
                        .tensor(self.step_execution.decode.result_output)
                        .ok_or_else(|| "attention decode result tensor is invalid".to_string())?;
                    let binding = self
                        .step_execution
                        .session
                        .compiled()
                        .bindings
                        .get(&self.step_execution.decode.result_output)
                        .ok_or_else(|| "missing attention result_output binding".to_string())?;
                    let output_ty = output.desc.ty;
                    let output_offset_bytes = binding.offset_bytes;
                    if !metal_decode_chain_active_fusion_enabled() {
                        tail.run_bf16_hidden_to_f32_graph_input(
                            input_hidden,
                            &main_buffer,
                            input_binding.offset_bytes,
                            spec.hidden_size,
                        )?;
                        execute_prepared_attention_decode_metal_no_readback_prepared_input(
                            self.step_execution.session.runtime(),
                            &mut self.ctx,
                            &self.spec,
                            &self.step_execution.decode,
                            self.step_execution.session.compiled(),
                            &positions,
                            cache_tokens,
                        )
                        .map_err(|err| err.to_string())?;
                        tail.run_from_graph_output_into_buffer(
                            weights,
                            spec,
                            input_hidden,
                            output_hidden,
                            &main_buffer,
                            output_offset_bytes,
                            output_ty,
                        )?;
                        return Ok(());
                    }
                    tail.runtime.begin_command_batch().map_err(|err| {
                        format!("begin attention decode chain batch failed: {err}")
                    })?;
                    let dispatch_result = (|| -> Result<()> {
                        tail.dispatch_bf16_hidden_to_f32_graph_input_with_tracking(
                            input_hidden,
                            &main_buffer,
                            input_binding.offset_bytes,
                            spec.hidden_size,
                            false,
                        )?;
                        tail.runtime
                            .memory_barrier_buffers()
                            .map_err(|err| err.to_string())?;
                        execute_prepared_attention_decode_metal_no_readback_prepared_input_in_active_batch(
                            self.step_execution.session.runtime(),
                            &mut self.ctx,
                            &self.spec,
                            &self.step_execution.decode,
                            self.step_execution.session.compiled(),
                            &positions,
                            cache_tokens,
                        )
                        .map_err(|err| err.to_string())?;
                        tail.runtime
                            .memory_barrier_buffers()
                            .map_err(|err| err.to_string())?;
                        tail.dispatch_run_from_graph_output_into_buffer(
                            weights,
                            spec,
                            input_hidden,
                            output_hidden,
                            &main_buffer,
                            output_offset_bytes,
                            output_ty,
                            false,
                        )
                    })();
                    if let Err(err) = dispatch_result {
                        let _ = tail.runtime.discard_command_batch();
                        return Err(err);
                    }
                    tail.runtime
                        .end_command_batch()
                        .map_err(|err| format!("end attention decode chain batch failed: {err}"))?;
                    return Ok(());
                }
                other => {
                    return Err(format!(
                        "unsupported attention metal embedding input type {}",
                        other.name()
                    ))
                }
            },
            ProbeInputKind::TokenIds { .. } => {
                return Err("attention metal layer expects embedding input".to_string())
            }
        }
        let output = self
            .ctx
            .tensor(self.step_execution.decode.result_output)
            .ok_or_else(|| "attention decode result tensor is invalid".to_string())?;
        let binding = self
            .step_execution
            .session
            .compiled()
            .bindings
            .get(&self.step_execution.decode.result_output)
            .ok_or_else(|| "missing attention result_output binding".to_string())?;
        tail.run_from_graph_output_into_buffer(
            weights,
            spec,
            input_hidden,
            output_hidden,
            &main_buffer,
            binding.offset_bytes,
            output.desc.ty,
        )
    }
}

#[derive(Clone, Debug)]
struct CpuRecurrentState {
    conv_state: Vec<f32>,
    state: Vec<f32>,
}

impl CpuRecurrentState {
    fn new(config: &QwenConfig) -> Self {
        let qkv_dim = recurrent_qkv_dim(config);
        let conv_prefix = config.text.linear_conv_kernel_dim.saturating_sub(1);
        let state_len = config.text.linear_num_value_heads
            * config.text.linear_key_head_dim
            * config.text.linear_value_head_dim;
        Self {
            conv_state: vec![0.0; conv_prefix * qkv_dim],
            state: vec![0.0; state_len],
        }
    }

    fn reset(&mut self) {
        self.conv_state.fill(0.0);
        self.state.fill(0.0);
    }
}

struct MlxRecurrentMetalExecution {
    decode: DeltaNetRecurrentDecodeGraph,
    session: MetalGraphSession,
}

impl MlxRecurrentMetalExecution {
    fn eval(
        &self,
        ctx: &mut Context,
        spec: &DeltaNetRecurrentDecodeSpec,
        input_words: &[u16],
        n_tokens: usize,
    ) -> Result<Vec<f32>> {
        let input_storage;
        let input_primary = match spec.block.input {
            ProbeInputKind::Embeddings { input_type, .. } => match input_type {
                TensorType::BF16 => bf16_words_as_bytes(input_words),
                TensorType::F32 => {
                    input_storage = input_words
                        .iter()
                        .copied()
                        .map(bf16_word_to_f32)
                        .collect::<Vec<_>>();
                    f32s_as_bytes(&input_storage)
                }
                other => {
                    return Err(format!(
                        "unsupported recurrent metal embedding input type {}",
                        other.name()
                    ))
                }
            },
            ProbeInputKind::TokenIds { .. } => {
                return Err("recurrent metal layer expects embedding input".to_string())
            }
        };
        execute_compiled_graph(
            self.session.runtime(),
            ctx,
            self.session.compiled(),
            &[
                MetalGraphTensorWrite {
                    tensor_id: self.decode.input_primary,
                    bytes: input_primary,
                },
                MetalGraphTensorWrite {
                    tensor_id: self.decode.input_state_rows,
                    bytes: i32s_as_bytes(&[0]),
                },
            ],
            &[],
        )
        .map_err(|err| err.to_string())?;

        let output = ctx
            .tensor(self.decode.result_output)
            .ok_or_else(|| "delta-net recurrent result tensor is invalid".to_string())?;
        let hidden_size = usize::try_from(output.ne[0]).map_err(|_| {
            format!(
                "delta-net recurrent output dim 0 does not fit usize: {}",
                output.ne[0]
            )
        })?;
        let run_tokens = usize::try_from(output.ne[1]).map_err(|_| {
            format!(
                "delta-net recurrent output dim 1 does not fit usize: {}",
                output.ne[1]
            )
        })?;
        if run_tokens != n_tokens {
            return Err(format!(
                "recurrent metal decode returned {} tokens, expected {}",
                run_tokens, n_tokens
            ));
        }
        let binding = self
            .session
            .compiled()
            .bindings
            .get(&self.decode.result_output)
            .ok_or_else(|| "missing recurrent result_output binding".to_string())?;
        let output_type = output.desc.ty;
        self.session
            .runtime()
            .with_readable_buffer_range(
                &self.session.compiled().main_buffer,
                binding.offset_bytes,
                binding.size_bytes,
                |bytes| {
                    if bytes.len() != binding.size_bytes {
                        return Err(format!(
                            "recurrent result byte length mismatch: got {} expected {}",
                            bytes.len(),
                            binding.size_bytes
                        ));
                    }
                    let expected_floats = hidden_size
                        .checked_mul(run_tokens)
                        .ok_or_else(|| "recurrent output size overflow".to_string())?;
                    let expected_bytes = match output_type {
                        TensorType::F32 => expected_floats * std::mem::size_of::<f32>(),
                        TensorType::BF16 => expected_floats * std::mem::size_of::<u16>(),
                        other => {
                            return Err(format!(
                                "unsupported recurrent result output type {}",
                                other.name()
                            ))
                        }
                    };
                    if bytes.len() != expected_bytes {
                        return Err(format!(
                            "recurrent output byte size mismatch: got {} expected {}",
                            bytes.len(),
                            expected_bytes
                        ));
                    }
                    let mut hidden = Vec::with_capacity(expected_floats);
                    match output_type {
                        TensorType::F32 => {
                            for chunk in bytes.chunks_exact(std::mem::size_of::<f32>()) {
                                hidden.push(f32::from_le_bytes([
                                    chunk[0], chunk[1], chunk[2], chunk[3],
                                ]));
                            }
                        }
                        TensorType::BF16 => {
                            for chunk in bytes.chunks_exact(std::mem::size_of::<u16>()) {
                                hidden.push(bf16_word_to_f32(u16::from_le_bytes([
                                    chunk[0], chunk[1],
                                ])));
                            }
                        }
                        _ => unreachable!(),
                    }
                    Ok(hidden)
                },
            )
            .map_err(|err| err.to_string())
    }
}

struct MlxRecurrentMetalLayer {
    spec: DeltaNetRecurrentDecodeSpec,
    prefill_spec: DeltaNetRecurrentDecodeSpec,
    tensor_ids: BTreeMap<String, TensorId>,
    ctx: Context,
    runtime: MetalRuntime,
    main_buffer: MetalBuffer,
    step_execution: MlxRecurrentMetalExecution,
    prefill_execution: Option<(usize, MlxRecurrentMetalExecution)>,
    r_cache_zeroes: Vec<u8>,
    s_cache_zeroes: Vec<u8>,
}

impl MlxRecurrentMetalLayer {
    fn new(weights: &QwenMlxWeights, layer: &QwenLayerNames) -> Result<Self> {
        Self::new_with_runtime(weights, layer, MetalRuntime::new()?)
    }

    fn new_with_runtime(
        weights: &QwenMlxWeights,
        layer: &QwenLayerNames,
        runtime: MetalRuntime,
    ) -> Result<Self> {
        let QwenLayerKindNames::Recurrent {
            qkv_proj_weight,
            z_proj_weight,
            beta_proj_weight,
            alpha_proj_weight,
            a_log_name,
            dt_bias_name,
            conv1d_weight,
            norm_weight,
            out_proj_weight,
        } = &layer.kind
        else {
            return Err("recurrent metal layer requires recurrent tensor names".to_string());
        };

        let input_norm_values = weights.read_tensor_f32(&layer.input_norm_weight)?;
        let qkv_bytes = weights.read_tensor_bytes(qkv_proj_weight)?;
        let z_bytes = weights.read_tensor_bytes(z_proj_weight)?;
        let beta_bytes = weights.read_tensor_bytes(beta_proj_weight)?;
        let alpha_bytes = weights.read_tensor_bytes(alpha_proj_weight)?;
        let qkv_shape = weights.tensor(qkv_proj_weight)?.shape.clone();
        let z_shape = weights.tensor(z_proj_weight)?.shape.clone();
        let beta_shape = weights.tensor(beta_proj_weight)?.shape.clone();
        let alpha_shape = weights.tensor(alpha_proj_weight)?.shape.clone();
        if qkv_shape.len() != 2
            || z_shape.len() != 2
            || beta_shape.len() != 2
            || alpha_shape.len() != 2
        {
            return Err("recurrent merged input projection expects rank-2 weights".to_string());
        }
        let merged_inner = qkv_shape[1];
        for (name, shape) in [
            (z_proj_weight.as_str(), &z_shape),
            (beta_proj_weight.as_str(), &beta_shape),
            (alpha_proj_weight.as_str(), &alpha_shape),
        ] {
            if shape[1] != merged_inner {
                return Err(format!(
                    "recurrent merged input projection inner dim mismatch for {}: got {} expected {}",
                    name, shape[1], merged_inner
                ));
            }
        }
        let merged_rows = qkv_shape[0]
            .checked_add(z_shape[0])
            .and_then(|value| value.checked_add(beta_shape[0]))
            .and_then(|value| value.checked_add(alpha_shape[0]))
            .ok_or_else(|| "recurrent merged input projection row count overflow".to_string())?;
        let merged_input_proj_name = format!("{}.merged_input_proj", layer.input_norm_weight);
        let mut merged_input_proj_bytes = Vec::with_capacity(
            qkv_bytes.len() + z_bytes.len() + beta_bytes.len() + alpha_bytes.len(),
        );
        merged_input_proj_bytes.extend_from_slice(&qkv_bytes);
        merged_input_proj_bytes.extend_from_slice(&z_bytes);
        merged_input_proj_bytes.extend_from_slice(&beta_bytes);
        merged_input_proj_bytes.extend_from_slice(&alpha_bytes);
        let dt_bias_values = weights.read_tensor_f32(dt_bias_name)?;
        let mut a_values = weights.read_tensor_f32(a_log_name)?;
        for value in &mut a_values {
            *value = -value.exp();
        }
        let conv_values = weights.read_tensor_f32(conv1d_weight)?;
        let norm_values = weights.read_tensor_f32(norm_weight)?;
        let out_proj_bytes = weights.read_tensor_bytes(out_proj_weight)?;

        let mut mem_size = 32usize << 20;
        for len in [
            input_norm_values.len() * std::mem::size_of::<f32>(),
            qkv_bytes.len(),
            z_bytes.len(),
            beta_bytes.len(),
            alpha_bytes.len(),
            merged_input_proj_bytes.len(),
            dt_bias_values.len() * std::mem::size_of::<f32>(),
            a_values.len() * std::mem::size_of::<f32>(),
            conv_values.len() * std::mem::size_of::<f32>(),
            norm_values.len() * std::mem::size_of::<f32>(),
            out_proj_bytes.len(),
        ] {
            mem_size = mem_size
                .checked_add(len)
                .ok_or_else(|| "recurrent metal context size overflow".to_string())?;
        }

        let mut ctx = Context::new(InitParams {
            mem_size,
            mem_buffer: None,
            no_alloc: false,
        });
        let mut tensor_ids = BTreeMap::<String, TensorId>::new();

        load_f32_tensor_1d(
            &mut ctx,
            &mut tensor_ids,
            &layer.input_norm_weight,
            &input_norm_values,
            BufferUsage::Weights,
        )?;
        load_bf16_tensor_rank2_from_mlx(
            &mut ctx,
            &mut tensor_ids,
            weights.tensor(qkv_proj_weight)?,
            qkv_proj_weight,
            &qkv_bytes,
            BufferUsage::Weights,
        )?;
        load_bf16_tensor_rank2_from_mlx(
            &mut ctx,
            &mut tensor_ids,
            weights.tensor(z_proj_weight)?,
            z_proj_weight,
            &z_bytes,
            BufferUsage::Weights,
        )?;
        load_bf16_tensor_rank2_from_mlx(
            &mut ctx,
            &mut tensor_ids,
            weights.tensor(beta_proj_weight)?,
            beta_proj_weight,
            &beta_bytes,
            BufferUsage::Weights,
        )?;
        load_bf16_tensor_rank2_from_mlx(
            &mut ctx,
            &mut tensor_ids,
            weights.tensor(alpha_proj_weight)?,
            alpha_proj_weight,
            &alpha_bytes,
            BufferUsage::Weights,
        )?;
        let merged_tensor = ctx.new_named_tensor(
            &merged_input_proj_name,
            TensorType::BF16,
            2,
            &[
                u64_to_i64(merged_inner, "recurrent merged input dim1")?,
                u64_to_i64(merged_rows, "recurrent merged input dim0")?,
            ],
            BufferUsage::Weights,
        )?;
        ctx.write_tensor_data(merged_tensor, &merged_input_proj_bytes)
            .map_err(|err| err.to_string())?;
        tensor_ids.insert(merged_input_proj_name.clone(), merged_tensor);
        load_f32_tensor_1d(
            &mut ctx,
            &mut tensor_ids,
            dt_bias_name,
            &dt_bias_values,
            BufferUsage::Weights,
        )?;
        let a_name = format!("{a_log_name}.neg_exp");
        load_f32_tensor_1d(
            &mut ctx,
            &mut tensor_ids,
            &a_name,
            &a_values,
            BufferUsage::Weights,
        )?;
        load_f32_conv1d_kernel_from_mlx(
            &mut ctx,
            &mut tensor_ids,
            weights.tensor(conv1d_weight)?,
            conv1d_weight,
            &conv_values,
            BufferUsage::Weights,
        )?;
        load_f32_tensor_1d(
            &mut ctx,
            &mut tensor_ids,
            norm_weight,
            &norm_values,
            BufferUsage::Weights,
        )?;
        load_bf16_tensor_rank2_from_mlx(
            &mut ctx,
            &mut tensor_ids,
            weights.tensor(out_proj_weight)?,
            out_proj_weight,
            &out_proj_bytes,
            BufferUsage::Weights,
        )?;

        let cfg = &weights.config().text;
        let hidden_size = u32::try_from(cfg._hidden_size)
            .map_err(|_| "hidden_size does not fit in u32".to_string())?;
        let prefill_spec = DeltaNetRecurrentDecodeSpec {
            block: DeltaNetRecurrentBlockSpec {
                input: ProbeInputKind::Embeddings {
                    hidden_size,
                    input_type: TensorType::F32,
                },
                embedding_length: hidden_size,
                input_norm_name: layer.input_norm_weight.clone(),
                merged_input_proj_name: None,
                qkv_proj_name: qkv_proj_weight.clone(),
                qkv_proj_scale_name: None,
                z_proj_name: z_proj_weight.clone(),
                z_proj_scale_name: None,
                beta_proj_name: beta_proj_weight.clone(),
                beta_proj_scale_name: None,
                alpha_proj_name: alpha_proj_weight.clone(),
                alpha_proj_scale_name: None,
                dt_bias_name: dt_bias_name.clone(),
                a_name,
                conv_kernel_name: conv1d_weight.clone(),
                norm_name: norm_weight.clone(),
                output_proj_name: out_proj_weight.clone(),
                output_proj_scale_name: None,
                key_head_dim: u32::try_from(cfg.linear_key_head_dim)
                    .map_err(|_| "linear_key_head_dim does not fit in u32".to_string())?,
                key_head_count: u32::try_from(cfg.linear_num_key_heads)
                    .map_err(|_| "linear_num_key_heads does not fit in u32".to_string())?,
                value_head_dim: u32::try_from(cfg.linear_value_head_dim)
                    .map_err(|_| "linear_value_head_dim does not fit in u32".to_string())?,
                value_head_count: u32::try_from(cfg.linear_num_value_heads)
                    .map_err(|_| "linear_num_value_heads does not fit in u32".to_string())?,
                rms_epsilon: cfg.rms_norm_eps,
                residual: false,
            },
            cache: DeltaNetRecurrentStateSpec {
                max_sequences: 1,
                r_type: TensorType::F32,
                s_type: TensorType::F32,
            },
        };
        let mut spec = prefill_spec.clone();
        if metal_recurrent_merged_input_enabled() {
            spec.block.merged_input_proj_name = Some(merged_input_proj_name);
        }

        let main_buffer = create_context_main_buffer(&runtime, &ctx, BufferStorageMode::Shared)?;
        let step_execution =
            Self::compile_execution(&runtime, &mut ctx, &tensor_ids, &spec, 1, &main_buffer)?;

        let r_cache_zeroes = vec![
            0u8;
            step_execution
                .session
                .compiled()
                .bindings
                .get(&step_execution.decode.r_cache)
                .ok_or_else(|| "missing recurrent r_cache binding".to_string())?
                .size_bytes
        ];
        let s_cache_zeroes = vec![
            0u8;
            step_execution
                .session
                .compiled()
                .bindings
                .get(&step_execution.decode.s_cache)
                .ok_or_else(|| "missing recurrent s_cache binding".to_string())?
                .size_bytes
        ];

        let mut layer = Self {
            spec,
            prefill_spec,
            tensor_ids,
            ctx,
            runtime,
            main_buffer,
            step_execution,
            prefill_execution: None,
            r_cache_zeroes,
            s_cache_zeroes,
        };
        layer.reset()?;
        Ok(layer)
    }

    fn compile_execution(
        runtime: &MetalRuntime,
        ctx: &mut Context,
        tensor_ids: &BTreeMap<String, TensorId>,
        spec: &DeltaNetRecurrentDecodeSpec,
        n_tokens: usize,
        main_buffer: &MetalBuffer,
    ) -> Result<MlxRecurrentMetalExecution> {
        let (decode, prepared) = prepare_delta_net_recurrent_decode_graph(
            ctx,
            tensor_ids,
            spec,
            n_tokens,
            runtime.features(),
        )
        .map_err(|err| err.to_string())?;
        let session = MetalGraphSession::from_runtime_with_main_buffer(
            runtime.clone(),
            &prepared,
            main_buffer,
            BufferStorageMode::Shared,
        )?;
        Ok(MlxRecurrentMetalExecution { decode, session })
    }

    fn reset(&mut self) -> Result<()> {
        let compiled = self.step_execution.session.compiled();
        let runtime = self.step_execution.session.runtime();
        let r_binding = compiled
            .bindings
            .get(&self.step_execution.decode.r_cache)
            .ok_or_else(|| "missing recurrent r_cache binding".to_string())?;
        runtime.write_buffer(
            &compiled.main_buffer,
            r_binding.offset_bytes,
            &self.r_cache_zeroes,
        )?;
        let s_binding = compiled
            .bindings
            .get(&self.step_execution.decode.s_cache)
            .ok_or_else(|| "missing recurrent s_cache binding".to_string())?;
        runtime.write_buffer(
            &compiled.main_buffer,
            s_binding.offset_bytes,
            &self.s_cache_zeroes,
        )?;
        if let Some((_, execution)) = &self.prefill_execution {
            let compiled = execution.session.compiled();
            let runtime = execution.session.runtime();
            let r_binding = compiled
                .bindings
                .get(&execution.decode.r_cache)
                .ok_or_else(|| "missing recurrent prefill r_cache binding".to_string())?;
            let r_zeroes = vec![0u8; r_binding.size_bytes];
            runtime.write_buffer(&compiled.main_buffer, r_binding.offset_bytes, &r_zeroes)?;
            let s_binding = compiled
                .bindings
                .get(&execution.decode.s_cache)
                .ok_or_else(|| "missing recurrent prefill s_cache binding".to_string())?;
            let s_zeroes = vec![0u8; s_binding.size_bytes];
            runtime.write_buffer(&compiled.main_buffer, s_binding.offset_bytes, &s_zeroes)?;
        }
        Ok(())
    }

    fn eval(&mut self, input_words: &[u16]) -> Result<Vec<f32>> {
        let spec = &self.spec;
        let ctx = &mut self.ctx;
        self.step_execution.eval(ctx, spec, input_words, 1)
    }

    fn eval_rows(&mut self, input_words: &[u16], n_tokens: usize) -> Result<Vec<f32>> {
        if n_tokens == 0 {
            return Ok(Vec::new());
        }
        if n_tokens == 1 {
            return self.eval(input_words);
        }
        let hidden_size = self.spec.block.embedding_length as usize;
        if input_words.len()
            != hidden_size
                .checked_mul(n_tokens)
                .ok_or_else(|| "recurrent metal input size overflow".to_string())?
        {
            return Err(format!(
                "recurrent metal input length mismatch: got {} expected {}",
                input_words.len(),
                hidden_size * n_tokens
            ));
        }
        let needs_compile = self
            .prefill_execution
            .as_ref()
            .map(|(compiled_tokens, _)| *compiled_tokens != n_tokens)
            .unwrap_or(true);
        if needs_compile {
            let execution = Self::compile_execution(
                &self.runtime,
                &mut self.ctx,
                &self.tensor_ids,
                &self.prefill_spec,
                n_tokens,
                &self.main_buffer,
            )?;
            self.prefill_execution = Some((n_tokens, execution));
        }
        let spec = &self.prefill_spec;
        let ctx = &mut self.ctx;
        let (_, execution) = self
            .prefill_execution
            .as_mut()
            .ok_or_else(|| "missing recurrent metal prefill execution".to_string())?;
        execution.eval(ctx, spec, input_words, n_tokens)
    }

    fn eval_with_decode_tail(
        &mut self,
        weights: &QwenMlxWeights,
        tail: &mut MlxQwenMetalDecodeTailBackend,
        spec: &MlxQwenDecodeTailSpec,
        input_words: &[u16],
    ) -> Result<Vec<u16>> {
        let input_primary = match self.spec.block.input {
            ProbeInputKind::Embeddings { input_type, .. } => match input_type {
                TensorType::BF16 => bf16_words_as_bytes(input_words),
                TensorType::F32 => {
                    let input_storage = input_words
                        .iter()
                        .copied()
                        .map(bf16_word_to_f32)
                        .collect::<Vec<_>>();
                    execute_compiled_graph(
                        self.step_execution.session.runtime(),
                        &self.ctx,
                        self.step_execution.session.compiled(),
                        &[
                            MetalGraphTensorWrite {
                                tensor_id: self.step_execution.decode.input_primary,
                                bytes: f32s_as_bytes(&input_storage),
                            },
                            MetalGraphTensorWrite {
                                tensor_id: self.step_execution.decode.input_state_rows,
                                bytes: i32s_as_bytes(&[0]),
                            },
                        ],
                        &[],
                    )
                    .map_err(|err| err.to_string())?;
                    let output = self
                        .ctx
                        .tensor(self.step_execution.decode.result_output)
                        .ok_or_else(|| {
                            "delta-net recurrent result tensor is invalid".to_string()
                        })?;
                    let binding = self
                        .step_execution
                        .session
                        .compiled()
                        .bindings
                        .get(&self.step_execution.decode.result_output)
                        .ok_or_else(|| "missing recurrent result_output binding".to_string())?;
                    return tail.run_from_graph_output(
                        weights,
                        spec,
                        input_words,
                        &self.step_execution.session.compiled().main_buffer,
                        binding.offset_bytes,
                        output.desc.ty,
                    );
                }
                other => {
                    return Err(format!(
                        "unsupported recurrent metal embedding input type {}",
                        other.name()
                    ))
                }
            },
            ProbeInputKind::TokenIds { .. } => {
                return Err("recurrent metal layer expects embedding input".to_string())
            }
        };
        execute_compiled_graph(
            self.step_execution.session.runtime(),
            &self.ctx,
            self.step_execution.session.compiled(),
            &[
                MetalGraphTensorWrite {
                    tensor_id: self.step_execution.decode.input_primary,
                    bytes: input_primary,
                },
                MetalGraphTensorWrite {
                    tensor_id: self.step_execution.decode.input_state_rows,
                    bytes: i32s_as_bytes(&[0]),
                },
            ],
            &[],
        )
        .map_err(|err| err.to_string())?;
        let output = self
            .ctx
            .tensor(self.step_execution.decode.result_output)
            .ok_or_else(|| "delta-net recurrent result tensor is invalid".to_string())?;
        let binding = self
            .step_execution
            .session
            .compiled()
            .bindings
            .get(&self.step_execution.decode.result_output)
            .ok_or_else(|| "missing recurrent result_output binding".to_string())?;
        tail.run_from_graph_output(
            weights,
            spec,
            input_words,
            &self.step_execution.session.compiled().main_buffer,
            binding.offset_bytes,
            output.desc.ty,
        )
    }

    fn eval_with_decode_tail_from_buffer(
        &mut self,
        weights: &QwenMlxWeights,
        tail: &mut MlxQwenMetalDecodeTailBackend,
        spec: &MlxQwenDecodeTailSpec,
        input_hidden: &MetalBuffer,
        output_hidden: &MetalBuffer,
    ) -> Result<()> {
        let compiled = self.step_execution.session.compiled();
        let input_binding = compiled
            .bindings
            .get(&self.step_execution.decode.input_primary)
            .ok_or_else(|| "missing recurrent input_primary binding".to_string())?;
        let main_buffer = compiled.main_buffer.clone();
        match self.spec.block.input {
            ProbeInputKind::Embeddings { input_type, .. } => match input_type {
                TensorType::BF16 => {
                    let expected_input_bytes = spec
                        .hidden_size
                        .checked_mul(std::mem::size_of::<u16>())
                        .ok_or_else(|| "recurrent input byte count overflow".to_string())?;
                    if input_binding.size_bytes != expected_input_bytes {
                        return Err(format!(
                            "recurrent input binding byte mismatch: got {} expected {}",
                            input_binding.size_bytes, expected_input_bytes
                        ));
                    }
                    execute_compiled_graph_with_buffer_inputs(
                        self.step_execution.session.runtime(),
                        &self.ctx,
                        self.step_execution.session.compiled(),
                        &[MetalGraphTensorWrite {
                            tensor_id: self.step_execution.decode.input_state_rows,
                            bytes: i32s_as_bytes(&[0]),
                        }],
                        &[MetalGraphTensorBufferCopy {
                            tensor_id: self.step_execution.decode.input_primary,
                            source_buffer: input_hidden,
                            source_offset_bytes: 0,
                        }],
                        &[],
                    )
                    .map_err(|err| err.to_string())?;
                }
                TensorType::F32 => {
                    let expected_input_bytes = spec
                        .hidden_size
                        .checked_mul(std::mem::size_of::<f32>())
                        .ok_or_else(|| "recurrent input byte count overflow".to_string())?;
                    if input_binding.size_bytes != expected_input_bytes {
                        return Err(format!(
                            "recurrent input binding byte mismatch: got {} expected {}",
                            input_binding.size_bytes, expected_input_bytes
                        ));
                    }
                    let output = self
                        .ctx
                        .tensor(self.step_execution.decode.result_output)
                        .ok_or_else(|| {
                            "delta-net recurrent result tensor is invalid".to_string()
                        })?;
                    let binding = self
                        .step_execution
                        .session
                        .compiled()
                        .bindings
                        .get(&self.step_execution.decode.result_output)
                        .ok_or_else(|| "missing recurrent result_output binding".to_string())?;
                    let output_ty = output.desc.ty;
                    let output_offset_bytes = binding.offset_bytes;
                    if !metal_decode_chain_active_fusion_enabled() {
                        tail.run_bf16_hidden_to_f32_graph_input(
                            input_hidden,
                            &main_buffer,
                            input_binding.offset_bytes,
                            spec.hidden_size,
                        )?;
                        execute_compiled_graph(
                            self.step_execution.session.runtime(),
                            &self.ctx,
                            self.step_execution.session.compiled(),
                            &[MetalGraphTensorWrite {
                                tensor_id: self.step_execution.decode.input_state_rows,
                                bytes: i32s_as_bytes(&[0]),
                            }],
                            &[],
                        )
                        .map_err(|err| err.to_string())?;
                        tail.run_from_graph_output_into_buffer(
                            weights,
                            spec,
                            input_hidden,
                            output_hidden,
                            &main_buffer,
                            output_offset_bytes,
                            output_ty,
                        )?;
                        return Ok(());
                    }
                    tail.runtime.begin_command_batch().map_err(|err| {
                        format!("begin recurrent decode chain batch failed: {err}")
                    })?;
                    let dispatch_result = (|| -> Result<()> {
                        tail.dispatch_bf16_hidden_to_f32_graph_input_with_tracking(
                            input_hidden,
                            &main_buffer,
                            input_binding.offset_bytes,
                            spec.hidden_size,
                            false,
                        )?;
                        tail.runtime
                            .memory_barrier_buffers()
                            .map_err(|err| err.to_string())?;
                        execute_compiled_graph_in_active_batch(
                            self.step_execution.session.runtime(),
                            &self.ctx,
                            self.step_execution.session.compiled(),
                            &[MetalGraphTensorWrite {
                                tensor_id: self.step_execution.decode.input_state_rows,
                                bytes: i32s_as_bytes(&[0]),
                            }],
                            &[],
                        )
                        .map_err(|err| err.to_string())?;
                        tail.runtime
                            .memory_barrier_buffers()
                            .map_err(|err| err.to_string())?;
                        tail.dispatch_run_from_graph_output_into_buffer(
                            weights,
                            spec,
                            input_hidden,
                            output_hidden,
                            &main_buffer,
                            output_offset_bytes,
                            output_ty,
                            false,
                        )
                    })();
                    if let Err(err) = dispatch_result {
                        let _ = tail.runtime.discard_command_batch();
                        return Err(err);
                    }
                    tail.runtime
                        .end_command_batch()
                        .map_err(|err| format!("end recurrent decode chain batch failed: {err}"))?;
                    return Ok(());
                }
                other => {
                    return Err(format!(
                        "unsupported recurrent metal embedding input type {}",
                        other.name()
                    ))
                }
            },
            ProbeInputKind::TokenIds { .. } => {
                return Err("recurrent metal layer expects embedding input".to_string())
            }
        }
        let output = self
            .ctx
            .tensor(self.step_execution.decode.result_output)
            .ok_or_else(|| "delta-net recurrent result tensor is invalid".to_string())?;
        let binding = self
            .step_execution
            .session
            .compiled()
            .bindings
            .get(&self.step_execution.decode.result_output)
            .ok_or_else(|| "missing recurrent result_output binding".to_string())?;
        tail.run_from_graph_output_into_buffer(
            weights,
            spec,
            input_hidden,
            output_hidden,
            &main_buffer,
            binding.offset_bytes,
            output.desc.ty,
        )
    }
}

#[derive(Clone, Debug)]
struct MlxQwenDecodeTailSpec {
    post_attention_norm_weight: String,
    gate_proj_weight: String,
    up_proj_weight: String,
    gate_up_qmv: MlxAffineQmvLayout,
    gate_rows: usize,
    down_weight: String,
    down_scales: String,
    down_biases: String,
    down_qmv: MlxAffineQmvLayout,
    hidden_size: usize,
    eps: f32,
}

impl MlxQwenDecodeTailSpec {
    fn maybe_new(weights: &QwenMlxWeights, layer: &QwenLayerNames) -> Result<Option<Arc<Self>>> {
        if !metal_decode_tail_enabled() {
            return Ok(None);
        }

        let gate_proj_weight = format!("{}.weight", layer.ffn_gate_base);
        let up_proj_weight = format!("{}.weight", layer.ffn_up_base);
        let gate_up_refs = vec![gate_proj_weight.as_str(), up_proj_weight.as_str()];
        let concat = match maybe_affine_concat_qmv_layout(weights, &gate_up_refs)? {
            Some(value) => value,
            None => return Ok(None),
        };
        if concat.1.len() != 2 || concat.1[0] != concat.1[1] || concat.1[0] == 0 {
            return Ok(None);
        }

        let down_weight = format!("{}.weight", layer.ffn_down_base);
        let down_scales = format!("{}.scales", layer.ffn_down_base);
        let down_biases = format!("{}.biases", layer.ffn_down_base);
        let down_qmv = match maybe_affine_qmv_layout(weights, &down_weight)? {
            Some(layout) => layout,
            None => return Ok(None),
        };

        if down_qmv.out_rows != weights.config().text._hidden_size {
            return Ok(None);
        }
        if weights
            .read_bf16_tensor_words_cached(&layer.post_attention_norm_weight)
            .is_err()
        {
            return Ok(None);
        }

        Ok(Some(Arc::new(Self {
            post_attention_norm_weight: layer.post_attention_norm_weight.clone(),
            gate_proj_weight,
            up_proj_weight,
            gate_up_qmv: concat.0,
            gate_rows: concat.1[0],
            down_weight,
            down_scales,
            down_biases,
            down_qmv,
            hidden_size: weights.config().text._hidden_size,
            eps: weights.config().text.rms_norm_eps,
        })))
    }
}

struct MlxQwenMetalDecodeTailBackend {
    runtime: MetalRuntime,
    tensor_buffers: HashMap<String, MetalBuffer>,
    qmv_pipelines: HashMap<u32, MetalPipeline>,
    cpy_f32_bf16: MetalPipeline,
    cpy_bf16_f32: MetalPipeline,
    argmax_bf16: MetalPipeline,
    rms: MetalPipeline,
    add: MetalPipeline,
    swiglu: MetalPipeline,
    chain_hidden_a: Option<MetalBuffer>,
    chain_hidden_a_capacity_words: usize,
    chain_hidden_b: Option<MetalBuffer>,
    chain_hidden_b_capacity_words: usize,
    input_hidden: Option<MetalBuffer>,
    input_hidden_capacity_words: usize,
    graph_output_bf16: Option<MetalBuffer>,
    graph_output_capacity_words: usize,
    residual_out: Option<MetalBuffer>,
    residual_capacity_words: usize,
    post_attention_norm_out: Option<MetalBuffer>,
    post_attention_norm_capacity_words: usize,
    gate_up_out: Option<MetalBuffer>,
    gate_up_capacity_words: usize,
    swiglu_out: Option<MetalBuffer>,
    swiglu_capacity_words: usize,
    down_out: Option<MetalBuffer>,
    down_capacity_words: usize,
    layer_out: Option<MetalBuffer>,
    layer_out_capacity_words: usize,
    final_norm_out: Option<MetalBuffer>,
    final_norm_capacity_words: usize,
    lm_head_out: Option<MetalBuffer>,
    lm_head_capacity_words: usize,
    argmax_values: Option<MetalBuffer>,
    argmax_indices: Option<MetalBuffer>,
    argmax_capacity: usize,
}

impl MlxQwenMetalDecodeTailBackend {
    fn new(runtime: MetalRuntime) -> Result<Self> {
        Ok(Self {
            cpy_f32_bf16: compile_default_pipeline(&runtime, "kernel_mlx_cpy_f32_bf16_row")?,
            cpy_bf16_f32: compile_default_pipeline(&runtime, "kernel_mlx_cpy_bf16_f32_row")?,
            argmax_bf16: compile_default_pipeline(&runtime, "kernel_mlx_argmax_bf16_partial")?,
            rms: compile_default_pipeline(&runtime, "kernel_mlx_rms_norm_row_bf16")?,
            add: compile_default_pipeline(&runtime, "kernel_mlx_add_row_bf16")?,
            swiglu: compile_default_pipeline(&runtime, "kernel_mlx_swiglu_row_bf16")?,
            runtime,
            tensor_buffers: HashMap::new(),
            qmv_pipelines: HashMap::new(),
            chain_hidden_a: None,
            chain_hidden_a_capacity_words: 0,
            chain_hidden_b: None,
            chain_hidden_b_capacity_words: 0,
            input_hidden: None,
            input_hidden_capacity_words: 0,
            graph_output_bf16: None,
            graph_output_capacity_words: 0,
            residual_out: None,
            residual_capacity_words: 0,
            post_attention_norm_out: None,
            post_attention_norm_capacity_words: 0,
            gate_up_out: None,
            gate_up_capacity_words: 0,
            swiglu_out: None,
            swiglu_capacity_words: 0,
            down_out: None,
            down_capacity_words: 0,
            layer_out: None,
            layer_out_capacity_words: 0,
            final_norm_out: None,
            final_norm_capacity_words: 0,
            lm_head_out: None,
            lm_head_capacity_words: 0,
            argmax_values: None,
            argmax_indices: None,
            argmax_capacity: 0,
        })
    }

    fn cached_tensor_buffer<F>(&mut self, key: &str, load_bytes: F) -> Result<MetalBuffer>
    where
        F: FnOnce() -> Result<Vec<u8>>,
    {
        if let Some(buffer) = self.tensor_buffers.get(key) {
            return Ok(buffer.clone());
        }
        let bytes = load_bytes()?;
        let buffer = self
            .runtime
            .create_buffer_with_bytes(&bytes, BufferStorageMode::Private)
            .map_err(|err| format!("upload decode tail tensor buffer failed: {err}"))?;
        self.tensor_buffers.insert(key.to_owned(), buffer.clone());
        Ok(buffer)
    }

    fn ensure_words_buffer(
        runtime: &MetalRuntime,
        slot: &mut Option<MetalBuffer>,
        capacity_words: &mut usize,
        len_words: usize,
        storage: BufferStorageMode,
    ) -> Result<MetalBuffer> {
        if *capacity_words < len_words || slot.is_none() {
            *slot = Some(
                runtime
                    .create_buffer(len_words * std::mem::size_of::<u16>(), storage)
                    .map_err(|err| format!("create decode tail buffer failed: {err}"))?,
            );
            *capacity_words = len_words;
        }
        slot.clone()
            .ok_or_else(|| "missing decode tail scratch buffer".to_string())
    }

    fn ensure_byte_buffer(
        runtime: &MetalRuntime,
        slot: &mut Option<MetalBuffer>,
        capacity_bytes: &mut usize,
        len_bytes: usize,
        storage: BufferStorageMode,
    ) -> Result<MetalBuffer> {
        if *capacity_bytes < len_bytes || slot.is_none() {
            *slot = Some(
                runtime
                    .create_buffer(len_bytes, storage)
                    .map_err(|err| format!("create decode tail byte buffer failed: {err}"))?,
            );
            *capacity_bytes = len_bytes;
        }
        slot.clone()
            .ok_or_else(|| "missing decode tail byte scratch buffer".to_string())
    }

    fn ensure_chain_hidden_buffers(
        &mut self,
        len_words: usize,
    ) -> Result<(MetalBuffer, MetalBuffer)> {
        let a = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.chain_hidden_a,
            &mut self.chain_hidden_a_capacity_words,
            len_words,
            BufferStorageMode::Shared,
        )?;
        let b = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.chain_hidden_b,
            &mut self.chain_hidden_b_capacity_words,
            len_words,
            BufferStorageMode::Shared,
        )?;
        Ok((a, b))
    }

    fn write_chain_hidden_input(&mut self, words: &[u16]) -> Result<MetalBuffer> {
        let (hidden_a, _) = self.ensure_chain_hidden_buffers(words.len())?;
        self.runtime
            .write_buffer(&hidden_a, 0, bf16_words_as_bytes(words))
            .map_err(|err| format!("write decode chain hidden input failed: {err}"))?;
        Ok(hidden_a)
    }

    fn read_hidden_buffer(&self, buffer: &MetalBuffer, len_words: usize) -> Result<Vec<u16>> {
        self.runtime
            .with_readable_buffer_range(
                buffer,
                0,
                len_words
                    .checked_mul(std::mem::size_of::<u16>())
                    .ok_or_else(|| "decode hidden read size overflow".to_string())?,
                |bytes| {
                    let expected = len_words
                        .checked_mul(std::mem::size_of::<u16>())
                        .ok_or_else(|| "decode hidden read size overflow".to_string())?;
                    if bytes.len() != expected {
                        return Err(format!(
                            "decode hidden byte length mismatch: got {} expected {}",
                            bytes.len(),
                            expected
                        ));
                    }
                    let mut words = Vec::with_capacity(len_words);
                    for chunk in bytes.chunks_exact(std::mem::size_of::<u16>()) {
                        words.push(u16::from_le_bytes([chunk[0], chunk[1]]));
                    }
                    Ok(words)
                },
            )
            .map_err(|err| err.to_string())
    }

    fn dispatch_bf16_hidden_to_f32_graph_input(
        &mut self,
        source_buffer: &MetalBuffer,
        graph_main_buffer: &MetalBuffer,
        graph_input_offset_bytes: usize,
        hidden_size: usize,
    ) -> Result<()> {
        self.dispatch_bf16_hidden_to_f32_graph_input_with_tracking(
            source_buffer,
            graph_main_buffer,
            graph_input_offset_bytes,
            hidden_size,
            true,
        )
    }

    fn dispatch_bf16_hidden_to_f32_graph_input_with_tracking(
        &mut self,
        source_buffer: &MetalBuffer,
        graph_main_buffer: &MetalBuffer,
        graph_input_offset_bytes: usize,
        hidden_size: usize,
        tracked: bool,
    ) -> Result<()> {
        let row_args = MlxAddRowArgs {
            n: u32::try_from(hidden_size)
                .map_err(|_| "decode chain hidden size does not fit in u32".to_string())?,
        };
        let row_threads = MetalSize {
            width: 256,
            height: 1,
            depth: 1,
        };
        let row_threadgroups = MetalSize {
            width: (hidden_size as u64).div_ceil(row_threads.width),
            height: 1,
            depth: 1,
        };
        let bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: source_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: graph_main_buffer,
                offset_bytes: graph_input_offset_bytes,
            },
        ];
        if tracked {
            dispatch_compute_tracked_split(
                &self.runtime,
                &self.cpy_bf16_f32,
                bytes_of(&row_args),
                bindings,
                1,
                &[],
                row_threadgroups,
                row_threads,
            )
        } else {
            dispatch_compute_untracked(
                &self.runtime,
                &self.cpy_bf16_f32,
                bytes_of(&row_args),
                bindings,
                &[],
                row_threadgroups,
                row_threads,
            )
        }
    }

    fn run_bf16_hidden_to_f32_graph_input(
        &mut self,
        source_buffer: &MetalBuffer,
        graph_main_buffer: &MetalBuffer,
        graph_input_offset_bytes: usize,
        hidden_size: usize,
    ) -> Result<()> {
        self.runtime
            .begin_command_batch()
            .map_err(|err| format!("begin bf16-to-f32 graph input batch failed: {err}"))?;
        let dispatch_result = self.dispatch_bf16_hidden_to_f32_graph_input(
            source_buffer,
            graph_main_buffer,
            graph_input_offset_bytes,
            hidden_size,
        );
        if let Err(err) = dispatch_result {
            let _ = self.runtime.discard_command_batch();
            return Err(err);
        }
        self.runtime
            .end_command_batch()
            .map_err(|err| format!("end bf16-to-f32 graph input batch failed: {err}"))?;
        Ok(())
    }

    fn qmv_pipeline(&mut self, bits: u32) -> Result<MetalPipeline> {
        if let Some(pipeline) = self.qmv_pipelines.get(&bits) {
            return Ok(pipeline.clone());
        }
        let name = affine_qmv_pipeline_name(bits)?;
        let pipeline = compile_default_pipeline(&self.runtime, name)?;
        self.qmv_pipelines.insert(bits, pipeline.clone());
        Ok(pipeline)
    }

    fn post_attention_norm_buffer(
        &mut self,
        weights: &QwenMlxWeights,
        spec: &MlxQwenDecodeTailSpec,
    ) -> Result<MetalBuffer> {
        let key = format!("qwen_tail:norm:{}", spec.post_attention_norm_weight);
        self.cached_tensor_buffer(&key, || {
            let words = weights.read_bf16_tensor_words_cached(&spec.post_attention_norm_weight)?;
            Ok(bf16_words_as_bytes(words.as_slice()).to_vec())
        })
    }

    fn gate_up_buffers(
        &mut self,
        weights: &QwenMlxWeights,
        spec: &MlxQwenDecodeTailSpec,
    ) -> Result<(MetalBuffer, MetalBuffer, MetalBuffer)> {
        let gate_up_refs = vec![spec.gate_proj_weight.as_str(), spec.up_proj_weight.as_str()];
        let concat = weights.concat_affine_tensor_cached(&gate_up_refs)?;
        let cache_key = format!(
            "qwen_tail:concat:{}|{}",
            spec.gate_proj_weight, spec.up_proj_weight
        );
        let weight = self.cached_tensor_buffer(&format!("{cache_key}:weight"), || {
            Ok(concat.weight_bytes.as_ref().clone())
        })?;
        let scales = self.cached_tensor_buffer(&format!("{cache_key}:scales"), || {
            Ok(concat.scales_bytes.as_ref().clone())
        })?;
        let biases = self.cached_tensor_buffer(&format!("{cache_key}:biases"), || {
            Ok(concat.biases_bytes.as_ref().clone())
        })?;
        Ok((weight, scales, biases))
    }

    fn down_buffers(
        &mut self,
        weights: &QwenMlxWeights,
        spec: &MlxQwenDecodeTailSpec,
    ) -> Result<(MetalBuffer, MetalBuffer, MetalBuffer)> {
        let cache_key = format!("qwen_tail:{}", spec.down_weight);
        let weight = self.cached_tensor_buffer(&format!("{cache_key}:weight"), || {
            weights.read_tensor_bytes(&spec.down_weight)
        })?;
        let scales = self.cached_tensor_buffer(&format!("{cache_key}:scales"), || {
            weights.read_tensor_bytes(&spec.down_scales)
        })?;
        let biases = self.cached_tensor_buffer(&format!("{cache_key}:biases"), || {
            weights.read_tensor_bytes(&spec.down_biases)
        })?;
        Ok((weight, scales, biases))
    }

    fn lm_head_buffers(
        &mut self,
        weights: &QwenMlxWeights,
    ) -> Result<(MetalBuffer, MetalBuffer, MetalBuffer)> {
        let weight_name = format!("{LM_HEAD_BASE}.weight");
        let scales_name = format!("{LM_HEAD_BASE}.scales");
        let biases_name = format!("{LM_HEAD_BASE}.biases");
        let cache_key = format!("qwen_lm_head:{weight_name}");
        let weight = self.cached_tensor_buffer(&format!("{cache_key}:weight"), || {
            weights.read_tensor_bytes(&weight_name)
        })?;
        let scales = self.cached_tensor_buffer(&format!("{cache_key}:scales"), || {
            weights.read_tensor_bytes(&scales_name)
        })?;
        let biases = self.cached_tensor_buffer(&format!("{cache_key}:biases"), || {
            weights.read_tensor_bytes(&biases_name)
        })?;
        Ok((weight, scales, biases))
    }

    fn dispatch_qmv(
        &mut self,
        layout: MlxAffineQmvLayout,
        input_buffer: &MetalBuffer,
        input_offset_bytes: usize,
        weight_buffer: &MetalBuffer,
        scales_buffer: &MetalBuffer,
        biases_buffer: &MetalBuffer,
        output_buffer: &MetalBuffer,
        output_offset_bytes: usize,
        n_in: usize,
    ) -> Result<()> {
        self.dispatch_qmv_with_tracking(
            layout,
            input_buffer,
            input_offset_bytes,
            weight_buffer,
            scales_buffer,
            biases_buffer,
            output_buffer,
            output_offset_bytes,
            n_in,
            true,
        )
    }

    fn dispatch_qmv_with_tracking(
        &mut self,
        layout: MlxAffineQmvLayout,
        input_buffer: &MetalBuffer,
        input_offset_bytes: usize,
        weight_buffer: &MetalBuffer,
        scales_buffer: &MetalBuffer,
        biases_buffer: &MetalBuffer,
        output_buffer: &MetalBuffer,
        output_offset_bytes: usize,
        n_in: usize,
        tracked: bool,
    ) -> Result<()> {
        let pipeline = self.qmv_pipeline(layout.bits)?;
        let args = layout.row_args(n_in)?;
        let bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: input_buffer,
                offset_bytes: input_offset_bytes,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: weight_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: scales_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: biases_buffer,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: output_buffer,
                offset_bytes: output_offset_bytes,
            },
        ];
        let threadgroups = MetalSize {
            width: 1,
            height: (layout.out_rows as u64).div_ceil(8),
            depth: 1,
        };
        let threads = MetalSize {
            width: 32,
            height: 2,
            depth: 1,
        };
        if tracked {
            dispatch_compute_tracked_split(
                &self.runtime,
                &pipeline,
                bytes_of(&args),
                bindings,
                4,
                &[],
                threadgroups,
                threads,
            )
        } else {
            dispatch_compute_untracked(
                &self.runtime,
                &pipeline,
                bytes_of(&args),
                bindings,
                &[],
                threadgroups,
                threads,
            )
        }
    }

    fn dispatch_run_from_graph_output_into_buffer(
        &mut self,
        weights: &QwenMlxWeights,
        spec: &MlxQwenDecodeTailSpec,
        input_hidden: &MetalBuffer,
        output_hidden: &MetalBuffer,
        graph_main_buffer: &MetalBuffer,
        graph_output_offset_bytes: usize,
        graph_output_ty: TensorType,
        tracked: bool,
    ) -> Result<()> {
        if graph_output_ty != TensorType::F32 {
            return Err(format!(
                "decode tail expects F32 graph output, got {}",
                graph_output_ty.name()
            ));
        }

        let graph_output_bf16 = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.graph_output_bf16,
            &mut self.graph_output_capacity_words,
            spec.hidden_size,
            BufferStorageMode::Private,
        )?;
        let residual_out = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.residual_out,
            &mut self.residual_capacity_words,
            spec.hidden_size,
            BufferStorageMode::Private,
        )?;
        let post_attention_norm_out = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.post_attention_norm_out,
            &mut self.post_attention_norm_capacity_words,
            spec.hidden_size,
            BufferStorageMode::Private,
        )?;
        let gate_up_out = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.gate_up_out,
            &mut self.gate_up_capacity_words,
            spec.gate_up_qmv.out_rows,
            BufferStorageMode::Private,
        )?;
        let swiglu_out = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.swiglu_out,
            &mut self.swiglu_capacity_words,
            spec.gate_rows,
            BufferStorageMode::Private,
        )?;
        let down_out = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.down_out,
            &mut self.down_capacity_words,
            spec.hidden_size,
            BufferStorageMode::Private,
        )?;

        let post_attention_norm_weight = self.post_attention_norm_buffer(weights, spec)?;
        let (gate_up_weight, gate_up_scales, gate_up_biases) =
            self.gate_up_buffers(weights, spec)?;
        let (down_weight, down_scales, down_biases) = self.down_buffers(weights, spec)?;

        macro_rules! dispatch_tail {
            ($pipeline:expr, $args:expr, $bindings:expr, $output_start:expr, $tgm:expr, $tgs:expr, $threads:expr $(,)?) => {{
                if tracked {
                    dispatch_compute_tracked_split(
                        &self.runtime,
                        $pipeline,
                        $args,
                        $bindings,
                        $output_start,
                        $tgm,
                        $tgs,
                        $threads,
                    )
                } else {
                    dispatch_compute_untracked(
                        &self.runtime,
                        $pipeline,
                        $args,
                        $bindings,
                        $tgm,
                        $tgs,
                        $threads,
                    )?;
                    self.runtime
                        .memory_barrier_buffers()
                        .map_err(|err| err.to_string())?;
                    Ok(())
                }
            }};
        }

        let dispatch_result = (|| -> Result<()> {
            let row_args = MlxAddRowArgs {
                n: u32::try_from(spec.hidden_size)
                    .map_err(|_| "decode tail hidden size does not fit in u32".to_string())?,
            };
            let row_threads = MetalSize {
                width: 256,
                height: 1,
                depth: 1,
            };
            let row_threadgroups = MetalSize {
                width: (spec.hidden_size as u64).div_ceil(row_threads.width),
                height: 1,
                depth: 1,
            };
            dispatch_tail!(
                &self.cpy_f32_bf16,
                bytes_of(&row_args),
                [
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: graph_main_buffer,
                        offset_bytes: graph_output_offset_bytes,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &graph_output_bf16,
                        offset_bytes: 0,
                    },
                ],
                1,
                &[],
                row_threadgroups,
                row_threads,
            )?;
            dispatch_tail!(
                &self.add,
                bytes_of(&row_args),
                [
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: input_hidden,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &graph_output_bf16,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: &residual_out,
                        offset_bytes: 0,
                    },
                ],
                2,
                &[],
                row_threadgroups,
                row_threads,
            )?;

            let rms_threads = mlx_norm_threads_per_threadgroup(
                spec.hidden_size,
                self.rms.max_threads_per_threadgroup,
            )?;
            let rms_args = MlxRmsNormRowArgs {
                n: u32::try_from(spec.hidden_size)
                    .map_err(|_| "decode tail hidden size does not fit in u32".to_string())?,
                eps: spec.eps,
                threadgroup_width: rms_threads.width as u32,
            };
            dispatch_tail!(
                &self.rms,
                bytes_of(&rms_args),
                [
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: &residual_out,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &post_attention_norm_weight,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: &post_attention_norm_out,
                        offset_bytes: 0,
                    },
                ],
                2,
                &[],
                MetalSize {
                    width: 1,
                    height: 1,
                    depth: 1,
                },
                rms_threads,
            )?;

            self.dispatch_qmv_with_tracking(
                spec.gate_up_qmv,
                &post_attention_norm_out,
                0,
                &gate_up_weight,
                &gate_up_scales,
                &gate_up_biases,
                &gate_up_out,
                0,
                spec.hidden_size,
                tracked,
            )?;
            if !tracked {
                self.runtime
                    .memory_barrier_buffers()
                    .map_err(|err| err.to_string())?;
            }

            let gate_bytes = spec
                .gate_rows
                .checked_mul(std::mem::size_of::<u16>())
                .ok_or_else(|| "decode tail gate split offset overflow".to_string())?;
            let swiglu_args = MlxGegluRowArgs {
                n: u32::try_from(spec.gate_rows)
                    .map_err(|_| "decode tail gate row count does not fit in u32".to_string())?,
            };
            let swiglu_threads = MetalSize {
                width: 256,
                height: 1,
                depth: 1,
            };
            let swiglu_threadgroups = MetalSize {
                width: (spec.gate_rows as u64).div_ceil(swiglu_threads.width),
                height: 1,
                depth: 1,
            };
            dispatch_tail!(
                &self.swiglu,
                bytes_of(&swiglu_args),
                [
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: &gate_up_out,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &gate_up_out,
                        offset_bytes: gate_bytes,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: &swiglu_out,
                        offset_bytes: 0,
                    },
                ],
                2,
                &[],
                swiglu_threadgroups,
                swiglu_threads,
            )?;

            self.dispatch_qmv_with_tracking(
                spec.down_qmv,
                &swiglu_out,
                0,
                &down_weight,
                &down_scales,
                &down_biases,
                &down_out,
                0,
                spec.gate_rows,
                tracked,
            )?;
            if !tracked {
                self.runtime
                    .memory_barrier_buffers()
                    .map_err(|err| err.to_string())?;
            }

            dispatch_tail!(
                &self.add,
                bytes_of(&row_args),
                [
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: &residual_out,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &down_out,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: output_hidden,
                        offset_bytes: 0,
                    },
                ],
                2,
                &[],
                row_threadgroups,
                row_threads,
            )?;
            Ok(())
        })();

        if let Err(err) = dispatch_result {
            return Err(err);
        }
        Ok(())
    }

    fn run_from_graph_output_into_buffer(
        &mut self,
        weights: &QwenMlxWeights,
        spec: &MlxQwenDecodeTailSpec,
        input_hidden: &MetalBuffer,
        output_hidden: &MetalBuffer,
        graph_main_buffer: &MetalBuffer,
        graph_output_offset_bytes: usize,
        graph_output_ty: TensorType,
    ) -> Result<()> {
        self.runtime
            .begin_command_batch()
            .map_err(|err| format!("begin decode tail command batch failed: {err}"))?;
        let dispatch_result = self.dispatch_run_from_graph_output_into_buffer(
            weights,
            spec,
            input_hidden,
            output_hidden,
            graph_main_buffer,
            graph_output_offset_bytes,
            graph_output_ty,
            true,
        );
        if let Err(err) = dispatch_result {
            let _ = self.runtime.discard_command_batch();
            return Err(err);
        }
        self.runtime
            .end_command_batch()
            .map_err(|err| format!("end decode tail command batch failed: {err}"))?;
        Ok(())
    }

    fn run_from_graph_output(
        &mut self,
        weights: &QwenMlxWeights,
        spec: &MlxQwenDecodeTailSpec,
        input_words: &[u16],
        graph_main_buffer: &MetalBuffer,
        graph_output_offset_bytes: usize,
        graph_output_ty: TensorType,
    ) -> Result<Vec<u16>> {
        if input_words.len() != spec.hidden_size {
            return Err(format!(
                "decode tail input length mismatch: got {} expected {}",
                input_words.len(),
                spec.hidden_size
            ));
        }
        let input_hidden = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.input_hidden,
            &mut self.input_hidden_capacity_words,
            spec.hidden_size,
            BufferStorageMode::Shared,
        )?;
        let layer_out = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.layer_out,
            &mut self.layer_out_capacity_words,
            spec.hidden_size,
            BufferStorageMode::Shared,
        )?;
        self.runtime
            .write_buffer(&input_hidden, 0, bf16_words_as_bytes(input_words))
            .map_err(|err| format!("write decode tail input buffer failed: {err}"))?;
        self.run_from_graph_output_into_buffer(
            weights,
            spec,
            &input_hidden,
            &layer_out,
            graph_main_buffer,
            graph_output_offset_bytes,
            graph_output_ty,
        )?;
        self.read_hidden_buffer(&layer_out, spec.hidden_size)
    }

    fn top1_from_hidden_words(
        &mut self,
        weights: &QwenMlxWeights,
        hidden_words: &[u16],
    ) -> Result<u32> {
        let hidden_size = weights.config().text._hidden_size;
        if hidden_words.len() != hidden_size {
            return Err(format!(
                "top1 hidden length mismatch: got {} expected {}",
                hidden_words.len(),
                hidden_size
            ));
        }
        let lm_head_weight = format!("{LM_HEAD_BASE}.weight");
        let lm_layout = maybe_affine_qmv_layout(weights, &lm_head_weight)?
            .ok_or_else(|| "LM head is not supported by Metal top1 path".to_string())?;
        let input_hidden = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.input_hidden,
            &mut self.input_hidden_capacity_words,
            hidden_size,
            BufferStorageMode::Shared,
        )?;
        let final_norm_out = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.final_norm_out,
            &mut self.final_norm_capacity_words,
            hidden_size,
            BufferStorageMode::Private,
        )?;
        let lm_head_out = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.lm_head_out,
            &mut self.lm_head_capacity_words,
            lm_layout.out_rows,
            BufferStorageMode::Private,
        )?;
        let partial_count = lm_layout.out_rows.div_ceil(256);
        let partial_values = Self::ensure_byte_buffer(
            &self.runtime,
            &mut self.argmax_values,
            &mut self.argmax_capacity,
            partial_count * std::mem::size_of::<f32>(),
            BufferStorageMode::Shared,
        )?;
        let mut partial_index_capacity = self.argmax_capacity;
        let partial_indices = Self::ensure_byte_buffer(
            &self.runtime,
            &mut self.argmax_indices,
            &mut partial_index_capacity,
            partial_count * std::mem::size_of::<u32>(),
            BufferStorageMode::Shared,
        )?;
        self.argmax_capacity = self.argmax_capacity.max(partial_index_capacity);

        let norm_weight = self.cached_tensor_buffer("qwen_lm_head:final_norm", || {
            let words = weights.read_bf16_tensor_words_cached(OUTPUT_NORM_WEIGHT)?;
            Ok(bf16_words_as_bytes(words.as_slice()).to_vec())
        })?;
        let (lm_weight, lm_scales, lm_biases) = self.lm_head_buffers(weights)?;

        self.runtime
            .write_buffer(&input_hidden, 0, bf16_words_as_bytes(hidden_words))
            .map_err(|err| format!("write top1 hidden buffer failed: {err}"))?;

        self.runtime
            .begin_command_batch()
            .map_err(|err| format!("begin top1 command batch failed: {err}"))?;
        let dispatch_result = (|| -> Result<()> {
            let rms_threads = mlx_norm_threads_per_threadgroup(
                hidden_size,
                self.rms.max_threads_per_threadgroup,
            )?;
            let rms_args = MlxRmsNormRowArgs {
                n: u32::try_from(hidden_size)
                    .map_err(|_| "top1 hidden size does not fit in u32".to_string())?,
                eps: weights.config().text.rms_norm_eps,
                threadgroup_width: rms_threads.width as u32,
            };
            dispatch_compute_tracked_split(
                &self.runtime,
                &self.rms,
                bytes_of(&rms_args),
                [
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: &input_hidden,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &norm_weight,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: &final_norm_out,
                        offset_bytes: 0,
                    },
                ],
                2,
                &[],
                MetalSize {
                    width: 1,
                    height: 1,
                    depth: 1,
                },
                rms_threads,
            )?;
            self.dispatch_qmv(
                lm_layout,
                &final_norm_out,
                0,
                &lm_weight,
                &lm_scales,
                &lm_biases,
                &lm_head_out,
                0,
                hidden_size,
            )?;
            let argmax_args = MlxAddRowArgs {
                n: u32::try_from(lm_layout.out_rows)
                    .map_err(|_| "LM head rows do not fit in u32".to_string())?,
            };
            dispatch_compute_tracked_split(
                &self.runtime,
                &self.argmax_bf16,
                bytes_of(&argmax_args),
                [
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: &lm_head_out,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &partial_values,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: &partial_indices,
                        offset_bytes: 0,
                    },
                ],
                1,
                &[],
                MetalSize {
                    width: partial_count as u64,
                    height: 1,
                    depth: 1,
                },
                MetalSize {
                    width: 256,
                    height: 1,
                    depth: 1,
                },
            )?;
            Ok(())
        })();
        if let Err(err) = dispatch_result {
            let _ = self.runtime.discard_command_batch();
            return Err(err);
        }
        self.runtime
            .end_command_batch()
            .map_err(|err| format!("end top1 command batch failed: {err}"))?;

        let values = self
            .runtime
            .with_readable_buffer_range(
                &partial_values,
                0,
                partial_count * std::mem::size_of::<f32>(),
                |bytes| {
                    let mut values = Vec::with_capacity(partial_count);
                    for chunk in bytes.chunks_exact(std::mem::size_of::<f32>()) {
                        values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                    }
                    Ok(values)
                },
            )
            .map_err(|err| err.to_string())?;
        let indices = self
            .runtime
            .with_readable_buffer_range(
                &partial_indices,
                0,
                partial_count * std::mem::size_of::<u32>(),
                |bytes| {
                    let mut indices = Vec::with_capacity(partial_count);
                    for chunk in bytes.chunks_exact(std::mem::size_of::<u32>()) {
                        indices.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                    }
                    Ok(indices)
                },
            )
            .map_err(|err| err.to_string())?;
        let mut best_token = 0u32;
        let mut best_logit = f32::NEG_INFINITY;
        for (&value, &token_id) in values.iter().zip(indices.iter()) {
            if token_id == u32::MAX {
                continue;
            }
            if value > best_logit || (value == best_logit && token_id < best_token) {
                best_logit = value;
                best_token = token_id;
            }
        }
        Ok(best_token)
    }

    fn top1_from_hidden_buffer(
        &mut self,
        weights: &QwenMlxWeights,
        input_hidden: &MetalBuffer,
    ) -> Result<u32> {
        let hidden_size = weights.config().text._hidden_size;
        let lm_head_weight = format!("{LM_HEAD_BASE}.weight");
        let lm_layout = maybe_affine_qmv_layout(weights, &lm_head_weight)?
            .ok_or_else(|| "LM head is not supported by Metal top1 path".to_string())?;
        let final_norm_out = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.final_norm_out,
            &mut self.final_norm_capacity_words,
            hidden_size,
            BufferStorageMode::Private,
        )?;
        let lm_head_out = Self::ensure_words_buffer(
            &self.runtime,
            &mut self.lm_head_out,
            &mut self.lm_head_capacity_words,
            lm_layout.out_rows,
            BufferStorageMode::Private,
        )?;
        let partial_count = lm_layout.out_rows.div_ceil(256);
        let partial_values = Self::ensure_byte_buffer(
            &self.runtime,
            &mut self.argmax_values,
            &mut self.argmax_capacity,
            partial_count * std::mem::size_of::<f32>(),
            BufferStorageMode::Shared,
        )?;
        let mut partial_index_capacity = self.argmax_capacity;
        let partial_indices = Self::ensure_byte_buffer(
            &self.runtime,
            &mut self.argmax_indices,
            &mut partial_index_capacity,
            partial_count * std::mem::size_of::<u32>(),
            BufferStorageMode::Shared,
        )?;
        self.argmax_capacity = self.argmax_capacity.max(partial_index_capacity);

        let norm_weight = self.cached_tensor_buffer("qwen_lm_head:final_norm", || {
            let words = weights.read_bf16_tensor_words_cached(OUTPUT_NORM_WEIGHT)?;
            Ok(bf16_words_as_bytes(words.as_slice()).to_vec())
        })?;
        let (lm_weight, lm_scales, lm_biases) = self.lm_head_buffers(weights)?;

        self.runtime
            .begin_command_batch()
            .map_err(|err| format!("begin top1 command batch failed: {err}"))?;
        let dispatch_result = (|| -> Result<()> {
            let rms_threads = mlx_norm_threads_per_threadgroup(
                hidden_size,
                self.rms.max_threads_per_threadgroup,
            )?;
            let rms_args = MlxRmsNormRowArgs {
                n: u32::try_from(hidden_size)
                    .map_err(|_| "top1 hidden size does not fit in u32".to_string())?,
                eps: weights.config().text.rms_norm_eps,
                threadgroup_width: rms_threads.width as u32,
            };
            dispatch_compute_tracked_split(
                &self.runtime,
                &self.rms,
                bytes_of(&rms_args),
                [
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: input_hidden,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &norm_weight,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: &final_norm_out,
                        offset_bytes: 0,
                    },
                ],
                2,
                &[],
                MetalSize {
                    width: 1,
                    height: 1,
                    depth: 1,
                },
                rms_threads,
            )?;
            self.dispatch_qmv(
                lm_layout,
                &final_norm_out,
                0,
                &lm_weight,
                &lm_scales,
                &lm_biases,
                &lm_head_out,
                0,
                hidden_size,
            )?;
            let argmax_args = MlxAddRowArgs {
                n: u32::try_from(lm_layout.out_rows)
                    .map_err(|_| "LM head rows do not fit in u32".to_string())?,
            };
            dispatch_compute_tracked_split(
                &self.runtime,
                &self.argmax_bf16,
                bytes_of(&argmax_args),
                [
                    MetalBufferBindingRef {
                        index: 1,
                        buffer: &lm_head_out,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 2,
                        buffer: &partial_values,
                        offset_bytes: 0,
                    },
                    MetalBufferBindingRef {
                        index: 3,
                        buffer: &partial_indices,
                        offset_bytes: 0,
                    },
                ],
                1,
                &[],
                MetalSize {
                    width: partial_count as u64,
                    height: 1,
                    depth: 1,
                },
                MetalSize {
                    width: 256,
                    height: 1,
                    depth: 1,
                },
            )?;
            Ok(())
        })();
        if let Err(err) = dispatch_result {
            let _ = self.runtime.discard_command_batch();
            return Err(err);
        }
        self.runtime
            .end_command_batch()
            .map_err(|err| format!("end top1 command batch failed: {err}"))?;

        let values = self
            .runtime
            .with_readable_buffer_range(
                &partial_values,
                0,
                partial_count * std::mem::size_of::<f32>(),
                |bytes| {
                    let mut values = Vec::with_capacity(partial_count);
                    for chunk in bytes.chunks_exact(std::mem::size_of::<f32>()) {
                        values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                    }
                    Ok(values)
                },
            )
            .map_err(|err| err.to_string())?;
        let indices = self
            .runtime
            .with_readable_buffer_range(
                &partial_indices,
                0,
                partial_count * std::mem::size_of::<u32>(),
                |bytes| {
                    let mut indices = Vec::with_capacity(partial_count);
                    for chunk in bytes.chunks_exact(std::mem::size_of::<u32>()) {
                        indices.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                    }
                    Ok(indices)
                },
            )
            .map_err(|err| err.to_string())?;
        let mut best_token = 0u32;
        let mut best_logit = f32::NEG_INFINITY;
        for (&value, &token_id) in values.iter().zip(indices.iter()) {
            if token_id == u32::MAX {
                continue;
            }
            if value > best_logit || (value == best_logit && token_id < best_token) {
                best_logit = value;
                best_token = token_id;
            }
        }
        Ok(best_token)
    }
}

enum LayerState {
    Attention(AttentionCache),
    AttentionHybrid {
        cpu: AttentionCache,
        metal: MlxAttentionMetalLayer,
        metal_ready: bool,
    },
    RecurrentCpu(CpuRecurrentState),
    RecurrentMetal(MlxRecurrentMetalLayer),
}

struct QwenRuntime {
    weights: QwenMlxWeights,
    layers: Vec<QwenLayerNames>,
    states: Vec<LayerState>,
    decode_tail_specs: Vec<Option<Arc<MlxQwenDecodeTailSpec>>>,
    metal_decode_tail: Option<MlxQwenMetalDecodeTailBackend>,
}

impl QwenRuntime {
    fn load(model_path: impl AsRef<Path>, max_context: usize) -> Result<Self> {
        let weights = QwenMlxWeights::load(model_path)?;
        if weights.config().model_type != "qwen3_5" {
            return Err(format!(
                "unexpected model_type {}, expected qwen3_5",
                weights.config().model_type
            ));
        }
        if weights.config().quantization_mode != "affine" {
            return Err(format!(
                "unsupported quantization mode {}",
                weights.config().quantization_mode
            ));
        }
        let shared_metal_runtime = MetalRuntime::new().ok();
        let mut metal_decode_tail = None;
        if let Some(runtime) = shared_metal_runtime.clone() {
            if metal_decode_tail_enabled() {
                match MlxQwenMetalDecodeTailBackend::new(runtime) {
                    Ok(backend) => metal_decode_tail = Some(backend),
                    Err(err) => {
                        eprintln!("[qwen_text_bench] decode tail Metal fallback: {}", err);
                    }
                }
            }
        }
        let mut layers = Vec::with_capacity(weights.config().text.num_hidden_layers);
        let mut states = Vec::with_capacity(weights.config().text.num_hidden_layers);
        let mut decode_tail_specs = Vec::with_capacity(weights.config().text.num_hidden_layers);
        for layer_idx in 0..weights.config().text.num_hidden_layers {
            let names = QwenLayerNames::for_layer(weights.config(), layer_idx)?;
            let decode_tail_spec = MlxQwenDecodeTailSpec::maybe_new(&weights, &names)?;
            let state = match &names.kind {
                QwenLayerKindNames::Attention { .. } => {
                    if ENABLE_EXPERIMENTAL_ATTENTION_METAL {
                        let Some(runtime) = shared_metal_runtime.clone() else {
                            decode_tail_specs.push(decode_tail_spec);
                            layers.push(names);
                            states.push(LayerState::Attention(AttentionCache::new(
                                weights.config().text.num_key_value_heads,
                                weights.config().text.head_dim,
                            )));
                            continue;
                        };
                        match MlxAttentionMetalLayer::new_with_runtime(
                            &weights,
                            layer_idx,
                            &names,
                            max_context,
                            runtime,
                        ) {
                            Ok(state) => LayerState::AttentionHybrid {
                                cpu: AttentionCache::new(
                                    weights.config().text.num_key_value_heads,
                                    weights.config().text.head_dim,
                                ),
                                metal: state,
                                metal_ready: false,
                            },
                            Err(err) => {
                                eprintln!(
                                    "[qwen_text_bench] attention Metal fallback for layer {}: {}",
                                    layer_idx, err
                                );
                                LayerState::Attention(AttentionCache::new(
                                    weights.config().text.num_key_value_heads,
                                    weights.config().text.head_dim,
                                ))
                            }
                        }
                    } else {
                        LayerState::Attention(AttentionCache::new(
                            weights.config().text.num_key_value_heads,
                            weights.config().text.head_dim,
                        ))
                    }
                }
                QwenLayerKindNames::Recurrent { .. } => {
                    if let Some(runtime) = shared_metal_runtime.clone() {
                        match MlxRecurrentMetalLayer::new_with_runtime(&weights, &names, runtime) {
                            Ok(state) => LayerState::RecurrentMetal(state),
                            Err(err) => {
                                eprintln!(
                                    "[qwen_text_bench] recurrent Metal fallback for layer {}: {}",
                                    layer_idx, err
                                );
                                LayerState::RecurrentCpu(CpuRecurrentState::new(weights.config()))
                            }
                        }
                    } else {
                        LayerState::RecurrentCpu(CpuRecurrentState::new(weights.config()))
                    }
                }
            };
            layers.push(names);
            states.push(state);
            decode_tail_specs.push(decode_tail_spec);
        }
        Ok(Self {
            weights,
            layers,
            states,
            decode_tail_specs,
            metal_decode_tail,
        })
    }

    fn reset(&mut self) -> Result<()> {
        for state in &mut self.states {
            match state {
                LayerState::Attention(cache) => {
                    cache.reset();
                }
                LayerState::AttentionHybrid {
                    cpu,
                    metal,
                    metal_ready,
                } => {
                    cpu.reset();
                    metal.reset()?;
                    *metal_ready = false;
                }
                LayerState::RecurrentCpu(state) => state.reset(),
                LayerState::RecurrentMetal(state) => state.reset()?,
            }
        }
        Ok(())
    }

    fn metal_runtime(&self) -> Option<MetalRuntime> {
        if let Some(tail) = &self.metal_decode_tail {
            return Some(tail.runtime.clone());
        }
        for state in &self.states {
            match state {
                LayerState::AttentionHybrid { metal, .. } => return Some(metal.runtime.clone()),
                LayerState::RecurrentMetal(metal) => return Some(metal.runtime.clone()),
                LayerState::Attention(_) | LayerState::RecurrentCpu(_) => {}
            }
        }
        None
    }

    fn eval_token_hidden(&mut self, token_id: u32, position: usize) -> Result<Vec<u16>> {
        if let Some(hidden_words) = self.eval_token_hidden_metal_chained(token_id, position)? {
            return Ok(hidden_words);
        }
        let mut hidden_words = self.weights.embed_token_words(token_id)?;
        for layer_idx in 0..self.layers.len() {
            hidden_words = self.eval_layer(layer_idx, &hidden_words, position)?;
        }
        Ok(hidden_words)
    }

    fn eval_token_hidden_metal_chained_buffer(
        &mut self,
        token_id: u32,
        position: usize,
    ) -> Result<Option<(MetalBuffer, usize)>> {
        if !metal_decode_chain_enabled() || self.metal_decode_tail.is_none() {
            if qwen_debug_chain_enabled() {
                eprintln!("[qwen_text_bench] decode chain disabled or tail backend missing");
            }
            return Ok(None);
        }
        for (layer_idx, state) in self.states.iter().enumerate() {
            if self
                .decode_tail_specs
                .get(layer_idx)
                .and_then(Option::as_ref)
                .is_none()
            {
                if qwen_debug_chain_enabled() {
                    eprintln!(
                        "[qwen_text_bench] decode chain missing tail spec at layer {}",
                        layer_idx
                    );
                }
                return Ok(None);
            }
            match state {
                LayerState::AttentionHybrid { .. } | LayerState::RecurrentMetal(_) => {}
                LayerState::Attention(_) | LayerState::RecurrentCpu(_) => {
                    if qwen_debug_chain_enabled() {
                        eprintln!(
                            "[qwen_text_bench] decode chain unsupported state at layer {}",
                            layer_idx
                        );
                    }
                    return Ok(None);
                }
            }
        }

        let weights = self.weights.clone();
        let hidden_size = weights.config().text._hidden_size;
        let initial_hidden = weights.embed_token_words(token_id)?;
        if initial_hidden.len() != hidden_size {
            return Err(format!(
                "embedding length mismatch: got {} expected {}",
                initial_hidden.len(),
                hidden_size
            ));
        }

        let specs = self.decode_tail_specs.clone();
        let tail = self
            .metal_decode_tail
            .as_mut()
            .ok_or_else(|| "decode chain tail backend disappeared".to_string())?;
        let mut current_hidden = tail.write_chain_hidden_input(&initial_hidden)?;
        let (_, mut next_hidden) = tail.ensure_chain_hidden_buffers(hidden_size)?;
        let states = &mut self.states;

        for layer_idx in 0..states.len() {
            let spec = specs[layer_idx]
                .as_deref()
                .ok_or_else(|| format!("missing decode tail spec for layer {}", layer_idx))?;
            if spec.hidden_size != hidden_size {
                return Err(format!(
                    "decode tail hidden size mismatch at layer {}: got {} expected {}",
                    layer_idx, spec.hidden_size, hidden_size
                ));
            }
            match &mut states[layer_idx] {
                LayerState::AttentionHybrid {
                    cpu,
                    metal,
                    metal_ready,
                } => {
                    if !*metal_ready {
                        if cpu.seq_len != position {
                            if qwen_debug_chain_enabled() {
                                eprintln!(
                                    "[qwen_text_bench] decode chain attention cache mismatch at layer {}: cpu_seq_len={} position={}",
                                    layer_idx, cpu.seq_len, position
                                );
                            }
                            return Ok(None);
                        }
                        let sync_started = Instant::now();
                        metal.sync_from_cpu_cache(cpu)?;
                        if qwen_debug_chain_enabled() {
                            eprintln!(
                                "[qwen_text_bench] decode chain synced attention cache at layer {} in {:.6}s",
                                layer_idx,
                                sync_started.elapsed().as_secs_f64()
                            );
                        }
                        *metal_ready = true;
                    }
                    metal.eval_with_decode_tail_from_buffer(
                        &weights,
                        tail,
                        spec,
                        &current_hidden,
                        &next_hidden,
                        position,
                    )?;
                }
                LayerState::RecurrentMetal(state) => {
                    state.eval_with_decode_tail_from_buffer(
                        &weights,
                        tail,
                        spec,
                        &current_hidden,
                        &next_hidden,
                    )?;
                }
                LayerState::Attention(_) | LayerState::RecurrentCpu(_) => return Ok(None),
            }
            std::mem::swap(&mut current_hidden, &mut next_hidden);
        }

        Ok(Some((current_hidden, hidden_size)))
    }

    fn eval_token_hidden_metal_chained(
        &mut self,
        token_id: u32,
        position: usize,
    ) -> Result<Option<Vec<u16>>> {
        let Some((hidden_buffer, hidden_size)) =
            self.eval_token_hidden_metal_chained_buffer(token_id, position)?
        else {
            return Ok(None);
        };
        let tail = self
            .metal_decode_tail
            .as_ref()
            .ok_or_else(|| "decode chain tail backend disappeared".to_string())?;
        tail.read_hidden_buffer(&hidden_buffer, hidden_size)
            .map(Some)
    }

    fn eval_token_top1_metal_chained(
        &mut self,
        token_id: u32,
        position: usize,
    ) -> Result<Option<u32>> {
        let Some((hidden_buffer, _hidden_size)) =
            self.eval_token_hidden_metal_chained_buffer(token_id, position)?
        else {
            return Ok(None);
        };
        let weights = self.weights.clone();
        let tail = self
            .metal_decode_tail
            .as_mut()
            .ok_or_else(|| "decode chain tail backend disappeared".to_string())?;
        tail.top1_from_hidden_buffer(&weights, &hidden_buffer)
            .map(Some)
    }

    fn eval_prompt_hidden(&mut self, prompt_token_ids: &[u32]) -> Result<Vec<u16>> {
        if prompt_token_ids.is_empty() {
            return Err("prompt must contain at least one token".to_string());
        }
        let hidden_size = self.weights.config().text._hidden_size;
        let mut hidden_words = Vec::with_capacity(
            prompt_token_ids
                .len()
                .checked_mul(hidden_size)
                .ok_or_else(|| "prompt embedding size overflow".to_string())?,
        );
        for &token_id in prompt_token_ids {
            hidden_words.extend(self.weights.embed_token_words(token_id)?);
        }
        for layer_idx in 0..self.layers.len() {
            hidden_words =
                self.eval_layer_rows(layer_idx, &hidden_words, 0, prompt_token_ids.len())?;
        }
        if hidden_words.len() < hidden_size {
            return Err("prefill produced no hidden rows".to_string());
        }
        Ok(hidden_words[hidden_words.len() - hidden_size..].to_vec())
    }

    fn eval_layer(
        &mut self,
        layer_idx: usize,
        input_words: &[u16],
        position: usize,
    ) -> Result<Vec<u16>> {
        let weights = &self.weights;
        let layer = &self.layers[layer_idx];
        let decode_tail_spec = self.decode_tail_specs[layer_idx].as_deref();
        let tail_backend = &mut self.metal_decode_tail;
        let attn_out = match (&layer.kind, &mut self.states[layer_idx]) {
            (
                QwenLayerKindNames::Attention {
                    q_proj_base,
                    q_norm_weight,
                    k_proj_base,
                    k_norm_weight,
                    v_proj_base,
                    o_proj_base,
                },
                LayerState::Attention(cache),
            ) => {
                let input_norm = self
                    .weights
                    .header_for_tensor(&layer.input_norm_weight)?
                    .rms_norm_weighted_f32(
                        input_words,
                        &layer.input_norm_weight,
                        weights.config().text.rms_norm_eps,
                    )
                    .map_err(|err| err.to_string())?;
                eval_attention_layer(
                    weights,
                    cache,
                    &f32s_to_bf16_words(&input_norm),
                    position,
                    q_proj_base,
                    q_norm_weight,
                    k_proj_base,
                    k_norm_weight,
                    v_proj_base,
                    o_proj_base,
                )?
            }
            (
                QwenLayerKindNames::Attention { .. },
                LayerState::AttentionHybrid {
                    cpu,
                    metal,
                    metal_ready,
                },
            ) => {
                if !*metal_ready && cpu.seq_len == position {
                    metal.sync_from_cpu_cache(cpu)?;
                    *metal_ready = true;
                }
                if *metal_ready {
                    if let (Some(tail), Some(spec)) = (tail_backend.as_mut(), decode_tail_spec) {
                        return metal.eval_with_decode_tail(
                            weights,
                            tail,
                            spec,
                            input_words,
                            position,
                        );
                    }
                    metal.eval(input_words, position)?
                } else {
                    let QwenLayerKindNames::Attention {
                        q_proj_base,
                        q_norm_weight,
                        k_proj_base,
                        k_norm_weight,
                        v_proj_base,
                        o_proj_base,
                    } = &layer.kind
                    else {
                        return Err(format!("layer state mismatch at layer {}", layer_idx));
                    };
                    let input_norm = self
                        .weights
                        .header_for_tensor(&layer.input_norm_weight)?
                        .rms_norm_weighted_f32(
                            input_words,
                            &layer.input_norm_weight,
                            weights.config().text.rms_norm_eps,
                        )
                        .map_err(|err| err.to_string())?;
                    eval_attention_layer(
                        weights,
                        cpu,
                        &f32s_to_bf16_words(&input_norm),
                        position,
                        q_proj_base,
                        q_norm_weight,
                        k_proj_base,
                        k_norm_weight,
                        v_proj_base,
                        o_proj_base,
                    )?
                }
            }
            (
                QwenLayerKindNames::Recurrent {
                    qkv_proj_weight,
                    z_proj_weight,
                    beta_proj_weight,
                    alpha_proj_weight,
                    a_log_name,
                    dt_bias_name,
                    conv1d_weight,
                    norm_weight,
                    out_proj_weight,
                },
                LayerState::RecurrentCpu(state),
            ) => {
                let input_norm = self
                    .weights
                    .header_for_tensor(&layer.input_norm_weight)?
                    .rms_norm_weighted_f32(
                        input_words,
                        &layer.input_norm_weight,
                        weights.config().text.rms_norm_eps,
                    )
                    .map_err(|err| err.to_string())?;
                eval_recurrent_layer(
                    weights,
                    state,
                    &f32s_to_bf16_words(&input_norm),
                    qkv_proj_weight,
                    z_proj_weight,
                    beta_proj_weight,
                    alpha_proj_weight,
                    a_log_name,
                    dt_bias_name,
                    conv1d_weight,
                    norm_weight,
                    out_proj_weight,
                )?
            }
            (QwenLayerKindNames::Recurrent { .. }, LayerState::RecurrentMetal(state)) => {
                if let (Some(tail), Some(spec)) = (tail_backend.as_mut(), decode_tail_spec) {
                    return state.eval_with_decode_tail(weights, tail, spec, input_words);
                }
                state.eval(input_words)?
            }
            _ => return Err(format!("layer state mismatch at layer {}", layer_idx)),
        };

        if attn_out.len() != input_words.len() {
            return Err(format!(
                "layer {} attention/recurrent output length mismatch: got {} expected {}",
                layer_idx,
                attn_out.len(),
                input_words.len()
            ));
        }
        let residual = add_bf16_and_f32(input_words, &attn_out)?;
        let residual_words = f32s_to_bf16_words(&residual);
        let post_attention_norm = self
            .weights
            .header_for_tensor(&layer.post_attention_norm_weight)?
            .rms_norm_weighted_f32(
                &residual_words,
                &layer.post_attention_norm_weight,
                self.weights.config().text.rms_norm_eps,
            )
            .map_err(|err| err.to_string())?;
        let post_attention_norm_words = f32s_to_bf16_words(&post_attention_norm);
        let (ffn_gate, ffn_up) = if let Some(mut outputs) = maybe_project_rank2_bases_affine_concat(
            &self.weights,
            &post_attention_norm_words,
            &[&layer.ffn_gate_base, &layer.ffn_up_base],
        )? {
            if outputs.len() != 2 {
                return Err(format!(
                    "layer {} fused FFN projection count mismatch: got {} expected 2",
                    layer_idx,
                    outputs.len()
                ));
            }
            (outputs.remove(0), outputs.remove(0))
        } else {
            (
                project_rank2_base(
                    &self.weights,
                    &post_attention_norm_words,
                    &layer.ffn_gate_base,
                )?,
                project_rank2_base(
                    &self.weights,
                    &post_attention_norm_words,
                    &layer.ffn_up_base,
                )?,
            )
        };
        if ffn_gate.len() != ffn_up.len() {
            return Err(format!(
                "layer {} FFN gate/up mismatch: {} vs {}",
                layer_idx,
                ffn_gate.len(),
                ffn_up.len()
            ));
        }
        let mut ffn_activated = Vec::with_capacity(ffn_gate.len());
        for (&gate, &up) in ffn_gate.iter().zip(ffn_up.iter()) {
            ffn_activated.push(silu(gate) * up);
        }
        let ffn_down = project_rank2_base(
            &self.weights,
            &f32s_to_bf16_words(&ffn_activated),
            &layer.ffn_down_base,
        )?;
        let layer_out = add_f32(&residual, &ffn_down)?;
        Ok(f32s_to_bf16_words(&layer_out))
    }

    fn eval_layer_rows(
        &mut self,
        layer_idx: usize,
        input_words: &[u16],
        start_position: usize,
        input_rows: usize,
    ) -> Result<Vec<u16>> {
        if input_rows == 0 {
            return Ok(Vec::new());
        }
        let hidden_size = self.weights.config().text._hidden_size;
        if input_words.len()
            != hidden_size
                .checked_mul(input_rows)
                .ok_or_else(|| format!("layer {} row input size overflow", layer_idx))?
        {
            return Err(format!(
                "layer {} row input length mismatch: got {} expected {}",
                layer_idx,
                input_words.len(),
                hidden_size * input_rows
            ));
        }

        let layer = &self.layers[layer_idx];
        let attn_out = match (&layer.kind, &mut self.states[layer_idx]) {
            (
                QwenLayerKindNames::Attention {
                    q_proj_base,
                    q_norm_weight,
                    k_proj_base,
                    k_norm_weight,
                    v_proj_base,
                    o_proj_base,
                },
                LayerState::Attention(cache),
            ) => {
                let input_norm = rms_norm_rows_weighted_f32(
                    &bf16_words_to_f32s(input_words),
                    input_rows,
                    hidden_size,
                    &self
                        .weights
                        .read_bf16_tensor_words_cached(&layer.input_norm_weight)?,
                    self.weights.config().text.rms_norm_eps,
                )?;
                eval_attention_layer_rows(
                    &self.weights,
                    cache,
                    &f32s_to_bf16_words(&input_norm),
                    start_position,
                    input_rows,
                    q_proj_base,
                    q_norm_weight,
                    k_proj_base,
                    k_norm_weight,
                    v_proj_base,
                    o_proj_base,
                )?
            }
            (
                QwenLayerKindNames::Attention { .. },
                LayerState::AttentionHybrid {
                    cpu,
                    metal,
                    metal_ready,
                },
            ) => {
                let QwenLayerKindNames::Attention {
                    q_proj_base,
                    q_norm_weight,
                    k_proj_base,
                    k_norm_weight,
                    v_proj_base,
                    o_proj_base,
                } = &layer.kind
                else {
                    return Err(format!("layer state mismatch at layer {}", layer_idx));
                };
                let input_norm = rms_norm_rows_weighted_f32(
                    &bf16_words_to_f32s(input_words),
                    input_rows,
                    hidden_size,
                    &self
                        .weights
                        .read_bf16_tensor_words_cached(&layer.input_norm_weight)?,
                    self.weights.config().text.rms_norm_eps,
                )?;
                let output = eval_attention_layer_rows(
                    &self.weights,
                    cpu,
                    &f32s_to_bf16_words(&input_norm),
                    start_position,
                    input_rows,
                    q_proj_base,
                    q_norm_weight,
                    k_proj_base,
                    k_norm_weight,
                    v_proj_base,
                    o_proj_base,
                )?;
                if metal_attention_prefill_cache_enabled() {
                    metal.eval_rows(input_words, start_position, input_rows)?;
                    *metal_ready = true;
                } else {
                    *metal_ready = false;
                }
                output
            }
            (
                QwenLayerKindNames::Recurrent {
                    qkv_proj_weight,
                    z_proj_weight,
                    beta_proj_weight,
                    alpha_proj_weight,
                    a_log_name,
                    dt_bias_name,
                    conv1d_weight,
                    norm_weight,
                    out_proj_weight,
                },
                LayerState::RecurrentCpu(state),
            ) => {
                let mut outputs = Vec::with_capacity(input_words.len());
                for row_idx in 0..input_rows {
                    let input_start = row_idx * hidden_size;
                    let input_end = input_start + hidden_size;
                    let input_row = &input_words[input_start..input_end];
                    let input_norm = self
                        .weights
                        .header_for_tensor(&layer.input_norm_weight)?
                        .rms_norm_weighted_f32(
                            input_row,
                            &layer.input_norm_weight,
                            self.weights.config().text.rms_norm_eps,
                        )
                        .map_err(|err| err.to_string())?;
                    outputs.extend(eval_recurrent_layer(
                        &self.weights,
                        state,
                        &f32s_to_bf16_words(&input_norm),
                        qkv_proj_weight,
                        z_proj_weight,
                        beta_proj_weight,
                        alpha_proj_weight,
                        a_log_name,
                        dt_bias_name,
                        conv1d_weight,
                        norm_weight,
                        out_proj_weight,
                    )?);
                }
                outputs
            }
            (QwenLayerKindNames::Recurrent { .. }, LayerState::RecurrentMetal(state)) => {
                state.eval_rows(input_words, input_rows)?
            }
            _ => return Err(format!("layer state mismatch at layer {}", layer_idx)),
        };

        if attn_out.len() != input_words.len() {
            return Err(format!(
                "layer {} row output length mismatch: got {} expected {}",
                layer_idx,
                attn_out.len(),
                input_words.len()
            ));
        }
        let residual = add_bf16_and_f32(input_words, &attn_out)?;
        let post_attention_norm = rms_norm_rows_weighted_f32(
            &residual,
            input_rows,
            hidden_size,
            &self
                .weights
                .read_bf16_tensor_words_cached(&layer.post_attention_norm_weight)?,
            self.weights.config().text.rms_norm_eps,
        )?;
        let post_attention_norm_words = f32s_to_bf16_words(&post_attention_norm);
        let (ffn_gate, ffn_up) = if let Some(mut outputs) =
            maybe_project_rank2_bases_affine_concat_rows(
                &self.weights,
                &post_attention_norm_words,
                input_rows,
                &[&layer.ffn_gate_base, &layer.ffn_up_base],
            )? {
            if outputs.len() != 2 {
                return Err(format!(
                    "layer {} fused FFN row projection count mismatch: got {} expected 2",
                    layer_idx,
                    outputs.len()
                ));
            }
            (outputs.remove(0), outputs.remove(0))
        } else {
            (
                project_rank2_base_rows(
                    &self.weights,
                    &post_attention_norm_words,
                    input_rows,
                    &layer.ffn_gate_base,
                )?,
                project_rank2_base_rows(
                    &self.weights,
                    &post_attention_norm_words,
                    input_rows,
                    &layer.ffn_up_base,
                )?,
            )
        };
        if ffn_gate.len() != ffn_up.len() {
            return Err(format!(
                "layer {} FFN row gate/up mismatch: {} vs {}",
                layer_idx,
                ffn_gate.len(),
                ffn_up.len()
            ));
        }
        let mut ffn_activated = Vec::with_capacity(ffn_gate.len());
        for (&gate, &up) in ffn_gate.iter().zip(ffn_up.iter()) {
            ffn_activated.push(silu(gate) * up);
        }
        let ffn_down = project_rank2_base_rows(
            &self.weights,
            &f32s_to_bf16_words(&ffn_activated),
            input_rows,
            &layer.ffn_down_base,
        )?;
        let layer_out = add_f32(&residual, &ffn_down)?;
        Ok(f32s_to_bf16_words(&layer_out))
    }

    fn next_token_logits_top1(&mut self, hidden_words: &[u16]) -> Result<u32> {
        let weights = self.weights.clone();
        if let Some(tail) = self.metal_decode_tail.as_mut() {
            if let Ok(token_id) = tail.top1_from_hidden_words(&weights, hidden_words) {
                return Ok(token_id);
            }
        }
        let final_norm = self
            .weights
            .header_for_tensor(OUTPUT_NORM_WEIGHT)?
            .rms_norm_weighted_f32(
                hidden_words,
                OUTPUT_NORM_WEIGHT,
                self.weights.config().text.rms_norm_eps,
            )
            .map_err(|err| err.to_string())?;
        let final_norm_words = f32s_to_bf16_words(&final_norm);
        if let Some(top1) = project_rank2_base_top1(&self.weights, &final_norm_words, LM_HEAD_BASE)?
        {
            return Ok(top1);
        }
        let logits = project_rank2_base(&self.weights, &final_norm_words, LM_HEAD_BASE)?;
        let mut best_token = 0u32;
        let mut best_logit = f32::NEG_INFINITY;
        for (token_idx, &logit) in logits.iter().enumerate() {
            let token_id = token_idx as u32;
            if logit > best_logit || (logit == best_logit && token_id < best_token) {
                best_logit = logit;
                best_token = token_id;
            }
        }
        Ok(best_token)
    }

    fn generate_greedy(
        &mut self,
        prompt_token_ids: &[u32],
        max_new_tokens: usize,
    ) -> Result<GenerationMetrics> {
        if prompt_token_ids.is_empty() {
            return Err("prompt must contain at least one token".to_string());
        }

        let ttft_started = Instant::now();
        let mut next_token = if prompt_token_ids.len() == 1 {
            if let Some(token_id) = self.eval_token_top1_metal_chained(prompt_token_ids[0], 0)? {
                token_id
            } else {
                let last_hidden = self.eval_token_hidden(prompt_token_ids[0], 0)?;
                self.next_token_logits_top1(&last_hidden)?
            }
        } else {
            let last_hidden = self.eval_prompt_hidden(prompt_token_ids)?;
            self.next_token_logits_top1(&last_hidden)?
        };

        let mut generated = Vec::with_capacity(max_new_tokens);
        let ttft_elapsed = ttft_started.elapsed();

        let steady_started = Instant::now();
        if max_new_tokens != 0 {
            generated.push(next_token);
        }
        while generated.len() < max_new_tokens {
            let position = prompt_token_ids
                .len()
                .checked_add(generated.len())
                .and_then(|value| value.checked_sub(1))
                .ok_or_else(|| "decode position overflow".to_string())?;
            let hidden_started = Instant::now();
            let chained_next_token = self.eval_token_top1_metal_chained(next_token, position)?;
            let hidden_elapsed = hidden_started.elapsed();
            let top1_started = Instant::now();
            next_token = if let Some(token_id) = chained_next_token {
                token_id
            } else {
                let hidden = self.eval_token_hidden(next_token, position)?;
                self.next_token_logits_top1(&hidden)?
            };
            let top1_elapsed = top1_started.elapsed();
            if qwen_debug_chain_enabled() {
                eprintln!(
                    "[qwen_text_bench] decode step position={} hidden_s={:.6} top1_s={:.6}",
                    position,
                    hidden_elapsed.as_secs_f64(),
                    top1_elapsed.as_secs_f64()
                );
            }
            generated.push(next_token);
        }
        let steady_elapsed = steady_started.elapsed();

        Ok(GenerationMetrics {
            generated_token_ids: generated,
            time_to_first_token_elapsed: ttft_elapsed,
            steady_state_elapsed: steady_elapsed,
        })
    }
}

#[derive(Clone, Debug)]
struct GenerationMetrics {
    generated_token_ids: Vec<u32>,
    time_to_first_token_elapsed: Duration,
    steady_state_elapsed: Duration,
}

#[derive(Clone, Debug)]
struct BenchmarkOutput {
    load_duration: Duration,
    elapsed: Duration,
    prompt_token_ids: Vec<u32>,
    total_generated_tokens: usize,
    time_to_first_token_elapsed: Duration,
    steady_state_elapsed: Duration,
    steady_state_generated_tokens: usize,
    last_generated_token_ids: Vec<u32>,
    metal_counters: Option<MetalRuntimeCounters>,
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut model_path = PathBuf::from(DEFAULT_HF_CACHE_MODEL_DIR);
    let mut prompt_len = 32usize;
    let mut prompt_token_id = None::<u32>;
    let mut max_new_tokens = 64usize;
    let mut warmup_iters = 1usize;
    let mut measured_iters = 3usize;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--prompt-len" => {
                prompt_len = args
                    .next()
                    .ok_or("--prompt-len requires a value")?
                    .parse()?;
            }
            "--prompt-token-id" => {
                prompt_token_id = Some(
                    args.next()
                        .ok_or("--prompt-token-id requires a value")?
                        .parse()?,
                );
            }
            "--max-new-tokens" => {
                max_new_tokens = args
                    .next()
                    .ok_or("--max-new-tokens requires a value")?
                    .parse()?;
            }
            "--warmup" => {
                warmup_iters = args.next().ok_or("--warmup requires a value")?.parse()?;
            }
            "--iters" => {
                measured_iters = args.next().ok_or("--iters requires a value")?.parse()?;
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown option {}", value).into());
            }
            value => {
                model_path = PathBuf::from(value);
            }
        }
    }

    let benchmark = benchmark_qwen_generation(
        &model_path,
        prompt_len,
        prompt_token_id,
        max_new_tokens,
        warmup_iters,
        measured_iters,
    )?;

    let elapsed_secs = benchmark.elapsed.as_secs_f64();
    let decode_tokens_per_second = if elapsed_secs > 0.0 {
        benchmark.total_generated_tokens as f64 / elapsed_secs
    } else {
        0.0
    };
    let total_prompt_tokens = benchmark.prompt_token_ids.len() * measured_iters;
    let total_tokens = total_prompt_tokens + benchmark.total_generated_tokens;
    let total_tokens_per_second = if elapsed_secs > 0.0 {
        total_tokens as f64 / elapsed_secs
    } else {
        0.0
    };
    let prompt_prefill_tokens_per_second = if benchmark.time_to_first_token_elapsed.is_zero() {
        0.0
    } else {
        total_prompt_tokens as f64 / benchmark.time_to_first_token_elapsed.as_secs_f64()
    };
    let steady_state_decode_tokens_per_second = if benchmark.steady_state_elapsed.is_zero() {
        0.0
    } else {
        benchmark.steady_state_generated_tokens as f64
            / benchmark.steady_state_elapsed.as_secs_f64()
    };

    println!("model_path={}", model_path.display());
    println!("prompt_ids={:?}", benchmark.prompt_token_ids);
    println!("prompt_token_count={}", benchmark.prompt_token_ids.len());
    println!(
        "last_generated_ids={:?}",
        benchmark.last_generated_token_ids
    );
    println!("warmup_iters={}", warmup_iters);
    println!("measured_iters={}", measured_iters);
    println!("max_new_tokens={}", max_new_tokens);
    println!("load_s={:.6}", benchmark.load_duration.as_secs_f64());
    println!("elapsed_s={:.6}", benchmark.elapsed.as_secs_f64());
    println!(
        "total_generated_tokens={}",
        benchmark.total_generated_tokens
    );
    println!(
        "ttft_s={:.6}",
        benchmark.time_to_first_token_elapsed.as_secs_f64()
    );
    println!(
        "ttft_ms_avg={:.3}",
        benchmark.time_to_first_token_elapsed.as_secs_f64() * 1000.0 / measured_iters as f64
    );
    println!(
        "prompt_prefill_tok_s={:.3}",
        prompt_prefill_tokens_per_second
    );
    println!(
        "steady_elapsed_s={:.6}",
        benchmark.steady_state_elapsed.as_secs_f64()
    );
    println!(
        "steady_generated_tokens={}",
        benchmark.steady_state_generated_tokens
    );
    println!(
        "steady_decode_tok_s={:.3}",
        steady_state_decode_tokens_per_second
    );
    if let Some(counters) = benchmark.metal_counters {
        println!("metal_command_batches={}", counters.command_batches_begun);
        println!("metal_batch_commits={}", counters.command_batches_committed);
        println!(
            "metal_command_buffer_commits={}",
            counters.command_buffer_commits
        );
        println!("metal_compute_dispatches={}", counters.compute_dispatches);
        println!("metal_buffer_barriers={}", counters.buffer_barriers);
        println!("metal_encoder_starts={}", counters.compute_encoder_starts);
        println!("metal_encoder_ends={}", counters.compute_encoder_ends);
        println!("metal_blit_copies={}", counters.blit_copy_calls);
        println!("metal_fence_waits={}", counters.fence_waits);
        println!("metal_fence_updates={}", counters.fence_updates);
        println!("metal_wait_idle_calls={}", counters.wait_idle_calls);
        println!(
            "metal_completion_wait_calls={}",
            counters.completion_wait_calls
        );
        println!("metal_readbacks={}", counters.readback_calls);
        println!(
            "metal_gpu_elapsed_s={:.6}",
            counters.gpu_elapsed_ns as f64 / 1e9
        );
    }
    println!("decode_tok_s={:.3}", decode_tokens_per_second);
    println!("total_tok_s={:.3}", total_tokens_per_second);

    Ok(())
}

fn benchmark_qwen_generation(
    model_path: &Path,
    prompt_len: usize,
    prompt_token_id: Option<u32>,
    max_new_tokens: usize,
    warmup_iters: usize,
    measured_iters: usize,
) -> Result<BenchmarkOutput> {
    let load_started = Instant::now();
    let max_context = prompt_len
        .max(1)
        .checked_add(max_new_tokens)
        .ok_or_else(|| "benchmark max_context overflow".to_string())?;
    let mut runtime = QwenRuntime::load(model_path, max_context)?;
    let load_duration = load_started.elapsed();
    let fallback_token = prompt_token_id.unwrap_or(runtime.weights.config().text.bos_token_id);
    let prompt_token_ids = vec![fallback_token; prompt_len.max(1)];

    for _ in 0..warmup_iters {
        runtime.reset()?;
        let _ = runtime.generate_greedy(&prompt_token_ids, max_new_tokens)?;
    }
    let metal_runtime = runtime.metal_runtime();
    if let Some(runtime) = &metal_runtime {
        runtime.reset_counters();
    }

    let started = Instant::now();
    let mut total_generated_tokens = 0usize;
    let mut time_to_first_token_elapsed = Duration::ZERO;
    let mut steady_state_elapsed = Duration::ZERO;
    let mut steady_state_generated_tokens = 0usize;
    let mut last_generated_token_ids = Vec::new();
    for _ in 0..measured_iters {
        runtime.reset()?;
        let metrics = runtime.generate_greedy(&prompt_token_ids, max_new_tokens)?;
        total_generated_tokens += metrics.generated_token_ids.len();
        time_to_first_token_elapsed += metrics.time_to_first_token_elapsed;
        steady_state_elapsed += metrics.steady_state_elapsed;
        steady_state_generated_tokens += metrics.generated_token_ids.len().saturating_sub(1);
        last_generated_token_ids = metrics.generated_token_ids;
    }
    Ok(BenchmarkOutput {
        load_duration,
        elapsed: started.elapsed(),
        prompt_token_ids,
        total_generated_tokens,
        time_to_first_token_elapsed,
        steady_state_elapsed,
        steady_state_generated_tokens,
        last_generated_token_ids,
        metal_counters: metal_runtime.map(|runtime| runtime.counters()),
    })
}

fn eval_attention_layer(
    weights: &QwenMlxWeights,
    cache: &mut AttentionCache,
    input_norm_words: &[u16],
    position: usize,
    q_proj_base: &str,
    q_norm_weight: &str,
    k_proj_base: &str,
    k_norm_weight: &str,
    v_proj_base: &str,
    o_proj_base: &str,
) -> Result<Vec<f32>> {
    let cfg = &weights.config().text;
    let q_head_count = cfg.num_attention_heads;
    let kv_head_count = cfg.num_key_value_heads;
    let head_dim = cfg.head_dim;
    let q_heads_per_kv = q_head_count
        .checked_div(kv_head_count)
        .ok_or_else(|| "invalid q/kv head count ratio".to_string())?;
    if q_heads_per_kv == 0 || q_head_count % kv_head_count != 0 {
        return Err(format!(
            "invalid attention head layout: q={} kv={}",
            q_head_count, kv_head_count
        ));
    }

    let (q_full, k_raw, v_raw) = if let Some(mut outputs) = maybe_project_rank2_bases_affine_concat(
        weights,
        input_norm_words,
        &[q_proj_base, k_proj_base, v_proj_base],
    )? {
        if outputs.len() != 3 {
            return Err(format!(
                "fused attention projection count mismatch: got {} expected 3",
                outputs.len()
            ));
        }
        (outputs.remove(0), outputs.remove(0), outputs.remove(0))
    } else {
        (
            project_rank2_base(weights, input_norm_words, q_proj_base)?,
            project_rank2_base(weights, input_norm_words, k_proj_base)?,
            project_rank2_base(weights, input_norm_words, v_proj_base)?,
        )
    };
    let expected_q_full = q_head_count
        .checked_mul(head_dim)
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| "q projection size overflow".to_string())?;
    if q_full.len() != expected_q_full {
        return Err(format!(
            "q_proj output length mismatch: got {} expected {}",
            q_full.len(),
            expected_q_full
        ));
    }

    let mut q_raw = Vec::with_capacity(q_head_count * head_dim);
    let mut gate_raw = Vec::with_capacity(q_head_count * head_dim);
    for head in 0..q_head_count {
        let head_base = head * head_dim * 2;
        q_raw.extend_from_slice(&q_full[head_base..head_base + head_dim]);
        gate_raw.extend_from_slice(&q_full[head_base + head_dim..head_base + head_dim * 2]);
    }
    let q_norm = rms_norm_rows_weighted_f32(
        &q_raw,
        q_head_count,
        head_dim,
        &weights.read_bf16_tensor_words_cached(q_norm_weight)?,
        weights.config().text.rms_norm_eps,
    )?;

    if k_raw.len() != kv_head_count * head_dim {
        return Err(format!(
            "k_proj output length mismatch: got {} expected {}",
            k_raw.len(),
            kv_head_count * head_dim
        ));
    }
    let mut k_norm = rms_norm_rows_weighted_f32(
        &k_raw,
        kv_head_count,
        head_dim,
        &weights.read_bf16_tensor_words_cached(k_norm_weight)?,
        weights.config().text.rms_norm_eps,
    )?;

    if v_raw.len() != kv_head_count * head_dim {
        return Err(format!(
            "v_proj output length mismatch: got {} expected {}",
            v_raw.len(),
            kv_head_count * head_dim
        ));
    }

    let rotary_dim = (cfg.partial_rotary_factor * head_dim as f32).round() as usize;
    let rotary_dim = rotary_dim.clamp(0, head_dim);
    let mut q_rope = q_norm;
    apply_rope_interleaved_rows_in_place(
        &mut q_rope,
        q_head_count,
        head_dim,
        rotary_dim,
        cfg.rope_theta,
        position,
    )?;
    apply_rope_interleaved_rows_in_place(
        &mut k_norm,
        kv_head_count,
        head_dim,
        rotary_dim,
        cfg.rope_theta,
        position,
    )?;
    cache.append(&k_norm, &v_raw)?;
    let scale = 1.0f32 / (head_dim as f32).sqrt();
    let mut attn_out = Vec::with_capacity(q_head_count * head_dim);
    for q_head in 0..q_head_count {
        let kv_head = q_head / q_heads_per_kv;
        let q_row = &q_rope[q_head * head_dim..(q_head + 1) * head_dim];
        let mut scores = Vec::with_capacity(cache.seq_len);
        for token_idx in 0..cache.seq_len {
            let k_base = token_idx * head_dim;
            let k_row = &cache.keys[kv_head][k_base..k_base + head_dim];
            let mut dot = 0.0f32;
            for (&q, &k) in q_row.iter().zip(k_row.iter()) {
                dot += q * k;
            }
            scores.push(dot * scale);
        }
        let probs = softmax(&scores);
        let mut out_row = vec![0.0f32; head_dim];
        for (token_idx, &prob) in probs.iter().enumerate() {
            let v_base = token_idx * head_dim;
            let v_row = &cache.values[kv_head][v_base..v_base + head_dim];
            for dim in 0..head_dim {
                out_row[dim] += prob * v_row[dim];
            }
        }
        let gate_row = &gate_raw[q_head * head_dim..(q_head + 1) * head_dim];
        for dim in 0..head_dim {
            out_row[dim] *= sigmoid(gate_row[dim]);
        }
        attn_out.extend_from_slice(&out_row);
    }

    project_rank2_base(weights, &f32s_to_bf16_words(&attn_out), o_proj_base)
}

fn eval_attention_layer_rows(
    weights: &QwenMlxWeights,
    cache: &mut AttentionCache,
    input_norm_words: &[u16],
    start_position: usize,
    input_rows: usize,
    q_proj_base: &str,
    q_norm_weight: &str,
    k_proj_base: &str,
    k_norm_weight: &str,
    v_proj_base: &str,
    o_proj_base: &str,
) -> Result<Vec<f32>> {
    if input_rows == 0 {
        return Ok(Vec::new());
    }
    let cfg = &weights.config().text;
    let q_head_count = cfg.num_attention_heads;
    let kv_head_count = cfg.num_key_value_heads;
    let head_dim = cfg.head_dim;
    let hidden_size = cfg._hidden_size;
    if input_norm_words.len()
        != hidden_size
            .checked_mul(input_rows)
            .ok_or_else(|| "attention row input size overflow".to_string())?
    {
        return Err(format!(
            "attention row input length mismatch: got {} expected {}",
            input_norm_words.len(),
            hidden_size * input_rows
        ));
    }
    let q_heads_per_kv = q_head_count
        .checked_div(kv_head_count)
        .ok_or_else(|| "invalid q/kv head count ratio".to_string())?;
    if q_heads_per_kv == 0 || q_head_count % kv_head_count != 0 {
        return Err(format!(
            "invalid attention head layout: q={} kv={}",
            q_head_count, kv_head_count
        ));
    }

    let (q_full, k_raw, v_raw) = if let Some(mut outputs) =
        maybe_project_rank2_bases_affine_concat_rows(
            weights,
            input_norm_words,
            input_rows,
            &[q_proj_base, k_proj_base, v_proj_base],
        )? {
        if outputs.len() != 3 {
            return Err(format!(
                "fused attention row projection count mismatch: got {} expected 3",
                outputs.len()
            ));
        }
        (outputs.remove(0), outputs.remove(0), outputs.remove(0))
    } else {
        (
            project_rank2_base_rows(weights, input_norm_words, input_rows, q_proj_base)?,
            project_rank2_base_rows(weights, input_norm_words, input_rows, k_proj_base)?,
            project_rank2_base_rows(weights, input_norm_words, input_rows, v_proj_base)?,
        )
    };
    let q_row_width = q_head_count
        .checked_mul(head_dim)
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| "attention q row width overflow".to_string())?;
    if q_full.len() != q_row_width * input_rows {
        return Err(format!(
            "q_proj row output length mismatch: got {} expected {}",
            q_full.len(),
            q_row_width * input_rows
        ));
    }
    let kv_row_width = kv_head_count
        .checked_mul(head_dim)
        .ok_or_else(|| "attention kv row width overflow".to_string())?;
    if k_raw.len() != kv_row_width * input_rows {
        return Err(format!(
            "k_proj row output length mismatch: got {} expected {}",
            k_raw.len(),
            kv_row_width * input_rows
        ));
    }
    if v_raw.len() != kv_row_width * input_rows {
        return Err(format!(
            "v_proj row output length mismatch: got {} expected {}",
            v_raw.len(),
            kv_row_width * input_rows
        ));
    }

    let mut q_raw = Vec::with_capacity(input_rows * q_head_count * head_dim);
    let mut gate_raw = Vec::with_capacity(input_rows * q_head_count * head_dim);
    for token_idx in 0..input_rows {
        let row = &q_full[token_idx * q_row_width..(token_idx + 1) * q_row_width];
        for head in 0..q_head_count {
            let head_base = head * head_dim * 2;
            q_raw.extend_from_slice(&row[head_base..head_base + head_dim]);
            gate_raw.extend_from_slice(&row[head_base + head_dim..head_base + head_dim * 2]);
        }
    }
    let q_norm = rms_norm_rows_weighted_f32(
        &q_raw,
        input_rows * q_head_count,
        head_dim,
        &weights.read_bf16_tensor_words_cached(q_norm_weight)?,
        cfg.rms_norm_eps,
    )?;
    let mut k_norm = rms_norm_rows_weighted_f32(
        &k_raw,
        input_rows * kv_head_count,
        head_dim,
        &weights.read_bf16_tensor_words_cached(k_norm_weight)?,
        cfg.rms_norm_eps,
    )?;

    let rotary_dim = (cfg.partial_rotary_factor * head_dim as f32).round() as usize;
    let rotary_dim = rotary_dim.clamp(0, head_dim);
    let mut q_rope = q_norm;
    apply_rope_interleaved_token_head_rows_in_place(
        &mut q_rope,
        input_rows,
        q_head_count,
        head_dim,
        rotary_dim,
        cfg.rope_theta,
        start_position,
    )?;
    apply_rope_interleaved_token_head_rows_in_place(
        &mut k_norm,
        input_rows,
        kv_head_count,
        head_dim,
        rotary_dim,
        cfg.rope_theta,
        start_position,
    )?;

    let scale = 1.0f32 / (head_dim as f32).sqrt();
    let mut attn_out = Vec::with_capacity(input_rows * q_head_count * head_dim);
    for token_idx in 0..input_rows {
        let kv_start = token_idx * kv_row_width;
        let kv_end = kv_start + kv_row_width;
        cache.append(&k_norm[kv_start..kv_end], &v_raw[kv_start..kv_end])?;

        let q_token_start = token_idx * q_head_count * head_dim;
        for q_head in 0..q_head_count {
            let kv_head = q_head / q_heads_per_kv;
            let q_row_start = q_token_start + q_head * head_dim;
            let q_row = &q_rope[q_row_start..q_row_start + head_dim];
            let mut scores = Vec::with_capacity(cache.seq_len);
            for token_cache_idx in 0..cache.seq_len {
                let k_base = token_cache_idx * head_dim;
                let k_row = &cache.keys[kv_head][k_base..k_base + head_dim];
                let mut dot = 0.0f32;
                for (&q, &k) in q_row.iter().zip(k_row.iter()) {
                    dot += q * k;
                }
                scores.push(dot * scale);
            }
            let probs = softmax(&scores);
            let mut out_row = vec![0.0f32; head_dim];
            for (token_cache_idx, &prob) in probs.iter().enumerate() {
                let v_base = token_cache_idx * head_dim;
                let v_row = &cache.values[kv_head][v_base..v_base + head_dim];
                for dim in 0..head_dim {
                    out_row[dim] += prob * v_row[dim];
                }
            }
            let gate_row = &gate_raw[q_row_start..q_row_start + head_dim];
            for dim in 0..head_dim {
                out_row[dim] *= sigmoid(gate_row[dim]);
            }
            attn_out.extend_from_slice(&out_row);
        }
    }

    project_rank2_base_rows(
        weights,
        &f32s_to_bf16_words(&attn_out),
        input_rows,
        o_proj_base,
    )
}

fn eval_recurrent_layer(
    weights: &QwenMlxWeights,
    state: &mut CpuRecurrentState,
    input_norm_words: &[u16],
    qkv_proj_weight: &str,
    z_proj_weight: &str,
    beta_proj_weight: &str,
    alpha_proj_weight: &str,
    a_log_name: &str,
    dt_bias_name: &str,
    conv1d_weight: &str,
    norm_weight: &str,
    out_proj_weight: &str,
) -> Result<Vec<f32>> {
    let cfg = &weights.config().text;
    let key_dim = cfg.linear_key_head_dim;
    let key_head_count = cfg.linear_num_key_heads;
    let value_head_count = cfg.linear_num_value_heads;
    let value_dim = cfg.linear_value_head_dim;
    let value_hidden = value_head_count
        .checked_mul(value_dim)
        .ok_or_else(|| "value hidden size overflow".to_string())?;

    let mut projected = dense_bf16_matmul_tensors_multi(
        weights,
        input_norm_words,
        &[
            qkv_proj_weight,
            z_proj_weight,
            beta_proj_weight,
            alpha_proj_weight,
        ],
    )?;
    if projected.len() != 4 {
        return Err(format!(
            "recurrent projection output count mismatch: got {} expected 4",
            projected.len()
        ));
    }
    let qkv_mixed = projected.remove(0);
    let qkv_dim = recurrent_qkv_dim(weights.config());
    if qkv_mixed.len() != qkv_dim {
        return Err(format!(
            "qkv_mixed length mismatch: got {} expected {}",
            qkv_mixed.len(),
            qkv_dim
        ));
    }
    let z = projected.remove(0);
    if z.len() != value_hidden {
        return Err(format!(
            "z projection length mismatch: got {} expected {}",
            z.len(),
            value_hidden
        ));
    }
    let mut beta = projected.remove(0);
    let alpha = projected.remove(0);
    if beta.len() != value_head_count || alpha.len() != value_head_count {
        return Err(format!(
            "alpha/beta projection mismatch: alpha={} beta={} expected={}",
            alpha.len(),
            beta.len(),
            value_head_count
        ));
    }
    for value in &mut beta {
        *value = sigmoid(*value);
    }

    let a_log = weights.read_tensor_f32(a_log_name)?;
    let dt_bias = weights.read_tensor_f32(dt_bias_name)?;
    if a_log.len() != value_head_count || dt_bias.len() != value_head_count {
        return Err(format!(
            "A_log/dt_bias length mismatch: A_log={} dt_bias={} expected={}",
            a_log.len(),
            dt_bias.len(),
            value_head_count
        ));
    }
    let mut gate = Vec::with_capacity(value_head_count);
    for head in 0..value_head_count {
        let alpha_softplus = softplus(alpha[head] + dt_bias[head]);
        gate.push(alpha_softplus * (-a_log[head].exp()));
    }

    let conv_input = update_recurrent_conv_state(
        &mut state.conv_state,
        &qkv_mixed,
        cfg.linear_conv_kernel_dim,
    )?;
    let conv_kernel = weights.read_tensor_f32(conv1d_weight)?;
    let conv_output = ssm_conv(
        &conv_input,
        &conv_kernel,
        cfg.linear_conv_kernel_dim,
        qkv_dim,
    )?;
    let conv_output = conv_output.into_iter().map(silu).collect::<Vec<_>>();

    let q_len = key_dim * key_head_count;
    let k_len = q_len;
    let v_len = value_dim * value_head_count;
    let q_conv = l2_norm_rows_f32(
        &conv_output[0..q_len],
        key_head_count,
        key_dim,
        cfg.rms_norm_eps,
    )?;
    let k_conv = l2_norm_rows_f32(
        &conv_output[q_len..q_len + k_len],
        key_head_count,
        key_dim,
        cfg.rms_norm_eps,
    )?;
    let v_conv = conv_output[q_len + k_len..q_len + k_len + v_len].to_vec();

    let norm_weights = weights.read_tensor_f32(norm_weight)?;
    if norm_weights.len() != value_dim {
        return Err(format!(
            "linear attention norm length mismatch: got {} expected {}",
            norm_weights.len(),
            value_dim
        ));
    }
    let out_proj = dense_matmul_tensor(
        weights,
        &f32s_to_bf16_words(&recurrent_step(
            &mut state.state,
            &q_conv,
            &k_conv,
            &v_conv,
            &gate,
            &beta,
            key_head_count,
            value_head_count,
            key_dim,
            value_dim,
            &norm_weights,
            &z,
            cfg.rms_norm_eps,
        )?),
        out_proj_weight,
    )?;
    Ok(out_proj)
}

fn recurrent_step(
    state: &mut [f32],
    q_conv: &[f32],
    k_conv: &[f32],
    v_conv: &[f32],
    gate: &[f32],
    beta: &[f32],
    key_head_count: usize,
    value_head_count: usize,
    key_dim: usize,
    value_dim: usize,
    norm_weights: &[f32],
    z: &[f32],
    eps: f32,
) -> Result<Vec<f32>> {
    if value_head_count == 0 || key_head_count == 0 || value_head_count % key_head_count != 0 {
        return Err(format!(
            "invalid recurrent head layout: key_heads={} value_heads={}",
            key_head_count, value_head_count
        ));
    }
    let value_per_key = value_head_count / key_head_count;
    let mut output = vec![0.0f32; value_head_count * value_dim];
    let q_scale = 1.0f32 / (key_dim as f32).sqrt();

    for value_head in 0..value_head_count {
        let key_head = value_head / value_per_key;
        let q = &q_conv[key_head * key_dim..(key_head + 1) * key_dim];
        let k = &k_conv[key_head * key_dim..(key_head + 1) * key_dim];
        let v = &v_conv[value_head * value_dim..(value_head + 1) * value_dim];
        let z_row = &z[value_head * value_dim..(value_head + 1) * value_dim];
        let matrix_base = value_head * key_dim * value_dim;
        let matrix = &mut state[matrix_base..matrix_base + key_dim * value_dim];

        let gate_exp = gate[value_head].exp();
        let mut sk = vec![0.0f32; value_dim];
        for key_idx in 0..key_dim {
            let key_value = k[key_idx];
            let row_base = key_idx * value_dim;
            for value_idx in 0..value_dim {
                let scaled = matrix[row_base + value_idx] * gate_exp;
                matrix[row_base + value_idx] = scaled;
                sk[value_idx] += scaled * key_value;
            }
        }

        let mut d = vec![0.0f32; value_dim];
        for value_idx in 0..value_dim {
            d[value_idx] = (v[value_idx] - sk[value_idx]) * beta[value_head];
        }

        for key_idx in 0..key_dim {
            let k_value = k[key_idx];
            let row_base = key_idx * value_dim;
            for value_idx in 0..value_dim {
                matrix[row_base + value_idx] += k_value * d[value_idx];
            }
        }

        let mut out_row = vec![0.0f32; value_dim];
        for key_idx in 0..key_dim {
            let q_value = q[key_idx] * q_scale;
            let row_base = key_idx * value_dim;
            for value_idx in 0..value_dim {
                out_row[value_idx] += matrix[row_base + value_idx] * q_value;
            }
        }
        let out_row = rms_norm_single_row_weighted_f32(&out_row, norm_weights, eps)?;
        for value_idx in 0..value_dim {
            output[value_head * value_dim + value_idx] =
                out_row[value_idx] * silu(z_row[value_idx]);
        }
    }

    Ok(output)
}

fn update_recurrent_conv_state(
    conv_state: &mut [f32],
    qkv_mixed: &[f32],
    conv_kernel_dim: usize,
) -> Result<Vec<f32>> {
    let qkv_dim = qkv_mixed.len();
    let conv_prefix = conv_kernel_dim.saturating_sub(1);
    if conv_state.len() != conv_prefix * qkv_dim {
        return Err(format!(
            "conv state length mismatch: got {} expected {}",
            conv_state.len(),
            conv_prefix * qkv_dim
        ));
    }
    let mut conv_input = Vec::with_capacity(conv_kernel_dim * qkv_dim);
    conv_input.extend_from_slice(conv_state);
    conv_input.extend_from_slice(qkv_mixed);
    if conv_prefix != 0 {
        conv_state.copy_from_slice(&conv_input[qkv_dim..qkv_dim + conv_prefix * qkv_dim]);
    }
    Ok(conv_input)
}

fn ssm_conv(
    conv_input: &[f32],
    conv_kernel: &[f32],
    conv_kernel_dim: usize,
    conv_channels: usize,
) -> Result<Vec<f32>> {
    if conv_input.len() != conv_kernel_dim * conv_channels {
        return Err(format!(
            "conv input length mismatch: got {} expected {}",
            conv_input.len(),
            conv_kernel_dim * conv_channels
        ));
    }
    if conv_kernel.len() != conv_kernel_dim * conv_channels {
        return Err(format!(
            "conv kernel length mismatch: got {} expected {}",
            conv_kernel.len(),
            conv_kernel_dim * conv_channels
        ));
    }
    let mut out = vec![0.0f32; conv_channels];
    for k in 0..conv_kernel_dim {
        let input_row = &conv_input[k * conv_channels..(k + 1) * conv_channels];
        let kernel_row = &conv_kernel[k * conv_channels..(k + 1) * conv_channels];
        for channel in 0..conv_channels {
            out[channel] += input_row[channel] * kernel_row[channel];
        }
    }
    Ok(out)
}

fn project_embedding_row(weights: &QwenMlxWeights, base: &str, row: u32) -> Result<Vec<f32>> {
    let weight_name = format!("{base}.weight");
    let entry = weights.tensor(&weight_name)?;
    match entry.dtype {
        MlxDType::U32 => {
            let params = weights.quant_params_for_weight(&weight_name);
            if params.bits == 0 || params.bits > 8 {
                return Err(format!(
                    "unsupported quant bits {} for {}",
                    params.bits, weight_name
                ));
            }
            let scales_name = format!("{base}.scales");
            let biases_name = format!("{base}.biases");
            let packed = weights.read_rank2_row_u32_words(&weight_name, row as u64)?;
            let scales = weights.read_rank2_row_bf16_words(&scales_name, row as u64)?;
            let biases = weights.read_rank2_row_bf16_words(&biases_name, row as u64)?;
            dequantize_affine_row(&packed, &scales, &biases, params.bits, params.group_size)
        }
        MlxDType::BF16 | MlxDType::F32 => weights.read_rank2_row_f32(&weight_name, row as u64),
        other => Err(format!(
            "unsupported embedding dtype {:?} for {}",
            other, weight_name
        )),
    }
}

fn project_rank2_base(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    base: &str,
) -> Result<Vec<f32>> {
    let weight_name = format!("{base}.weight");
    project_rank2(weights, input_words, &weight_name)
}

fn project_rank2_base_top1(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    base: &str,
) -> Result<Option<u32>> {
    let weight_name = format!("{base}.weight");
    project_rank2_top1(weights, input_words, &weight_name)
}

fn project_rank2_base_rows(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    input_rows: usize,
    base: &str,
) -> Result<Vec<f32>> {
    let weight_name = format!("{base}.weight");
    project_rank2_rows(weights, input_words, input_rows, &weight_name)
}

fn project_rank2(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    weight_name: &str,
) -> Result<Vec<f32>> {
    let entry = weights.tensor(weight_name)?;
    match entry.dtype {
        MlxDType::U32 => {
            let base = strip_quant_suffix(weight_name);
            quantized_matmul_tensor(
                weights,
                input_words,
                weight_name,
                &format!("{base}.scales"),
                &format!("{base}.biases"),
            )
        }
        MlxDType::BF16 | MlxDType::F32 => dense_matmul_tensor(weights, input_words, weight_name),
        other => Err(format!(
            "unsupported projection dtype {:?} for {}",
            other, weight_name
        )),
    }
}

fn project_rank2_top1(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    weight_name: &str,
) -> Result<Option<u32>> {
    let entry = weights.tensor(weight_name)?;
    if entry.shape.len() != 2 {
        return Err(format!(
            "projection expects rank-2 tensor, got {:?} for {}",
            entry.shape, weight_name
        ));
    }
    match entry.dtype {
        MlxDType::U32 => {
            let base = strip_quant_suffix(weight_name);
            quantized_matmul_tensor_top1(
                weights,
                input_words,
                weight_name,
                &format!("{base}.scales"),
                &format!("{base}.biases"),
            )
        }
        MlxDType::BF16 | MlxDType::F32 => Ok(None),
        other => Err(format!(
            "unsupported projection dtype {:?} for {}",
            other, weight_name
        )),
    }
}

fn project_rank2_rows(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    input_rows: usize,
    weight_name: &str,
) -> Result<Vec<f32>> {
    if input_rows == 0 {
        return Ok(Vec::new());
    }
    let entry = weights.tensor(weight_name)?;
    if entry.shape.len() != 2 {
        return Err(format!(
            "projection expects rank-2 tensor, got {:?} for {}",
            entry.shape, weight_name
        ));
    }
    match entry.dtype {
        MlxDType::U32 => {
            let base = strip_quant_suffix(weight_name);
            quantized_matmul_tensor_rows(
                weights,
                input_words,
                input_rows,
                weight_name,
                &format!("{base}.scales"),
                &format!("{base}.biases"),
            )
        }
        MlxDType::BF16 | MlxDType::F32 => {
            dense_matmul_tensor_rows(weights, input_words, input_rows, weight_name)
        }
        other => Err(format!(
            "unsupported projection dtype {:?} for {}",
            other, weight_name
        )),
    }
}

fn dense_matmul_tensor(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    weight_name: &str,
) -> Result<Vec<f32>> {
    let entry = weights.tensor(weight_name)?;
    if entry.shape.len() != 2 {
        return Err(format!(
            "dense matmul expects rank-2 tensor, got {:?} for {}",
            entry.shape, weight_name
        ));
    }
    let rows = entry.shape[0] as usize;
    let inner_dim = entry.shape[1] as usize;
    if input_words.len() != inner_dim {
        return Err(format!(
            "dense matmul input mismatch for {}: got {} expected {}",
            weight_name,
            input_words.len(),
            inner_dim
        ));
    }
    let input = input_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    match entry.dtype {
        MlxDType::BF16 => {
            let weight_words = weights.read_bf16_tensor_words_cached(weight_name)?;
            if let Some(out) = try_matmul_nt_ggml_bytes(
                &input,
                bf16_words_as_bytes(weight_words.as_slice()),
                GGML_TYPE_BF16,
                1,
                inner_dim,
                rows,
            ) {
                return Ok(out);
            }
            let mut out = Vec::with_capacity(rows);
            for row in 0..rows {
                let row_base = row * inner_dim;
                let row_slice = &weight_words[row_base..row_base + inner_dim];
                let mut sum = 0.0f32;
                for (&weight, &x) in row_slice.iter().zip(input.iter()) {
                    sum += bf16_word_to_f32(weight) * x;
                }
                out.push(sum);
            }
            Ok(out)
        }
        MlxDType::F32 => {
            let weight_values = weights.read_tensor_f32(weight_name)?;
            if let Some(out) = try_matmul_nt_ggml_bytes(
                &input,
                f32s_as_bytes(weight_values.as_slice()),
                GGML_TYPE_F32,
                1,
                inner_dim,
                rows,
            ) {
                return Ok(out);
            }
            let mut out = Vec::with_capacity(rows);
            for row in 0..rows {
                let row_base = row * inner_dim;
                let row_slice = &weight_values[row_base..row_base + inner_dim];
                let mut sum = 0.0f32;
                for (&weight, &x) in row_slice.iter().zip(input.iter()) {
                    sum += weight * x;
                }
                out.push(sum);
            }
            Ok(out)
        }
        other => Err(format!(
            "dense matmul only supports BF16/F32, got {:?} for {}",
            other, weight_name
        )),
    }
}

fn dense_matmul_tensor_rows(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    input_rows: usize,
    weight_name: &str,
) -> Result<Vec<f32>> {
    if input_rows == 0 {
        return Ok(Vec::new());
    }
    let entry = weights.tensor(weight_name)?;
    if entry.shape.len() != 2 {
        return Err(format!(
            "dense matmul expects rank-2 tensor, got {:?} for {}",
            entry.shape, weight_name
        ));
    }
    let rows = entry.shape[0] as usize;
    let inner_dim = entry.shape[1] as usize;
    if input_words.len()
        != inner_dim
            .checked_mul(input_rows)
            .ok_or_else(|| format!("dense batched input size overflow for {}", weight_name))?
    {
        return Err(format!(
            "dense batched input mismatch for {}: got {} expected {}",
            weight_name,
            input_words.len(),
            inner_dim * input_rows
        ));
    }
    let input = bf16_words_to_f32s(input_words);
    match entry.dtype {
        MlxDType::BF16 => {
            let weight_words = weights.read_bf16_tensor_words_cached(weight_name)?;
            if let Some(out) = try_matmul_nt_ggml_bytes(
                &input,
                bf16_words_as_bytes(weight_words.as_slice()),
                GGML_TYPE_BF16,
                input_rows,
                inner_dim,
                rows,
            ) {
                return Ok(out);
            }
        }
        MlxDType::F32 => {
            let weight_values = weights.read_tensor_f32(weight_name)?;
            if let Some(out) = try_matmul_nt_ggml_bytes(
                &input,
                f32s_as_bytes(weight_values.as_slice()),
                GGML_TYPE_F32,
                input_rows,
                inner_dim,
                rows,
            ) {
                return Ok(out);
            }
        }
        other => {
            return Err(format!(
                "dense matmul only supports BF16/F32, got {:?} for {}",
                other, weight_name
            ));
        }
    }

    let mut out = Vec::with_capacity(
        input_rows
            .checked_mul(rows)
            .ok_or_else(|| format!("dense batched output size overflow for {}", weight_name))?,
    );
    for row_idx in 0..input_rows {
        let input_start = row_idx * inner_dim;
        let input_end = input_start + inner_dim;
        out.extend(dense_matmul_tensor(
            weights,
            &input_words[input_start..input_end],
            weight_name,
        )?);
    }
    Ok(out)
}

fn dense_bf16_matmul_tensors_multi(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    weight_names: &[&str],
) -> Result<Vec<Vec<f32>>> {
    if weight_names.is_empty() {
        return Ok(Vec::new());
    }

    let mut inner_dim = None::<usize>;
    let mut weight_buffers = Vec::with_capacity(weight_names.len());
    let mut row_counts = Vec::with_capacity(weight_names.len());
    for &weight_name in weight_names {
        let entry = weights.tensor(weight_name)?;
        if entry.shape.len() != 2 {
            return Err(format!(
                "dense multi matmul expects rank-2 tensor, got {:?} for {}",
                entry.shape, weight_name
            ));
        }
        if entry.dtype != MlxDType::BF16 {
            return Err(format!(
                "dense multi matmul currently expects BF16 tensors, got {:?} for {}",
                entry.dtype, weight_name
            ));
        }
        let rows = entry.shape[0] as usize;
        let tensor_inner_dim = entry.shape[1] as usize;
        if let Some(expected_inner_dim) = inner_dim {
            if tensor_inner_dim != expected_inner_dim {
                return Err(format!(
                    "dense multi matmul inner dim mismatch: got {} expected {} for {}",
                    tensor_inner_dim, expected_inner_dim, weight_name
                ));
            }
        } else {
            inner_dim = Some(tensor_inner_dim);
        }
        if input_words.len() != tensor_inner_dim {
            return Err(format!(
                "dense multi matmul input mismatch for {}: got {} expected {}",
                weight_name,
                input_words.len(),
                tensor_inner_dim
            ));
        }
        weight_buffers.push(weights.read_bf16_tensor_words_cached(weight_name)?);
        row_counts.push(rows);
    }

    let mut matrices = Vec::with_capacity(weight_names.len());
    for (weight_words, &rows) in weight_buffers.iter().zip(row_counts.iter()) {
        matrices.push(MatmulNtGgmlBytesMatrix {
            bt_bytes: bf16_words_as_bytes(weight_words.as_slice()),
            bt_ggml_type: GGML_TYPE_BF16,
            n: rows,
        });
    }

    let input = input_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    if let Some(outputs) =
        try_matmul_nt_ggml_bytes_multi(&input, 1, inner_dim.unwrap_or(0), &matrices)
    {
        return Ok(outputs);
    }

    let _weight_buffers = weight_buffers;
    let mut outputs = Vec::with_capacity(weight_names.len());
    for &weight_name in weight_names {
        outputs.push(dense_matmul_tensor(weights, input_words, weight_name)?);
    }
    Ok(outputs)
}

fn maybe_project_rank2_bases_affine_concat(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    bases: &[&str],
) -> Result<Option<Vec<Vec<f32>>>> {
    if bases.is_empty() {
        return Ok(Some(Vec::new()));
    }

    let weight_names = bases
        .iter()
        .map(|base| format!("{base}.weight"))
        .collect::<Vec<_>>();
    let weight_name_refs = weight_names.iter().map(String::as_str).collect::<Vec<_>>();
    let mut bits = None::<u32>;
    let mut group_size = None::<u64>;
    for &weight_name in &weight_name_refs {
        let entry = weights.tensor(weight_name)?;
        if entry.shape.len() != 2 || entry.dtype != MlxDType::U32 {
            return Ok(None);
        }
        let params = weights.quant_params_for_weight(weight_name);
        let this_group_size = params.group_size as u64;
        if !matches!(params.bits, 4 | 8) {
            return Ok(None);
        }
        if let Some(expected) = bits {
            if params.bits != expected {
                return Ok(None);
            }
        } else {
            bits = Some(params.bits);
        }
        if let Some(expected) = group_size {
            if this_group_size != expected {
                return Ok(None);
            }
        } else {
            group_size = Some(this_group_size);
        }
    }

    let concat = weights.concat_affine_tensor_cached(&weight_name_refs)?;
    let root = weights.model.root_dir.to_string_lossy();
    let cache_key = weight_name_refs.join("|");
    let weight_key = format!("{root}:{cache_key}:weight");
    let scales_key = format!("{root}:{cache_key}:scales");
    let biases_key = format!("{root}:{cache_key}:biases");
    let flat = if let Some(result) = try_affine_quantized_matmul_bf16(
        AffineQuantizedMatmulSpec {
            input_bf16_words: input_words,
            out_rows: concat.total_rows,
            weight_words_per_row: concat.weight_words_per_row,
            qparams_per_row: concat.qparams_per_row,
            bits: concat.bits,
            group_size: concat.group_size,
            cache_namespace: root.as_ref(),
        },
        &weight_key,
        &scales_key,
        &biases_key,
        || Ok(concat.weight_bytes.as_ref().clone()),
        || Ok(concat.scales_bytes.as_ref().clone()),
        || Ok(concat.biases_bytes.as_ref().clone()),
    ) {
        result?
    } else {
        return Ok(None);
    };

    if flat.len() != concat.total_rows {
        return Err(format!(
            "concat affine output length mismatch: got {} expected {}",
            flat.len(),
            concat.total_rows
        ));
    }
    let mut outputs = Vec::with_capacity(concat.row_counts.len());
    let mut offset = 0usize;
    for &rows in &concat.row_counts {
        outputs.push(flat[offset..offset + rows].to_vec());
        offset += rows;
    }
    Ok(Some(outputs))
}

fn maybe_project_rank2_bases_affine_concat_rows(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    input_rows: usize,
    bases: &[&str],
) -> Result<Option<Vec<Vec<f32>>>> {
    if bases.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if input_rows == 0 {
        return Ok(Some(bases.iter().map(|_| Vec::new()).collect()));
    }

    let weight_names = bases
        .iter()
        .map(|base| format!("{base}.weight"))
        .collect::<Vec<_>>();
    let weight_name_refs = weight_names.iter().map(String::as_str).collect::<Vec<_>>();
    let mut bits = None::<u32>;
    let mut group_size = None::<u64>;
    for &weight_name in &weight_name_refs {
        let entry = weights.tensor(weight_name)?;
        if entry.shape.len() != 2 || entry.dtype != MlxDType::U32 {
            return Ok(None);
        }
        let params = weights.quant_params_for_weight(weight_name);
        let this_group_size = params.group_size as u64;
        if !matches!(params.bits, 4 | 8) {
            return Ok(None);
        }
        if let Some(expected) = bits {
            if params.bits != expected {
                return Ok(None);
            }
        } else {
            bits = Some(params.bits);
        }
        if let Some(expected) = group_size {
            if this_group_size != expected {
                return Ok(None);
            }
        } else {
            group_size = Some(this_group_size);
        }
    }

    let concat = weights.concat_affine_tensor_cached(&weight_name_refs)?;
    let root = weights.model.root_dir.to_string_lossy();
    let cache_key = weight_name_refs.join("|");
    let weight_key = format!("{root}:{cache_key}:weight");
    let scales_key = format!("{root}:{cache_key}:scales");
    let biases_key = format!("{root}:{cache_key}:biases");
    let flat = if let Some(result) = try_affine_quantized_matmul_bf16_rows(
        AffineQuantizedMatmulRowsSpec {
            input_bf16_words: input_words,
            input_rows,
            out_rows: concat.total_rows,
            weight_words_per_row: concat.weight_words_per_row,
            qparams_per_row: concat.qparams_per_row,
            bits: concat.bits,
            group_size: concat.group_size,
            cache_namespace: root.as_ref(),
        },
        &weight_key,
        &scales_key,
        &biases_key,
        || Ok(concat.weight_bytes.as_ref().clone()),
        || Ok(concat.scales_bytes.as_ref().clone()),
        || Ok(concat.biases_bytes.as_ref().clone()),
    ) {
        result?
    } else {
        return Ok(None);
    };

    split_batched_projection_outputs(&flat, input_rows, &concat.row_counts).map(Some)
}

fn quantized_matmul_tensor(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
) -> Result<Vec<f32>> {
    let weight_entry = weights.tensor(weight_name)?;
    if weight_entry.shape.len() != 2 {
        return Err(format!(
            "quantized matmul expects rank-2 tensor, got {:?} for {}",
            weight_entry.shape, weight_name
        ));
    }
    let params = weights.quant_params_for_weight(weight_name);
    let bits = params.bits;
    let group_size = params.group_size as u64;
    if bits == 0 || bits > 8 {
        return Err(format!(
            "unsupported quant bits {} for {}",
            bits, weight_name
        ));
    }

    let scales_entry = weights.tensor(scales_name)?;
    let biases_entry = weights.tensor(biases_name)?;
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "scale/bias shape mismatch for {}: {:?} vs {:?}",
            weight_name, scales_entry.shape, biases_entry.shape
        ));
    }
    let inner_dim = scales_entry.shape[1]
        .checked_mul(group_size)
        .ok_or_else(|| format!("inner dim overflow for {}", weight_name))?;
    if input_words.len() as u64 != inner_dim {
        return Err(format!(
            "quantized input mismatch for {}: got {} expected {}",
            weight_name,
            input_words.len(),
            inner_dim
        ));
    }
    let packed_bits_per_group = group_size
        .checked_mul(u64::from(bits))
        .ok_or_else(|| format!("packed bit count overflow for {}", weight_name))?;
    let words_per_group = packed_bits_per_group.div_ceil(32);
    if words_per_group == 0 || weight_entry.shape[1] != scales_entry.shape[1] * words_per_group {
        return Err(format!("invalid words_per_group for {}", weight_name));
    }

    let root = weights.model.root_dir.to_string_lossy();
    let weight_key = format!("{root}:{weight_name}");
    let scales_key = format!("{root}:{scales_name}");
    let biases_key = format!("{root}:{biases_name}");
    if matches!(bits, 4 | 8) {
        if let Some(result) = try_affine_quantized_matmul_bf16(
            AffineQuantizedMatmulSpec {
                input_bf16_words: input_words,
                out_rows: weight_entry.shape[0] as usize,
                weight_words_per_row: weight_entry.shape[1] as usize,
                qparams_per_row: scales_entry.shape[1] as usize,
                bits,
                group_size,
                cache_namespace: root.as_ref(),
            },
            &weight_key,
            &scales_key,
            &biases_key,
            || weights.read_tensor_bytes(weight_name),
            || weights.read_tensor_bytes(scales_name),
            || weights.read_tensor_bytes(biases_name),
        ) {
            return result;
        }
    }

    if matches!(bits, 1..=8) && !matches!(bits, 4 | 8) {
        let dequantized = weights.dequantized_bf16_tensor_words_cached(
            weight_name,
            scales_name,
            biases_name,
            bits,
            params.group_size,
        )?;
        if let Some(out) = try_matmul_nt_ggml_bytes(
            &input_words
                .iter()
                .copied()
                .map(bf16_word_to_f32)
                .collect::<Vec<_>>(),
            bf16_words_as_bytes(dequantized.as_slice()),
            GGML_TYPE_BF16,
            1,
            inner_dim as usize,
            weight_entry.shape[0] as usize,
        ) {
            return Ok(out);
        }
    }

    let packed_weights = weights
        .header_for_tensor(weight_name)?
        .read_u32_tensor_words(weight_name)
        .map_err(|err| err.to_string())?;
    let scales = weights.read_bf16_tensor_words(scales_name)?;
    let biases = weights.read_bf16_tensor_words(biases_name)?;
    let input = input_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    let rows = weight_entry.shape[0] as usize;
    let weight_stride = weight_entry.shape[1] as usize;
    let groups_per_row = scales_entry.shape[1] as usize;
    let mask = (1u32 << bits) - 1;
    let mut out = Vec::with_capacity(rows);
    for row in 0..rows {
        let weight_row_start = row * weight_stride;
        let qparam_row_start = row * groups_per_row;
        let mut total = 0.0f32;
        for group in 0..groups_per_row {
            let scale = bf16_word_to_f32(scales[qparam_row_start + group]);
            let bias = bf16_word_to_f32(biases[qparam_row_start + group]);
            let group_start = weight_row_start + group * words_per_group as usize;
            let group_end = group_start + words_per_group as usize;
            let mut group_sum = 0.0f32;
            let mut group_accum = 0.0f32;
            let input_group_base = group * group_size as usize;
            for group_index in 0..group_size as usize {
                let q = unpack_affine_value(
                    &packed_weights[group_start..group_end],
                    bits,
                    mask,
                    group_index,
                ) as f32;
                let x = input[input_group_base + group_index];
                group_sum += x;
                group_accum += x * q;
            }
            total += scale * group_accum + bias * group_sum;
        }
        out.push(total);
    }
    Ok(out)
}

fn quantized_matmul_tensor_top1(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
) -> Result<Option<u32>> {
    let weight_entry = weights.tensor(weight_name)?;
    if weight_entry.shape.len() != 2 {
        return Err(format!(
            "quantized matmul expects rank-2 tensor, got {:?} for {}",
            weight_entry.shape, weight_name
        ));
    }
    let params = weights.quant_params_for_weight(weight_name);
    let bits = params.bits;
    let group_size = params.group_size as u64;
    if bits == 0 || bits > 8 {
        return Err(format!(
            "unsupported quant bits {} for {}",
            bits, weight_name
        ));
    }

    let scales_entry = weights.tensor(scales_name)?;
    let biases_entry = weights.tensor(biases_name)?;
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "scale/bias shape mismatch for {}: {:?} vs {:?}",
            weight_name, scales_entry.shape, biases_entry.shape
        ));
    }
    let inner_dim = scales_entry.shape[1]
        .checked_mul(group_size)
        .ok_or_else(|| format!("inner dim overflow for {}", weight_name))?;
    if input_words.len() as u64 != inner_dim {
        return Err(format!(
            "quantized input mismatch for {}: got {} expected {}",
            weight_name,
            input_words.len(),
            inner_dim
        ));
    }

    let root = weights.model.root_dir.to_string_lossy();
    let weight_key = format!("{root}:{weight_name}");
    let scales_key = format!("{root}:{scales_name}");
    let biases_key = format!("{root}:{biases_name}");
    if matches!(bits, 4 | 8) {
        if let Some(result) = try_affine_quantized_matmul_bf16_top1(
            AffineQuantizedMatmulSpec {
                input_bf16_words: input_words,
                out_rows: weight_entry.shape[0] as usize,
                weight_words_per_row: weight_entry.shape[1] as usize,
                qparams_per_row: scales_entry.shape[1] as usize,
                bits,
                group_size,
                cache_namespace: root.as_ref(),
            },
            &weight_key,
            &scales_key,
            &biases_key,
            || weights.read_tensor_bytes(weight_name),
            || weights.read_tensor_bytes(scales_name),
            || weights.read_tensor_bytes(biases_name),
        ) {
            return result.map(Some);
        }
    }
    Ok(None)
}

fn quantized_matmul_tensor_rows(
    weights: &QwenMlxWeights,
    input_words: &[u16],
    input_rows: usize,
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
) -> Result<Vec<f32>> {
    if input_rows == 0 {
        return Ok(Vec::new());
    }
    let weight_entry = weights.tensor(weight_name)?;
    if weight_entry.shape.len() != 2 {
        return Err(format!(
            "quantized matmul expects rank-2 tensor, got {:?} for {}",
            weight_entry.shape, weight_name
        ));
    }
    let params = weights.quant_params_for_weight(weight_name);
    let bits = params.bits;
    let group_size = params.group_size as u64;
    if bits == 0 || bits > 8 {
        return Err(format!(
            "unsupported quant bits {} for {}",
            bits, weight_name
        ));
    }

    let scales_entry = weights.tensor(scales_name)?;
    let biases_entry = weights.tensor(biases_name)?;
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "scale/bias shape mismatch for {}: {:?} vs {:?}",
            weight_name, scales_entry.shape, biases_entry.shape
        ));
    }
    let inner_dim = scales_entry.shape[1]
        .checked_mul(group_size)
        .ok_or_else(|| format!("inner dim overflow for {}", weight_name))?
        as usize;
    if input_words.len()
        != inner_dim
            .checked_mul(input_rows)
            .ok_or_else(|| format!("quantized batched input size overflow for {}", weight_name))?
    {
        return Err(format!(
            "quantized input mismatch for {}: got {} expected {}",
            weight_name,
            input_words.len(),
            inner_dim * input_rows
        ));
    }

    let root = weights.model.root_dir.to_string_lossy();
    let weight_key = format!("{root}:{weight_name}");
    let scales_key = format!("{root}:{scales_name}");
    let biases_key = format!("{root}:{biases_name}");
    if matches!(bits, 4 | 8) {
        if let Some(result) = try_affine_quantized_matmul_bf16_rows(
            AffineQuantizedMatmulRowsSpec {
                input_bf16_words: input_words,
                input_rows,
                out_rows: weight_entry.shape[0] as usize,
                weight_words_per_row: weight_entry.shape[1] as usize,
                qparams_per_row: scales_entry.shape[1] as usize,
                bits,
                group_size,
                cache_namespace: root.as_ref(),
            },
            &weight_key,
            &scales_key,
            &biases_key,
            || weights.read_tensor_bytes(weight_name),
            || weights.read_tensor_bytes(scales_name),
            || weights.read_tensor_bytes(biases_name),
        ) {
            return result;
        }
    }

    if matches!(bits, 1..=8) && !matches!(bits, 4 | 8) {
        let dequantized = weights.dequantized_bf16_tensor_words_cached(
            weight_name,
            scales_name,
            biases_name,
            bits,
            params.group_size,
        )?;
        if let Some(out) = try_matmul_nt_ggml_bytes(
            &bf16_words_to_f32s(input_words),
            bf16_words_as_bytes(dequantized.as_slice()),
            GGML_TYPE_BF16,
            input_rows,
            inner_dim,
            weight_entry.shape[0] as usize,
        ) {
            return Ok(out);
        }
    }

    let mut out = Vec::with_capacity(
        input_rows
            .checked_mul(weight_entry.shape[0] as usize)
            .ok_or_else(|| format!("quantized batched output size overflow for {}", weight_name))?,
    );
    for row_idx in 0..input_rows {
        let input_start = row_idx * inner_dim;
        let input_end = input_start + inner_dim;
        out.extend(quantized_matmul_tensor(
            weights,
            &input_words[input_start..input_end],
            weight_name,
            scales_name,
            biases_name,
        )?);
    }
    Ok(out)
}

fn split_batched_projection_outputs(
    flat: &[f32],
    input_rows: usize,
    row_counts: &[usize],
) -> Result<Vec<Vec<f32>>> {
    let total_rows = row_counts.iter().copied().sum::<usize>();
    if flat.len()
        != input_rows
            .checked_mul(total_rows)
            .ok_or_else(|| "batched projection size overflow".to_string())?
    {
        return Err(format!(
            "batched projection output length mismatch: got {} expected {}",
            flat.len(),
            input_rows * total_rows
        ));
    }
    let mut outputs = row_counts
        .iter()
        .copied()
        .map(|rows| Vec::with_capacity(input_rows * rows))
        .collect::<Vec<_>>();
    for input_row in 0..input_rows {
        let mut offset = input_row * total_rows;
        for (output, &rows) in outputs.iter_mut().zip(row_counts.iter()) {
            output.extend_from_slice(&flat[offset..offset + rows]);
            offset += rows;
        }
    }
    Ok(outputs)
}

fn metal_decode_tail_enabled() -> bool {
    ENABLE_METAL_DECODE_TAIL && env::var_os("QWEN_DISABLE_METAL_DECODE_TAIL").is_none()
}

fn metal_decode_chain_enabled() -> bool {
    metal_decode_tail_enabled()
        && ENABLE_METAL_DECODE_CHAIN
        && env::var_os("QWEN_DISABLE_METAL_DECODE_CHAIN").is_none()
}

fn metal_decode_chain_active_fusion_enabled() -> bool {
    env::var_os("QWEN_ENABLE_ACTIVE_CHAIN_FUSION").is_some()
}

fn metal_recurrent_merged_input_enabled() -> bool {
    env::var_os("QWEN_ENABLE_MERGED_RECURRENT_INPUT").is_some()
}

fn metal_attention_prefill_cache_enabled() -> bool {
    env::var_os("QWEN_ENABLE_METAL_ATTENTION_PREFILL_CACHE").is_some()
}

fn qwen_debug_chain_enabled() -> bool {
    env::var_os("QWEN_DEBUG_CHAIN").is_some()
}

fn maybe_affine_qmv_layout(
    weights: &QwenMlxWeights,
    weight_name: &str,
) -> Result<Option<MlxAffineQmvLayout>> {
    let entry = weights.tensor(weight_name)?;
    if entry.shape.len() != 2 || entry.dtype != MlxDType::U32 {
        return Ok(None);
    }
    let params = weights.quant_params_for_weight(weight_name);
    if !matches!(params.bits, 4 | 5 | 8) {
        return Ok(None);
    }
    let base = strip_quant_suffix(weight_name);
    let scales_entry = weights.tensor(&format!("{base}.scales"))?;
    Ok(Some(MlxAffineQmvLayout {
        bits: params.bits,
        weight_words_per_row: entry.shape[1] as usize,
        qparams_per_row: scales_entry.shape[1] as usize,
        out_rows: entry.shape[0] as usize,
    }))
}

fn maybe_affine_concat_qmv_layout(
    weights: &QwenMlxWeights,
    weight_names: &[&str],
) -> Result<Option<(MlxAffineQmvLayout, Vec<usize>)>> {
    if weight_names.is_empty() {
        return Ok(None);
    }
    let mut bits = None::<u32>;
    let mut group_size = None::<u64>;
    for &weight_name in weight_names {
        let entry = weights.tensor(weight_name)?;
        if entry.shape.len() != 2 || entry.dtype != MlxDType::U32 {
            return Ok(None);
        }
        let params = weights.quant_params_for_weight(weight_name);
        let this_group_size = params.group_size as u64;
        if !matches!(params.bits, 4 | 5 | 8) {
            return Ok(None);
        }
        if let Some(expected) = bits {
            if params.bits != expected {
                return Ok(None);
            }
        } else {
            bits = Some(params.bits);
        }
        if let Some(expected) = group_size {
            if this_group_size != expected {
                return Ok(None);
            }
        } else {
            group_size = Some(this_group_size);
        }
    }

    let concat = weights.concat_affine_tensor_cached(weight_names)?;
    Ok(Some((
        MlxAffineQmvLayout {
            bits: concat.bits,
            weight_words_per_row: concat.weight_words_per_row,
            qparams_per_row: concat.qparams_per_row,
            out_rows: concat.total_rows,
        },
        concat.row_counts.clone(),
    )))
}

fn affine_qmv_pipeline_name(bits: u32) -> Result<&'static str> {
    match bits {
        4 => Ok("kernel_mlx_affine_qmv_row_bf16"),
        5 => Ok("kernel_mlx_affine_qmv_row_bf16_q5"),
        8 => Ok("kernel_mlx_affine_qmv_row_bf16_q8"),
        _ => Err(format!(
            "no decode tail affine qmv pipeline for {bits}-bit weights"
        )),
    }
}

fn compile_default_pipeline(runtime: &MetalRuntime, name: &str) -> Result<MetalPipeline> {
    runtime
        .get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: name.to_string(),
            base_name: name.to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })
        .map_err(|err| format!("compile pipeline {name} failed: {err}"))
}

fn mlx_norm_threads_per_threadgroup(
    n: usize,
    max_threads_per_threadgroup: u64,
) -> Result<MetalSize> {
    const MLX_NORM_N_READS: usize = 4;
    const MLX_SIMD_WIDTH: usize = 32;

    let max_threads = usize::try_from(max_threads_per_threadgroup)
        .map_err(|_| "norm max_threads_per_threadgroup does not fit in usize".to_string())?;
    let max_simd_groups = (max_threads / MLX_SIMD_WIDTH).max(1);
    let needed_threads = n.max(1).div_ceil(MLX_NORM_N_READS);
    let needed_simd_groups = needed_threads.div_ceil(MLX_SIMD_WIDTH).max(1);
    let simd_groups = needed_simd_groups.min(max_simd_groups);
    let threadgroup_width = MLX_SIMD_WIDTH
        .checked_mul(simd_groups)
        .ok_or_else(|| "norm threadgroup size overflow".to_string())?;
    Ok(MetalSize {
        width: u64::try_from(threadgroup_width)
            .map_err(|_| "norm threadgroup width does not fit in u64".to_string())?,
        height: 1,
        depth: 1,
    })
}

fn dispatch_compute_tracked_split<const N: usize>(
    runtime: &MetalRuntime,
    pipeline: &MetalPipeline,
    args_bytes: &[u8],
    bindings: [MetalBufferBindingRef<'_>; N],
    output_start: usize,
    threadgroup_memory_lengths: &[(u64, usize)],
    threadgroups: MetalSize,
    threads_per_threadgroup: MetalSize,
) -> Result<()> {
    runtime
        .dispatch_compute_tracked(
            pipeline,
            args_bytes,
            &bindings[..output_start],
            &bindings[output_start..],
            threadgroup_memory_lengths,
            threadgroups,
            threads_per_threadgroup,
        )
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn dispatch_compute_untracked<const N: usize>(
    runtime: &MetalRuntime,
    pipeline: &MetalPipeline,
    args_bytes: &[u8],
    bindings: [MetalBufferBindingRef<'_>; N],
    threadgroup_memory_lengths: &[(u64, usize)],
    threadgroups: MetalSize,
    threads_per_threadgroup: MetalSize,
) -> Result<()> {
    runtime
        .dispatch_compute(
            pipeline,
            args_bytes,
            &bindings,
            threadgroup_memory_lengths,
            threadgroups,
            threads_per_threadgroup,
        )
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
    }
}

fn unpack_affine_value(group_words: &[u32], bits: u32, mask: u32, index: usize) -> u32 {
    let bit_index = index * bits as usize;
    let word_index = bit_index / 32;
    let bit_offset = bit_index % 32;
    let mut value = group_words[word_index] >> bit_offset;
    let spill = bit_offset + bits as usize;
    if spill > 32 {
        value |= group_words[word_index + 1] << (32 - bit_offset);
    }
    value & mask
}

fn decode_tensor_bytes_to_f32(bytes: &[u8], dtype: MlxDType) -> Result<Vec<f32>> {
    match dtype {
        MlxDType::BF16 => {
            if bytes.len() % 2 != 0 {
                return Err("BF16 byte slice length is not divisible by 2".to_string());
            }
            let mut out = Vec::with_capacity(bytes.len() / 2);
            for chunk in bytes.chunks_exact(2) {
                out.push(bf16_word_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])));
            }
            Ok(out)
        }
        MlxDType::F32 => {
            if bytes.len() % 4 != 0 {
                return Err("F32 byte slice length is not divisible by 4".to_string());
            }
            let mut out = Vec::with_capacity(bytes.len() / 4);
            for chunk in bytes.chunks_exact(4) {
                out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
            Ok(out)
        }
        other => Err(format!("unsupported dense decode dtype {:?}", other)),
    }
}

fn dequantize_affine_row(
    packed: &[u32],
    scales: &[u16],
    biases: &[u16],
    bits: u32,
    group_size: u32,
) -> Result<Vec<f32>> {
    if scales.len() != biases.len() {
        return Err(format!(
            "scale/bias length mismatch: {} vs {}",
            scales.len(),
            biases.len()
        ));
    }
    let packed_bits_per_group = u64::from(group_size)
        .checked_mul(u64::from(bits))
        .ok_or_else(|| "packed bit count overflow".to_string())?;
    let words_per_group = packed_bits_per_group.div_ceil(32) as usize;
    if words_per_group == 0 || packed.len() != scales.len() * words_per_group {
        return Err("invalid affine row layout".to_string());
    }
    let mask = (1u32 << bits) - 1;
    let mut out = Vec::with_capacity(scales.len() * group_size as usize);
    for group_idx in 0..scales.len() {
        let scale = bf16_word_to_f32(scales[group_idx]);
        let bias = bf16_word_to_f32(biases[group_idx]);
        let group_start = group_idx * words_per_group;
        let group_end = group_start + words_per_group;
        for group_index in 0..group_size as usize {
            let q = unpack_affine_value(&packed[group_start..group_end], bits, mask, group_index)
                as f32;
            out.push(scale * q + bias);
        }
    }
    Ok(out)
}

fn u64_to_i64(value: u64, what: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| format!("{what} {} does not fit in i64", value))
}

fn load_f32_tensor_1d(
    ctx: &mut Context,
    tensor_ids: &mut BTreeMap<String, TensorId>,
    name: &str,
    values: &[f32],
    usage: BufferUsage,
) -> Result<()> {
    let tensor = ctx.new_named_tensor(name, TensorType::F32, 1, &[values.len() as i64], usage)?;
    ctx.write_tensor_data(tensor, f32s_as_bytes(values))?;
    tensor_ids.insert(name.to_owned(), tensor);
    Ok(())
}

fn load_bf16_tensor_rank2_from_mlx(
    ctx: &mut Context,
    tensor_ids: &mut BTreeMap<String, TensorId>,
    entry: &MlxTensorEntry,
    name: &str,
    bytes: &[u8],
    usage: BufferUsage,
) -> Result<()> {
    if entry.dtype != MlxDType::BF16 || entry.shape.len() != 2 {
        return Err(format!(
            "expected rank-2 BF16 tensor for {}, got {:?} {:?}",
            name, entry.dtype, entry.shape
        ));
    }
    let ne0 = u64_to_i64(entry.shape[1], &format!("{name} dim1"))?;
    let ne1 = u64_to_i64(entry.shape[0], &format!("{name} dim0"))?;
    let tensor = ctx.new_named_tensor(name, TensorType::BF16, 2, &[ne0, ne1], usage)?;
    ctx.write_tensor_data(tensor, bytes)?;
    tensor_ids.insert(name.to_owned(), tensor);
    Ok(())
}

fn load_bf16_tensor_rank2_words(
    ctx: &mut Context,
    tensor_ids: &mut BTreeMap<String, TensorId>,
    name: &str,
    row_count: usize,
    inner_dim: usize,
    words: &[u16],
    usage: BufferUsage,
) -> Result<()> {
    let expected_len = row_count
        .checked_mul(inner_dim)
        .ok_or_else(|| format!("rank-2 BF16 tensor length overflow for {}", name))?;
    if words.len() != expected_len {
        return Err(format!(
            "rank-2 BF16 tensor length mismatch for {}: got {} expected {}",
            name,
            words.len(),
            expected_len
        ));
    }
    let tensor = ctx.new_named_tensor(
        name,
        TensorType::BF16,
        2,
        &[inner_dim as i64, row_count as i64],
        usage,
    )?;
    ctx.write_tensor_data(tensor, bf16_words_as_bytes(words))?;
    tensor_ids.insert(name.to_owned(), tensor);
    Ok(())
}

fn load_f32_conv1d_kernel_from_mlx(
    ctx: &mut Context,
    tensor_ids: &mut BTreeMap<String, TensorId>,
    entry: &MlxTensorEntry,
    name: &str,
    values: &[f32],
    usage: BufferUsage,
) -> Result<()> {
    if entry.shape.len() != 3 || entry.shape[2] != 1 {
        return Err(format!(
            "expected rank-3 conv kernel with trailing dim 1 for {}, got {:?} {:?}",
            name, entry.dtype, entry.shape
        ));
    }
    let kernel_size = u64_to_i64(entry.shape[1], &format!("{name} kernel"))?;
    let channel_count = u64_to_i64(entry.shape[0], &format!("{name} channels"))?;
    let tensor = ctx.new_named_tensor(
        name,
        TensorType::F32,
        2,
        &[kernel_size, channel_count],
        usage,
    )?;
    ctx.write_tensor_data(tensor, f32s_as_bytes(values))?;
    tensor_ids.insert(name.to_owned(), tensor);
    Ok(())
}

fn recurrent_qkv_dim(config: &QwenConfig) -> usize {
    config.text.linear_key_head_dim * config.text.linear_num_key_heads * 2
        + config.text.linear_value_head_dim * config.text.linear_num_value_heads
}

fn rms_norm_rows_weighted_f32(
    input: &[f32],
    row_count: usize,
    row_len: usize,
    weights: &[u16],
    eps: f32,
) -> Result<Vec<f32>> {
    if weights.len() != row_len {
        return Err(format!(
            "row RMS weight length mismatch: got {} expected {}",
            weights.len(),
            row_len
        ));
    }
    let weight_f32 = weights
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    rms_norm_rows_weighted_f32_from_f32(input, row_count, row_len, &weight_f32, eps)
}

fn rms_norm_rows_weighted_f32_from_f32(
    input: &[f32],
    row_count: usize,
    row_len: usize,
    weights: &[f32],
    eps: f32,
) -> Result<Vec<f32>> {
    if input.len() != row_count * row_len {
        return Err(format!(
            "row RMS input length mismatch: got {} expected {}",
            input.len(),
            row_count * row_len
        ));
    }
    if weights.len() != row_len {
        return Err(format!(
            "row RMS weight length mismatch: got {} expected {}",
            weights.len(),
            row_len
        ));
    }
    let mut out = Vec::with_capacity(input.len());
    for row_idx in 0..row_count {
        let row = &input[row_idx * row_len..(row_idx + 1) * row_len];
        let inv_rms = inv_rms(row, eps);
        for dim in 0..row_len {
            out.push(row[dim] * inv_rms * weights[dim]);
        }
    }
    Ok(out)
}

fn rms_norm_single_row_weighted_f32(input: &[f32], weights: &[f32], eps: f32) -> Result<Vec<f32>> {
    rms_norm_rows_weighted_f32_from_f32(input, 1, input.len(), weights, eps)
}

fn l2_norm_rows_f32(input: &[f32], row_count: usize, row_len: usize, eps: f32) -> Result<Vec<f32>> {
    if input.len() != row_count * row_len {
        return Err(format!(
            "L2 norm input length mismatch: got {} expected {}",
            input.len(),
            row_count * row_len
        ));
    }
    let mut out = Vec::with_capacity(input.len());
    for row_idx in 0..row_count {
        let row = &input[row_idx * row_len..(row_idx + 1) * row_len];
        let norm = row.iter().map(|value| value * value).sum::<f32>().sqrt() + eps;
        for &value in row {
            out.push(value / norm);
        }
    }
    Ok(out)
}

fn apply_rope_interleaved_rows_in_place(
    values: &mut [f32],
    row_count: usize,
    head_dim: usize,
    rotary_dim: usize,
    base: f32,
    position: usize,
) -> Result<()> {
    if values.len() != row_count * head_dim {
        return Err(format!(
            "rope input length mismatch: got {} expected {}",
            values.len(),
            row_count * head_dim
        ));
    }
    if rotary_dim == 0 {
        return Ok(());
    }
    if rotary_dim > head_dim || rotary_dim % 2 != 0 {
        return Err(format!(
            "invalid rotary dim {} for head_dim {}",
            rotary_dim, head_dim
        ));
    }
    for row_idx in 0..row_count {
        let row = &mut values[row_idx * head_dim..(row_idx + 1) * head_dim];
        for pair_idx in 0..(rotary_dim / 2) {
            let even = pair_idx * 2;
            let odd = even + 1;
            let exponent = even as f32 / rotary_dim as f32;
            let inv_freq = base.powf(-exponent);
            let theta = position as f32 * inv_freq;
            let cos_theta = theta.cos();
            let sin_theta = theta.sin();
            let left = row[even];
            let right = row[odd];
            row[even] = left * cos_theta - right * sin_theta;
            row[odd] = left * sin_theta + right * cos_theta;
        }
    }
    Ok(())
}

fn apply_rope_interleaved_token_head_rows_in_place(
    values: &mut [f32],
    token_count: usize,
    heads_per_token: usize,
    head_dim: usize,
    rotary_dim: usize,
    base: f32,
    start_position: usize,
) -> Result<()> {
    if values.len()
        != token_count
            .checked_mul(heads_per_token)
            .and_then(|rows| rows.checked_mul(head_dim))
            .ok_or_else(|| "rope token-head input size overflow".to_string())?
    {
        return Err(format!(
            "rope token-head input length mismatch: got {} expected {}",
            values.len(),
            token_count * heads_per_token * head_dim
        ));
    }
    if rotary_dim == 0 {
        return Ok(());
    }
    if rotary_dim > head_dim || rotary_dim % 2 != 0 {
        return Err(format!(
            "invalid rotary dim {} for head_dim {}",
            rotary_dim, head_dim
        ));
    }
    for token_idx in 0..token_count {
        let position = start_position
            .checked_add(token_idx)
            .ok_or_else(|| "rope position overflow".to_string())?;
        let token_start = token_idx * heads_per_token * head_dim;
        let token_end = token_start + heads_per_token * head_dim;
        apply_rope_interleaved_rows_in_place(
            &mut values[token_start..token_end],
            heads_per_token,
            head_dim,
            rotary_dim,
            base,
            position,
        )?;
    }
    Ok(())
}

fn add_bf16_and_f32(left: &[u16], right: &[f32]) -> Result<Vec<f32>> {
    if left.len() != right.len() {
        return Err(format!(
            "add length mismatch: left={} right={}",
            left.len(),
            right.len()
        ));
    }
    Ok(left
        .iter()
        .copied()
        .zip(right.iter().copied())
        .map(|(left, right)| bf16_word_to_f32(left) + right)
        .collect())
}

fn add_f32(left: &[f32], right: &[f32]) -> Result<Vec<f32>> {
    if left.len() != right.len() {
        return Err(format!(
            "add length mismatch: left={} right={}",
            left.len(),
            right.len()
        ));
    }
    Ok(left
        .iter()
        .copied()
        .zip(right.iter().copied())
        .map(|(left, right)| left + right)
        .collect())
}

fn softmax(values: &[f32]) -> Vec<f32> {
    let max_value = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps = values
        .iter()
        .copied()
        .map(|value| (value - max_value).exp())
        .collect::<Vec<_>>();
    let sum = exps.iter().copied().sum::<f32>();
    exps.into_iter().map(|value| value / sum).collect()
}

fn sigmoid(value: f32) -> f32 {
    1.0 / (1.0 + (-value).exp())
}

fn silu(value: f32) -> f32 {
    value * sigmoid(value)
}

fn softplus(value: f32) -> f32 {
    if value > 20.0 {
        value
    } else {
        (1.0 + value.exp()).ln()
    }
}

fn inv_rms(values: &[f32], eps: f32) -> f32 {
    let mean_square = values.iter().map(|value| value * value).sum::<f32>() / values.len() as f32;
    1.0 / (mean_square + eps).sqrt()
}

fn bf16_words_to_f32s(words: &[u16]) -> Vec<f32> {
    words.iter().copied().map(bf16_word_to_f32).collect()
}

fn f32s_to_bf16_words(values: &[f32]) -> Vec<u16> {
    values.iter().copied().map(f32_to_bf16_word).collect()
}

fn f32s_as_bytes(values: &[f32]) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        std::slice::from_raw_parts(
            values.as_ptr().cast::<u8>(),
            values.len() * std::mem::size_of::<f32>(),
        )
    }
    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("f32 byte reinterpret currently assumes little-endian targets")
    }
}

fn i32s_as_bytes(values: &[i32]) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        std::slice::from_raw_parts(
            values.as_ptr().cast::<u8>(),
            values.len() * std::mem::size_of::<i32>(),
        )
    }
    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("i32 byte reinterpret currently assumes little-endian targets")
    }
}

fn bf16_words_as_bytes(words: &[u16]) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        std::slice::from_raw_parts(
            words.as_ptr().cast::<u8>(),
            words.len() * std::mem::size_of::<u16>(),
        )
    }
    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("bf16 byte reinterpret currently assumes little-endian targets")
    }
}

fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn f32_to_bf16_word(value: f32) -> u16 {
    let bits = value.to_bits();
    let lsb = (bits >> 16) & 1;
    ((bits.wrapping_add(0x7FFF + lsb) & 0xFFFF_0000) >> 16) as u16
}

fn strip_quant_suffix(weight_name: &str) -> &str {
    weight_name
        .strip_suffix(".weight")
        .or_else(|| weight_name.strip_suffix(".scales"))
        .or_else(|| weight_name.strip_suffix(".biases"))
        .unwrap_or(weight_name)
}

fn parse_qwen_config(path: &Path, root: &HashMap<String, JsonValue>) -> Result<QwenConfig> {
    let model_type = json_required_string(path, "model_type", root.get("model_type"))?;
    let quant_obj = json_required_object(path, "quantization", root.get("quantization"))?;
    let default_quant = QuantParams {
        group_size: json_required_u32(
            path,
            "quantization.group_size",
            quant_obj.get("group_size"),
        )?,
        bits: json_required_u32(path, "quantization.bits", quant_obj.get("bits"))?,
    };
    let quantization_mode = json_required_string(path, "quantization.mode", quant_obj.get("mode"))?;
    let mut quant_overrides = HashMap::new();
    for (name, value) in quant_obj {
        if matches!(name.as_str(), "group_size" | "bits" | "mode") {
            continue;
        }
        let override_obj = json_required_object(path, name, Some(value))?;
        quant_overrides.insert(
            name.clone(),
            QuantParams {
                group_size: json_required_u32(
                    path,
                    &format!("{name}.group_size"),
                    override_obj.get("group_size"),
                )?,
                bits: json_required_u32(path, &format!("{name}.bits"), override_obj.get("bits"))?,
            },
        );
    }

    let text_obj = json_required_object(path, "text_config", root.get("text_config"))?;
    let layer_types =
        json_required_string_array(path, "text_config.layer_types", text_obj.get("layer_types"))?;
    let rope_obj = json_required_object(
        path,
        "text_config.rope_parameters",
        text_obj.get("rope_parameters"),
    )?;
    let bos_token_id = json_required_u32(
        path,
        "text_config.bos_token_id",
        text_obj.get("bos_token_id"),
    )?;
    let eos_token_ids = json_u32_list(path, "eos_token_id", root.get("eos_token_id"))?;
    let partial_rotary_factor = json_required_f32(
        path,
        "text_config.partial_rotary_factor",
        text_obj.get("partial_rotary_factor"),
    )?;
    let rope_theta = json_required_f32(path, "text_config.rope_theta", rope_obj.get("rope_theta"))?;

    let text = QwenTextConfig {
        _hidden_size: json_required_u32(
            path,
            "text_config.hidden_size",
            text_obj.get("hidden_size"),
        )? as usize,
        _intermediate_size: json_required_u32(
            path,
            "text_config.intermediate_size",
            text_obj.get("intermediate_size"),
        )? as usize,
        num_hidden_layers: json_required_u32(
            path,
            "text_config.num_hidden_layers",
            text_obj.get("num_hidden_layers"),
        )? as usize,
        num_attention_heads: json_required_u32(
            path,
            "text_config.num_attention_heads",
            text_obj.get("num_attention_heads"),
        )? as usize,
        num_key_value_heads: json_required_u32(
            path,
            "text_config.num_key_value_heads",
            text_obj.get("num_key_value_heads"),
        )? as usize,
        head_dim: json_required_u32(path, "text_config.head_dim", text_obj.get("head_dim"))?
            as usize,
        linear_key_head_dim: json_required_u32(
            path,
            "text_config.linear_key_head_dim",
            text_obj.get("linear_key_head_dim"),
        )? as usize,
        linear_num_key_heads: json_required_u32(
            path,
            "text_config.linear_num_key_heads",
            text_obj.get("linear_num_key_heads"),
        )? as usize,
        linear_num_value_heads: json_required_u32(
            path,
            "text_config.linear_num_value_heads",
            text_obj.get("linear_num_value_heads"),
        )? as usize,
        linear_value_head_dim: json_required_u32(
            path,
            "text_config.linear_value_head_dim",
            text_obj.get("linear_value_head_dim"),
        )? as usize,
        linear_conv_kernel_dim: json_required_u32(
            path,
            "text_config.linear_conv_kernel_dim",
            text_obj.get("linear_conv_kernel_dim"),
        )? as usize,
        full_attention_interval: json_required_u32(
            path,
            "text_config.full_attention_interval",
            text_obj.get("full_attention_interval"),
        )? as usize,
        layer_types,
        rms_norm_eps: json_required_f32(
            path,
            "text_config.rms_norm_eps",
            text_obj.get("rms_norm_eps"),
        )?,
        partial_rotary_factor,
        rope_theta,
        _vocab_size: json_required_u32(path, "text_config.vocab_size", text_obj.get("vocab_size"))?
            as usize,
        bos_token_id,
        _eos_token_ids: eos_token_ids,
    };

    if text.num_hidden_layers != text.layer_types.len() {
        return Err(format!(
            "layer type count mismatch: num_hidden_layers={} layer_types={}",
            text.num_hidden_layers,
            text.layer_types.len()
        ));
    }
    if text.full_attention_interval == 0 {
        return Err("full_attention_interval must be greater than zero".to_string());
    }

    Ok(QwenConfig {
        model_type,
        quantization_mode,
        default_quant,
        quant_overrides,
        text,
    })
}

fn json_required_object<'a>(
    path: &Path,
    name: &str,
    value: Option<&'a JsonValue>,
) -> Result<&'a HashMap<String, JsonValue>> {
    match value {
        Some(JsonValue::Object(obj)) => Ok(obj),
        Some(other) => Err(format!(
            "{}: expected object for {}, got {:?}",
            path.display(),
            name,
            other
        )),
        None => Err(format!("{}: missing object {}", path.display(), name)),
    }
}

fn json_required_string(path: &Path, name: &str, value: Option<&JsonValue>) -> Result<String> {
    match value {
        Some(JsonValue::String(string)) => Ok(string.clone()),
        Some(other) => Err(format!(
            "{}: expected string for {}, got {:?}",
            path.display(),
            name,
            other
        )),
        None => Err(format!("{}: missing string {}", path.display(), name)),
    }
}

fn json_required_u32(path: &Path, name: &str, value: Option<&JsonValue>) -> Result<u32> {
    match value {
        Some(JsonValue::U64(number)) => u32::try_from(*number)
            .map_err(|_| format!("{}: {} does not fit in u32", path.display(), name)),
        Some(JsonValue::U128(number)) => u32::try_from(*number)
            .map_err(|_| format!("{}: {} does not fit in u32", path.display(), name)),
        Some(JsonValue::I64(number)) => u32::try_from(*number)
            .map_err(|_| format!("{}: {} must be non-negative u32", path.display(), name)),
        Some(JsonValue::I128(number)) => u32::try_from(*number)
            .map_err(|_| format!("{}: {} must be non-negative u32", path.display(), name)),
        Some(other) => Err(format!(
            "{}: expected integer for {}, got {:?}",
            path.display(),
            name,
            other
        )),
        None => Err(format!("{}: missing integer {}", path.display(), name)),
    }
}

fn json_required_f32(path: &Path, name: &str, value: Option<&JsonValue>) -> Result<f32> {
    match value {
        Some(JsonValue::F64(number)) => Ok(*number as f32),
        Some(JsonValue::U64(number)) => Ok(*number as f32),
        Some(JsonValue::U128(number)) => Ok(*number as f32),
        Some(JsonValue::I64(number)) => Ok(*number as f32),
        Some(JsonValue::I128(number)) => Ok(*number as f32),
        Some(other) => Err(format!(
            "{}: expected float for {}, got {:?}",
            path.display(),
            name,
            other
        )),
        None => Err(format!("{}: missing float {}", path.display(), name)),
    }
}

fn json_required_string_array(
    path: &Path,
    name: &str,
    value: Option<&JsonValue>,
) -> Result<Vec<String>> {
    match value {
        Some(JsonValue::Array(values)) => values
            .iter()
            .enumerate()
            .map(|(index, value)| match value {
                JsonValue::String(string) => Ok(string.clone()),
                other => Err(format!(
                    "{}: expected string at {}[{}], got {:?}",
                    path.display(),
                    name,
                    index,
                    other
                )),
            })
            .collect(),
        Some(other) => Err(format!(
            "{}: expected array for {}, got {:?}",
            path.display(),
            name,
            other
        )),
        None => Err(format!("{}: missing array {}", path.display(), name)),
    }
}

fn json_u32_list(path: &Path, name: &str, value: Option<&JsonValue>) -> Result<Vec<u32>> {
    match value {
        Some(JsonValue::Array(values)) => values
            .iter()
            .enumerate()
            .map(|(index, value)| json_required_u32(path, &format!("{name}[{index}]"), Some(value)))
            .collect(),
        Some(other) => Ok(vec![json_required_u32(path, name, Some(other))?]),
        None => Err(format!("{}: missing token list {}", path.display(), name)),
    }
}
