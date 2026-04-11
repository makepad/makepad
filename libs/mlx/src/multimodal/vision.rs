use crate::{MlxIndexedSafetensors, MlxRtError, Result};
use crate::multimodal::GemmaImagePixels;
use crate::text_runtime::try_affine_quantized_matmul_rows_metal;
use makepad_ggml::backend::metal::{
    try_add_f32, try_flash_attn_f32_packed, try_gelu_f32, try_matmul_nt_ggml_bytes,
    try_vision_mlp_bf16_fused,
    try_mul_f32, try_rms_norm_f32, try_rms_norm_mul_f32, BufferStorageMode, MetalBuffer,
    MetalBufferBindingRef, MetalPipeline, MetalPipelineDescriptor, MetalRuntime, MetalSize,
};
use makepad_ggml::quant::GGML_TYPE_BF16;
use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::size_of;
use std::path::PathBuf;
use std::slice;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct GemmaVisionProfile {
    pub patch_embed: Duration,
    pub layers_total: Duration,
    pub pool: Duration,
    pub standardize: Duration,
    pub project: Duration,
    pub layer_input_norm: Duration,
    pub layer_qkv_proj: Duration,
    pub layer_qk_norm_v_norm: Duration,
    pub layer_rope: Duration,
    pub layer_attention: Duration,
    pub layer_o_proj_post_norm_residual: Duration,
    pub layer_pre_ffn_norm: Duration,
    pub layer_gate_up_proj: Duration,
    pub layer_geglu: Duration,
    pub layer_down_proj_post_norm_residual: Duration,
    pub layer_fused_mlp: Duration,
    pub flash_attn_successes: u64,
    pub flash_attn_fallbacks: u64,
}

#[derive(Clone)]
pub struct GemmaVisionRuntime {
    weights: MlxIndexedSafetensors,
    tensor_bytes_cache: HashMap<String, Arc<Vec<u8>>>,
    packed_tensor_bytes_cache: HashMap<String, Arc<Vec<u8>>>,
    bf16_tensor_cache: HashMap<String, Arc<Vec<u16>>>,
    f32_tensor_cache: HashMap<String, Arc<Vec<f32>>>,
}

impl GemmaVisionRuntime {
    pub fn load(weights: &MlxIndexedSafetensors) -> Self {
        Self {
            weights: weights.clone(),
            tensor_bytes_cache: HashMap::new(),
            packed_tensor_bytes_cache: HashMap::new(),
            bf16_tensor_cache: HashMap::new(),
            f32_tensor_cache: HashMap::new(),
        }
    }

    pub fn encode_image_to_text_embeddings(
        &mut self,
        image: &GemmaImagePixels,
    ) -> Result<Vec<Vec<u16>>> {
        Ok(self.encode_image_to_text_embeddings_profiled(image)?.0)
    }

    pub fn encode_image_to_text_embeddings_profiled(
        &mut self,
        image: &GemmaImagePixels,
    ) -> Result<(Vec<Vec<u16>>, GemmaVisionProfile)> {
        let mut profile = GemmaVisionProfile::default();
        let vision_config = &self.weights.snapshot.config.vision_config;
        let patch_size = vision_config.patch_size as usize;
        let num_hidden_layers = vision_config.num_hidden_layers as usize;
        let standardize = vision_config.standardize;
        let head_dim = vision_config.head_dim as usize;
        let rope_theta = vision_config.rope_parameters.rope_theta;
        let num_real_patches = image
            .patch_grid_width
            .checked_mul(image.patch_grid_height)
            .ok_or_else(|| invalid_model("vision patch count overflow"))?;
        let stage_started = Instant::now();
        let mut hidden = self.patch_embed(image, patch_size, num_real_patches)?;
        profile.patch_embed = stage_started.elapsed();
        let patch_positions = patch_positions(image.patch_grid_width, image.patch_grid_height);
        let rope_cache = Rope2DCache::new(&patch_positions, head_dim, rope_theta);
        let layers_started = Instant::now();
        for layer_idx in 0..num_hidden_layers {
            hidden = self.apply_vision_layer(
                layer_idx,
                &hidden,
                &rope_cache,
                &mut profile,
            )?;
        }
        profile.layers_total = layers_started.elapsed();
        let pool_started = Instant::now();
        let pooled = self.pool_hidden_states(&hidden, &patch_positions)?;
        profile.pool = pool_started.elapsed();
        if pooled.rows != image.soft_token_count {
            return Err(invalid_model(format!(
                "vision pooler token count mismatch: expected {} got {}",
                image.soft_token_count,
                pooled.rows
            )));
        }
        let standardized = if standardize {
            let standardize_started = Instant::now();
            let out = self.standardize_hidden_states(&pooled)?;
            profile.standardize = standardize_started.elapsed();
            out
        } else {
            pooled
        };
        let project_started = Instant::now();
        let projected = self.project_soft_tokens_to_text_space(&standardized)?;
        profile.project = project_started.elapsed();
        Ok((projected, profile))
    }

    fn patch_embed(
        &mut self,
        image: &GemmaImagePixels,
        patch_size: usize,
        num_real_patches: usize,
    ) -> Result<RowsTensor> {
        let input_proj_name = "vision_tower.patch_embedder.input_proj.weight";
        let input_proj = self.read_tensor_bytes(input_proj_name)?;
        let input_proj_shape = self.weights.tensor(input_proj_name)?.shape.clone();
        let out_dim = usize::try_from(input_proj_shape[0]).map_err(|_| invalid_model("patch embed out_dim overflow"))?;
        let in_dim = usize::try_from(input_proj_shape[1]).map_err(|_| invalid_model("patch embed in_dim overflow"))?;
        let expected_patch_dim = 3 * patch_size * patch_size;
        if in_dim != expected_patch_dim {
            return Err(invalid_model(format!(
                "patch embed input dim mismatch: expected {} got {}",
                expected_patch_dim, in_dim
            )));
        }

        let pixel_plane_len = image.width * image.height;
        let mut patch_rows = Vec::with_capacity(num_real_patches * in_dim);
        for patch_y in 0..image.patch_grid_height {
            for patch_x in 0..image.patch_grid_width {
                for dy in 0..patch_size {
                    let src_y = patch_y * patch_size + dy;
                    for dx in 0..patch_size {
                        let src_x = patch_x * patch_size + dx;
                        let pixel_index = src_y * image.width + src_x;
                        for channel in 0..3 {
                            let value = image.pixels_chw[channel * pixel_plane_len + pixel_index];
                            patch_rows.push(2.0 * (value - 0.5));
                        }
                    }
                }
            }
        }
        let mut hidden = linear_bf16_rows(
            &RowsTensor::new(num_real_patches, in_dim, patch_rows)?,
            input_proj.as_ref(),
            out_dim,
            in_dim,
        )?;

        let pos_table = self.read_bf16_tensor_words("vision_tower.patch_embedder.position_embedding_table")?;
        let pos_shape = self
            .weights
            .tensor("vision_tower.patch_embedder.position_embedding_table")?
            .shape
            .clone();
        let axes = usize::try_from(pos_shape[0]).map_err(|_| invalid_model("position axes overflow"))?;
        let pos_size = usize::try_from(pos_shape[1]).map_err(|_| invalid_model("position size overflow"))?;
        let hidden_size = usize::try_from(pos_shape[2]).map_err(|_| invalid_model("position hidden overflow"))?;
        if axes != 2 || hidden_size != out_dim {
            return Err(invalid_model("unexpected vision position embedding shape"));
        }
        let positions = patch_positions(image.patch_grid_width, image.patch_grid_height);
        for (row_idx, [x, y]) in positions.iter().copied().enumerate() {
            if x >= pos_size || y >= pos_size {
                return Err(invalid_model(format!(
                    "patch position out of embedding range: ({x},{y}) >= {pos_size}"
                )));
            }
            let x_offset = x * hidden_size;
            let y_offset = (pos_size + y) * hidden_size;
            let row = hidden.row_mut(row_idx);
            for idx in 0..hidden_size {
                row[idx] += bf16_to_f32(pos_table[x_offset + idx]);
                row[idx] += bf16_to_f32(pos_table[y_offset + idx]);
            }
        }
        Ok(hidden)
    }

    fn apply_vision_layer(
        &mut self,
        layer_idx: usize,
        hidden: &RowsTensor,
        rope_cache: &Rope2DCache,
        profile: &mut GemmaVisionProfile,
    ) -> Result<RowsTensor> {
        let base = format!("vision_tower.encoder.layers.{layer_idx}");
        let hidden_size = hidden.cols;
        if hidden.rows == 0 {
            return Err(invalid_model("vision layer received empty hidden state"));
        }
        let num_heads = self.weights.snapshot.config.vision_config.num_attention_heads as usize;
        let num_kv_heads = self.weights.snapshot.config.vision_config.num_key_value_heads as usize;
        let head_dim = self.weights.snapshot.config.vision_config.head_dim as usize;
        let rms_norm_eps = self.weights.snapshot.config.vision_config.rms_norm_eps;
        let stage_started = Instant::now();
        let input_norm_weights = self.read_f32_tensor(&format!("{base}.input_layernorm.weight"))?;
        let normed = rms_norm_weighted_rows(hidden, input_norm_weights.as_ref(), rms_norm_eps)?;
        profile.layer_input_norm += stage_started.elapsed();

        let stage_started = Instant::now();
        let q_weight_name = format!("{base}.self_attn.q_proj.linear.weight");
        let k_weight_name = format!("{base}.self_attn.k_proj.linear.weight");
        let v_weight_name = format!("{base}.self_attn.v_proj.linear.weight");
        let qkv_weight = self.read_packed_tensor_bytes(
            &format!("{base}.self_attn.qkv_proj.linear.weight"),
            &[q_weight_name.as_str(), k_weight_name.as_str(), v_weight_name.as_str()],
        )?;
        let o_weight = self.read_tensor_bytes(&format!("{base}.self_attn.o_proj.linear.weight"))?;
        let q_norm_weights = self.read_f32_tensor(&format!("{base}.self_attn.q_norm.weight"))?;
        let k_norm_weights = self.read_f32_tensor(&format!("{base}.self_attn.k_norm.weight"))?;

        let qkv = linear_bf16_rows(&normed, qkv_weight.as_ref(), hidden_size * 3, hidden_size)?;
        let (q, k, v) = qkv.split3_cols(hidden_size, hidden_size, hidden_size)?;
        profile.layer_qkv_proj += stage_started.elapsed();

        let stage_started = Instant::now();
        let q = rms_norm_partitioned_rows(&q, num_heads, head_dim, q_norm_weights.as_ref(), rms_norm_eps)?;
        let k = rms_norm_partitioned_rows(&k, num_kv_heads, head_dim, k_norm_weights.as_ref(), rms_norm_eps)?;
        let v = rms_norm_partitioned_rows_no_scale(&v, num_kv_heads, head_dim, rms_norm_eps)?;
        profile.layer_qk_norm_v_norm += stage_started.elapsed();

        let stage_started = Instant::now();
        let q = apply_2d_rope_rows(&q, num_heads, rope_cache)?;
        let k = apply_2d_rope_rows(&k, num_kv_heads, rope_cache)?;
        profile.layer_rope += stage_started.elapsed();

        let stage_started = Instant::now();
        let attn = attention_rows(&q, &k, &v, num_heads, num_kv_heads, head_dim, profile)?;
        profile.layer_attention += stage_started.elapsed();

        let stage_started = Instant::now();
        let attn_proj = linear_bf16_rows(&attn, o_weight.as_ref(), hidden_size, hidden_size)?;
        let post_attention_norm_weights =
            self.read_f32_tensor(&format!("{base}.post_attention_layernorm.weight"))?;
        let attn_out = rms_norm_weighted_rows(
            &attn_proj,
            post_attention_norm_weights.as_ref(),
            rms_norm_eps,
        )?;
        let residual = add_rows(hidden, &attn_out)?;
        profile.layer_o_proj_post_norm_residual += stage_started.elapsed();

        let stage_started = Instant::now();
        let pre_ffn_norm_weights =
            self.read_f32_tensor(&format!("{base}.pre_feedforward_layernorm.weight"))?;
        let ff_in = rms_norm_weighted_rows(
            &residual,
            pre_ffn_norm_weights.as_ref(),
            rms_norm_eps,
        )?;
        profile.layer_pre_ffn_norm += stage_started.elapsed();

        let stage_started = Instant::now();
        let gate_weight_name = format!("{base}.mlp.gate_proj.linear.weight");
        let up_weight_name = format!("{base}.mlp.up_proj.linear.weight");
        let gate_up_weight = self.read_packed_tensor_bytes(
            &format!("{base}.mlp.gate_up_proj.linear.weight"),
            &[gate_weight_name.as_str(), up_weight_name.as_str()],
        )?;
        let down_weight = self.read_tensor_bytes(&format!("{base}.mlp.down_proj.linear.weight"))?;
        let intermediate_size = usize::try_from(
            self.weights
                .tensor(&gate_weight_name)?
                .shape[0],
        )
        .map_err(|_| invalid_model("vision mlp intermediate size overflow"))?;
        let ff_out = if let Some(out_flat) = try_vision_mlp_bf16_fused(
            &ff_in.data,
            gate_up_weight.as_ref(),
            down_weight.as_ref(),
            ff_in.rows,
            hidden_size,
            intermediate_size,
        ) {
            profile.layer_fused_mlp += stage_started.elapsed();
            RowsTensor::new(ff_in.rows, hidden_size, out_flat)?
        } else {
            let gate_up = linear_bf16_rows(
                &ff_in,
                gate_up_weight.as_ref(),
                intermediate_size * 2,
                hidden_size,
            )?;
            profile.layer_gate_up_proj += stage_started.elapsed();

            let stage_started = Instant::now();
            let geglu = if let Some(out) = try_geglu_packed_rows_metal(&gate_up, intermediate_size)
            {
                out?
            } else {
                let (gate, up) = gate_up.split2_cols(intermediate_size, intermediate_size)?;
                geglu_rows(&gate, &up)?
            };
            profile.layer_geglu += stage_started.elapsed();

            let stage_started = Instant::now();
            let ff_out =
                linear_bf16_rows(&geglu, down_weight.as_ref(), hidden_size, intermediate_size)?;
            profile.layer_down_proj_post_norm_residual += stage_started.elapsed();
            ff_out
        };
        let stage_started = Instant::now();
        let post_ffn_norm_weights =
            self.read_f32_tensor(&format!("{base}.post_feedforward_layernorm.weight"))?;
        let ff_out = rms_norm_weighted_rows(
            &ff_out,
            post_ffn_norm_weights.as_ref(),
            rms_norm_eps,
        )?;
        let out = add_rows(&residual, &ff_out)?;
        profile.layer_down_proj_post_norm_residual += stage_started.elapsed();
        Ok(out)
    }

    fn pool_hidden_states(
        &self,
        hidden: &RowsTensor,
        patch_positions: &[[usize; 2]],
    ) -> Result<RowsTensor> {
        let vision_config = &self.weights.snapshot.config.vision_config;
        let kernel_size = vision_config.pooling_kernel_size as usize;
        if hidden.rows == 0 {
            return Err(invalid_model("vision pooler received empty hidden state"));
        }
        let hidden_size = hidden.cols;
        let max_x = patch_positions
            .iter()
            .map(|position| position[0])
            .max()
            .unwrap_or(0)
            + 1;
        let width_groups = max_x / kernel_size;
        if width_groups == 0 {
            return Err(invalid_model("vision pooler width_groups became zero"));
        }
        let mut pooled = vec![0.0f32; vision_config.default_output_length as usize * hidden_size];
        let mut valid = vec![false; vision_config.default_output_length as usize];
        for (row_idx, [x, y]) in patch_positions.iter().copied().enumerate() {
            let idx = (x / kernel_size) + width_groups * (y / kernel_size);
            if idx >= pooled.len() {
                return Err(invalid_model(format!(
                    "vision pooler output index {idx} exceeded {}",
                    vision_config.default_output_length
                )));
            }
            valid[idx] = true;
            let row = hidden.row(row_idx);
            let pooled_row = &mut pooled[idx * hidden_size..(idx + 1) * hidden_size];
            for dim in 0..hidden_size {
                pooled_row[dim] += row[dim] / (kernel_size * kernel_size) as f32;
            }
        }
        let root_hidden = (vision_config.hidden_size as f32).sqrt();
        let mut compact = Vec::new();
        for (row_idx, is_valid) in valid.into_iter().enumerate() {
            if !is_valid {
                continue;
            }
            let row = &pooled[row_idx * hidden_size..(row_idx + 1) * hidden_size];
            compact.extend(row.iter().map(|value| value * root_hidden));
        }
        RowsTensor::new(compact.len() / hidden_size, hidden_size, compact)
    }

    fn standardize_hidden_states(&mut self, hidden: &RowsTensor) -> Result<RowsTensor> {
        let bias = self.read_f32_tensor("vision_tower.std_bias")?;
        let scale = self.read_f32_tensor("vision_tower.std_scale")?;
        let hidden_size = bias.len();
        if scale.len() != hidden_size {
            return Err(invalid_model("vision std_scale length mismatch"));
        }
        let mut out = Vec::with_capacity(hidden.data.len());
        for row in hidden.rows_iter() {
            if row.len() != hidden_size {
                return Err(invalid_model("vision standardize row length mismatch"));
            }
            for idx in 0..hidden_size {
                out.push((row[idx] - bias[idx]) * scale[idx]);
            }
        }
        RowsTensor::new(hidden.rows, hidden.cols, out)
    }

    fn project_soft_tokens_to_text_space(&mut self, hidden: &RowsTensor) -> Result<Vec<Vec<u16>>> {
        let weight_name = "embed_vision.embedding_projection.weight";
        let scales_name = "embed_vision.embedding_projection.scales";
        let biases_name = "embed_vision.embedding_projection.biases";
        let text_hidden = self.weights.snapshot.config.text_config.hidden_size as usize;
        let eps = self.weights.snapshot.config.vision_config.rms_norm_eps;
        let hidden_words = hidden
            .data
            .iter()
            .copied()
            .map(f32_to_bf16)
            .collect::<Vec<_>>();
        if let Some(projected) = try_affine_quantized_matmul_rows_metal(
            &self.weights,
            &hidden_words,
            hidden.rows,
            weight_name,
            scales_name,
            biases_name,
        ) {
            let projected = projected.map_err(invalid_model)?;
            if projected.len() != hidden.rows * text_hidden {
                return Err(invalid_model(format!(
                    "embed_vision projected {} values, expected {}",
                    projected.len(),
                    hidden.rows * text_hidden
                )));
            }
            let normalized = try_rms_norm_f32(&projected, &[hidden.rows, text_hidden], eps)
                .unwrap_or_else(|| {
                    let mut out = Vec::with_capacity(projected.len());
                    for row in projected.chunks_exact(text_hidden) {
                        out.extend(rms_norm_no_scale_row(row, eps));
                    }
                    out
                });
            return Ok(normalized
                .chunks_exact(text_hidden)
                .map(|row| row.iter().copied().map(f32_to_bf16).collect())
                .collect());
        }

        let mut out = Vec::with_capacity(hidden.rows);
        for row in hidden.rows_iter() {
            let row_words = row.iter().copied().map(f32_to_bf16).collect::<Vec<_>>();
            let projected = self
                .weights
                .header_for_tensor(weight_name)?
                .affine_quantized_matmul_t_f32(
                    &row_words,
                    weight_name,
                    scales_name,
                    biases_name,
                    self.weights.snapshot.config.quantization.group_size as u64,
                    self.weights.snapshot.config.quantization.bits,
                )?;
            if projected.len() != text_hidden {
                return Err(invalid_model(format!(
                    "embed_vision projected {} values, expected {}",
                    projected.len(),
                    text_hidden
                )));
            }
            let normalized = rms_norm_no_scale_row(&projected, eps);
            out.push(normalized.into_iter().map(f32_to_bf16).collect());
        }
        Ok(out)
    }

    fn read_tensor_bytes(&mut self, name: &str) -> Result<Arc<Vec<u8>>> {
        if let Some(cached) = self.tensor_bytes_cache.get(name) {
            return Ok(cached.clone());
        }
        let bytes = Arc::new(self.weights.read_tensor_bytes(name)?);
        self.tensor_bytes_cache.insert(name.to_owned(), bytes.clone());
        Ok(bytes)
    }

    fn read_bf16_tensor_words(&mut self, name: &str) -> Result<Arc<Vec<u16>>> {
        if let Some(cached) = self.bf16_tensor_cache.get(name) {
            return Ok(cached.clone());
        }
        let words = Arc::new(self.weights.read_bf16_tensor_words(name)?);
        self.bf16_tensor_cache.insert(name.to_owned(), words.clone());
        Ok(words)
    }

    fn read_packed_tensor_bytes(&mut self, packed_name: &str, names: &[&str]) -> Result<Arc<Vec<u8>>> {
        if let Some(cached) = self.packed_tensor_bytes_cache.get(packed_name) {
            return Ok(cached.clone());
        }
        let mut packed = Vec::new();
        let mut expected_row_width_bytes = None::<usize>;
        for &name in names {
            let entry = self.weights.tensor(name)?;
            if entry.shape.len() != 2 {
                return Err(invalid_model(format!("packed tensor source {name} is not rank-2")));
            }
            let row_width = usize::try_from(entry.shape[1])
                .map_err(|_| invalid_model("packed tensor row width overflow"))?
                .checked_mul(2)
                .ok_or_else(|| invalid_model("packed tensor row width bytes overflow"))?;
            match expected_row_width_bytes {
                Some(expected) if expected != row_width => {
                    return Err(invalid_model(format!(
                        "packed tensor row width mismatch: expected {} got {} for {}",
                        expected, row_width, name
                    )));
                }
                None => expected_row_width_bytes = Some(row_width),
                _ => {}
            }
            packed.extend_from_slice(self.read_tensor_bytes(name)?.as_ref());
        }
        let packed = Arc::new(packed);
        self.packed_tensor_bytes_cache
            .insert(packed_name.to_owned(), packed.clone());
        Ok(packed)
    }

    fn read_f32_tensor(&mut self, name: &str) -> Result<Arc<Vec<f32>>> {
        if let Some(cached) = self.f32_tensor_cache.get(name) {
            return Ok(cached.clone());
        }
        let words = self.read_bf16_tensor_words(name)?;
        let values: Arc<Vec<f32>> = Arc::new(words.iter().copied().map(bf16_to_f32).collect());
        self.f32_tensor_cache.insert(name.to_owned(), values.clone());
        Ok(values)
    }
}

fn patch_positions(width: usize, height: usize) -> Vec<[usize; 2]> {
    let mut positions = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            positions.push([x, y]);
        }
    }
    positions
}

#[derive(Clone, Debug)]
struct RowsTensor {
    rows: usize,
    cols: usize,
    data: Vec<f32>,
}

impl RowsTensor {
    fn new(rows: usize, cols: usize, data: Vec<f32>) -> Result<Self> {
        let expected = rows
            .checked_mul(cols)
            .ok_or_else(|| invalid_model("rows tensor size overflow"))?;
        if data.len() != expected {
            return Err(invalid_model(format!(
                "rows tensor data len mismatch: expected {} got {}",
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

    fn row_mut(&mut self, row_idx: usize) -> &mut [f32] {
        let start = row_idx * self.cols;
        &mut self.data[start..start + self.cols]
    }

    fn rows_iter(&self) -> impl Iterator<Item = &[f32]> {
        self.data.chunks_exact(self.cols)
    }

    fn split2_cols(self, left_cols: usize, right_cols: usize) -> Result<(RowsTensor, RowsTensor)> {
        let (left, middle, right) = self.split3_cols(left_cols, right_cols, 0)?;
        if right.cols != 0 || !right.data.is_empty() {
            return Err(invalid_model("split2_cols produced unexpected remainder"));
        }
        Ok((left, middle))
    }

    fn split3_cols(
        self,
        first_cols: usize,
        second_cols: usize,
        third_cols: usize,
    ) -> Result<(RowsTensor, RowsTensor, RowsTensor)> {
        let expected_cols = first_cols
            .checked_add(second_cols)
            .and_then(|sum| sum.checked_add(third_cols))
            .ok_or_else(|| invalid_model("split cols overflow"))?;
        if self.cols != expected_cols {
            return Err(invalid_model(format!(
                "split cols mismatch: tensor has {} cols, expected {}",
                self.cols, expected_cols
            )));
        }
        let mut first = Vec::with_capacity(self.rows * first_cols);
        let mut second = Vec::with_capacity(self.rows * second_cols);
        let mut third = Vec::with_capacity(self.rows * third_cols);
        for row in self.rows_iter() {
            first.extend_from_slice(&row[..first_cols]);
            second.extend_from_slice(&row[first_cols..first_cols + second_cols]);
            third.extend_from_slice(
                &row[first_cols + second_cols..first_cols + second_cols + third_cols],
            );
        }
        Ok((
            RowsTensor::new(self.rows, first_cols, first)?,
            RowsTensor::new(self.rows, second_cols, second)?,
            RowsTensor::new(self.rows, third_cols, third)?,
        ))
    }
}

#[derive(Clone, Debug)]
struct Rope2DCache {
    half_per_dim: usize,
    head_dim: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Rope2DCache {
    fn new(positions: &[[usize; 2]], head_dim: usize, rope_theta: f32) -> Self {
        let ndim = 2usize;
        let channels_per_dim = 2 * (head_dim / (2 * ndim));
        let half_per_dim = channels_per_dim / 2;
        let inv_timescales = (0..half_per_dim)
            .map(|pair_idx| {
                let exponent = (2.0 / channels_per_dim as f32) * pair_idx as f32;
                1.0 / rope_theta.powf(exponent)
            })
            .collect::<Vec<_>>();
        let mut cos = Vec::with_capacity(positions.len() * ndim * half_per_dim);
        let mut sin = Vec::with_capacity(positions.len() * ndim * half_per_dim);
        for [x, y] in positions.iter().copied() {
            for axis_position in [x as f32, y as f32] {
                for &inv_timescale in &inv_timescales {
                    let angle = axis_position * inv_timescale;
                    cos.push(angle.cos());
                    sin.push(angle.sin());
                }
            }
        }
        Self {
            half_per_dim,
            head_dim,
            cos,
            sin,
        }
    }

    fn pair_offset(&self, row_idx: usize, axis: usize, pair_idx: usize) -> usize {
        row_idx * 2 * self.half_per_dim + axis * self.half_per_dim + pair_idx
    }
}

fn linear_bf16_rows(
    input: &RowsTensor,
    weight_bytes: &[u8],
    out_dim: usize,
    in_dim: usize,
) -> Result<RowsTensor> {
    let flat_input = &input.data;
    let rows = input.rows;
    let width = input.cols;
    if width != in_dim {
        return Err(invalid_model(format!(
            "linear input width mismatch: expected {} got {}",
            in_dim, width
        )));
    }
    let expected_weight_bytes = out_dim
        .checked_mul(in_dim)
        .and_then(|elements| elements.checked_mul(2))
        .ok_or_else(|| invalid_model("linear weight size overflow"))?;
    if weight_bytes.len() != expected_weight_bytes {
        return Err(invalid_model(format!(
            "linear weight byte len mismatch: expected {} got {}",
            expected_weight_bytes,
            weight_bytes.len()
        )));
    }
    if rows == 0 {
        return RowsTensor::new(0, out_dim, Vec::new());
    }
    if let Some(out_flat) = try_matmul_nt_ggml_bytes(
        &flat_input,
        weight_bytes,
        GGML_TYPE_BF16,
        rows,
        in_dim,
        out_dim,
    ) {
        return RowsTensor::new(rows, out_dim, out_flat);
    }

    let mut out_data = vec![0.0f32; rows * out_dim];
    for row_idx in 0..rows {
        let row = input.row(row_idx);
        let out_row = &mut out_data[row_idx * out_dim..(row_idx + 1) * out_dim];
        for (out_idx, slot) in out_row.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            let base = out_idx * in_dim * 2;
            for (in_idx, input_value) in row.iter().copied().enumerate().take(in_dim) {
                acc += input_value * bf16_from_bytes(weight_bytes, base + in_idx * 2);
            }
            *slot = acc;
        }
    }
    RowsTensor::new(rows, out_dim, out_data)
}

fn rms_norm_weighted_rows(input: &RowsTensor, weight: &[f32], eps: f32) -> Result<RowsTensor> {
    let hidden_size = weight.len();
    let flat_input = &input.data;
    let rows = input.rows;
    let width = input.cols;
    if width != hidden_size {
        return Err(invalid_model("rms norm row length mismatch"));
    }
    if rows == 0 {
        return RowsTensor::new(0, hidden_size, Vec::new());
    }
    if let Some(out_flat) =
        try_rms_norm_mul_f32(&flat_input, &[rows, hidden_size], weight, &[hidden_size], eps)
    {
        return RowsTensor::new(rows, hidden_size, out_flat);
    }
    let mut out = Vec::with_capacity(input.data.len());
    for row in input.rows_iter() {
        if row.len() != hidden_size {
            return Err(invalid_model("rms norm row length mismatch"));
        }
        let mean_square = row.iter().map(|value| value * value).sum::<f32>() / hidden_size as f32;
        let inv_rms = 1.0 / (mean_square + eps).sqrt();
        for idx in 0..hidden_size {
            out.push(row[idx] * inv_rms * weight[idx]);
        }
    }
    RowsTensor::new(rows, hidden_size, out)
}

fn rms_norm_partitioned_rows(
    input: &RowsTensor,
    num_heads: usize,
    head_dim: usize,
    weight: &[f32],
    eps: f32,
) -> Result<RowsTensor> {
    if weight.len() != head_dim {
        return Err(invalid_model("partitioned rms norm weight length mismatch"));
    }
    let flat_input = &input.data;
    let rows = input.rows;
    let width = input.cols;
    if width != num_heads * head_dim {
        return Err(invalid_model("partitioned rms norm row length mismatch"));
    }
    if rows == 0 {
        return RowsTensor::new(0, width, Vec::new());
    }
    if let Some(out_flat) = try_rms_norm_mul_f32(
        &flat_input,
        &[rows * num_heads, head_dim],
        weight,
        &[head_dim],
        eps,
    ) {
        return RowsTensor::new(rows, width, out_flat);
    }
    let mut out = Vec::with_capacity(input.data.len());
    for row in input.rows_iter() {
        if row.len() != num_heads * head_dim {
            return Err(invalid_model("partitioned rms norm row length mismatch"));
        }
        let row_start = out.len();
        out.extend_from_slice(row);
        for head_idx in 0..num_heads {
            let start = head_idx * head_dim;
            let end = start + head_dim;
            let mean_square = row[start..end]
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                / head_dim as f32;
            let inv_rms = 1.0 / (mean_square + eps).sqrt();
            for dim in 0..head_dim {
                out[row_start + start + dim] = row[start + dim] * inv_rms * weight[dim];
            }
        }
    }
    RowsTensor::new(rows, width, out)
}

fn rms_norm_partitioned_rows_no_scale(
    input: &RowsTensor,
    num_heads: usize,
    head_dim: usize,
    eps: f32,
) -> Result<RowsTensor> {
    let flat_input = &input.data;
    let rows = input.rows;
    let width = input.cols;
    if width != num_heads * head_dim {
        return Err(invalid_model("partitioned rms norm row length mismatch"));
    }
    if rows == 0 {
        return RowsTensor::new(0, width, Vec::new());
    }
    if let Some(out_flat) = try_rms_norm_f32(&flat_input, &[rows * num_heads, head_dim], eps)
    {
        return RowsTensor::new(rows, width, out_flat);
    }
    let mut out = Vec::with_capacity(input.data.len());
    for row in input.rows_iter() {
        if row.len() != num_heads * head_dim {
            return Err(invalid_model("partitioned rms norm row length mismatch"));
        }
        let row_start = out.len();
        out.extend_from_slice(row);
        for head_idx in 0..num_heads {
            let start = head_idx * head_dim;
            let end = start + head_dim;
            let mean_square = row[start..end]
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                / head_dim as f32;
            let inv_rms = 1.0 / (mean_square + eps).sqrt();
            for dim in start..end {
                out[row_start + dim] = row[dim] * inv_rms;
            }
        }
    }
    RowsTensor::new(rows, width, out)
}

fn apply_2d_rope_rows(
    input: &RowsTensor,
    num_heads: usize,
    rope_cache: &Rope2DCache,
) -> Result<RowsTensor> {
    let head_dim = rope_cache.head_dim;
    if input.cols != num_heads * head_dim {
        return Err(invalid_model("2d rope row width mismatch"));
    }
    if input.rows == 0 {
        return RowsTensor::new(0, input.cols, Vec::new());
    }
    let channels_per_dim = rope_cache.half_per_dim * 2;
    let mut out = vec![0.0f32; input.data.len()];
    for row_idx in 0..input.rows {
        let row = input.row(row_idx);
        let out_row = &mut out[row_idx * input.cols..(row_idx + 1) * input.cols];
        for head_idx in 0..num_heads {
            let head_start = head_idx * head_dim;
            for axis in 0..2 {
                let part_start = head_start + axis * channels_per_dim;
                for pair_idx in 0..rope_cache.half_per_dim {
                    let rope_idx = rope_cache.pair_offset(row_idx, axis, pair_idx);
                    let cos = rope_cache.cos[rope_idx];
                    let sin = rope_cache.sin[rope_idx];
                    let a = row[part_start + pair_idx];
                    let b = row[part_start + rope_cache.half_per_dim + pair_idx];
                    out_row[part_start + pair_idx] = a * cos - b * sin;
                    out_row[part_start + rope_cache.half_per_dim + pair_idx] = b * cos + a * sin;
                }
            }
        }
    }
    RowsTensor::new(input.rows, input.cols, out)
}

fn attention_rows(
    q: &RowsTensor,
    k: &RowsTensor,
    v: &RowsTensor,
    num_heads: usize,
    num_kv_heads: usize,
    head_dim: usize,
    profile: &mut GemmaVisionProfile,
) -> Result<RowsTensor> {
    let seq_len = q.rows;
    if k.rows != seq_len || v.rows != seq_len {
        return Err(invalid_model("attention q/k/v sequence length mismatch"));
    }
    let q_width = q.cols;
    let kv_width = k.cols;
    if q_width != num_heads * head_dim || kv_width != num_kv_heads * head_dim {
        return Err(invalid_model("attention q/k/v head width mismatch"));
    }
    if seq_len == 0 {
        return RowsTensor::new(0, q_width, Vec::new());
    }
    if num_heads == num_kv_heads {
        if let Some(out_flat) =
            try_flash_attn_f32_packed(&q.data, &k.data, &v.data, seq_len, seq_len, num_heads, head_dim, 1.0)
        {
            profile.flash_attn_successes += 1;
            return RowsTensor::new(seq_len, q_width, out_flat);
        }
    }
    profile.flash_attn_fallbacks += 1;
    let q_heads_per_kv = num_heads / num_kv_heads;
    let mut out = vec![0.0f32; seq_len * q_width];
    for q_idx in 0..seq_len {
        let out_row = &mut out[q_idx * q_width..(q_idx + 1) * q_width];
        let q_row = q.row(q_idx);
        for head_idx in 0..num_heads {
            let kv_head_idx = head_idx / q_heads_per_kv;
            let q_start = head_idx * head_dim;
            let k_start = kv_head_idx * head_dim;
            let mut logits = vec![0.0f32; seq_len];
            let mut max_logit = f32::NEG_INFINITY;
            for k_idx in 0..seq_len {
                let k_row = k.row(k_idx);
                let mut dot = 0.0f32;
                for dim in 0..head_dim {
                    dot += q_row[q_start + dim] * k_row[k_start + dim];
                }
                logits[k_idx] = dot;
                max_logit = max_logit.max(dot);
            }
            let mut denom = 0.0f32;
            for logit in &mut logits {
                *logit = (*logit - max_logit).exp();
                denom += *logit;
            }
            for k_idx in 0..seq_len {
                let weight = logits[k_idx] / denom;
                let v_start = kv_head_idx * head_dim;
                let v_row = v.row(k_idx);
                for dim in 0..head_dim {
                    out_row[q_start + dim] += weight * v_row[v_start + dim];
                }
            }
        }
    }
    RowsTensor::new(seq_len, q_width, out)
}

fn add_rows(lhs: &RowsTensor, rhs: &RowsTensor) -> Result<RowsTensor> {
    if lhs.rows != rhs.rows {
        return Err(invalid_model("row add length mismatch"));
    }
    let lhs_flat = &lhs.data;
    let rhs_flat = &rhs.data;
    let rows = lhs.rows;
    let width = lhs.cols;
    let rhs_rows = rhs.rows;
    let rhs_width = rhs.cols;
    if rows != rhs_rows || width != rhs_width {
        return Err(invalid_model("row add width mismatch"));
    }
    if rows == 0 {
        return RowsTensor::new(0, width, Vec::new());
    }
    if let Some(out_flat) = try_add_f32(&lhs_flat, &[rows, width], &rhs_flat, &[rows, width]) {
        return RowsTensor::new(rows, width, out_flat);
    }
    let mut out = Vec::with_capacity(lhs.data.len());
    for (lhs_row, rhs_row) in lhs.rows_iter().zip(rhs.rows_iter()) {
        if lhs_row.len() != rhs_row.len() {
            return Err(invalid_model("row add width mismatch"));
        }
        out.extend(
            lhs_row
                .iter()
                .zip(rhs_row.iter())
                .map(|(lhs_value, rhs_value)| lhs_value + rhs_value),
        );
    }
    RowsTensor::new(rows, width, out)
}

fn geglu_rows(gate: &RowsTensor, up: &RowsTensor) -> Result<RowsTensor> {
    let gate_flat = &gate.data;
    let rows = gate.rows;
    let width = gate.cols;
    let up_flat = &up.data;
    let up_rows = up.rows;
    let up_width = up.cols;
    if rows != up_rows || width != up_width {
        return Err(invalid_model("geglu width mismatch"));
    }
    if rows == 0 {
        return RowsTensor::new(0, width, Vec::new());
    }
    if let Some(gelu_flat) = try_gelu_f32(&gate_flat, &[rows, width]) {
        if let Some(out_flat) = try_mul_f32(&gelu_flat, &[rows, width], &up_flat, &[rows, width]) {
            return RowsTensor::new(rows, width, out_flat);
        }
    }
    let mut out = Vec::with_capacity(gate.data.len());
    for (gate_row, up_row) in gate.rows_iter().zip(up.rows_iter()) {
        for idx in 0..width {
            out.push(gelu_approx(gate_row[idx]) * up_row[idx]);
        }
    }
    RowsTensor::new(rows, width, out)
}

#[derive(Clone, Copy)]
#[repr(C)]
struct MlxVisionGegluArgs {
    n: u32,
    row_width: u32,
    input_row_stride: u32,
    input_split_offset: u32,
}

struct VisionGegluMetalBackend {
    runtime: MetalRuntime,
    pipeline: MetalPipeline,
    input_buffer: Option<MetalBuffer>,
    input_capacity_words: usize,
    output_buffer: Option<MetalBuffer>,
    output_capacity_words: usize,
}

impl VisionGegluMetalBackend {
    fn load() -> std::result::Result<Self, String> {
        let runtime = MetalRuntime::new().map_err(|err| format!("MetalRuntime::new failed: {err}"))?;
        let pipeline = runtime
            .get_or_compile_pipeline(&MetalPipelineDescriptor {
                cache_name: "kernel_mlx_geglu_strided_rows_bf16".to_string(),
                base_name: "kernel_mlx_geglu_strided_rows_bf16".to_string(),
                constants: Vec::new(),
                smem_bytes: 0,
                nr0: 0,
                nr1: 0,
                nsg: 0,
            })
            .map_err(|err| format!("compile kernel_mlx_geglu_strided_rows_bf16 failed: {err}"))?;
        Ok(Self {
            runtime,
            pipeline,
            input_buffer: None,
            input_capacity_words: 0,
            output_buffer: None,
            output_capacity_words: 0,
        })
    }

    fn ensure_input_buffer(&mut self, len_words: usize) -> std::result::Result<MetalBuffer, String> {
        if self.input_capacity_words < len_words || self.input_buffer.is_none() {
            self.input_buffer = Some(
                self.runtime
                    .create_buffer(len_words * size_of::<u16>(), BufferStorageMode::Shared)
                    .map_err(|err| format!("create vision geglu input buffer failed: {err}"))?,
            );
            self.input_capacity_words = len_words;
        }
        self.input_buffer
            .as_ref()
            .cloned()
            .ok_or_else(|| "missing vision geglu input buffer".to_string())
    }

    fn ensure_output_buffer(&mut self, len_words: usize) -> std::result::Result<MetalBuffer, String> {
        if self.output_capacity_words < len_words || self.output_buffer.is_none() {
            self.output_buffer = Some(
                self.runtime
                    .create_buffer(len_words * size_of::<u16>(), BufferStorageMode::Shared)
                    .map_err(|err| format!("create vision geglu output buffer failed: {err}"))?,
            );
            self.output_capacity_words = len_words;
        }
        self.output_buffer
            .as_ref()
            .cloned()
            .ok_or_else(|| "missing vision geglu output buffer".to_string())
    }

    fn geglu_packed_rows(
        &mut self,
        gate_up_words: &[u16],
        rows: usize,
        half_width: usize,
    ) -> std::result::Result<Vec<f32>, String> {
        if rows == 0 {
            return Ok(Vec::new());
        }
        let expected_input_words = rows
            .checked_mul(half_width)
            .and_then(|n| n.checked_mul(2))
            .ok_or_else(|| "vision geglu input size overflow".to_string())?;
        if gate_up_words.len() != expected_input_words {
            return Err(format!(
                "vision geglu input len mismatch: got {} expected {}",
                gate_up_words.len(),
                expected_input_words
            ));
        }
        let output_words = rows
            .checked_mul(half_width)
            .ok_or_else(|| "vision geglu output size overflow".to_string())?;
        let input_buffer = self.ensure_input_buffer(gate_up_words.len())?;
        let output_buffer = self.ensure_output_buffer(output_words)?;
        self.runtime
            .write_buffer(&input_buffer, 0, u16_words_as_le_bytes(gate_up_words))
            .map_err(|err| format!("write vision geglu input failed: {err}"))?;
        let args = MlxVisionGegluArgs {
            n: output_words as u32,
            row_width: half_width as u32,
            input_row_stride: (half_width * 2) as u32,
            input_split_offset: half_width as u32,
        };
        self.runtime
            .begin_command_batch()
            .map_err(|err| format!("begin vision geglu batch failed: {err}"))?;
        let dispatch = self.runtime.dispatch_compute_tracked(
            &self.pipeline,
            bytes_of_val(&args),
            &[MetalBufferBindingRef {
                index: 1,
                buffer: &input_buffer,
                offset_bytes: 0,
            }],
            &[MetalBufferBindingRef {
                index: 2,
                buffer: &output_buffer,
                offset_bytes: 0,
            }],
            &[],
            MetalSize {
                width: (output_words as u64).div_ceil(256),
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: 256,
                height: 1,
                depth: 1,
            },
        );
        if let Err(err) = dispatch {
            let _ = self.runtime.discard_command_batch();
            return Err(format!("dispatch vision geglu failed: {err}"));
        }
        self.runtime
            .end_command_batch()
            .map_err(|err| format!("end vision geglu batch failed: {err}"))?;
        let bytes = self
            .runtime
            .read_buffer(&output_buffer, output_words * size_of::<u16>())
            .map_err(|err| format!("read vision geglu output failed: {err}"))?;
        let mut out = Vec::with_capacity(output_words);
        for chunk in bytes.chunks_exact(size_of::<u16>()) {
            out.push(bf16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]])));
        }
        Ok(out)
    }
}

fn try_geglu_packed_rows_metal(gate_up: &RowsTensor, half_width: usize) -> Option<Result<RowsTensor>> {
    thread_local! {
        static VISION_GEGLU_METAL_BACKEND: RefCell<Option<VisionGegluMetalBackend>> = const { RefCell::new(None) };
    }
    if gate_up.cols != half_width * 2 {
        return Some(Err(invalid_model("packed geglu width mismatch")));
    }
    let gate_up_words = gate_up
        .data
        .iter()
        .copied()
        .map(f32_to_bf16)
        .collect::<Vec<_>>();
    VISION_GEGLU_METAL_BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        if backend.is_none() {
            match VisionGegluMetalBackend::load() {
                Ok(loaded) => *backend = Some(loaded),
                Err(_) => return None,
            }
        }
        Some(
            backend
                .as_mut()
                .expect("vision geglu metal backend was just initialized")
                .geglu_packed_rows(&gate_up_words, gate_up.rows, half_width)
                .map_err(invalid_model)
                .and_then(|out| RowsTensor::new(gate_up.rows, half_width, out)),
        )
    })
}

fn rms_norm_no_scale_row(row: &[f32], eps: f32) -> Vec<f32> {
    let mean_square = row.iter().map(|value| value * value).sum::<f32>() / row.len() as f32;
    let inv_rms = 1.0 / (mean_square + eps).sqrt();
    row.iter().map(|value| value * inv_rms).collect()
}

fn gelu_approx(x: f32) -> f32 {
    let inner = 0.797_884_6 * (x + 0.044_715 * x * x * x);
    0.5 * x * (1.0 + inner.tanh())
}

fn bf16_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn u16_words_as_le_bytes(words: &[u16]) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * size_of::<u16>())
    }

    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("u16 byte reinterpreting currently assumes little-endian targets")
    }
}

fn bytes_of_val<T>(value: &T) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>())
    }

    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("byte reinterpreting currently assumes little-endian targets")
    }
}

fn bf16_from_bytes(bytes: &[u8], offset: usize) -> f32 {
    let word = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    bf16_to_f32(word)
}

fn f32_to_bf16(value: f32) -> u16 {
    let bits = value.to_bits();
    let lsb = (bits >> 16) & 1;
    ((bits.wrapping_add(0x7FFF + lsb) & 0xFFFF_0000) >> 16) as u16
}

fn invalid_model(message: impl Into<String>) -> MlxRtError {
    MlxRtError::InvalidModelDir {
        path: PathBuf::new(),
        message: message.into(),
    }
}
