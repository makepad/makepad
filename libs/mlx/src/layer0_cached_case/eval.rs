    fn eval_layer_hidden_state_core(
        &mut self,
        layer_idx: usize,
        input_words: Option<&[u16]>,
        input_hidden_buffer: Option<&MetalBuffer>,
        output_hidden_buffer: Option<&MetalBuffer>,
        position: usize,
        read_output: bool,
    ) -> Result<Option<Vec<u16>>, Box<dyn Error>> {
        if let Some(input_words) = input_words {
            if input_words.len() != NORM_LEN {
                return Err(format!(
                    "exact metal layer input length mismatch: got {} expected {}",
                    input_words.len(),
                    NORM_LEN
                )
                .into());
            }
        }

        let runtime = self.session.runtime.clone();
        let workspace = self.layer_workspace(layer_idx)?;
        let input_hidden_buffer = input_hidden_buffer.unwrap_or(&workspace.buffers.x);
        let output_hidden_buffer =
            output_hidden_buffer.unwrap_or(&workspace.buffers.post_ffn_residual_out);
        let owns_command_batch = !runtime.command_batch_is_active();
        if read_output && !owns_command_batch {
            return Err(
                "cannot read exact layer output while Metal command batch is active".into(),
            );
        }

        let n_reads = 4usize;
        let simd_size = 32usize;
        let rms_threadgroup_size = simd_size * NORM_LEN.div_ceil(n_reads).div_ceil(simd_size);
        let head_norm_threadgroup_size =
            simd_size * workspace.head_dim.div_ceil(n_reads).div_ceil(simd_size);
        let rms_threadgroups = MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        };
        let rms_threads_per_threadgroup = MetalSize {
            width: rms_threadgroup_size as u64,
            height: 1,
            depth: 1,
        };
        let proj_threads_per_threadgroup = MetalSize {
            width: 32,
            height: 2,
            depth: 1,
        };
        let qkv_proj_threadgroups = MetalSize {
            width: 1,
            height: (workspace.qkv_proj.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let o_proj_threadgroups = MetalSize {
            width: 1,
            height: (workspace.o_proj.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let q_head_norm_threadgroups = MetalSize {
            width: workspace.q_head_count as u64,
            height: 1,
            depth: 1,
        };
        let k_head_norm_threadgroups = MetalSize {
            width: workspace.k_head_count as u64,
            height: 1,
            depth: 1,
        };
        let v_head_norm_threadgroups = MetalSize {
            width: workspace.v_head_count as u64,
            height: 1,
            depth: 1,
        };
        let head_norm_threads_per_threadgroup = MetalSize {
            width: head_norm_threadgroup_size as u64,
            height: 1,
            depth: 1,
        };
        let q_rope_threadgroups = MetalSize {
            width: (workspace.head_dim as u64).div_ceil(32),
            height: workspace.q_head_count as u64,
            depth: 1,
        };
        let k_rope_threadgroups = MetalSize {
            width: (workspace.head_dim as u64).div_ceil(32),
            height: workspace.k_head_count as u64,
            depth: 1,
        };
        let rope_threads_per_threadgroup = MetalSize {
            width: 32,
            height: 1,
            depth: 1,
        };
        let attention_output_threadgroups = MetalSize {
            width: (workspace.head_dim as u64).div_ceil(64),
            height: 1,
            depth: workspace.q_head_count as u64,
        };
        let attention_logits_threads_per_threadgroup = MetalSize {
            width: 32,
            height: 1,
            depth: 1,
        };
        let attention_output_threads_per_threadgroup = MetalSize {
            width: 32,
            height: 4,
            depth: 1,
        };
        let residual_threads_per_threadgroup = MetalSize {
            width: 256,
            height: 1,
            depth: 1,
        };
        let residual_threadgroups = MetalSize {
            width: (workspace.post_attention_norm_len as u64)
                .div_ceil(residual_threads_per_threadgroup.width),
            height: 1,
            depth: 1,
        };
        let mlp_gate_up_threadgroups = MetalSize {
            width: 1,
            height: (workspace.mlp_gate_up.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let mlp_down_threadgroups = MetalSize {
            width: 1,
            height: (workspace.mlp_down.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let geglu_threads_per_threadgroup = MetalSize {
            width: 256,
            height: 1,
            depth: 1,
        };
        let geglu_threadgroups = MetalSize {
            width: (workspace.mlp_gate.out_len() as u64)
                .div_ceil(geglu_threads_per_threadgroup.width),
            height: 1,
            depth: 1,
        };
        let router_scale_threads_per_threadgroup = MetalSize {
            width: rms_threadgroup_size as u64,
            height: 1,
            depth: 1,
        };
        let router_scale_threadgroups = MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        };
        let router_proj_threadgroups = MetalSize {
            width: 1,
            height: (workspace.router_proj.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let router_softmax_threadgroups = MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        };
        let router_topk_threadgroups = MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        };
        let router_topk_threads_per_threadgroup = MetalSize {
            width: 1,
            height: 1,
            depth: 1,
        };
        let selected_expert_threadgroups = MetalSize {
            width: ROUTER_TOP_K as u64,
            height: (workspace.expert_gate_up.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let selected_expert_down_threadgroups = MetalSize {
            width: ROUTER_TOP_K as u64,
            height: (workspace.expert_down.out_len() as u64).div_ceil(8),
            depth: 1,
        };
        let selected_expert_threads_per_threadgroup = MetalSize {
            width: 32,
            height: 2,
            depth: 1,
        };
        let expert_geglu_threadgroups = MetalSize {
            width: ((ROUTER_TOP_K * workspace.expert_gate.out_len()) as u64)
                .div_ceil(geglu_threads_per_threadgroup.width),
            height: 1,
            depth: 1,
        };

        let rms_args = MlxRmsNormRowArgs {
            n: NORM_LEN as u32,
            eps: workspace.eps,
        };
        let qkv_proj_args = workspace.qkv_proj.row_args(NORM_LEN as u32);
        let o_proj_args = workspace.o_proj.row_args(workspace.q_proj.out_rows);
        let q_head_norm_args = MlxRmsNormRowsArgs {
            n: workspace.head_dim as u32,
            row_stride: workspace.head_dim as u32,
            row_count: workspace.q_head_count as u32,
            eps: workspace.eps,
        };
        let k_head_norm_args = MlxRmsNormRowsArgs {
            n: workspace.head_dim as u32,
            row_stride: workspace.head_dim as u32,
            row_count: workspace.k_head_count as u32,
            eps: workspace.eps,
        };
        let v_head_norm_args = MlxRmsNormRowsArgs {
            n: workspace.head_dim as u32,
            row_stride: workspace.head_dim as u32,
            row_count: workspace.v_head_count as u32,
            eps: workspace.eps,
        };
        let q_rope_args = workspace.q_rope.args(position)?;
        let k_rope_args = workspace.k_rope.args(position)?;
        let residual_args = MlxAddRowArgs {
            n: workspace.post_attention_norm_len as u32,
        };
        let q_proj_offset_bytes = 0usize;
        let k_proj_offset_bytes = workspace
            .q_proj
            .out_len()
            .checked_mul(size_of::<u16>())
            .ok_or("q projection offset overflow")?;
        let v_proj_offset_bytes = workspace
            .q_proj
            .out_len()
            .checked_add(workspace.k_proj.out_len())
            .and_then(|value| value.checked_mul(size_of::<u16>()))
            .ok_or("v projection offset overflow")?;
        let post_attention_norm_args = MlxRmsNormRowArgs {
            n: workspace.post_attention_norm_len as u32,
            eps: workspace.eps,
        };
        let pre_ffn_norm_args = MlxRmsNormRowArgs {
            n: workspace.pre_feedforward_norm_len as u32,
            eps: workspace.eps,
        };
        let mlp_gate_up_args = workspace
            .mlp_gate_up
            .row_args(workspace.pre_feedforward_norm_len as u32);
        let geglu_args = MlxGegluRowArgs {
            n: workspace.mlp_gate.out_rows,
        };
        let mlp_down_args = workspace.mlp_down.row_args(workspace.mlp_gate.out_rows);
        let mlp_gate_up_split_offset_bytes = workspace
            .mlp_gate
            .out_len()
            .checked_mul(size_of::<u16>())
            .ok_or("mlp gate/up split offset overflow")?;
        let router_scale_args = MlxRouterScaleArgs {
            n: workspace.post_attention_norm_len as u32,
            eps: workspace.eps,
            root_size: bf16_round_to_f32((workspace.post_attention_norm_len as f32).powf(-0.5)),
        };
        let router_proj_args = workspace
            .router_proj
            .row_args(workspace.post_attention_norm_len as u32);
        let router_softmax_args = MlxSoftmaxRowsArgs {
            row_stride: workspace.router_proj.out_rows,
            row_count: 1,
            seq_len: workspace.router_proj.out_rows,
        };
        let router_topk_args = MlxRouterTopKArgs {
            expert_count: workspace.router_proj.out_rows,
            top_k: ROUTER_TOP_K as u32,
        };
        let expert_gate_args = workspace
            .expert_gate_up
            .selected_experts_args(workspace.pre_feedforward_norm2_len as u32, 0);
        let expert_geglu_args = MlxGegluStridedRowsArgs {
            n: (ROUTER_TOP_K * workspace.expert_gate.out_len()) as u32,
            row_width: workspace.expert_gate.out_rows,
            input_row_stride: workspace.expert_gate_up.out_rows,
            input_split_offset: workspace.expert_gate.out_rows,
        };
        let expert_down_args = workspace.expert_down.selected_experts_args(
            workspace.expert_gate.out_rows,
            workspace.expert_gate.out_rows,
        );
        let moe_weighted_args = MlxWeightedRowsArgs {
            n: workspace.expert_down.out_rows,
            row_stride: workspace.expert_down.out_rows,
            row_count: ROUTER_TOP_K as u32,
        };
        let post_ffn_norm1_args = MlxRmsNormRowArgs {
            n: workspace.post_feedforward_norm1_len as u32,
            eps: workspace.eps,
        };
        let post_ffn_norm2_args = MlxRmsNormRowArgs {
            n: workspace.post_feedforward_norm2_len as u32,
            eps: workspace.eps,
        };

        if let Some(input_words) = input_words {
            runtime.write_buffer(input_hidden_buffer, 0, &bytes_from_bf16_words(input_words))?;
        }
        if owns_command_batch {
            runtime.begin_command_batch()?;
        }
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&rms_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: input_hidden_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.input_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.h,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &workspace.pipelines.proj,
            &workspace.pipelines.proj_fast,
            workspace.qkv_proj,
            &qkv_proj_args,
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.h,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.qkv_proj_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.qkv_proj_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.qkv_proj_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.qkv_proj_out,
                    offset_bytes: 0,
                },
            ],
            qkv_proj_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.head_norm,
            bytes_of(&q_head_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.qkv_proj_out,
                    offset_bytes: q_proj_offset_bytes,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.q_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.q_norm,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            q_head_norm_threadgroups,
            head_norm_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.head_norm,
            bytes_of(&k_head_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.qkv_proj_out,
                    offset_bytes: k_proj_offset_bytes,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.k_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.k_norm,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            k_head_norm_threadgroups,
            head_norm_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.head_norm,
            bytes_of(&v_head_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.qkv_proj_out,
                    offset_bytes: v_proj_offset_bytes,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.v_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.v_norm,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            v_head_norm_threadgroups,
            head_norm_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rope,
            bytes_of(&q_rope_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.q_norm,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.q_rope,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            q_rope_threadgroups,
            rope_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rope,
            bytes_of(&k_rope_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.k_norm,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.k_rope,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            k_rope_threadgroups,
            rope_threads_per_threadgroup,
        )?;

        let (
            attention_seq_len,
            attention_start_slot,
            attention_kv_row_stride,
            attention_key_buffer,
            attention_value_buffer,
        ) = {
            let mut layer_cache = self.kv_cache_for_layer(layer_idx)?;
            layer_cache.append_token_from_buffers_compute(
                &runtime,
                &self.kv_append_pipeline,
                &workspace.buffers.k_rope,
                &workspace.buffers.v_norm,
            )?;
            (
                layer_cache.seq_len(),
                layer_cache.start_slot(),
                layer_cache.row_stride_words()?,
                layer_cache.key_buffer.clone(),
                layer_cache.value_buffer.clone(),
            )
        };
        let attention_logits_args = MlxGqaAttentionLogitsSeqArgs {
            head_dim: workspace.head_dim as u32,
            q_head_stride: workspace.head_dim as u32,
            kv_row_stride: attention_kv_row_stride as u32,
            q_head_count: workspace.q_head_count as u32,
            q_heads_per_kv: workspace.q_heads_per_kv as u32,
            seq_len: attention_seq_len as u32,
            start_slot: attention_start_slot as u32,
            capacity: workspace.kv_cache_capacity_tokens as u32,
        };
        let attention_softmax_args = MlxSoftmaxRowsArgs {
            row_stride: workspace.kv_cache_capacity_tokens as u32,
            row_count: workspace.q_head_count as u32,
            seq_len: attention_seq_len as u32,
        };
        let attention_logits_threadgroups = MetalSize {
            width: attention_seq_len as u64,
            height: workspace.q_head_count as u64,
            depth: 1,
        };
        let attention_weighted_sum_args = MlxGqaAttentionWeightedSumArgs {
            probs_row_stride: workspace.kv_cache_capacity_tokens as u32,
            head_dim: workspace.head_dim as u32,
            kv_row_stride: attention_kv_row_stride as u32,
            out_head_stride: workspace.head_dim as u32,
            q_head_count: workspace.q_head_count as u32,
            q_heads_per_kv: workspace.q_heads_per_kv as u32,
            seq_len: attention_seq_len as u32,
            start_slot: attention_start_slot as u32,
            capacity: workspace.kv_cache_capacity_tokens as u32,
        };

        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.attention_logits_seq,
            bytes_of(&attention_logits_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.q_rope,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &attention_key_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.attention_logits,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            attention_logits_threadgroups,
            attention_logits_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.attention_softmax_rows,
            bytes_of(&attention_softmax_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.attention_logits,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.attention_probs,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            MetalSize {
                width: workspace.q_head_count as u64,
                height: 1,
                depth: 1,
            },
            mlx_softmax_threads_per_threadgroup(
                attention_seq_len,
                workspace
                    .pipelines
                    .attention_softmax_rows
                    .max_threads_per_threadgroup,
            )?,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.attention_weighted_sum,
            bytes_of(&attention_weighted_sum_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.attention_probs,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &attention_value_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.attn_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            attention_output_threadgroups,
            attention_output_threads_per_threadgroup,
        )?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &workspace.pipelines.proj,
            &workspace.pipelines.proj_fast,
            workspace.o_proj,
            &o_proj_args,
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.attn_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.o_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.o_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.o_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.o_proj_out,
                    offset_bytes: 0,
                },
            ],
            o_proj_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&post_attention_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.o_proj_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.post_attention_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.post_attention_norm_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.residual,
            bytes_of(&residual_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: input_hidden_buffer,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.post_attention_norm_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.residual_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&pre_ffn_norm_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.residual_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.pre_feedforward_norm_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.pre_feedforward_norm_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &workspace.pipelines.proj,
            &workspace.pipelines.proj_fast,
            workspace.mlp_gate_up,
            &mlp_gate_up_args,
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.pre_feedforward_norm_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.mlp_gate_up_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.mlp_gate_up_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.mlp_gate_up_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.mlp_gate_up_out,
                    offset_bytes: 0,
                },
            ],
            mlp_gate_up_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.geglu,
            bytes_of(&geglu_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.mlp_gate_up_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.mlp_gate_up_out,
                    offset_bytes: mlp_gate_up_split_offset_bytes,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.geglu_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            geglu_threadgroups,
            geglu_threads_per_threadgroup,
        )?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &workspace.pipelines.proj,
            &workspace.pipelines.proj_fast,
            workspace.mlp_down,
            &mlp_down_args,
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.geglu_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.mlp_down_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.mlp_down_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.mlp_down_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.mlp_down_out,
                    offset_bytes: 0,
                },
            ],
            mlp_down_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&post_ffn_norm1_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.mlp_down_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.post_feedforward_norm1_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.post_feedforward_norm1_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.router_scale_pair,
            bytes_of(&router_scale_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.residual_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.router_scale_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.pre_feedforward_norm2_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.buffers.router_scaled_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.pre_feedforward_norm2_out,
                    offset_bytes: 0,
                },
            ],
            3,
            &[],
            router_scale_threadgroups,
            router_scale_threads_per_threadgroup,
        )?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &workspace.pipelines.proj,
            &workspace.pipelines.proj_fast,
            workspace.router_proj,
            &router_proj_args,
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.router_scaled_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.router_proj_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.router_proj_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.router_proj_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.router_proj_out,
                    offset_bytes: 0,
                },
            ],
            router_proj_threadgroups,
            proj_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.attention_softmax_rows,
            bytes_of(&router_softmax_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.router_proj_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.router_probs_out,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            router_softmax_threadgroups,
            mlx_softmax_threads_per_threadgroup(
                workspace.router_proj.out_rows as usize,
                workspace
                    .pipelines
                    .attention_softmax_rows
                    .max_threads_per_threadgroup,
            )?,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.router_topk,
            bytes_of(&router_topk_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.router_proj_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.router_probs_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.router_per_expert_scale,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.buffers.moe_top_k_indices,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.buffers.moe_top_k_weights,
                    offset_bytes: 0,
                },
            ],
            3,
            &[],
            router_topk_threadgroups,
            router_topk_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.selected_expert_proj,
            bytes_of(&expert_gate_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.pre_feedforward_norm2_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.moe_top_k_indices,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.expert_gate_up_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.expert_gate_up_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.weights.expert_gate_up_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 6,
                    buffer: &workspace.buffers.expert_gate_up_out,
                    offset_bytes: 0,
                },
            ],
            5,
            &[],
            selected_expert_threadgroups,
            selected_expert_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.geglu_strided,
            bytes_of(&expert_geglu_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.expert_gate_up_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.expert_geglu_out,
                    offset_bytes: 0,
                },
            ],
            1,
            &[],
            expert_geglu_threadgroups,
            geglu_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.selected_expert_proj,
            bytes_of(&expert_down_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.expert_geglu_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.moe_top_k_indices,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.weights.expert_down_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: &workspace.weights.expert_down_scales,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: &workspace.weights.expert_down_biases,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 6,
                    buffer: &workspace.buffers.expert_down_out,
                    offset_bytes: 0,
                },
            ],
            5,
            &[],
            selected_expert_down_threadgroups,
            selected_expert_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.weighted_sum_rows,
            bytes_of(&moe_weighted_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.expert_down_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.moe_top_k_weights,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.moe_weighted_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            MetalSize {
                width: workspace.expert_down.out_len() as u64,
                height: 1,
                depth: 1,
            },
            MetalSize {
                width: 1,
                height: 1,
                depth: 1,
            },
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.rms,
            bytes_of(&post_ffn_norm2_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.moe_weighted_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.weights.post_feedforward_norm2_weight,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.moe_post_ffn_norm2_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.residual,
            bytes_of(&residual_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.post_feedforward_norm1_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.moe_post_ffn_norm2_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: &workspace.buffers.moe_merge_out,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        dispatch_compute_tracked_split(
            &runtime,
            &workspace.pipelines.residual,
            bytes_of(&residual_args),
            [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &workspace.buffers.residual_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &workspace.buffers.moe_merge_out,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: output_hidden_buffer,
                    offset_bytes: 0,
                },
            ],
            2,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        if owns_command_batch {
            runtime.end_command_batch()?;
        }

        if read_output {
            Ok(Some(bf16_words_from_f32_bits(&read_bf16_buffer_bits(
                &runtime,
                output_hidden_buffer,
                workspace.post_feedforward_norm1_len,
            )?)))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn eval_layer_hidden_state(
        &mut self,
        layer_idx: usize,
        input_words: &[u16],
        position: usize,
    ) -> Result<Vec<u16>, Box<dyn Error>> {
        self.eval_layer_hidden_state_core(layer_idx, Some(input_words), None, None, position, true)?
            .ok_or_else(|| "exact metal layer eval did not return output".into())
    }

    fn eval_token_hidden_state_core(
        &mut self,
        input_words: Option<&[u16]>,
        position: usize,
        read_output: bool,
    ) -> Result<Option<Vec<u16>>, Box<dyn Error>> {
        if let Some(input_words) = input_words {
            if input_words.len() != NORM_LEN {
                return Err(format!(
                    "exact metal token input length mismatch: got {} expected {}",
                    input_words.len(),
                    NORM_LEN
                )
                .into());
            }
        }

        let layer_count = self
            .session
            .weights
            .snapshot
            .config
            .text_config
            .num_hidden_layers as usize;
        if layer_count == 0 {
            if !read_output {
                return Ok(None);
            }
            if let Some(input_words) = input_words {
                return Ok(Some(input_words.to_vec()));
            }
            return Ok(Some(bf16_words_from_f32_bits(&read_bf16_buffer_bits(
                &self.session.runtime,
                &self.text_io.buffers.standalone_hidden,
                NORM_LEN,
            )?)));
        }

        let hidden_a = self.text_io.buffers.standalone_hidden.clone();
        let hidden_b = self.text_io.buffers.hidden_scratch.clone();
        for layer_idx in 0..layer_count {
            let (input_buffer, output_buffer) = if layer_idx % 2 == 0 {
                (&hidden_a, &hidden_b)
            } else {
                (&hidden_b, &hidden_a)
            };
            let maybe_words = self.eval_layer_hidden_state_core(
                layer_idx,
                if layer_idx == 0 { input_words } else { None },
                Some(input_buffer),
                Some(output_buffer),
                position,
                read_output && layer_idx + 1 == layer_count,
            )?;
            if let Some(words) = maybe_words {
                return Ok(Some(words));
            }
        }

        if read_output {
            Err("exact metal token eval completed without a final layer output".into())
        } else {
            Ok(None)
        }
    }

    pub(crate) fn eval_token_hidden_state(
        &mut self,
        input_words: &[u16],
        position: usize,
    ) -> Result<Vec<u16>, Box<dyn Error>> {
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = self.eval_token_hidden_state_core(Some(input_words), position, false);
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        let hidden_buffer = self.final_hidden_buffer()?;
        self.read_hidden_words_from_buffer(&hidden_buffer)
    }

    pub(crate) fn eval_token_hidden_state_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
    ) -> Result<Vec<u16>, Box<dyn Error>> {
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
            self.eval_token_hidden_state_core(None, position, false)?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        let hidden_buffer = self.final_hidden_buffer()?;
        self.read_hidden_words_from_buffer(&hidden_buffer)
    }

    pub(crate) fn eval_token_greedy_token_id_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
    ) -> Result<u32, Box<dyn Error>> {
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
            self.eval_token_hidden_state_core(None, position, false)?;
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        Ok(self.read_shared_logits_greedy_token()?.token_id)
    }

    pub(crate) fn eval_token_greedy_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
            self.eval_token_hidden_state_core(None, position, false)?;
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        self.read_shared_logits_greedy_token()
    }

    pub(crate) fn eval_token_greedy_token_chunk_from_token_id(
        &mut self,
        token_id: u32,
        position: usize,
        token_count: usize,
    ) -> Result<Vec<u32>, Box<dyn Error>> {
        if token_count == 0 {
            return Ok(Vec::new());
        }
        if token_count > DEVICE_GREEDY_DECODE_CHUNK_TOKENS {
            return Err(format!(
                "device greedy decode chunk {} exceeds capacity {}",
                token_count, DEVICE_GREEDY_DECODE_CHUNK_TOKENS
            )
            .into());
        }
        if position == 0 {
            self.reset_kv_caches();
        }
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
            for step_idx in 0..token_count {
                self.eval_token_hidden_state_core(None, position + step_idx, false)?;
                let hidden_buffer = self.final_hidden_buffer()?;
                self.dispatch_greedy_head_on_hidden_buffer(&hidden_buffer)?;
                self.dequantize_next_token_embedding_from_device_buffer(&input_buffer, step_idx)?;
                if step_idx + 1 < token_count {
                    runtime.seal_command_batch_encoder()?;
                }
            }
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        self.read_generated_token_chunk(token_count)
    }

    pub(crate) fn prefill_prompt_greedy_token_id_from_token_ids(
        &mut self,
        prompt_token_ids: &[u32],
        start_position: usize,
    ) -> Result<u32, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("prompt prefill requires at least one token".into());
        }
        if start_position == 0 {
            self.reset_kv_caches();
        }
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            for (offset, token_id) in prompt_token_ids.iter().copied().enumerate() {
                let position = start_position + offset;
                self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
                self.eval_token_hidden_state_core(None, position, false)?;
            }
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        Ok(self.read_shared_logits_greedy_token()?.token_id)
    }

    pub(crate) fn prefill_prompt_greedy_from_token_ids(
        &mut self,
        prompt_token_ids: &[u32],
        start_position: usize,
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("prompt prefill requires at least one token".into());
        }
        if start_position == 0 {
            self.reset_kv_caches();
        }
        let input_buffer = self.token_input_buffer()?;
        let runtime = self.session.runtime.clone();
        runtime.begin_command_batch()?;
        let batch_result = (|| -> Result<(), Box<dyn Error>> {
            for (offset, token_id) in prompt_token_ids.iter().copied().enumerate() {
                let position = start_position + offset;
                self.dequantize_token_embedding_into_buffer(token_id, &input_buffer)?;
                self.eval_token_hidden_state_core(None, position, false)?;
            }
            let hidden_buffer = self.final_hidden_buffer()?;
            self.dispatch_final_text_norm_on_hidden_buffer(&hidden_buffer)?;
            self.dispatch_logits_projection_from_final_norm()?;
            Ok(())
        })();
        if let Err(err) = batch_result {
            let _ = runtime.discard_command_batch();
            return Err(err);
        }
        runtime.end_command_batch()?;
        self.read_shared_logits_greedy_token()
    }

    pub(crate) fn greedy_token_from_hidden_words(
        &mut self,
        hidden_words: &[u16],
    ) -> Result<MlxGreedyToken, Box<dyn Error>> {
        if hidden_words.len() != NORM_LEN {
            return Err(format!(
                "exact metal hidden-word length mismatch: got {} expected {}",
                hidden_words.len(),
                NORM_LEN
            )
            .into());
        }
        self.session.runtime.write_buffer(
            &self.text_io.buffers.standalone_hidden,
            0,
            &bytes_from_bf16_words(hidden_words),
        )?;
        let hidden_buffer = self.text_io.buffers.standalone_hidden.clone();
        self.greedy_token_from_hidden_buffer(&hidden_buffer)
    }

    #[cfg(test)]
    pub(crate) fn compare_greedy_token_paths_from_hidden_words(
        &mut self,
        hidden_words: &[u16],
    ) -> Result<(MlxGreedyToken, MlxGreedyToken), Box<dyn Error>> {
        if hidden_words.len() != NORM_LEN {
            return Err(format!(
                "exact metal hidden-word length mismatch: got {} expected {}",
                hidden_words.len(),
                NORM_LEN
            )
            .into());
        }
        self.session.runtime.write_buffer(
            &self.text_io.buffers.standalone_hidden,
            0,
            &bytes_from_bf16_words(hidden_words),
        )?;
        let hidden_buffer = self.text_io.buffers.standalone_hidden.clone();
        self.dispatch_greedy_head_on_hidden_buffer(&hidden_buffer)?;
        let device = self.read_device_greedy_token()?;
        let shared = self.read_shared_logits_greedy_token()?;
        Ok((device, shared))
    }
}

