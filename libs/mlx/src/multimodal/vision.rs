use crate::{MlxIndexedSafetensors, MlxRtError, Result};
use crate::multimodal::GemmaImagePixels;
use makepad_ggml::backend::metal::{
    try_add_f32, try_flash_attn_f32_packed, try_gelu_f32, try_matmul_nt_ggml_bytes,
    try_mul_f32, try_rms_norm_f32, try_rms_norm_mul_f32,
};
use makepad_ggml::quant::GGML_TYPE_BF16;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct GemmaVisionRuntime {
    weights: MlxIndexedSafetensors,
    tensor_bytes_cache: HashMap<String, Arc<Vec<u8>>>,
    bf16_tensor_cache: HashMap<String, Arc<Vec<u16>>>,
    f32_tensor_cache: HashMap<String, Arc<Vec<f32>>>,
}

impl GemmaVisionRuntime {
    pub fn load(weights: &MlxIndexedSafetensors) -> Self {
        Self {
            weights: weights.clone(),
            tensor_bytes_cache: HashMap::new(),
            bf16_tensor_cache: HashMap::new(),
            f32_tensor_cache: HashMap::new(),
        }
    }

    pub fn encode_image_to_text_embeddings(
        &mut self,
        image: &GemmaImagePixels,
    ) -> Result<Vec<Vec<u16>>> {
        let patch_size = self.weights.snapshot.config.vision_config.patch_size as usize;
        let num_hidden_layers = self.weights.snapshot.config.vision_config.num_hidden_layers as usize;
        let standardize = self.weights.snapshot.config.vision_config.standardize;
        let num_real_patches = image
            .patch_grid_width
            .checked_mul(image.patch_grid_height)
            .ok_or_else(|| invalid_model("vision patch count overflow"))?;
        let mut hidden = self.patch_embed(image, patch_size, num_real_patches)?;
        let patch_positions = patch_positions(image.patch_grid_width, image.patch_grid_height);
        for layer_idx in 0..num_hidden_layers {
            hidden = self.apply_vision_layer(layer_idx, &hidden, &patch_positions)?;
        }
        let pooled = self.pool_hidden_states(&hidden, &patch_positions)?;
        if pooled.len() != image.soft_token_count {
            return Err(invalid_model(format!(
                "vision pooler token count mismatch: expected {} got {}",
                image.soft_token_count,
                pooled.len()
            )));
        }
        let standardized = if standardize {
            self.standardize_hidden_states(&pooled)?
        } else {
            pooled
        };
        self.project_soft_tokens_to_text_space(&standardized)
    }

    fn patch_embed(
        &mut self,
        image: &GemmaImagePixels,
        patch_size: usize,
        num_real_patches: usize,
    ) -> Result<Vec<Vec<f32>>> {
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
        let mut patch_rows = Vec::with_capacity(num_real_patches);
        for patch_y in 0..image.patch_grid_height {
            for patch_x in 0..image.patch_grid_width {
                let mut row = Vec::with_capacity(in_dim);
                for dy in 0..patch_size {
                    let src_y = patch_y * patch_size + dy;
                    for dx in 0..patch_size {
                        let src_x = patch_x * patch_size + dx;
                        let pixel_index = src_y * image.width + src_x;
                        for channel in 0..3 {
                            let value = image.pixels_chw[channel * pixel_plane_len + pixel_index];
                            row.push(2.0 * (value - 0.5));
                        }
                    }
                }
                patch_rows.push(row);
            }
        }
        let mut hidden = linear_bf16_rows(&patch_rows, input_proj.as_ref(), out_dim, in_dim)?;

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
        for (row, [x, y]) in hidden.iter_mut().zip(positions.iter().copied()) {
            if x >= pos_size || y >= pos_size {
                return Err(invalid_model(format!(
                    "patch position out of embedding range: ({x},{y}) >= {pos_size}"
                )));
            }
            let x_offset = x * hidden_size;
            let y_offset = (pos_size + y) * hidden_size;
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
        hidden: &[Vec<f32>],
        patch_positions: &[[usize; 2]],
    ) -> Result<Vec<Vec<f32>>> {
        let base = format!("vision_tower.encoder.layers.{layer_idx}");
        let hidden_size = hidden
            .first()
            .map(|row| row.len())
            .ok_or_else(|| invalid_model("vision layer received empty hidden state"))?;
        let num_heads = self.weights.snapshot.config.vision_config.num_attention_heads as usize;
        let num_kv_heads = self.weights.snapshot.config.vision_config.num_key_value_heads as usize;
        let head_dim = self.weights.snapshot.config.vision_config.head_dim as usize;
        let rope_theta = self.weights.snapshot.config.vision_config.rope_parameters.rope_theta;
        let rms_norm_eps = self.weights.snapshot.config.vision_config.rms_norm_eps;
        let input_norm_weights = self.read_f32_tensor(&format!("{base}.input_layernorm.weight"))?;
        let normed = rms_norm_weighted_rows(hidden, input_norm_weights.as_ref(), rms_norm_eps)?;

        let q_weight = self.read_tensor_bytes(&format!("{base}.self_attn.q_proj.linear.weight"))?;
        let k_weight = self.read_tensor_bytes(&format!("{base}.self_attn.k_proj.linear.weight"))?;
        let v_weight = self.read_tensor_bytes(&format!("{base}.self_attn.v_proj.linear.weight"))?;
        let o_weight = self.read_tensor_bytes(&format!("{base}.self_attn.o_proj.linear.weight"))?;
        let q_norm_weights = self.read_f32_tensor(&format!("{base}.self_attn.q_norm.weight"))?;
        let k_norm_weights = self.read_f32_tensor(&format!("{base}.self_attn.k_norm.weight"))?;

        let q = linear_bf16_rows(&normed, q_weight.as_ref(), hidden_size, hidden_size)?;
        let k = linear_bf16_rows(&normed, k_weight.as_ref(), hidden_size, hidden_size)?;
        let v = linear_bf16_rows(&normed, v_weight.as_ref(), hidden_size, hidden_size)?;
        let q = rms_norm_partitioned_rows(&q, num_heads, head_dim, q_norm_weights.as_ref(), rms_norm_eps)?;
        let k = rms_norm_partitioned_rows(&k, num_kv_heads, head_dim, k_norm_weights.as_ref(), rms_norm_eps)?;
        let v = rms_norm_partitioned_rows_no_scale(&v, num_kv_heads, head_dim, rms_norm_eps)?;
        let q = apply_2d_rope_rows(&q, patch_positions, num_heads, head_dim, rope_theta);
        let k = apply_2d_rope_rows(&k, patch_positions, num_kv_heads, head_dim, rope_theta);
        let attn = attention_rows(&q, &k, &v, num_heads, num_kv_heads, head_dim)?;
        let attn_proj = linear_bf16_rows(&attn, o_weight.as_ref(), hidden_size, hidden_size)?;
        let post_attention_norm_weights =
            self.read_f32_tensor(&format!("{base}.post_attention_layernorm.weight"))?;
        let attn_out = rms_norm_weighted_rows(
            &attn_proj,
            post_attention_norm_weights.as_ref(),
            rms_norm_eps,
        )?;
        let residual = add_rows(hidden, &attn_out)?;

        let pre_ffn_norm_weights =
            self.read_f32_tensor(&format!("{base}.pre_feedforward_layernorm.weight"))?;
        let ff_in = rms_norm_weighted_rows(
            &residual,
            pre_ffn_norm_weights.as_ref(),
            rms_norm_eps,
        )?;
        let gate_weight = self.read_tensor_bytes(&format!("{base}.mlp.gate_proj.linear.weight"))?;
        let up_weight = self.read_tensor_bytes(&format!("{base}.mlp.up_proj.linear.weight"))?;
        let down_weight = self.read_tensor_bytes(&format!("{base}.mlp.down_proj.linear.weight"))?;
        let intermediate_size = usize::try_from(
            self.weights
                .tensor(&format!("{base}.mlp.gate_proj.linear.weight"))?
                .shape[0],
        )
        .map_err(|_| invalid_model("vision mlp intermediate size overflow"))?;
        let gate = linear_bf16_rows(&ff_in, gate_weight.as_ref(), intermediate_size, hidden_size)?;
        let up = linear_bf16_rows(&ff_in, up_weight.as_ref(), intermediate_size, hidden_size)?;
        let geglu = geglu_rows(&gate, &up)?;
        let ff_out =
            linear_bf16_rows(&geglu, down_weight.as_ref(), hidden_size, intermediate_size)?;
        let post_ffn_norm_weights =
            self.read_f32_tensor(&format!("{base}.post_feedforward_layernorm.weight"))?;
        let ff_out = rms_norm_weighted_rows(
            &ff_out,
            post_ffn_norm_weights.as_ref(),
            rms_norm_eps,
        )?;
        add_rows(&residual, &ff_out)
    }

    fn pool_hidden_states(
        &self,
        hidden: &[Vec<f32>],
        patch_positions: &[[usize; 2]],
    ) -> Result<Vec<Vec<f32>>> {
        let vision_config = &self.weights.snapshot.config.vision_config;
        let kernel_size = vision_config.pooling_kernel_size as usize;
        let hidden_size = hidden
            .first()
            .map(|row| row.len())
            .ok_or_else(|| invalid_model("vision pooler received empty hidden state"))?;
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
        let mut pooled = vec![vec![0.0f32; hidden_size]; vision_config.default_output_length as usize];
        let mut valid = vec![false; vision_config.default_output_length as usize];
        for (row, [x, y]) in hidden.iter().zip(patch_positions.iter().copied()) {
            let idx = (x / kernel_size) + width_groups * (y / kernel_size);
            if idx >= pooled.len() {
                return Err(invalid_model(format!(
                    "vision pooler output index {idx} exceeded {}",
                    pooled.len()
                )));
            }
            valid[idx] = true;
            for dim in 0..hidden_size {
                pooled[idx][dim] += row[dim] / (kernel_size * kernel_size) as f32;
            }
        }
        let root_hidden = (vision_config.hidden_size as f32).sqrt();
        let mut compact = Vec::new();
        for (row, is_valid) in pooled.into_iter().zip(valid.into_iter()) {
            if !is_valid {
                continue;
            }
            compact.push(row.into_iter().map(|value| value * root_hidden).collect());
        }
        Ok(compact)
    }

    fn standardize_hidden_states(&mut self, hidden: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        let bias = self.read_f32_tensor("vision_tower.std_bias")?;
        let scale = self.read_f32_tensor("vision_tower.std_scale")?;
        let hidden_size = bias.len();
        if scale.len() != hidden_size {
            return Err(invalid_model("vision std_scale length mismatch"));
        }
        let mut out = Vec::with_capacity(hidden.len());
        for row in hidden {
            if row.len() != hidden_size {
                return Err(invalid_model("vision standardize row length mismatch"));
            }
            let mut new_row = Vec::with_capacity(hidden_size);
            for idx in 0..hidden_size {
                new_row.push((row[idx] - bias[idx]) * scale[idx]);
            }
            out.push(new_row);
        }
        Ok(out)
    }

    fn project_soft_tokens_to_text_space(&mut self, hidden: &[Vec<f32>]) -> Result<Vec<Vec<u16>>> {
        let weight_name = "embed_vision.embedding_projection.weight";
        let scales_name = "embed_vision.embedding_projection.scales";
        let biases_name = "embed_vision.embedding_projection.biases";
        let text_hidden = self.weights.snapshot.config.text_config.hidden_size as usize;
        let eps = self.weights.snapshot.config.vision_config.rms_norm_eps;
        let mut out = Vec::with_capacity(hidden.len());
        for row in hidden {
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

fn linear_bf16_rows(
    input: &[Vec<f32>],
    weight_bytes: &[u8],
    out_dim: usize,
    in_dim: usize,
) -> Result<Vec<Vec<f32>>> {
    let (flat_input, rows, width) = flatten_rows(input)?;
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
        return Ok(Vec::new());
    }
    if let Some(out_flat) = try_matmul_nt_ggml_bytes(
        &flat_input,
        weight_bytes,
        GGML_TYPE_BF16,
        rows,
        in_dim,
        out_dim,
    ) {
        return reshape_rows(out_flat, rows, out_dim);
    }

    let mut out = Vec::with_capacity(rows);
    for row in input {
        let mut out_row = vec![0.0f32; out_dim];
        for (out_idx, slot) in out_row.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            let base = out_idx * in_dim * 2;
            for (in_idx, input_value) in row.iter().copied().enumerate().take(in_dim) {
                acc += input_value * bf16_from_bytes(weight_bytes, base + in_idx * 2);
            }
            *slot = acc;
        }
        out.push(out_row);
    }
    Ok(out)
}

fn rms_norm_weighted_rows(input: &[Vec<f32>], weight: &[f32], eps: f32) -> Result<Vec<Vec<f32>>> {
    let hidden_size = weight.len();
    let (flat_input, rows, width) = flatten_rows(input)?;
    if width != hidden_size {
        return Err(invalid_model("rms norm row length mismatch"));
    }
    if rows == 0 {
        return Ok(Vec::new());
    }
    if let Some(out_flat) =
        try_rms_norm_mul_f32(&flat_input, &[rows, hidden_size], weight, &[hidden_size], eps)
    {
        return reshape_rows(out_flat, rows, hidden_size);
    }
    let mut out = Vec::with_capacity(input.len());
    for row in input {
        if row.len() != hidden_size {
            return Err(invalid_model("rms norm row length mismatch"));
        }
        let mean_square = row.iter().map(|value| value * value).sum::<f32>() / hidden_size as f32;
        let inv_rms = 1.0 / (mean_square + eps).sqrt();
        let mut out_row = Vec::with_capacity(hidden_size);
        for idx in 0..hidden_size {
            out_row.push(row[idx] * inv_rms * weight[idx]);
        }
        out.push(out_row);
    }
    Ok(out)
}

fn rms_norm_partitioned_rows(
    input: &[Vec<f32>],
    num_heads: usize,
    head_dim: usize,
    weight: &[f32],
    eps: f32,
) -> Result<Vec<Vec<f32>>> {
    if weight.len() != head_dim {
        return Err(invalid_model("partitioned rms norm weight length mismatch"));
    }
    let (flat_input, rows, width) = flatten_rows(input)?;
    if width != num_heads * head_dim {
        return Err(invalid_model("partitioned rms norm row length mismatch"));
    }
    if rows == 0 {
        return Ok(Vec::new());
    }
    if let Some(out_flat) = try_rms_norm_mul_f32(
        &flat_input,
        &[rows * num_heads, head_dim],
        weight,
        &[head_dim],
        eps,
    ) {
        return reshape_rows(out_flat, rows, width);
    }
    let mut out = Vec::with_capacity(input.len());
    for row in input {
        if row.len() != num_heads * head_dim {
            return Err(invalid_model("partitioned rms norm row length mismatch"));
        }
        let mut out_row = row.clone();
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
                out_row[start + dim] = row[start + dim] * inv_rms * weight[dim];
            }
        }
        out.push(out_row);
    }
    Ok(out)
}

fn rms_norm_partitioned_rows_no_scale(
    input: &[Vec<f32>],
    num_heads: usize,
    head_dim: usize,
    eps: f32,
) -> Result<Vec<Vec<f32>>> {
    let (flat_input, rows, width) = flatten_rows(input)?;
    if width != num_heads * head_dim {
        return Err(invalid_model("partitioned rms norm row length mismatch"));
    }
    if rows == 0 {
        return Ok(Vec::new());
    }
    if let Some(out_flat) = try_rms_norm_f32(&flat_input, &[rows * num_heads, head_dim], eps)
    {
        return reshape_rows(out_flat, rows, width);
    }
    let mut out = Vec::with_capacity(input.len());
    for row in input {
        if row.len() != num_heads * head_dim {
            return Err(invalid_model("partitioned rms norm row length mismatch"));
        }
        let mut out_row = row.clone();
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
                out_row[dim] = row[dim] * inv_rms;
            }
        }
        out.push(out_row);
    }
    Ok(out)
}

fn apply_2d_rope_rows(
    input: &[Vec<f32>],
    positions: &[[usize; 2]],
    num_heads: usize,
    head_dim: usize,
    rope_theta: f32,
) -> Vec<Vec<f32>> {
    let ndim = 2usize;
    let channels_per_dim = 2 * (head_dim / (2 * ndim));
    let half_per_dim = channels_per_dim / 2;
    let mut out = Vec::with_capacity(input.len());
    for (row, [x, y]) in input.iter().zip(positions.iter().copied()) {
        let mut out_row = row.clone();
        for head_idx in 0..num_heads {
            let head_start = head_idx * head_dim;
            for dim_axis in 0..2 {
                let axis_position = if dim_axis == 0 { x as f32 } else { y as f32 };
                let part_start = head_start + dim_axis * channels_per_dim;
                for pair_idx in 0..half_per_dim {
                    let exponent = (2.0 / channels_per_dim as f32) * pair_idx as f32;
                    let timescale = rope_theta.powf(exponent);
                    let angle = axis_position / timescale;
                    let cos = angle.cos();
                    let sin = angle.sin();
                    let a = row[part_start + pair_idx];
                    let b = row[part_start + half_per_dim + pair_idx];
                    out_row[part_start + pair_idx] = a * cos - b * sin;
                    out_row[part_start + half_per_dim + pair_idx] = b * cos + a * sin;
                }
            }
        }
        out.push(out_row);
    }
    out
}

fn attention_rows(
    q: &[Vec<f32>],
    k: &[Vec<f32>],
    v: &[Vec<f32>],
    num_heads: usize,
    num_kv_heads: usize,
    head_dim: usize,
) -> Result<Vec<Vec<f32>>> {
    let seq_len = q.len();
    if k.len() != seq_len || v.len() != seq_len {
        return Err(invalid_model("attention q/k/v sequence length mismatch"));
    }
    let q_width = q.first().map(|row| row.len()).unwrap_or(0);
    let kv_width = k.first().map(|row| row.len()).unwrap_or(0);
    if q_width != num_heads * head_dim || kv_width != num_kv_heads * head_dim {
        return Err(invalid_model("attention q/k/v head width mismatch"));
    }
    if seq_len == 0 {
        return Ok(Vec::new());
    }
    if num_heads == num_kv_heads {
        let (q_flat, _, _) = flatten_rows(q)?;
        let (k_flat, _, _) = flatten_rows(k)?;
        let (v_flat, _, _) = flatten_rows(v)?;
        if let Some(out_flat) =
            try_flash_attn_f32_packed(&q_flat, &k_flat, &v_flat, seq_len, seq_len, num_heads, head_dim, 1.0)
        {
            return reshape_rows(out_flat, seq_len, q_width);
        }
    }
    let q_heads_per_kv = num_heads / num_kv_heads;
    let mut out = Vec::with_capacity(seq_len);
    for q_idx in 0..seq_len {
        let mut out_row = vec![0.0f32; num_heads * head_dim];
        for head_idx in 0..num_heads {
            let kv_head_idx = head_idx / q_heads_per_kv;
            let q_start = head_idx * head_dim;
            let k_start = kv_head_idx * head_dim;
            let mut logits = vec![0.0f32; seq_len];
            let mut max_logit = f32::NEG_INFINITY;
            for k_idx in 0..seq_len {
                let mut dot = 0.0f32;
                for dim in 0..head_dim {
                    dot += q[q_idx][q_start + dim] * k[k_idx][k_start + dim];
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
                for dim in 0..head_dim {
                    out_row[q_start + dim] += weight * v[k_idx][v_start + dim];
                }
            }
        }
        out.push(out_row);
    }
    Ok(out)
}

fn add_rows(lhs: &[Vec<f32>], rhs: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
    if lhs.len() != rhs.len() {
        return Err(invalid_model("row add length mismatch"));
    }
    let (lhs_flat, rows, width) = flatten_rows(lhs)?;
    let (rhs_flat, rhs_rows, rhs_width) = flatten_rows(rhs)?;
    if rows != rhs_rows || width != rhs_width {
        return Err(invalid_model("row add width mismatch"));
    }
    if rows == 0 {
        return Ok(Vec::new());
    }
    if let Some(out_flat) = try_add_f32(&lhs_flat, &[rows, width], &rhs_flat, &[rows, width]) {
        return reshape_rows(out_flat, rows, width);
    }
    let mut out = Vec::with_capacity(lhs.len());
    for (lhs_row, rhs_row) in lhs.iter().zip(rhs.iter()) {
        if lhs_row.len() != rhs_row.len() {
            return Err(invalid_model("row add width mismatch"));
        }
        out.push(
            lhs_row
                .iter()
                .zip(rhs_row.iter())
                .map(|(lhs_value, rhs_value)| lhs_value + rhs_value)
                .collect(),
        );
    }
    Ok(out)
}

fn geglu_rows(gate: &[Vec<f32>], up: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
    let (gate_flat, rows, width) = flatten_rows(gate)?;
    let (up_flat, up_rows, up_width) = flatten_rows(up)?;
    if rows != up_rows || width != up_width {
        return Err(invalid_model("geglu width mismatch"));
    }
    if rows == 0 {
        return Ok(Vec::new());
    }
    if let Some(gelu_flat) = try_gelu_f32(&gate_flat, &[rows, width]) {
        if let Some(out_flat) = try_mul_f32(&gelu_flat, &[rows, width], &up_flat, &[rows, width]) {
            return reshape_rows(out_flat, rows, width);
        }
    }
    let mut out = Vec::with_capacity(rows);
    for (gate_row, up_row) in gate.iter().zip(up.iter()) {
        let mut row = Vec::with_capacity(width);
        for idx in 0..width {
            row.push(gelu_approx(gate_row[idx]) * up_row[idx]);
        }
        out.push(row);
    }
    Ok(out)
}

fn flatten_rows(rows: &[Vec<f32>]) -> Result<(Vec<f32>, usize, usize)> {
    let row_count = rows.len();
    let row_width = rows.first().map(|row| row.len()).unwrap_or(0);
    let mut flat = Vec::with_capacity(
        row_count
            .checked_mul(row_width)
            .ok_or_else(|| invalid_model("row flatten overflow"))?,
    );
    for row in rows {
        if row.len() != row_width {
            return Err(invalid_model("ragged rows are not supported"));
        }
        flat.extend_from_slice(row);
    }
    Ok((flat, row_count, row_width))
}

fn reshape_rows(flat: Vec<f32>, row_count: usize, row_width: usize) -> Result<Vec<Vec<f32>>> {
    if flat.len()
        != row_count
            .checked_mul(row_width)
            .ok_or_else(|| invalid_model("row reshape overflow"))?
    {
        return Err(invalid_model("reshape row size mismatch"));
    }
    let mut out = Vec::with_capacity(row_count);
    for chunk in flat.chunks_exact(row_width) {
        out.push(chunk.to_vec());
    }
    Ok(out)
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
