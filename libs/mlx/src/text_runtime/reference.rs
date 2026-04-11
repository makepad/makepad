impl GemmaTextRuntimeSession {
    fn supports_exact_backend(weights: &MlxIndexedSafetensors) -> bool {
        let config = &weights.snapshot.config;
        let text_config = &config.text_config;
        config.quantization.mode == "affine"
            && matches!(config.quantization.bits, 4 | 8)
            && config.quantization.group_size == 64
            && text_config.hidden_size_per_layer_input == 0
            && text_config.num_kv_shared_layers == 0
            && (!text_config.enable_moe_block || text_config.top_k_experts_or_zero() == 8)
    }

    fn load(model_path: &Path) -> Result<Arc<Self>, String> {
        let model_root = model_root_dir(model_path).map_err(|err| err.to_string())?;
        let weights = MlxIndexedSafetensors::load(&model_root).map_err(|err| err.to_string())?;
        let tokenizer =
            MlxTokenizer::from_snapshot(&weights.snapshot).map_err(|err| err.to_string())?;
        let config = &weights.snapshot.config.text_config;
        let kv_layout =
            GemmaKvCacheLayout::from_text_config(config, 1).map_err(|err| err.to_string())?;
        let stop_tokens = weights
            .snapshot
            .generation_config
            .eos_token_id
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let exact_backend = if makepad_ggml::backend::metal::MetalRuntime::is_available()
            && Self::supports_exact_backend(&weights)
        {
            Some(Arc::new(Mutex::new(
                ExactMetalTextRuntimeSession::load(model_path.to_path_buf())
                    .map_err(|err| err.to_string())?,
            )))
        } else {
            None
        };
        Ok(Arc::new(Self {
            model_path: model_path.to_path_buf(),
            weights,
            tokenizer,
            kv_layout,
            stop_tokens,
            exact_backend,
        }))
    }

    fn has_exact_backend(&self) -> bool {
        self.exact_backend.is_some()
    }

    fn exact_backend(&self) -> Result<Arc<Mutex<ExactMetalTextRuntimeSession>>, String> {
        self.exact_backend
            .as_ref()
            .cloned()
            .ok_or_else(|| "exact metal text runtime is unavailable for this Gemma family member".to_string())
    }

    fn format_prompt_text(&self, prompt_text: &str, prompt_format: GemmaPromptFormat) -> String {
        match prompt_format {
            GemmaPromptFormat::RawBos => format!(
                "{}{}",
                self.weights.snapshot.tokenizer_config.bos_token, prompt_text
            ),
            GemmaPromptFormat::Gemma4UserTurn => format!(
                "{}{}user\n{}{}\n{}model\n{}thought\n{}",
                self.weights.snapshot.tokenizer_config.bos_token,
                self.weights.snapshot.tokenizer_config.sot_token,
                prompt_text,
                self.weights.snapshot.tokenizer_config.eot_token,
                self.weights.snapshot.tokenizer_config.sot_token,
                self.weights.snapshot.tokenizer_config.soc_token,
                self.weights.snapshot.tokenizer_config.eoc_token,
            ),
        }
    }

    fn tokenize_prompt(&self, formatted_prompt: &str) -> Result<Arc<[u32]>, String> {
        let ids = self
            .tokenizer
            .encode(formatted_prompt)
            .map_err(|err| err.to_string())?;
        if ids.is_empty() {
            return Err("formatted prompt encoded to zero tokens".to_string());
        }
        Ok(Arc::<[u32]>::from(ids))
    }

    #[cfg(test)]
    fn start_generation_cursor(
        self: &Arc<Self>,
        prompt_token_ids: Arc<[u32]>,
        max_new_tokens: Option<usize>,
    ) -> Result<ExactMetalGenerationCursor, String> {
        ExactMetalTextRuntimeSession::generation_cursor(
            self.exact_backend()?,
            prompt_token_ids,
            self.stop_tokens.clone(),
            max_new_tokens,
        )
        .map_err(|err| err.to_string())
    }

    fn start_generation_graph(
        self: &Arc<Self>,
        prompt_token_ids: Arc<[u32]>,
        max_new_tokens: Option<usize>,
    ) -> Result<ExactMetalGenerationGraph, String> {
        ExactMetalTextRuntimeSession::generation_graph(
            self.exact_backend()?,
            prompt_token_ids,
            self.stop_tokens.clone(),
            max_new_tokens,
        )
        .map_err(|err| err.to_string())
    }
}

impl GemmaTextRuntimeSession {
    #[cfg_attr(not(test), allow(dead_code))]
    fn greedy_token_from_hidden(&self, hidden_words: &[u16]) -> Result<MlxGreedyToken, String> {
        let final_norm_words = self
            .weights
            .final_text_norm_bf16_words(hidden_words)
            .map_err(|err| err.to_string())?;
        let mut logits = quantized_matmul_tensor(
            &self.weights,
            &final_norm_words,
            EMBED_TOKENS_WEIGHT_NAME,
            EMBED_TOKENS_SCALES_NAME,
            EMBED_TOKENS_BIASES_NAME,
        )?;
        if let Some(softcap) = Some(self.weights.snapshot.config.text_config.final_logit_softcapping)
            .filter(|softcap| *softcap > 0.0)
        {
            for logit in &mut logits {
                *logit = bf16_round_to_f32((*logit / softcap).tanh() * softcap);
            }
        }
        let mut best_token_id = 0u32;
        let mut best_logit = f32::NEG_INFINITY;
        for (token_idx, logit) in logits.into_iter().enumerate() {
            let token_id = token_idx as u32;
            if logit > best_logit || (logit == best_logit && token_id < best_token_id) {
                best_token_id = token_id;
                best_logit = logit;
            }
        }
        Ok(MlxGreedyToken {
            token_id: best_token_id,
            logit: best_logit,
        })
    }

    fn sampled_token_from_hidden(
        &self,
        hidden_words: &[u16],
        disallowed_token_ids: &[u32],
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
    ) -> Result<MlxGreedyToken, String> {
        let final_norm_words = self
            .weights
            .final_text_norm_bf16_words(hidden_words)
            .map_err(|err| err.to_string())?;
        let mut logits = quantized_matmul_tensor(
            &self.weights,
            &final_norm_words,
            EMBED_TOKENS_WEIGHT_NAME,
            EMBED_TOKENS_SCALES_NAME,
            EMBED_TOKENS_BIASES_NAME,
        )?;
        if let Some(softcap) = Some(self.weights.snapshot.config.text_config.final_logit_softcapping)
            .filter(|softcap| *softcap > 0.0)
        {
            for logit in &mut logits {
                *logit = bf16_round_to_f32((*logit / softcap).tanh() * softcap);
            }
        }
        sample_token_from_logits_f32(&logits, disallowed_token_ids, sampling_options, rng)
    }

    fn raw_per_layer_inputs_for_token(&self, token_id: u32) -> Result<Option<Vec<Vec<u16>>>, String> {
        let config = &self.weights.snapshot.config.text_config;
        let per_layer_dim = config.hidden_size_per_layer_input as usize;
        if per_layer_dim == 0 {
            return Ok(None);
        }
        let per_layer_token_id = if token_id < config.vocab_size_per_layer_input {
            token_id
        } else {
            0
        };
        let header = self
            .weights
            .header_for_tensor("language_model.model.embed_tokens_per_layer.weight")
            .map_err(|err| err.to_string())?;
        let mut values = header
            .affine_dequantize_row_f32(
                "language_model.model.embed_tokens_per_layer.weight",
                "language_model.model.embed_tokens_per_layer.scales",
                "language_model.model.embed_tokens_per_layer.biases",
                per_layer_token_id as u64,
                self.weights.snapshot.config.quantization.group_size as u64,
                self.weights.snapshot.config.quantization.bits,
            )
            .map_err(|err| err.to_string())?;
        let embed_scale = bf16_round_to_f32((per_layer_dim as f32).sqrt());
        for value in &mut values {
            *value = bf16_round_to_f32(*value * embed_scale);
        }
        split_f32_rows_to_bf16_words(
            &values,
            config.num_hidden_layers as usize,
            per_layer_dim,
            "embed_tokens_per_layer",
        )
        .map(Some)
    }

    fn project_per_layer_inputs_for_token(
        &self,
        token_id: u32,
        input_words: &[u16],
    ) -> Result<Option<Vec<Vec<u16>>>, String> {
        let config = &self.weights.snapshot.config.text_config;
        let per_layer_dim = config.hidden_size_per_layer_input as usize;
        if per_layer_dim == 0 {
            return Ok(None);
        }

        let mut projection = quantized_matmul_tensor(
            &self.weights,
            input_words,
            "language_model.model.per_layer_model_projection.weight",
            "language_model.model.per_layer_model_projection.scales",
            "language_model.model.per_layer_model_projection.biases",
        )?;
        let projection_scale = bf16_round_to_f32((config.hidden_size as f32).powf(-0.5));
        for value in &mut projection {
            *value = bf16_round_to_f32(*value * projection_scale);
        }

        let row_count = config.num_hidden_layers as usize;
        let norm_weights = self
            .weights
            .read_bf16_tensor_words("language_model.model.per_layer_projection_norm.weight")
            .map_err(|err| err.to_string())?;
        let mut projection = rms_norm_rows_weighted_f32(
            &projection,
            row_count,
            per_layer_dim,
            &norm_weights,
        )?;

        if let Some(raw_rows) = self.raw_per_layer_inputs_for_token(token_id)? {
            let combine_scale = bf16_round_to_f32(2.0f32.powf(-0.5));
            for (index, raw_word) in raw_rows.iter().flatten().enumerate() {
                projection[index] = bf16_round_to_f32(
                    bf16_round_to_f32(projection[index] + bf16_word_to_f32(*raw_word))
                        * combine_scale,
                );
            }
        }

        split_f32_rows_to_bf16_words(
            &projection,
            row_count,
            per_layer_dim,
            "per_layer_model_projection",
        )
        .map(Some)
    }
}

impl GemmaTextRuntimeSession {
    fn eval_input_row_hidden_state(
        &self,
        token_id: u32,
        input_words: Vec<u16>,
        position: usize,
        caches: &mut GemmaKvCacheSet<f32>,
    ) -> Result<Vec<u16>, String> {
        let per_layer_inputs = self.project_per_layer_inputs_for_token(token_id, &input_words)?;
        let mut hidden_words = input_words;
        for layer_idx in 0..self.weights.snapshot.config.text_config.num_hidden_layers as usize {
            hidden_words = self.eval_layer_hidden_state(
                layer_idx,
                &hidden_words,
                per_layer_inputs
                    .as_ref()
                    .map(|rows| rows[layer_idx].as_slice()),
                position,
                caches,
            )?;
        }
        Ok(hidden_words)
    }

    fn eval_token_hidden_state(
        &self,
        token_id: u32,
        position: usize,
        caches: &mut GemmaKvCacheSet<f32>,
    ) -> Result<Vec<u16>, String> {
        let hidden_words = self
            .weights
            .embed_token_bf16_words(token_id)
            .map_err(|err| err.to_string())?;
        self.eval_input_row_hidden_state(token_id, hidden_words, position, caches)
    }

    fn eval_layer_hidden_state(
        &self,
        layer_idx: usize,
        input_words: &[u16],
        per_layer_input_words: Option<&[u16]>,
        position: usize,
        caches: &mut GemmaKvCacheSet<f32>,
    ) -> Result<Vec<u16>, String> {
        let config = &self.weights.snapshot.config.text_config;
        let layer_type = config
            .layer_types
            .get(layer_idx)
            .ok_or_else(|| format!("missing text layer type for layer {layer_idx}"))?;
        let attention_k_eq_v = config.attention_k_eq_v && layer_type == "full_attention";
        let head_dim = if layer_type == "full_attention" {
            config.global_head_dim as usize
        } else {
            config.head_dim as usize
        };
        let k_head_count = if attention_k_eq_v && layer_type == "full_attention" {
            config.num_global_key_value_heads_or_default() as usize
        } else {
            config.num_key_value_heads as usize
        };
        let q_head_count = config.num_attention_heads as usize;
        let v_head_count = k_head_count;
        if k_head_count == 0 || q_head_count == 0 || q_head_count % k_head_count != 0 {
            return Err(format!(
                "invalid Gemma attention head layout for layer {layer_idx}: q={q_head_count} kv={k_head_count}"
            ));
        }
        let q_heads_per_kv = q_head_count / k_head_count;
        let rope_params = if layer_type == "full_attention" {
            &config.rope_parameters.full_attention
        } else {
            &config.rope_parameters.sliding_attention
        };
        let rope_rotary_dim = if let Some(partial_factor) = rope_params.partial_rotary_factor {
            let rotary_dim = (head_dim as f32 * partial_factor).round() as usize;
            if rotary_dim == 0 || rotary_dim > head_dim || rotary_dim % 2 != 0 {
                return Err(format!(
                    "invalid rope rotary dim {} for layer {} head_dim {} factor {}",
                    rotary_dim, layer_idx, head_dim, partial_factor
                ));
            }
            rotary_dim
        } else {
            head_dim
        };
        let rope = RopeSpec {
            head_dim,
            rotary_dim: rope_rotary_dim,
            base: rope_params.rope_theta,
        };
        let names = TextLayerTensorNames::for_layer(layer_idx, attention_k_eq_v);
        let first_kv_shared_layer_idx =
            config.num_hidden_layers as usize - config.num_kv_shared_layers as usize;
        let is_kv_shared_layer = layer_idx >= first_kv_shared_layer_idx && config.num_kv_shared_layers != 0;

        let input_norm =
            rms_norm_weighted_tensor(&self.weights, input_words, &names.input_norm_weight_name)?;
        let input_norm_words = f32s_to_bf16_words(&input_norm);

        let mut qkv_specs = Vec::with_capacity(if is_kv_shared_layer {
            1
        } else if attention_k_eq_v {
            2
        } else {
            3
        });
        qkv_specs.push(QuantizedTensorSpec {
            weight_name: &names.q.weight_name,
            scales_name: &names.q.scales_name,
            biases_name: &names.q.biases_name,
        });
        if !is_kv_shared_layer {
            qkv_specs.push(QuantizedTensorSpec {
                weight_name: &names.k.weight_name,
                scales_name: &names.k.scales_name,
                biases_name: &names.k.biases_name,
            });
            if !attention_k_eq_v {
                qkv_specs.push(QuantizedTensorSpec {
                    weight_name: &names.v.weight_name,
                    scales_name: &names.v.scales_name,
                    biases_name: &names.v.biases_name,
                });
            }
        }
        let mut qkv_outs =
            quantized_matmul_tensors_shared_input(&self.weights, &input_norm_words, &qkv_specs)?
                .into_iter();
        let q_raw = qkv_outs
            .next()
            .ok_or_else(|| format!("missing q projection output for layer {layer_idx}"))?;
        let q_norm_weight_name = names
            .q
            .norm_weight_name
            .as_deref()
            .ok_or_else(|| format!("missing q norm weight name for layer {layer_idx}"))?;
        let q_norm_weights = self
            .weights
            .read_bf16_tensor_words(q_norm_weight_name)
            .map_err(|err| err.to_string())?;
        let mut q_norm =
            rms_norm_rows_weighted_f32(&q_raw, q_head_count, head_dim, &q_norm_weights)?;
        apply_rope_rows_in_place(&mut q_norm, q_head_count, rope, position)?;
        let attention_out = if is_kv_shared_layer {
            let layer_cache = caches
                .cache_for_layer(layer_idx)
                .map_err(|err| err.to_string())?;
            compute_attention_output_f32(
                &q_norm,
                layer_cache,
                q_head_count,
                q_heads_per_kv,
                head_dim,
            )
            .map_err(|err| err.to_string())?
        } else {
            let k_raw = qkv_outs
                .next()
                .ok_or_else(|| format!("missing k projection output for layer {layer_idx}"))?;
            let k_norm_weight_name = names
                .k
                .norm_weight_name
                .as_deref()
                .ok_or_else(|| format!("missing k norm weight name for layer {layer_idx}"))?;
            let k_norm_weights = self
                .weights
                .read_bf16_tensor_words(k_norm_weight_name)
                .map_err(|err| err.to_string())?;
            let mut k_norm =
                rms_norm_rows_weighted_f32(&k_raw, k_head_count, head_dim, &k_norm_weights)?;
            apply_rope_rows_in_place(&mut k_norm, k_head_count, rope, position)?;

            let v_norm = if attention_k_eq_v {
                rms_norm_rows_no_scale_f32(&k_raw, v_head_count, head_dim, config.rms_norm_eps)?
            } else {
                let v_raw = qkv_outs
                    .next()
                    .ok_or_else(|| format!("missing v projection output for layer {layer_idx}"))?;
                rms_norm_rows_no_scale_f32(&v_raw, v_head_count, head_dim, config.rms_norm_eps)?
            };

            let k_tensor = single_token_tensor(k_head_count, head_dim, k_norm)
                .map_err(|err| err.to_string())?;
            let v_tensor = single_token_tensor(v_head_count, head_dim, v_norm)
                .map_err(|err| err.to_string())?;
            let layer_cache = caches
                .cache_for_layer_mut(layer_idx)
                .map_err(|err| err.to_string())?;
            layer_cache
                .update_and_fetch(k_tensor.view(), v_tensor.view())
                .map_err(|err| err.to_string())?;

            compute_attention_output_f32(
                &q_norm,
                layer_cache,
                q_head_count,
                q_heads_per_kv,
                head_dim,
            )
            .map_err(|err| err.to_string())?
        };
        let attention_out_words = f32s_to_bf16_words(&attention_out);
        let attention_oproj = quantized_matmul_tensor(
            &self.weights,
            &attention_out_words,
            &names.o.weight_name,
            &names.o.scales_name,
            &names.o.biases_name,
        )?;
        let attention_oproj_words = f32s_to_bf16_words(&attention_oproj);
        let post_attention_norm = rms_norm_weighted_tensor(
            &self.weights,
            &attention_oproj_words,
            &names.post_attention_norm_weight_name,
        )?;
        let post_attention_residual = add_bf16_and_f32(input_words, &post_attention_norm)?;
        let post_attention_residual_words = f32s_to_bf16_words(&post_attention_residual);

        let pre_feedforward_norm = rms_norm_weighted_tensor(
            &self.weights,
            &post_attention_residual_words,
            &names.pre_feedforward_norm_weight_name,
        )?;
        let pre_feedforward_norm_words = f32s_to_bf16_words(&pre_feedforward_norm);
        let mut dense_gate_up = quantized_matmul_tensors_shared_input(
            &self.weights,
            &pre_feedforward_norm_words,
            &[
                QuantizedTensorSpec {
                    weight_name: &names.mlp_gate_weight_name,
                    scales_name: &names.mlp_gate_scales_name,
                    biases_name: &names.mlp_gate_biases_name,
                },
                QuantizedTensorSpec {
                    weight_name: &names.mlp_up_weight_name,
                    scales_name: &names.mlp_up_scales_name,
                    biases_name: &names.mlp_up_biases_name,
                },
            ],
        )?
        .into_iter();
        let dense_gate = dense_gate_up
            .next()
            .ok_or_else(|| format!("missing dense gate output for layer {layer_idx}"))?;
        let dense_up = dense_gate_up
            .next()
            .ok_or_else(|| format!("missing dense up output for layer {layer_idx}"))?;
        let dense_geglu = geglu_f32(&dense_gate, &dense_up)?;
        let dense_geglu_words = f32s_to_bf16_words(&dense_geglu);
        let dense_down = quantized_matmul_tensor(
            &self.weights,
            &dense_geglu_words,
            &names.mlp_down_weight_name,
            &names.mlp_down_scales_name,
            &names.mlp_down_biases_name,
        )?;

        let feedforward_out = if config.enable_moe_block {
            let dense_down_words = f32s_to_bf16_words(&dense_down);
            let dense_branch = rms_norm_weighted_tensor(
                &self.weights,
                &dense_down_words,
                &names.post_feedforward_norm1_weight_name,
            )?;
            let router = gemma_router_topk_from_residual_bf16(
                &self.weights,
                &post_attention_residual_words,
                &names.router_scale_name,
                &names.router_per_expert_scale_name,
                &names.router_proj_weight_name,
                &names.router_proj_scales_name,
                &names.router_proj_biases_name,
                config.rms_norm_eps,
                config.top_k_experts_or_zero() as usize,
            )?;
            let moe = gemma_moe_expert_block_from_residual_bf16(
                &self.weights,
                &post_attention_residual_words,
                &names.pre_feedforward_norm2_weight_name,
                &names.expert_gate_weight_name,
                &names.expert_gate_scales_name,
                &names.expert_gate_biases_name,
                &names.expert_up_weight_name,
                &names.expert_up_scales_name,
                &names.expert_up_biases_name,
                &names.expert_down_weight_name,
                &names.expert_down_scales_name,
                &names.expert_down_biases_name,
                &router.top_k_indices,
                &router.top_k_weights,
            )?;
            let moe_out_words = f32s_to_bf16_words(&moe.expert_out);
            let moe_branch = rms_norm_weighted_tensor(
                &self.weights,
                &moe_out_words,
                &names.post_feedforward_norm2_weight_name,
            )?;
            let merged = add_f32(&dense_branch, &moe_branch)?;
            let merged_words = f32s_to_bf16_words(&merged);
            rms_norm_weighted_tensor(
                &self.weights,
                &merged_words,
                &names.post_feedforward_norm_weight_name,
            )?
        } else {
            let dense_down_words = f32s_to_bf16_words(&dense_down);
            rms_norm_weighted_tensor(
                &self.weights,
                &dense_down_words,
                &names.post_feedforward_norm_weight_name,
            )?
        };

        let mut output = add_f32(&post_attention_residual, &feedforward_out)?;
        if let Some(per_layer_input_words) = per_layer_input_words {
            let gate = quantized_matmul_tensor(
                &self.weights,
                &f32s_to_bf16_words(&output),
                &names.per_layer_input_gate_weight_name,
                &names.per_layer_input_gate_scales_name,
                &names.per_layer_input_gate_biases_name,
            )?;
            if gate.len() != per_layer_input_words.len() {
                return Err(format!(
                    "per-layer input gate length mismatch in layer {layer_idx}: gate={} ple={}",
                    gate.len(),
                    per_layer_input_words.len()
                ));
            }
            let gated_per_layer = gate
                .iter()
                .copied()
                .zip(per_layer_input_words.iter().copied())
                .map(|(gate_value, per_layer_value)| {
                    bf16_round_to_f32(gelu_approx_f32(gate_value) * bf16_word_to_f32(per_layer_value))
                })
                .collect::<Vec<_>>();
            let gated_words = f32s_to_bf16_words(&gated_per_layer);
            let per_layer_contribution = quantized_matmul_tensor(
                &self.weights,
                &gated_words,
                &names.per_layer_projection_weight_name,
                &names.per_layer_projection_scales_name,
                &names.per_layer_projection_biases_name,
            )?;
            let per_layer_contribution_words = f32s_to_bf16_words(&per_layer_contribution);
            let per_layer_contribution = rms_norm_weighted_tensor(
                &self.weights,
                &per_layer_contribution_words,
                &names.post_per_layer_input_norm_weight_name,
            )?;
            output = add_bf16_and_f32(&f32s_to_bf16_words(&output), &per_layer_contribution)?;
        }
        if let Some(layer_scalar) =
            load_optional_scalar_f32(&self.weights, &names.layer_scalar_name)?
        {
            scale_in_place(&mut output, layer_scalar);
        }
        Ok(f32s_to_bf16_words(&output))
    }
}

impl GemmaTextRuntimeSession {
    fn eval_token_sampled_from_token_id_reference(
        &self,
        token_id: u32,
        position: usize,
        disallowed_token_ids: &[u32],
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
        caches: &mut GemmaKvCacheSet<f32>,
    ) -> Result<MlxGreedyToken, String> {
        let hidden_words = self.eval_token_hidden_state(token_id, position, caches)?;
        self.sampled_token_from_hidden(&hidden_words, disallowed_token_ids, sampling_options, rng)
    }

    fn prefill_prompt_sampled_from_embedding_rows_reference(
        &self,
        prompt_token_ids: &[u32],
        prompt_embedding_rows: &[Vec<u16>],
        start_position: usize,
        disallowed_token_ids: &[u32],
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
        caches: &mut GemmaKvCacheSet<f32>,
    ) -> Result<MlxGreedyToken, String> {
        if prompt_token_ids.is_empty() {
            return Err("generation requires at least one prompt token".to_string());
        }
        if prompt_token_ids.len() != prompt_embedding_rows.len() {
            return Err(format!(
                "prompt token/embedding row mismatch: {} ids vs {} rows",
                prompt_token_ids.len(),
                prompt_embedding_rows.len()
            ));
        }
        let mut last_hidden_words = None;
        for (offset, (&token_id, input_words)) in prompt_token_ids
            .iter()
            .zip(prompt_embedding_rows.iter())
            .enumerate()
        {
            last_hidden_words = Some(self.eval_input_row_hidden_state(
                token_id,
                input_words.clone(),
                start_position + offset,
                caches,
            )?);
        }
        self.sampled_token_from_hidden(
            last_hidden_words
                .as_deref()
                .ok_or_else(|| "prompt prefill produced no hidden state".to_string())?,
            disallowed_token_ids,
            sampling_options,
            rng,
        )
    }
}

impl GemmaTextRuntimeSession {
    fn generate_sampled_token_ids_reference<F>(
        self: &Arc<Self>,
        prompt_token_ids: Arc<[u32]>,
        max_new_tokens: Option<usize>,
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
        on_generated_ids: F,
    ) -> Result<(Arc<[u32]>, GemmaStopReason), String>
    where
        F: FnMut(&[u32]) -> Result<(), String>,
    {
        let prompt_embedding_rows = prompt_token_ids
            .iter()
            .copied()
            .map(|token_id| {
                self.weights
                    .embed_token_bf16_words(token_id)
                    .map_err(|err| err.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.generate_sampled_token_ids_from_embedding_rows_reference(
            prompt_token_ids,
            prompt_embedding_rows,
            max_new_tokens,
            sampling_options,
            rng,
            on_generated_ids,
        )
    }

    fn generate_sampled_token_ids_from_embedding_rows_reference<F>(
        self: &Arc<Self>,
        prompt_token_ids: Arc<[u32]>,
        prompt_embedding_rows: Vec<Vec<u16>>,
        max_new_tokens: Option<usize>,
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
        mut on_generated_ids: F,
    ) -> Result<(Arc<[u32]>, GemmaStopReason), String>
    where
        F: FnMut(&[u32]) -> Result<(), String>,
    {
        if prompt_token_ids.is_empty() {
            return Err("generation requires at least one prompt token".to_string());
        }

        let stop_tokens = &self.stop_tokens;
        let constraints = ChatSamplingConstraints::from_runtime(self);
        let mut sampling_state = ChatSamplingState::new();
        let mut caches = GemmaKvCacheSet::<f32>::new(self.kv_layout.clone())
            .map_err(|err| err.to_string())?;

        let mut generated_token_ids = Vec::with_capacity(max_new_tokens.unwrap_or(32));
        let mut next_token = self.prefill_prompt_sampled_from_embedding_rows_reference(
            prompt_token_ids.as_ref(),
            &prompt_embedding_rows,
            0,
            &sampling_state.disallowed_token_ids(&constraints, stop_tokens, sampling_options),
            sampling_options,
            rng,
            &mut caches,
        )?;

        loop {
            generated_token_ids.push(next_token.token_id);
            sampling_state.observe_token(next_token.token_id, &constraints);
            on_generated_ids(&generated_token_ids)?;

            if stop_tokens.contains(&next_token.token_id) {
                return Ok((
                    Arc::<[u32]>::from(generated_token_ids),
                    GemmaStopReason::EosToken(next_token.token_id),
                ));
            }
            if max_new_tokens.is_some_and(|limit| generated_token_ids.len() >= limit) {
                return Ok((
                    Arc::<[u32]>::from(generated_token_ids),
                    GemmaStopReason::MaxNewTokens,
                ));
            }

            let position = prompt_token_ids
                .len()
                .checked_add(generated_token_ids.len())
                .and_then(|value| value.checked_sub(1))
                .ok_or_else(|| "generation cursor position overflow".to_string())?;
            next_token = self.eval_token_sampled_from_token_id_reference(
                next_token.token_id,
                position,
                &sampling_state.disallowed_token_ids(&constraints, stop_tokens, sampling_options),
                sampling_options,
                rng,
                &mut caches,
            )?;
        }
    }
}

fn split_f32_rows_to_bf16_words(
    values: &[f32],
    row_count: usize,
    row_len: usize,
    context: &str,
) -> Result<Vec<Vec<u16>>, String> {
    let expected = row_count
        .checked_mul(row_len)
        .ok_or_else(|| format!("{context} row shape overflow"))?;
    if values.len() != expected {
        return Err(format!(
            "{context} length mismatch: got {} expected {}",
            values.len(),
            expected
        ));
    }
    let mut rows = Vec::with_capacity(row_count);
    for row_idx in 0..row_count {
        let start = row_idx * row_len;
        let end = start + row_len;
        rows.push(f32s_to_bf16_words(&values[start..end]));
    }
    Ok(rows)
}

fn rms_norm_weighted_tensor(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
) -> Result<Vec<f32>, String> {
    weights
        .header_for_tensor(weight_name)
        .map_err(|err| err.to_string())?
        .rms_norm_weighted_f32(
            input_words,
            weight_name,
            weights.snapshot.config.text_config.rms_norm_eps,
        )
        .map_err(|err| err.to_string())
}

fn quantized_matmul_tensor(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
) -> Result<Vec<f32>, String> {
    let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
    if weight_entry.dtype == MlxDType::BF16 {
        return dense_bf16_matmul_tensor(weights, input_words, weight_name);
    }
    if weight_entry.dtype != MlxDType::U32 {
        return Err(format!(
            "tensor {weight_name} expected U32 or BF16, got {:?}",
            weight_entry.dtype
        ));
    }

    if weights.quantization_mode() == "nvfp4" {
        let (rows, _, inner_dim) = weights
            .nvfp4_rank2_layout(weight_name, scales_name)
            .map_err(|err| err.to_string())?;
        if input_words.len() != inner_dim {
            return Err(format!(
                "NVFP4 activation length mismatch: got {} expected {}",
                input_words.len(),
                inner_dim
            ));
        }

        let root = weights.snapshot.paths.root_dir.to_string_lossy();
        let weight_key = format!("{root}:{weight_name}");
        if let Some(result) = try_matmul_nt_ggml_bytes_cached_bf16_words(
            input_words,
            GGML_TYPE_NVFP4,
            1,
            inner_dim,
            rows,
            root.as_ref(),
            &weight_key,
            || {
                weights
                    .repack_nvfp4_tensor_to_ggml_bytes(weight_name, scales_name)
                    .map_err(|err| err.to_string())
            },
        ) {
            return result;
        }

        let x = input_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();

        let mut out = Vec::with_capacity(rows);
        for row in 0..rows {
            let row_bytes = weights
                .repack_nvfp4_row_to_ggml_bytes(weight_name, scales_name, row as u64)
                .map_err(|err| err.to_string())?;
            let mut sum = 0.0f32;
            for (block, input_block) in row_bytes.chunks_exact(36).zip(x.chunks_exact(64)) {
                sum += vec_dot_nvfp4_f32(block, input_block);
            }
            out.push(sum);
        }
        return Ok(out);
    }

    let bits = weights.snapshot.config.quantization.bits;
    let group_size = weights.snapshot.config.quantization.group_size as u64;
    if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
        return Err(format!("unsupported affine quantized matmul bits {bits}"));
    }

    let scales_entry = weights.tensor(scales_name).map_err(|err| err.to_string())?;
    let biases_entry = weights.tensor(biases_name).map_err(|err| err.to_string())?;
    if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
        return Err(format!(
            "tensors {scales_name} / {biases_name} expected BF16, got {:?} / {:?}",
            scales_entry.dtype, biases_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 2
        || scales_entry.shape.len() != 2
        || biases_entry.shape.len() != 2
    {
        return Err(format!(
            "quantized matmul expects rank-2 tensors, got {:?} {:?} {:?}",
            weight_entry.shape, scales_entry.shape, biases_entry.shape
        ));
    }
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "scale/bias shape mismatch: {:?} vs {:?}",
            scales_entry.shape, biases_entry.shape
        ));
    }
    if weight_entry.shape[0] != scales_entry.shape[0] {
        return Err(format!(
            "weight/scales outer shape mismatch: {:?} vs {:?}",
            weight_entry.shape, scales_entry.shape
        ));
    }

    let values_per_word = 32 / bits as u64;
    let inner_dim = weight_entry.shape[1] * values_per_word;
    if inner_dim != scales_entry.shape[1] * group_size {
        return Err(format!(
            "packed/scales shape mismatch for group_size={group_size} bits={bits}"
        ));
    }
    if input_words.len() as u64 != inner_dim {
        return Err(format!(
            "activation length mismatch: got {} expected {inner_dim}",
            input_words.len()
        ));
    }
    let words_per_group = group_size / values_per_word;
    if words_per_group == 0 || weight_entry.shape[1] != scales_entry.shape[1] * words_per_group {
        return Err(format!("invalid words_per_group {words_per_group}"));
    }

    let root = weights.snapshot.paths.root_dir.to_string_lossy();
    let weight_key = format!("{root}:{weight_name}");
    let scales_key = format!("{root}:{scales_name}");
    let biases_key = format!("{root}:{biases_name}");
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
        || {
            weights
                .read_tensor_bytes(weight_name)
                .map_err(|err| err.to_string())
        },
        || {
            weights
                .read_tensor_bytes(scales_name)
                .map_err(|err| err.to_string())
        },
        || {
            weights
                .read_tensor_bytes(biases_name)
                .map_err(|err| err.to_string())
        },
    ) {
        return result;
    }

    let packed_weights = weights
        .header_for_tensor(weight_name)
        .map_err(|err| err.to_string())?
        .read_u32_tensor_words(weight_name)
        .map_err(|err| err.to_string())?;
    let scales = weights
        .header_for_tensor(scales_name)
        .map_err(|err| err.to_string())?
        .read_bf16_tensor_words(scales_name)
        .map_err(|err| err.to_string())?;
    let biases = weights
        .header_for_tensor(biases_name)
        .map_err(|err| err.to_string())?
        .read_bf16_tensor_words(biases_name)
        .map_err(|err| err.to_string())?;
    let x = input_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    let rows = weight_entry.shape[0] as usize;
    let weight_stride = weight_entry.shape[1] as usize;
    let groups_per_row = scales_entry.shape[1] as usize;
    let pack_factor = values_per_word as usize;
    let mask = (1u32 << bits) - 1;
    let mut out = Vec::with_capacity(rows);

    for row in 0..rows {
        let weight_row_start = row * weight_stride;
        let qparam_row_start = row * groups_per_row;
        let mut total = 0.0f32;
        let mut x_index = 0usize;
        for group in 0..groups_per_row {
            let scale = bf16_word_to_f32(scales[qparam_row_start + group]);
            let bias = bf16_word_to_f32(biases[qparam_row_start + group]);
            let group_start = weight_row_start + group * words_per_group as usize;
            let group_end = group_start + words_per_group as usize;
            let mut group_sum = 0.0f32;
            let mut group_accum = 0.0f32;
            for packed in &packed_weights[group_start..group_end] {
                let mut packed_word = *packed;
                for _ in 0..pack_factor {
                    let q = (packed_word & mask) as f32;
                    let xi = x[x_index];
                    group_sum += xi;
                    group_accum += xi * q;
                    x_index += 1;
                    if bits != 8 {
                        packed_word >>= bits;
                    }
                }
            }
            total += bf16_round_to_f32(scale * group_accum) + bf16_round_to_f32(bias * group_sum);
        }
        out.push(bf16_round_to_f32(total));
    }

    Ok(out)
}

struct QuantizedTensorSpec<'a> {
    weight_name: &'a str,
    scales_name: &'a str,
    biases_name: &'a str,
}

fn quantized_matmul_tensors_shared_input(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    specs: &[QuantizedTensorSpec<'_>],
) -> Result<Vec<Vec<f32>>, String> {
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    if specs.len() == 1 || weights.quantization_mode() != "nvfp4" {
        return specs
            .iter()
            .map(|spec| {
                quantized_matmul_tensor(
                    weights,
                    input_words,
                    spec.weight_name,
                    spec.scales_name,
                    spec.biases_name,
                )
            })
            .collect();
    }

    let mut total_rows = 0usize;
    let mut row_counts = Vec::with_capacity(specs.len());
    let mut tensor_pairs = Vec::with_capacity(specs.len());
    let mut expected_inner_dim = None::<usize>;
    for spec in specs {
        let (rows, _, inner_dim) = weights
            .nvfp4_rank2_layout(spec.weight_name, spec.scales_name)
            .map_err(|err| err.to_string())?;
        if input_words.len() != inner_dim {
            return Err(format!(
                "NVFP4 activation length mismatch for {}: got {} expected {}",
                spec.weight_name,
                input_words.len(),
                inner_dim
            ));
        }
        if let Some(expected) = expected_inner_dim {
            if inner_dim != expected {
                return Err(format!(
                    "NVFP4 concatenation expects shared inner dim, got {} for {} vs {}",
                    inner_dim, spec.weight_name, expected
                ));
            }
        } else {
            expected_inner_dim = Some(inner_dim);
        }
        total_rows = total_rows
            .checked_add(rows)
            .ok_or_else(|| "NVFP4 concatenated output row count overflow".to_string())?;
        row_counts.push(rows);
        tensor_pairs.push((spec.weight_name, spec.scales_name));
    }

    let inner_dim = expected_inner_dim.unwrap_or(0);
    let root = weights.snapshot.paths.root_dir.to_string_lossy();
    let mut cache_key = format!("{root}:nvfp4:");
    for (index, spec) in specs.iter().enumerate() {
        if index != 0 {
            cache_key.push('|');
        }
        cache_key.push_str(spec.weight_name);
    }
    if let Some(result) = try_matmul_nt_ggml_bytes_cached_bf16_words(
        input_words,
        GGML_TYPE_NVFP4,
        1,
        inner_dim,
        total_rows,
        root.as_ref(),
        &cache_key,
        || {
            weights
                .repack_nvfp4_tensors_to_ggml_bytes(&tensor_pairs)
                .map_err(|err| err.to_string())
        },
    ) {
        let flat = result?;
        if flat.len() != total_rows {
            return Err(format!(
                "NVFP4 concatenated output length mismatch: got {} expected {}",
                flat.len(),
                total_rows
            ));
        }
        let mut outputs = Vec::with_capacity(row_counts.len());
        let mut offset = 0usize;
        for rows in row_counts {
            outputs.push(flat[offset..offset + rows].to_vec());
            offset += rows;
        }
        return Ok(outputs);
    }

    specs
        .iter()
        .map(|spec| {
            quantized_matmul_tensor(
                weights,
                input_words,
                spec.weight_name,
                spec.scales_name,
                spec.biases_name,
            )
        })
        .collect()
}

fn dense_bf16_matmul_tensor(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
) -> Result<Vec<f32>, String> {
    let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
    if weight_entry.dtype != MlxDType::BF16 {
        return Err(format!(
            "tensor {weight_name} expected BF16, got {:?}",
            weight_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 2 {
        return Err(format!(
            "dense bf16 matmul expects rank-2 tensor, got {:?}",
            weight_entry.shape
        ));
    }
    let rows = weight_entry.shape[0] as usize;
    let inner_dim = weight_entry.shape[1] as usize;
    if input_words.len() != inner_dim {
        return Err(format!(
            "activation length mismatch: got {} expected {}",
            input_words.len(),
            inner_dim
        ));
    }

    let weight_words = weights
        .read_bf16_tensor_words_cached(weight_name)
        .map_err(|err| err.to_string())?;
    let expected_words = rows
        .checked_mul(inner_dim)
        .ok_or_else(|| format!("dense bf16 matmul shape overflow for {weight_name}"))?;
    if weight_words.len() != expected_words {
        return Err(format!(
            "tensor {weight_name} word count mismatch: got {} expected {}",
            weight_words.len(),
            expected_words
        ));
    }

    let x = input_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    if let Some(out) = try_matmul_nt_ggml_bytes(
        &x,
        bf16_words_as_bytes(weight_words.as_slice()),
        GGML_TYPE_BF16,
        1,
        inner_dim,
        rows,
    ) {
        return Ok(out);
    }

    let mut out = Vec::with_capacity(rows);
    for row_idx in 0..rows {
        let row_start = row_idx * inner_dim;
        let row_end = row_start + inner_dim;
        let mut sum = 0.0f32;
        for (weight_word, x_value) in weight_words[row_start..row_end].iter().zip(x.iter()) {
            let product = bf16_round_to_f32(bf16_word_to_f32(*weight_word) * *x_value);
            sum = bf16_round_to_f32(sum + product);
        }
        out.push(sum);
    }
    Ok(out)
}

fn bf16_words_as_bytes(words: &[u16]) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * size_of::<u16>())
    }

    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("bf16 byte reinterpreting currently assumes little-endian targets")
    }
}

fn quantized_matmul_rank3_plane_tensor(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
    plane: u64,
) -> Result<Vec<f32>, String> {
    let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
    if weight_entry.dtype == MlxDType::BF16 {
        return dense_bf16_matmul_rank3_plane_tensor(weights, input_words, weight_name, plane);
    }
    if weight_entry.dtype != MlxDType::U32 {
        return Err(format!(
            "tensor {weight_name} expected U32 or BF16, got {:?}",
            weight_entry.dtype
        ));
    }

    let bits = weights.snapshot.config.quantization.bits;
    let group_size = weights.snapshot.config.quantization.group_size as u64;
    if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
        return Err(format!("unsupported affine quantized matmul bits {bits}"));
    }

    let scales_entry = weights.tensor(scales_name).map_err(|err| err.to_string())?;
    let biases_entry = weights.tensor(biases_name).map_err(|err| err.to_string())?;
    if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
        return Err(format!(
            "tensors {scales_name} / {biases_name} expected BF16, got {:?} / {:?}",
            scales_entry.dtype, biases_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 3
        || scales_entry.shape.len() != 3
        || biases_entry.shape.len() != 3
    {
        return Err(format!(
            "rank-3 affine quantized matmul expects rank-3 tensors, got {:?} {:?} {:?}",
            weight_entry.shape, scales_entry.shape, biases_entry.shape
        ));
    }
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "scale/bias shape mismatch: {:?} vs {:?}",
            scales_entry.shape, biases_entry.shape
        ));
    }
    if weight_entry.shape[0] != scales_entry.shape[0]
        || weight_entry.shape[1] != scales_entry.shape[1]
    {
        return Err(format!(
            "weight/scales outer shape mismatch: {:?} vs {:?}",
            weight_entry.shape, scales_entry.shape
        ));
    }
    if plane >= weight_entry.shape[0] {
        return Err(format!(
            "plane {plane} out of range for tensor {weight_name} with {} planes",
            weight_entry.shape[0]
        ));
    }

    let values_per_word = 32 / bits as u64;
    let inner_dim = weight_entry.shape[2] * values_per_word;
    if inner_dim != scales_entry.shape[2] * group_size {
        return Err(format!(
            "packed/scales plane shape mismatch for group_size={group_size} bits={bits}"
        ));
    }
    if input_words.len() as u64 != inner_dim {
        return Err(format!(
            "activation length mismatch: got {} expected {inner_dim}",
            input_words.len()
        ));
    }
    let words_per_group = group_size / values_per_word;
    if words_per_group == 0 || weight_entry.shape[2] != scales_entry.shape[2] * words_per_group {
        return Err(format!("invalid words_per_group {words_per_group}"));
    }

    let root = weights.snapshot.paths.root_dir.to_string_lossy();
    let weight_key = format!("{root}:{weight_name}@{plane}");
    let scales_key = format!("{root}:{scales_name}@{plane}");
    let biases_key = format!("{root}:{biases_name}@{plane}");
    if let Some(result) = try_affine_quantized_matmul_bf16(
        AffineQuantizedMatmulSpec {
            input_bf16_words: input_words,
            out_rows: weight_entry.shape[1] as usize,
            weight_words_per_row: weight_entry.shape[2] as usize,
            qparams_per_row: scales_entry.shape[2] as usize,
            bits,
            group_size,
            cache_namespace: root.as_ref(),
        },
        &weight_key,
        &scales_key,
        &biases_key,
        || {
            let header = weights
                .header_for_tensor(weight_name)
                .map_err(|err| err.to_string())?;
            let words = header
                .read_rank3_plane_u32_words(weight_name, plane)
                .map_err(|err| err.to_string())?;
            Ok(words
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .collect::<Vec<_>>())
        },
        || {
            let header = weights
                .header_for_tensor(scales_name)
                .map_err(|err| err.to_string())?;
            let words = header
                .read_rank3_plane_bf16_words(scales_name, plane)
                .map_err(|err| err.to_string())?;
            Ok(bf16_words_as_bytes(&words).to_vec())
        },
        || {
            let header = weights
                .header_for_tensor(biases_name)
                .map_err(|err| err.to_string())?;
            let words = header
                .read_rank3_plane_bf16_words(biases_name, plane)
                .map_err(|err| err.to_string())?;
            Ok(bf16_words_as_bytes(&words).to_vec())
        },
    ) {
        return result;
    }

    let packed_weights = weights
        .header_for_tensor(weight_name)
        .map_err(|err| err.to_string())?
        .read_rank3_plane_u32_words(weight_name, plane)
        .map_err(|err| err.to_string())?;
    let scales = weights
        .header_for_tensor(scales_name)
        .map_err(|err| err.to_string())?
        .read_rank3_plane_bf16_words(scales_name, plane)
        .map_err(|err| err.to_string())?;
    let biases = weights
        .header_for_tensor(biases_name)
        .map_err(|err| err.to_string())?
        .read_rank3_plane_bf16_words(biases_name, plane)
        .map_err(|err| err.to_string())?;
    let x = input_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    let rows = weight_entry.shape[1] as usize;
    let weight_stride = weight_entry.shape[2] as usize;
    let groups_per_row = scales_entry.shape[2] as usize;
    let pack_factor = values_per_word as usize;
    let mask = (1u32 << bits) - 1;
    let mut out = Vec::with_capacity(rows);

    for row in 0..rows {
        let weight_row_start = row * weight_stride;
        let qparam_row_start = row * groups_per_row;
        let mut sum = 0.0f32;
        let mut x_index = 0usize;
        for group in 0..groups_per_row {
            let scale = bf16_word_to_f32(scales[qparam_row_start + group]);
            let bias = bf16_word_to_f32(biases[qparam_row_start + group]);
            let group_start = weight_row_start + group * words_per_group as usize;
            let group_end = group_start + words_per_group as usize;
            for packed in &packed_weights[group_start..group_end] {
                let mut packed_word = *packed;
                for _ in 0..pack_factor {
                    let q = (packed_word & mask) as f32;
                    let deq_mul = bf16_round_to_f32(scale * q);
                    let deq = bf16_round_to_f32(deq_mul + bias);
                    let prod = bf16_round_to_f32(x[x_index] * deq);
                    sum = bf16_round_to_f32(sum + prod);
                    x_index += 1;
                    if bits != 8 {
                        packed_word >>= bits;
                    }
                }
            }
        }
        out.push(sum);
    }

    Ok(out)
}

fn dense_bf16_matmul_rank3_plane_tensor(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
    plane: u64,
) -> Result<Vec<f32>, String> {
    let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
    if weight_entry.dtype != MlxDType::BF16 {
        return Err(format!(
            "tensor {weight_name} expected BF16, got {:?}",
            weight_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 3 {
        return Err(format!(
            "dense bf16 plane matmul expects rank-3 tensor, got {:?}",
            weight_entry.shape
        ));
    }
    if plane >= weight_entry.shape[0] {
        return Err(format!(
            "plane {plane} out of range for tensor {weight_name} with {} planes",
            weight_entry.shape[0]
        ));
    }

    let rows = weight_entry.shape[1] as usize;
    let inner_dim = weight_entry.shape[2] as usize;
    if input_words.len() != inner_dim {
        return Err(format!(
            "activation length mismatch: got {} expected {}",
            input_words.len(),
            inner_dim
        ));
    }

    let plane_words = weights
        .header_for_tensor(weight_name)
        .map_err(|err| err.to_string())?
        .read_rank3_plane_bf16_words(weight_name, plane)
        .map_err(|err| err.to_string())?;
    let expected_words = rows
        .checked_mul(inner_dim)
        .ok_or_else(|| format!("dense bf16 plane matmul shape overflow for {weight_name}"))?;
    if plane_words.len() != expected_words {
        return Err(format!(
            "tensor {weight_name} plane word count mismatch: got {} expected {}",
            plane_words.len(),
            expected_words
        ));
    }

    let x = input_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    if let Some(out) = try_matmul_nt_ggml_bytes(
        &x,
        bf16_words_as_bytes(&plane_words),
        GGML_TYPE_BF16,
        1,
        inner_dim,
        rows,
    ) {
        return Ok(out);
    }

    let mut out = Vec::with_capacity(rows);
    for row_idx in 0..rows {
        let row_start = row_idx * inner_dim;
        let row_end = row_start + inner_dim;
        let mut sum = 0.0f32;
        for (weight_word, x_value) in plane_words[row_start..row_end].iter().zip(x.iter()) {
            let product = bf16_round_to_f32(bf16_word_to_f32(*weight_word) * *x_value);
            sum = bf16_round_to_f32(sum + product);
        }
        out.push(sum);
    }
    Ok(out)
}

fn gemma_router_topk_from_residual_bf16(
    weights: &MlxIndexedSafetensors,
    residual_bf16_words: &[u16],
    router_scale_name: &str,
    per_expert_scale_name: &str,
    proj_weight_name: &str,
    proj_scales_name: &str,
    proj_biases_name: &str,
    eps: f32,
    top_k: usize,
) -> Result<MlxRouterTopKOutput, String> {
    if top_k == 0 {
        return Err("router top_k must be greater than zero".to_string());
    }

    let hidden = residual_bf16_words.len();
    let router_scale_words = weights
        .read_bf16_tensor_words(router_scale_name)
        .map_err(|err| err.to_string())?;
    if router_scale_words.len() != hidden {
        return Err(format!(
            "router scale length mismatch: got {} expected {}",
            router_scale_words.len(),
            hidden
        ));
    }
    let per_expert_scale_words = weights
        .read_bf16_tensor_words(per_expert_scale_name)
        .map_err(|err| err.to_string())?;
    if top_k > per_expert_scale_words.len() {
        return Err(format!(
            "router top_k {top_k} exceeds num_experts {}",
            per_expert_scale_words.len()
        ));
    }

    let residual = residual_bf16_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    let mut mean_square = 0.0f32;
    for value in &residual {
        mean_square += value * value;
    }
    mean_square /= hidden as f32;
    let inv_rms = 1.0f32 / (mean_square + eps).sqrt();

    let root_size = bf16_round_to_f32((hidden as f32).powf(-0.5));
    let mut router_scaled = Vec::with_capacity(hidden);
    let mut router_scaled_words = Vec::with_capacity(hidden);
    for (index, value) in residual.iter().copied().enumerate() {
        let normed = bf16_round_to_f32(value * inv_rms);
        let scaled_root = bf16_round_to_f32(normed * root_size);
        let scaled = bf16_round_to_f32(scaled_root * bf16_word_to_f32(router_scale_words[index]));
        router_scaled.push(scaled);
        router_scaled_words.push(f32_to_bf16_word(scaled));
    }

    let expert_scores = quantized_matmul_tensor(
        weights,
        &router_scaled_words,
        proj_weight_name,
        proj_scales_name,
        proj_biases_name,
    )?;
    if expert_scores.is_empty() {
        return Err("router projection produced no scores".to_string());
    }
    if top_k > expert_scores.len() {
        return Err(format!(
            "router top_k {top_k} exceeds expert_scores length {}",
            expert_scores.len()
        ));
    }

    let max_score = expert_scores
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let exp_scores = expert_scores
        .iter()
        .copied()
        .map(|value| (value - max_score).exp())
        .collect::<Vec<_>>();
    let exp_sum = exp_scores.iter().copied().sum::<f32>();
    let router_probs = exp_scores
        .iter()
        .copied()
        .map(|value| bf16_round_to_f32(value / exp_sum))
        .collect::<Vec<_>>();

    let mut indices = (0..expert_scores.len()).collect::<Vec<_>>();
    indices.sort_by(|&lhs, &rhs| {
        expert_scores[rhs]
            .total_cmp(&expert_scores[lhs])
            .then_with(|| lhs.cmp(&rhs))
    });
    let top_k_indices = indices
        .into_iter()
        .take(top_k)
        .map(|index| index as u32)
        .collect::<Vec<_>>();

    let mut top_k_weights = top_k_indices
        .iter()
        .copied()
        .map(|index| router_probs[index as usize])
        .collect::<Vec<_>>();
    let mut top_k_sum = 0.0f32;
    for weight in &top_k_weights {
        top_k_sum = bf16_round_to_f32(top_k_sum + *weight);
    }
    for (slot, weight) in top_k_weights.iter_mut().enumerate() {
        let normalized = bf16_round_to_f32(*weight / top_k_sum);
        let expert_scale = bf16_word_to_f32(per_expert_scale_words[top_k_indices[slot] as usize]);
        *weight = bf16_round_to_f32(normalized * expert_scale);
    }

    Ok(MlxRouterTopKOutput {
        router_scaled,
        expert_scores,
        router_probs,
        top_k_indices,
        top_k_weights,
    })
}

fn gemma_moe_expert_block_from_residual_bf16(
    weights: &MlxIndexedSafetensors,
    residual_bf16_words: &[u16],
    pre_feedforward_norm2_weight_name: &str,
    expert_gate_weight_name: &str,
    expert_gate_scales_name: &str,
    expert_gate_biases_name: &str,
    expert_up_weight_name: &str,
    expert_up_scales_name: &str,
    expert_up_biases_name: &str,
    expert_down_weight_name: &str,
    expert_down_scales_name: &str,
    expert_down_biases_name: &str,
    top_k_indices: &[u32],
    top_k_weights: &[f32],
) -> Result<MlxGemmaMoeExpertOutput, String> {
    if top_k_indices.is_empty() {
        return Err("moe expert path needs at least one routed expert".to_string());
    }
    if top_k_indices.len() != top_k_weights.len() {
        return Err(format!(
            "top_k index/weight length mismatch: {} vs {}",
            top_k_indices.len(),
            top_k_weights.len()
        ));
    }

    let pre_feedforward_norm2 = rms_norm_weighted_tensor(
        weights,
        residual_bf16_words,
        pre_feedforward_norm2_weight_name,
    )?;
    let pre_feedforward_norm2_words = pre_feedforward_norm2
        .iter()
        .copied()
        .map(f32_to_bf16_word)
        .collect::<Vec<_>>();

    let mut gate_proj = Vec::new();
    let mut up_proj = Vec::new();
    let mut geglu = Vec::new();
    let mut down_proj = Vec::new();
    let hidden = residual_bf16_words.len();

    for &expert_index in top_k_indices {
        let gate_row = quantized_matmul_rank3_plane_tensor(
            weights,
            &pre_feedforward_norm2_words,
            expert_gate_weight_name,
            expert_gate_scales_name,
            expert_gate_biases_name,
            expert_index as u64,
        )?;
        let up_row = quantized_matmul_rank3_plane_tensor(
            weights,
            &pre_feedforward_norm2_words,
            expert_up_weight_name,
            expert_up_scales_name,
            expert_up_biases_name,
            expert_index as u64,
        )?;
        if gate_row.len() != up_row.len() {
            return Err(format!(
                "expert {expert_index} gate/up output length mismatch: {} vs {}",
                gate_row.len(),
                up_row.len()
            ));
        }

        let mut geglu_row = Vec::with_capacity(gate_row.len());
        for (&gate, &up) in gate_row.iter().zip(up_row.iter()) {
            geglu_row.push(bf16_round_to_f32(gelu_approx_f32(gate) * up));
        }
        let geglu_words = geglu_row
            .iter()
            .copied()
            .map(f32_to_bf16_word)
            .collect::<Vec<_>>();
        let down_row = quantized_matmul_rank3_plane_tensor(
            weights,
            &geglu_words,
            expert_down_weight_name,
            expert_down_scales_name,
            expert_down_biases_name,
            expert_index as u64,
        )?;
        if down_row.len() != hidden {
            return Err(format!(
                "expert {expert_index} down projection length mismatch: got {} expected {hidden}",
                down_row.len()
            ));
        }

        gate_proj.extend_from_slice(&gate_row);
        up_proj.extend_from_slice(&up_row);
        geglu.extend_from_slice(&geglu_row);
        down_proj.extend_from_slice(&down_row);
    }

    let mut expert_out = vec![0.0f32; hidden];
    for (expert_slot, &weight) in top_k_weights.iter().enumerate() {
        for hidden_index in 0..hidden {
            let weighted =
                bf16_round_to_f32(down_proj[expert_slot * hidden + hidden_index] * weight);
            expert_out[hidden_index] = bf16_round_to_f32(expert_out[hidden_index] + weighted);
        }
    }

    Ok(MlxGemmaMoeExpertOutput {
        pre_feedforward_norm2,
        gate_proj,
        up_proj,
        geglu,
        down_proj,
        expert_out,
    })
}

fn load_optional_scalar_f32(
    weights: &MlxIndexedSafetensors,
    tensor_name: &str,
) -> Result<Option<f32>, String> {
    let tensor = match weights.tensor(tensor_name) {
        Ok(tensor) => tensor,
        Err(_) => return Ok(None),
    };
    if tensor.shape.iter().product::<u64>() != 1 {
        return Err(format!("layer scalar tensor {} is not scalar", tensor_name));
    }
    let words = weights
        .read_bf16_tensor_words(tensor_name)
        .map_err(|err| err.to_string())?;
    let word = words
        .first()
        .copied()
        .ok_or_else(|| format!("layer scalar tensor {} is empty", tensor_name))?;
    Ok(Some(bf16_word_to_f32(word)))
}

fn rms_norm_rows_weighted_f32(
    input: &[f32],
    row_count: usize,
    row_len: usize,
    weight_words: &[u16],
) -> Result<Vec<f32>, String> {
    if input.len() != row_count * row_len {
        return Err(format!(
            "row RMS input length mismatch: got {} expected {}",
            input.len(),
            row_count * row_len
        ));
    }
    if weight_words.len() != row_len {
        return Err(format!(
            "row RMS weight length mismatch: got {} expected {}",
            weight_words.len(),
            row_len
        ));
    }
    let weights = weight_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    let mut out = Vec::with_capacity(input.len());
    for row_idx in 0..row_count {
        let row = &input[row_idx * row_len..(row_idx + 1) * row_len];
        let inv_rms = inv_rms_f32(row, 1e-6f32);
        for (index, value) in row.iter().copied().enumerate() {
            let normalized = bf16_round_to_f32(value * inv_rms);
            out.push(bf16_round_to_f32(normalized * weights[index]));
        }
    }
    Ok(out)
}

fn rms_norm_rows_no_scale_f32(
    input: &[f32],
    row_count: usize,
    row_len: usize,
    eps: f32,
) -> Result<Vec<f32>, String> {
    if input.len() != row_count * row_len {
        return Err(format!(
            "row RMS no-scale input length mismatch: got {} expected {}",
            input.len(),
            row_count * row_len
        ));
    }
    let mut out = Vec::with_capacity(input.len());
    for row_idx in 0..row_count {
        let row = &input[row_idx * row_len..(row_idx + 1) * row_len];
        let inv_rms = inv_rms_f32(row, eps);
        for value in row {
            out.push(bf16_round_to_f32(*value * inv_rms));
        }
    }
    Ok(out)
}

fn apply_rope_rows_in_place(
    values: &mut [f32],
    row_count: usize,
    rope: RopeSpec,
    position: usize,
) -> Result<(), String> {
    if values.len() != row_count * rope.head_dim {
        return Err(format!(
            "rope row length mismatch: got {} expected {}",
            values.len(),
            row_count * rope.head_dim
        ));
    }
    if rope.rotary_dim == 0 || rope.rotary_dim > rope.head_dim || rope.rotary_dim % 2 != 0 {
        return Err(format!(
            "invalid rope rotary dimension {} for head_dim {}",
            rope.rotary_dim, rope.head_dim
        ));
    }
    let half = rope.head_dim / 2;
    let rotary_pairs = rope.rotary_dim / 2;
    if rotary_pairs > half {
        return Err(format!(
            "rope rotary pair count {} exceeds half-head {}",
            rotary_pairs, half
        ));
    }
    for row_idx in 0..row_count {
        let row = &mut values[row_idx * rope.head_dim..(row_idx + 1) * rope.head_dim];
        for pair_idx in 0..rotary_pairs {
            let exponent = (2.0f32 * pair_idx as f32) / rope.head_dim as f32;
            let inv_freq = rope.base.powf(-exponent);
            let theta = position as f32 * inv_freq;
            let cos_theta = theta.cos();
            let sin_theta = theta.sin();
            let left = row[pair_idx];
            let right = row[half + pair_idx];
            row[pair_idx] = bf16_round_to_f32(left * cos_theta - right * sin_theta);
            row[half + pair_idx] = bf16_round_to_f32(left * sin_theta + right * cos_theta);
        }
    }
    Ok(())
}

fn compute_attention_output_f32(
    q_values: &[f32],
    cache: &crate::GemmaKvCache<f32>,
    q_head_count: usize,
    q_heads_per_kv: usize,
    head_dim: usize,
) -> Result<Vec<f32>, Box<dyn Error>> {
    let cache_state = cache.fetch()?;
    let seq_len = cache_state.stored_tokens();
    let mut out = Vec::with_capacity(q_head_count * head_dim);

    for q_head in 0..q_head_count {
        let q_row = &q_values[q_head * head_dim..(q_head + 1) * head_dim];
        let kv_head = q_head / q_heads_per_kv;

        let mut scores = Vec::with_capacity(seq_len);
        for token in 0..seq_len {
            let k_row = cache_state.keys.row(0, kv_head, token)?;
            let mut sum = 0.0f32;
            for dim in 0..head_dim {
                sum += q_row[dim] * k_row[dim];
            }
            scores.push(bf16_round_to_f32(sum));
        }

        let max_score = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let exp_scores = scores
            .iter()
            .copied()
            .map(|score| (score - max_score).exp())
            .collect::<Vec<_>>();
        let exp_sum = exp_scores.iter().copied().sum::<f32>();
        let probs = exp_scores
            .iter()
            .copied()
            .map(|score| bf16_round_to_f32(score / exp_sum))
            .collect::<Vec<_>>();

        for dim in 0..head_dim {
            let mut acc = 0.0f32;
            for (token, prob) in probs.iter().enumerate() {
                let v_row = cache_state.values.row(0, kv_head, token)?;
                acc = bf16_round_to_f32(acc + bf16_round_to_f32(*prob * v_row[dim]));
            }
            out.push(acc);
        }
    }

    Ok(out)
}

fn single_token_tensor(
    head_count: usize,
    head_dim: usize,
    values: Vec<f32>,
) -> crate::kv::Result<KvTensor<f32>> {
    KvTensor::from_vec(
        KvTensorShape {
            batch_size: 1,
            kv_head_count: head_count,
            seq_len: 1,
            head_dim,
        },
        values,
    )
}

fn geglu_f32(gate: &[f32], up: &[f32]) -> Result<Vec<f32>, String> {
    if gate.len() != up.len() {
        return Err(format!(
            "GeGLU input length mismatch: gate={} up={}",
            gate.len(),
            up.len()
        ));
    }
    let mut out = Vec::with_capacity(gate.len());
    for (&gate_value, &up_value) in gate.iter().zip(up.iter()) {
        out.push(bf16_round_to_f32(gelu_approx_f32(gate_value) * up_value));
    }
    Ok(out)
}

fn gelu_approx_f32(value: f32) -> f32 {
    let squared = bf16_round_to_f32(value * value);
    let cubic = bf16_round_to_f32(squared * value);
    let poly = bf16_round_to_f32(value + bf16_round_to_f32(0.044_715f32 * cubic));
    let tanh_input = bf16_round_to_f32(0.797_884_6f32 * poly);
    let tanh_value = bf16_round_to_f32(tanh_input.tanh());
    let half = bf16_round_to_f32(0.5f32 * value);
    bf16_round_to_f32(half * bf16_round_to_f32(1.0f32 + tanh_value))
}

fn add_bf16_and_f32(left_words: &[u16], right: &[f32]) -> Result<Vec<f32>, String> {
    if left_words.len() != right.len() {
        return Err(format!(
            "add input length mismatch: left={} right={}",
            left_words.len(),
            right.len()
        ));
    }
    Ok(left_words
        .iter()
        .copied()
        .zip(right.iter().copied())
        .map(|(left, right)| bf16_round_to_f32(bf16_word_to_f32(left) + right))
        .collect())
}

fn add_f32(left: &[f32], right: &[f32]) -> Result<Vec<f32>, String> {
    if left.len() != right.len() {
        return Err(format!(
            "add input length mismatch: left={} right={}",
            left.len(),
            right.len()
        ));
    }
    Ok(left
        .iter()
        .copied()
        .zip(right.iter().copied())
        .map(|(left, right)| bf16_round_to_f32(left + right))
        .collect())
}

fn scale_in_place(values: &mut [f32], scale: f32) {
    for value in values {
        *value = bf16_round_to_f32(*value * scale);
    }
}

fn inv_rms_f32(values: &[f32], eps: f32) -> f32 {
    let mean_square = values
        .iter()
        .copied()
        .map(|value| value * value)
        .sum::<f32>()
        / values.len() as f32;
    1.0f32 / (mean_square + eps).sqrt()
}

fn f32s_to_bf16_words(values: &[f32]) -> Vec<u16> {
    values.iter().copied().map(f32_to_bf16_word).collect()
}

fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn f32_to_bf16_word(value: f32) -> u16 {
    (bf16_round_to_f32(value).to_bits() >> 16) as u16
}

fn bf16_round_to_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let lsb = (bits >> 16) & 1;
    f32::from_bits(bits.wrapping_add(0x7FFF + lsb) & 0xFFFF_0000)
}

fn model_root_dir(model_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if model_path.is_dir() {
        return Ok(model_path.to_path_buf());
    }
    model_path.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "model path {} has no parent directory",
            model_path.display()
        )
        .into()
    })
}
