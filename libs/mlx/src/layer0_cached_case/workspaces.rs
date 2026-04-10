    fn load(session: &mut LayerExecutionSession, layer_idx: usize) -> Result<Self, Box<dyn Error>> {
        let indexed = session.weights.clone();
        let runtime = session.runtime.clone();
        let text_config = &indexed.snapshot.config.text_config;
        let kv_layout = GemmaKvCacheLayout::from_text_config(text_config, 1)?;
        let cache_spec = kv_layout.cache_spec_for_layer(layer_idx)?.clone();
        if !text_config.enable_moe_block {
            return Err("exact metal text runtime currently expects Gemma MoE layers".into());
        }
        if text_config.top_k_experts_or_zero() as usize != ROUTER_TOP_K {
            return Err(format!(
                "exact metal text runtime expects top_k_experts={}, got {}",
                ROUTER_TOP_K,
                text_config.top_k_experts_or_zero()
            )
            .into());
        }

        let layer_type = text_config
            .layer_types
            .get(layer_idx)
            .ok_or_else(|| format!("missing text layer type for layer {layer_idx}"))?;
        let attention_k_eq_v = text_config.attention_k_eq_v && layer_type == "full_attention";
        let layer_names = LayerTensorNames::for_layer(layer_idx, attention_k_eq_v);
        let q_norm_weight_name = layer_names
            .q
            .norm_weight_name
            .as_deref()
            .ok_or("missing q norm weight name")?;
        let k_norm_weight_name = layer_names
            .k
            .norm_weight_name
            .as_deref()
            .ok_or("missing k norm weight name")?;

        let q_weight_entry = indexed.tensor(&layer_names.q.weight_name)?;
        let q_scales_entry = indexed.tensor(&layer_names.q.scales_name)?;
        let q_norm_weight_entry = indexed.tensor(q_norm_weight_name)?;
        let k_weight_entry = indexed.tensor(&layer_names.k.weight_name)?;
        let k_scales_entry = indexed.tensor(&layer_names.k.scales_name)?;
        let k_norm_weight_entry = indexed.tensor(k_norm_weight_name)?;
        let v_weight_entry = indexed.tensor(&layer_names.v.weight_name)?;
        let v_scales_entry = indexed.tensor(&layer_names.v.scales_name)?;
        let o_weight_entry = indexed.tensor(&layer_names.o.weight_name)?;
        let o_scales_entry = indexed.tensor(&layer_names.o.scales_name)?;
        let mlp_gate_weight_entry = indexed.tensor(&layer_names.mlp_gate_weight_name)?;
        let mlp_gate_scales_entry = indexed.tensor(&layer_names.mlp_gate_scales_name)?;
        let mlp_up_weight_entry = indexed.tensor(&layer_names.mlp_up_weight_name)?;
        let mlp_up_scales_entry = indexed.tensor(&layer_names.mlp_up_scales_name)?;
        let mlp_down_weight_entry = indexed.tensor(&layer_names.mlp_down_weight_name)?;
        let mlp_down_scales_entry = indexed.tensor(&layer_names.mlp_down_scales_name)?;
        let router_proj_weight_entry = indexed.tensor(&layer_names.router_proj_weight_name)?;
        let router_proj_scales_entry = indexed.tensor(&layer_names.router_proj_scales_name)?;
        let expert_gate_weight_entry = indexed.tensor(&layer_names.expert_gate_weight_name)?;
        let expert_gate_scales_entry = indexed.tensor(&layer_names.expert_gate_scales_name)?;
        let expert_up_weight_entry = indexed.tensor(&layer_names.expert_up_weight_name)?;
        let expert_up_scales_entry = indexed.tensor(&layer_names.expert_up_scales_name)?;
        let expert_down_weight_entry = indexed.tensor(&layer_names.expert_down_weight_name)?;
        let expert_down_scales_entry = indexed.tensor(&layer_names.expert_down_scales_name)?;

        let q_proj = ExactMetalQprojLayout {
            weight_words_per_row: q_weight_entry.shape[1] as u32,
            qparams_per_row: q_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(q_weight_entry.shape[0])?,
        };
        let k_proj = ExactMetalQprojLayout {
            weight_words_per_row: k_weight_entry.shape[1] as u32,
            qparams_per_row: k_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(k_weight_entry.shape[0])?,
        };
        let v_proj = ExactMetalQprojLayout {
            weight_words_per_row: v_weight_entry.shape[1] as u32,
            qparams_per_row: v_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(v_weight_entry.shape[0])?,
        };
        if q_proj.weight_words_per_row != k_proj.weight_words_per_row
            || q_proj.weight_words_per_row != v_proj.weight_words_per_row
            || q_proj.qparams_per_row != k_proj.qparams_per_row
            || q_proj.qparams_per_row != v_proj.qparams_per_row
        {
            return Err(format!(
                "q/k/v projection layout mismatch in layer {layer_idx}: q=({}, {}) k=({}, {}) v=({}, {})",
                q_proj.weight_words_per_row,
                q_proj.qparams_per_row,
                k_proj.weight_words_per_row,
                k_proj.qparams_per_row,
                v_proj.weight_words_per_row,
                v_proj.qparams_per_row,
            )
            .into());
        }
        let qkv_proj = ExactMetalQprojLayout {
            weight_words_per_row: q_proj.weight_words_per_row,
            qparams_per_row: q_proj.qparams_per_row,
            out_rows: q_proj
                .out_rows
                .checked_add(k_proj.out_rows)
                .and_then(|value| value.checked_add(v_proj.out_rows))
                .ok_or("qkv combined out_rows overflow")?,
        };
        let o_proj = ExactMetalQprojLayout {
            weight_words_per_row: o_weight_entry.shape[1] as u32,
            qparams_per_row: o_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(o_weight_entry.shape[0])?,
        };
        let mlp_gate = ExactMetalQprojLayout {
            weight_words_per_row: mlp_gate_weight_entry.shape[1] as u32,
            qparams_per_row: mlp_gate_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(mlp_gate_weight_entry.shape[0])?,
        };
        let mlp_up = ExactMetalQprojLayout {
            weight_words_per_row: mlp_up_weight_entry.shape[1] as u32,
            qparams_per_row: mlp_up_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(mlp_up_weight_entry.shape[0])?,
        };
        if mlp_gate.weight_words_per_row != mlp_up.weight_words_per_row
            || mlp_gate.qparams_per_row != mlp_up.qparams_per_row
        {
            return Err(format!(
                "dense MLP gate/up layout mismatch in layer {layer_idx}: gate=({}, {}) up=({}, {})",
                mlp_gate.weight_words_per_row,
                mlp_gate.qparams_per_row,
                mlp_up.weight_words_per_row,
                mlp_up.qparams_per_row,
            )
            .into());
        }
        let mlp_gate_up = ExactMetalQprojLayout {
            weight_words_per_row: mlp_gate.weight_words_per_row,
            qparams_per_row: mlp_gate.qparams_per_row,
            out_rows: mlp_gate
                .out_rows
                .checked_add(mlp_up.out_rows)
                .ok_or("mlp gate/up combined out_rows overflow")?,
        };
        let mlp_down = ExactMetalQprojLayout {
            weight_words_per_row: mlp_down_weight_entry.shape[1] as u32,
            qparams_per_row: mlp_down_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(mlp_down_weight_entry.shape[0])?,
        };
        let router_proj = ExactMetalQprojLayout {
            weight_words_per_row: router_proj_weight_entry.shape[1] as u32,
            qparams_per_row: router_proj_scales_entry.shape[1] as u32,
            out_rows: u32::try_from(router_proj_weight_entry.shape[0])?,
        };
        let expert_gate = ExactMetalQprojLayout {
            weight_words_per_row: expert_gate_weight_entry.shape[2] as u32,
            qparams_per_row: expert_gate_scales_entry.shape[2] as u32,
            out_rows: u32::try_from(expert_gate_weight_entry.shape[1])?,
        };
        let expert_up = ExactMetalQprojLayout {
            weight_words_per_row: expert_up_weight_entry.shape[2] as u32,
            qparams_per_row: expert_up_scales_entry.shape[2] as u32,
            out_rows: u32::try_from(expert_up_weight_entry.shape[1])?,
        };
        if expert_gate.weight_words_per_row != expert_up.weight_words_per_row
            || expert_gate.qparams_per_row != expert_up.qparams_per_row
            || expert_gate.out_rows != expert_up.out_rows
        {
            return Err(format!(
                "expert gate/up layout mismatch in layer {layer_idx}: gate=({}, {}, {}) up=({}, {}, {})",
                expert_gate.weight_words_per_row,
                expert_gate.qparams_per_row,
                expert_gate.out_rows,
                expert_up.weight_words_per_row,
                expert_up.qparams_per_row,
                expert_up.out_rows,
            )
            .into());
        }
        let expert_gate_up = ExactMetalQprojLayout {
            weight_words_per_row: expert_gate.weight_words_per_row,
            qparams_per_row: expert_gate.qparams_per_row,
            out_rows: expert_gate
                .out_rows
                .checked_add(expert_up.out_rows)
                .ok_or("expert gate/up combined out_rows overflow")?,
        };
        let expert_down = ExactMetalQprojLayout {
            weight_words_per_row: expert_down_weight_entry.shape[2] as u32,
            qparams_per_row: expert_down_scales_entry.shape[2] as u32,
            out_rows: u32::try_from(expert_down_weight_entry.shape[1])?,
        };

        let post_attention_norm_len = usize::try_from(
            indexed
                .tensor(&layer_names.post_attention_norm_weight_name)?
                .shape[0],
        )?;
        let pre_feedforward_norm_len = usize::try_from(
            indexed
                .tensor(&layer_names.pre_feedforward_norm_weight_name)?
                .shape[0],
        )?;
        let pre_feedforward_norm2_len = usize::try_from(
            indexed
                .tensor(&layer_names.pre_feedforward_norm2_weight_name)?
                .shape[0],
        )?;
        let post_feedforward_norm1_len = usize::try_from(
            indexed
                .tensor(&layer_names.post_feedforward_norm1_weight_name)?
                .shape[0],
        )?;
        let post_feedforward_norm2_len = usize::try_from(
            indexed
                .tensor(&layer_names.post_feedforward_norm2_weight_name)?
                .shape[0],
        )?;
        if post_attention_norm_len != NORM_LEN
            || pre_feedforward_norm_len != NORM_LEN
            || pre_feedforward_norm2_len != NORM_LEN
            || post_feedforward_norm1_len != NORM_LEN
            || post_feedforward_norm2_len != NORM_LEN
            || o_proj.out_len() != NORM_LEN
            || mlp_down.out_len() != NORM_LEN
            || expert_down.out_len() != NORM_LEN
        {
            return Err(format!(
                "exact metal text runtime expects hidden-size-preserving layer {layer_idx}"
            )
            .into());
        }

        let head_dim = usize::try_from(q_norm_weight_entry.shape[0])?;
        let k_head_dim = usize::try_from(k_norm_weight_entry.shape[0])?;
        if head_dim == 0 || head_dim != k_head_dim {
            return Err(format!("invalid q/k head_dim: q={head_dim} k={k_head_dim}").into());
        }
        if q_proj.out_len() % head_dim != 0
            || k_proj.out_len() % head_dim != 0
            || v_proj.out_len() % head_dim != 0
        {
            return Err(format!(
                "invalid q/k/v head layout: q_out_len={} k_out_len={} v_out_len={} head_dim={}",
                q_proj.out_len(),
                k_proj.out_len(),
                v_proj.out_len(),
                head_dim
            )
            .into());
        }
        let q_head_count = q_proj.out_len() / head_dim;
        let k_head_count = k_proj.out_len() / head_dim;
        let v_head_count = v_proj.out_len() / head_dim;
        if k_head_count == 0 || v_head_count != k_head_count || q_head_count % k_head_count != 0 {
            return Err(format!(
                "invalid grouped-query head layout: q_head_count={} k_head_count={} v_head_count={}",
                q_head_count, k_head_count, v_head_count
            )
            .into());
        }
        let q_heads_per_kv = q_head_count / k_head_count;
        let rope_params = if layer_type == "full_attention" {
            &text_config.rope_parameters.full_attention
        } else {
            &text_config.rope_parameters.sliding_attention
        };
        let rope_rotary_dim = if let Some(partial_factor) = rope_params.partial_rotary_factor {
            let rotary_dim = (head_dim as f32 * partial_factor).round() as usize;
            if rotary_dim == 0 || rotary_dim > head_dim || rotary_dim % 2 != 0 {
                return Err(format!(
                    "invalid rope rotary dim {} for layer {} head_dim {} factor {}",
                    rotary_dim, layer_idx, head_dim, partial_factor
                )
                .into());
            }
            rotary_dim
        } else {
            head_dim
        };
        let rope_half_dims = rope_rotary_dim / 2;
        let rope_base_log2 = (rope_params.rope_theta as f32).log2();

        let buffers = ExactMetalLayerBuffers {
            x: create_bf16_buffer(&runtime, NORM_LEN, BufferStorageMode::Shared)?,
            h: create_bf16_buffer(&runtime, NORM_LEN, BufferStorageMode::Private)?,
            qkv_proj_out: create_bf16_buffer(
                &runtime,
                qkv_proj.out_len(),
                BufferStorageMode::Private,
            )?,
            q_norm: create_bf16_buffer(&runtime, q_proj.out_len(), BufferStorageMode::Private)?,
            q_rope: create_bf16_buffer(&runtime, q_proj.out_len(), BufferStorageMode::Private)?,
            k_norm: create_bf16_buffer(&runtime, k_proj.out_len(), BufferStorageMode::Private)?,
            k_rope: create_bf16_buffer(&runtime, k_proj.out_len(), BufferStorageMode::Private)?,
            v_norm: create_bf16_buffer(&runtime, v_proj.out_len(), BufferStorageMode::Private)?,
            attention_logits: create_bf16_buffer(
                &runtime,
                q_head_count * cache_spec.max_tokens,
                BufferStorageMode::Private,
            )?,
            attention_probs: create_bf16_buffer(
                &runtime,
                q_head_count * cache_spec.max_tokens,
                BufferStorageMode::Private,
            )?,
            attn_out: create_bf16_buffer(&runtime, q_proj.out_len(), BufferStorageMode::Private)?,
            o_proj_out: create_bf16_buffer(&runtime, o_proj.out_len(), BufferStorageMode::Private)?,
            post_attention_norm_out: create_bf16_buffer(
                &runtime,
                post_attention_norm_len,
                BufferStorageMode::Private,
            )?,
            residual_out: create_bf16_buffer(
                &runtime,
                post_attention_norm_len,
                BufferStorageMode::Private,
            )?,
            pre_feedforward_norm_out: create_bf16_buffer(
                &runtime,
                pre_feedforward_norm_len,
                BufferStorageMode::Private,
            )?,
            mlp_gate_up_out: create_bf16_buffer(
                &runtime,
                mlp_gate_up.out_len(),
                BufferStorageMode::Private,
            )?,
            geglu_out: create_bf16_buffer(
                &runtime,
                mlp_gate.out_len(),
                BufferStorageMode::Private,
            )?,
            mlp_down_out: create_bf16_buffer(
                &runtime,
                mlp_down.out_len(),
                BufferStorageMode::Private,
            )?,
            router_scaled_out: create_bf16_buffer(
                &runtime,
                post_attention_norm_len,
                BufferStorageMode::Private,
            )?,
            router_proj_out: create_bf16_buffer(
                &runtime,
                router_proj.out_len(),
                BufferStorageMode::Private,
            )?,
            router_probs_out: create_bf16_buffer(
                &runtime,
                router_proj.out_len(),
                BufferStorageMode::Private,
            )?,
            pre_feedforward_norm2_out: create_bf16_buffer(
                &runtime,
                pre_feedforward_norm2_len,
                BufferStorageMode::Private,
            )?,
            moe_top_k_indices: runtime
                .create_buffer(ROUTER_TOP_K * size_of::<u32>(), BufferStorageMode::Private)?,
            moe_top_k_weights: create_bf16_buffer(
                &runtime,
                ROUTER_TOP_K,
                BufferStorageMode::Private,
            )?,
            expert_gate_up_out: create_bf16_buffer(
                &runtime,
                ROUTER_TOP_K * expert_gate_up.out_len(),
                BufferStorageMode::Private,
            )?,
            expert_geglu_out: create_bf16_buffer(
                &runtime,
                ROUTER_TOP_K * expert_gate.out_len(),
                BufferStorageMode::Private,
            )?,
            expert_down_out: create_bf16_buffer(
                &runtime,
                ROUTER_TOP_K * expert_down.out_len(),
                BufferStorageMode::Private,
            )?,
            post_feedforward_norm1_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm1_len,
                BufferStorageMode::Private,
            )?,
            moe_weighted_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm2_len,
                BufferStorageMode::Private,
            )?,
            moe_post_ffn_norm2_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm2_len,
                BufferStorageMode::Private,
            )?,
            moe_merge_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm1_len,
                BufferStorageMode::Private,
            )?,
            post_ffn_residual_out: create_bf16_buffer(
                &runtime,
                post_feedforward_norm1_len,
                BufferStorageMode::Shared,
            )?,
        };

        let v_norm_weight = runtime.create_buffer_with_bytes(
            &bytes_from_bf16_words(&vec![0x3F80u16; head_dim]),
            BufferStorageMode::Private,
        )?;
        let weights = ExactMetalLayerWeights {
            input_norm_weight: session
                .private_weight_buffer(&layer_names.input_norm_weight_name)?,
            qkv_proj_weight: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.q.weight_name,
                    &layer_names.k.weight_name,
                    &layer_names.v.weight_name,
                ],
            )?,
            qkv_proj_scales: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.q.scales_name,
                    &layer_names.k.scales_name,
                    &layer_names.v.scales_name,
                ],
            )?,
            qkv_proj_biases: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.q.biases_name,
                    &layer_names.k.biases_name,
                    &layer_names.v.biases_name,
                ],
            )?,
            q_norm_weight: session.private_weight_buffer(q_norm_weight_name)?,
            k_norm_weight: session.private_weight_buffer(k_norm_weight_name)?,
            v_norm_weight,
            o_weight: session.private_weight_buffer(&layer_names.o.weight_name)?,
            o_scales: session.private_weight_buffer(&layer_names.o.scales_name)?,
            o_biases: session.private_weight_buffer(&layer_names.o.biases_name)?,
            post_attention_norm_weight: session
                .private_weight_buffer(&layer_names.post_attention_norm_weight_name)?,
            pre_feedforward_norm_weight: session
                .private_weight_buffer(&layer_names.pre_feedforward_norm_weight_name)?,
            pre_feedforward_norm2_weight: session
                .private_weight_buffer(&layer_names.pre_feedforward_norm2_weight_name)?,
            mlp_gate_up_weight: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.mlp_gate_weight_name,
                    &layer_names.mlp_up_weight_name,
                ],
            )?,
            mlp_gate_up_scales: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.mlp_gate_scales_name,
                    &layer_names.mlp_up_scales_name,
                ],
            )?,
            mlp_gate_up_biases: create_private_buffer_with_concatenated_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.mlp_gate_biases_name,
                    &layer_names.mlp_up_biases_name,
                ],
            )?,
            mlp_down_weight: session.private_weight_buffer(&layer_names.mlp_down_weight_name)?,
            mlp_down_scales: session.private_weight_buffer(&layer_names.mlp_down_scales_name)?,
            mlp_down_biases: session.private_weight_buffer(&layer_names.mlp_down_biases_name)?,
            router_scale_weight: session.private_weight_buffer(&layer_names.router_scale_name)?,
            router_proj_weight: session
                .private_weight_buffer(&layer_names.router_proj_weight_name)?,
            router_proj_scales: session
                .private_weight_buffer(&layer_names.router_proj_scales_name)?,
            router_proj_biases: session
                .private_weight_buffer(&layer_names.router_proj_biases_name)?,
            router_per_expert_scale: session
                .private_weight_buffer(&layer_names.router_per_expert_scale_name)?,
            expert_gate_up_weight: create_private_buffer_with_concatenated_expert_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.expert_gate_weight_name,
                    &layer_names.expert_up_weight_name,
                ],
            )?,
            expert_gate_up_scales: create_private_buffer_with_concatenated_expert_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.expert_gate_scales_name,
                    &layer_names.expert_up_scales_name,
                ],
            )?,
            expert_gate_up_biases: create_private_buffer_with_concatenated_expert_tensors(
                &runtime,
                &indexed,
                &[
                    &layer_names.expert_gate_biases_name,
                    &layer_names.expert_up_biases_name,
                ],
            )?,
            expert_down_weight: session
                .private_weight_buffer(&layer_names.expert_down_weight_name)?,
            expert_down_scales: session
                .private_weight_buffer(&layer_names.expert_down_scales_name)?,
            expert_down_biases: session
                .private_weight_buffer(&layer_names.expert_down_biases_name)?,
            post_feedforward_norm1_weight: session
                .private_weight_buffer(&layer_names.post_feedforward_norm1_weight_name)?,
            post_feedforward_norm2_weight: session
                .private_weight_buffer(&layer_names.post_feedforward_norm2_weight_name)?,
        };

        let pipelines = ExactMetalLayerPipelines {
            rms: compile_default_pipeline(&runtime, "kernel_mlx_rms_norm_row_bf16")?,
            proj: compile_default_pipeline(&runtime, "kernel_mlx_affine_qmv_row_bf16")?,
            proj_fast: compile_default_pipeline(&runtime, "kernel_mlx_affine_qmv_fast_row_bf16")?,
            head_norm: compile_default_pipeline(&runtime, "kernel_mlx_rms_norm_rows_bf16")?,
            rope: compile_default_pipeline(&runtime, "kernel_mlx_rope_single_bf16")?,
            attention_logits_seq: compile_default_pipeline(
                &runtime,
                "kernel_mlx_gqa_attention_logits_seq_bf16",
            )?,
            attention_softmax_rows: compile_default_pipeline(
                &runtime,
                "kernel_mlx_softmax_rows_bf16",
            )?,
            attention_weighted_sum: compile_default_pipeline(
                &runtime,
                "kernel_mlx_gqa_attention_weighted_sum_bf16",
            )?,
            o_proj_fast: compile_default_pipeline(&runtime, "kernel_mlx_affine_qmv_fast_row_bf16")?,
            residual: compile_default_pipeline(&runtime, "kernel_mlx_add_row_bf16")?,
            weighted_sum_rows: compile_default_pipeline(
                &runtime,
                "kernel_mlx_weighted_sum_rows_bf16",
            )?,
            geglu: compile_default_pipeline(&runtime, "kernel_mlx_geglu_row_bf16")?,
            geglu_strided: compile_default_pipeline(
                &runtime,
                "kernel_mlx_geglu_strided_rows_bf16",
            )?,
            router_scale_pair: compile_default_pipeline(
                &runtime,
                "kernel_mlx_router_scale_pair_bf16",
            )?,
            router_topk: compile_default_pipeline(&runtime, "kernel_mlx_router_topk_bf16")?,
            selected_expert_proj: compile_default_pipeline(
                &runtime,
                "kernel_mlx_affine_qmv_selected_experts_row_bf16",
            )?,
        };

        Ok(Self {
            qkv_proj,
            q_proj,
            k_proj,
            o_proj,
            mlp_gate_up,
            mlp_gate,
            mlp_down,
            router_proj,
            expert_gate_up,
            expert_gate,
            expert_down,
            post_attention_norm_len,
            pre_feedforward_norm_len,
            pre_feedforward_norm2_len,
            post_feedforward_norm1_len,
            post_feedforward_norm2_len,
            q_head_count,
            k_head_count,
            v_head_count,
            q_heads_per_kv,
            head_dim,
            kv_cache_capacity_tokens: cache_spec.max_tokens,
            eps: text_config.rms_norm_eps,
            q_rope: ExactMetalRopeLayout {
                half_dims: rope_half_dims as u32,
                row_stride: head_dim as u32,
                row_count: q_head_count as u32,
                base_log2: rope_base_log2,
            },
            k_rope: ExactMetalRopeLayout {
                half_dims: rope_half_dims as u32,
                row_stride: head_dim as u32,
                row_count: k_head_count as u32,
                base_log2: rope_base_log2,
            },
            buffers,
            weights,
            pipelines,
        })
    }
}

impl ExactMetalTextIoWorkspace {
    fn load(session: &mut LayerExecutionSession) -> Result<Self, Box<dyn Error>> {
        let indexed = session.weights.clone();
        let runtime = session.runtime.clone();
        let embed_weight_entry = indexed.tensor(EMBED_TOKENS_WEIGHT_NAME)?;
        let embed_scales_entry = indexed.tensor(EMBED_TOKENS_SCALES_NAME)?;
        let embed_biases_entry = indexed.tensor(EMBED_TOKENS_BIASES_NAME)?;
        let final_norm_entry = indexed.tensor(FINAL_TEXT_NORM_WEIGHT_NAME)?;
        if embed_weight_entry.shape.len() != 2
            || embed_scales_entry.shape.len() != 2
            || embed_biases_entry.shape.len() != 2
        {
            return Err("exact text IO expects rank-2 embed tensors".into());
        }
        if final_norm_entry.shape.len() != 1 {
            return Err("exact text IO expects rank-1 final norm weight".into());
        }
        let hidden_size = usize::try_from(final_norm_entry.shape[0])?;
        if hidden_size != NORM_LEN {
            return Err(format!(
                "exact text IO hidden size mismatch: got {} expected {}",
                hidden_size, NORM_LEN
            )
            .into());
        }
        if embed_weight_entry.shape[0] != embed_scales_entry.shape[0]
            || embed_weight_entry.shape[0] != embed_biases_entry.shape[0]
            || embed_scales_entry.shape != embed_biases_entry.shape
        {
            return Err("exact text IO embed tensor shape mismatch".into());
        }
        let logits_qproj = ExactMetalQprojLayout {
            weight_words_per_row: u32::try_from(embed_weight_entry.shape[1])?,
            qparams_per_row: u32::try_from(embed_scales_entry.shape[1])?,
            out_rows: u32::try_from(embed_weight_entry.shape[0])?,
        };
        let embed_weight_row_bytes = usize::try_from(
            embed_weight_entry.shape[1]
                .checked_mul(embed_weight_entry.dtype.byte_width())
                .ok_or("exact text IO embed weight row stride overflow")?,
        )?;
        let embed_qparams_row_bytes = usize::try_from(
            embed_scales_entry.shape[1]
                .checked_mul(embed_scales_entry.dtype.byte_width())
                .ok_or("exact text IO embed qparams row stride overflow")?,
        )?;
        let vocab_size = usize::try_from(embed_weight_entry.shape[0])?;

        Ok(Self {
            embed_weight_row_bytes,
            embed_qparams_row_bytes,
            logits_qproj,
            vocab_size,
            eps: indexed.snapshot.config.text_config.rms_norm_eps,
            softcap: Some(indexed.snapshot.config.text_config.final_logit_softcapping)
                .filter(|softcap| *softcap > 0.0),
            buffers: ExactMetalTextIoBuffers {
                standalone_hidden: create_bf16_buffer(
                    &runtime,
                    NORM_LEN,
                    BufferStorageMode::Private,
                )?,
                hidden_scratch: create_bf16_buffer(&runtime, NORM_LEN, BufferStorageMode::Private)?,
                final_norm_out: create_bf16_buffer(&runtime, NORM_LEN, BufferStorageMode::Private)?,
                logits_out: create_bf16_buffer(&runtime, vocab_size, BufferStorageMode::Shared)?,
                argmax_index_out: runtime
                    .create_buffer(size_of::<u32>(), BufferStorageMode::Shared)?,
                generated_token_chunk_out: runtime.create_buffer(
                    DEVICE_GREEDY_DECODE_CHUNK_TOKENS * size_of::<u32>(),
                    BufferStorageMode::Shared,
                )?,
            },
            weights: ExactMetalTextIoWeights {
                embed_weight: session.private_weight_buffer(EMBED_TOKENS_WEIGHT_NAME)?,
                embed_scales: session.private_weight_buffer(EMBED_TOKENS_SCALES_NAME)?,
                embed_biases: session.private_weight_buffer(EMBED_TOKENS_BIASES_NAME)?,
                final_norm_weight: session.private_weight_buffer(FINAL_TEXT_NORM_WEIGHT_NAME)?,
            },
            pipelines: ExactMetalTextIoPipelines {
                dequant_row: compile_default_pipeline(
                    &runtime,
                    "kernel_mlx_affine_dequant_row_bf16",
                )?,
                dequant_row_from_token_buffer: compile_default_pipeline(
                    &runtime,
                    "kernel_mlx_affine_dequant_row_from_token_buffer_bf16",
                )?,
                rms: compile_default_pipeline(&runtime, "kernel_mlx_rms_norm_row_bf16")?,
                logits_proj: compile_default_pipeline(&runtime, "kernel_mlx_affine_qmv_row_bf16")?,
                argmax_softcapped_bf16: compile_default_pipeline(
                    &runtime,
                    "kernel_mlx_argmax_softcapped_bf16_single",
                )?,
            },
        })
    }
}

pub(crate) struct ExactMetalTextRuntimeSession {
    session: LayerExecutionSession,
    kv_layout: GemmaKvCacheLayout,
    kv_caches: Vec<RefCell<ExactMetalKvCache>>,
    kv_append_pipeline: MetalPipeline,
    text_io: ExactMetalTextIoWorkspace,
    layer_workspaces: HashMap<usize, ExactMetalLayerWorkspace>,
}

#[derive(Clone, Debug)]
pub struct ExactMetalStageProfile {
    pub stage_name: &'static str,
    pub elapsed: Duration,
    pub counters: MetalRuntimeCounters,
}

#[derive(Clone, Debug)]
pub struct ExactMetalLayerProfile {
    pub layer_idx: usize,
    pub attention: GemmaAttentionKind,
    pub elapsed: Duration,
    pub counters: MetalRuntimeCounters,
}

#[derive(Clone, Debug)]
pub struct ExactMetalDecodeStepProfile {
    pub prompt_token_count: usize,
    pub first_generated_token_id: u32,
    pub profiled_token_id: u32,
    pub profiled_position: usize,
    pub embed: ExactMetalStageProfile,
    pub layers: Vec<ExactMetalLayerProfile>,
    pub head: ExactMetalStageProfile,
    pub head_stages: Vec<ExactMetalStageProfile>,
    pub predicted_token_id: u32,
}

fn sum_metal_runtime_counters(stages: &[ExactMetalStageProfile]) -> MetalRuntimeCounters {
