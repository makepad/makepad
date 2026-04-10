    use super::{
        apply_rope_rows_in_place, bf16_word_to_f32, compute_attention_output_f32, lazy_text_plan,
        quantized_matmul_tensor, rms_norm_rows_no_scale_f32, rms_norm_rows_weighted_f32,
        rms_norm_weighted_tensor, run_two_token_ids, run_two_token_prompt, single_token_tensor,
        GemmaPromptFormat, GemmaStopReason, GemmaTextGenerationOptions, GemmaTextRuntimeSession,
        GemmaTextStepOutput, RopeSpec, TextLayerTensorNames,
    };
    use crate::GemmaKvCacheSet;
    use crate::fnv1a64_u32_words;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn default_model_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
    }

    fn assert_valid_step(output: &GemmaTextStepOutput) {
        assert_eq!(output.layers.len(), 30);
        assert_eq!(output.final_hidden_bf16_words.len(), 2_816);
        assert_eq!(output.final_norm_bf16_words.len(), 2_816);
        assert!(output.next_token.logit.is_finite());
        assert!(!output.next_token_text.is_empty());
    }

    #[test]
    #[ignore]
    fn two_token_prompt_executes_end_to_end() {
        let output = run_two_token_prompt(default_model_path(), "say hi").unwrap();
        assert_eq!(output.prompt_token_ids, vec![30_468, 5_631]);
        let layer_hashes = output
            .layers
            .iter()
            .map(|layer| {
                format!(
                    "0x{:016X}",
                    fnv1a64_u32_words(
                        layer
                            .layer_output_bits()
                            .expect("missing layer output bits in two-token step"),
                    )
                )
            })
            .collect::<Vec<_>>();
        let final_hidden_bits = output
            .final_hidden_bf16_words
            .iter()
            .map(|word| (*word as u32) << 16)
            .collect::<Vec<_>>();
        let final_norm_bits = output
            .final_norm_bf16_words
            .iter()
            .map(|word| (*word as u32) << 16)
            .collect::<Vec<_>>();
        println!("layer_post_ffn_residual_hashes={:?}", layer_hashes);
        println!(
            "final_hidden_fnv1a64=0x{:016X} final_norm_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&final_hidden_bits),
            fnv1a64_u32_words(&final_norm_bits)
        );
        println!(
            "next_token_id={} next_token_logit={} next_token_text={:?}",
            output.next_token.token_id, output.next_token.logit, output.next_token_text
        );
        assert_valid_step(&output);
    }

    #[test]
    #[ignore]
    fn two_token_prompt_writes_final_hidden_f32_for_mlx_oracle() {
        let output = run_two_token_prompt(default_model_path(), "say hi").unwrap();
        let dump_path = std::env::temp_dir().join("gemma_two_token_final_hidden_f32.bin");
        let mut bytes = Vec::with_capacity(output.final_hidden_bf16_words.len() * 4);
        for word in &output.final_hidden_bf16_words {
            let value = f32::from_bits((*word as u32) << 16);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        fs::write(&dump_path, bytes).unwrap();
        let final_hidden_bits = output
            .final_hidden_bf16_words
            .iter()
            .map(|word| (*word as u32) << 16)
            .collect::<Vec<_>>();
        println!("final_hidden_dump_path={}", dump_path.display());
        println!(
            "final_hidden_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&final_hidden_bits)
        );
        assert_eq!(output.final_hidden_bf16_words.len(), 2_816);
    }

    #[test]
    #[ignore]
    fn host_runtime_reports_against_exact_raw_two_token_path() {
        let exact = run_two_token_prompt(default_model_path(), "say hi").unwrap();
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let runtime = Arc::unwrap_or_clone(runtime);
        let mut caches = GemmaKvCacheSet::<f32>::new(runtime.kv_layout.clone()).unwrap();
        let num_layers = runtime
            .weights
            .snapshot
            .config
            .text_config
            .num_hidden_layers as usize;

        let mut prefill_hidden = runtime
            .weights
            .embed_token_bf16_words(exact.prompt_token_ids[0])
            .unwrap();
        for layer_idx in 0..num_layers {
            prefill_hidden = runtime
                .eval_layer_hidden_state(layer_idx, &prefill_hidden, None, 0, &mut caches)
                .unwrap();
        }

        let mut host_hidden = runtime
            .weights
            .embed_token_bf16_words(exact.prompt_token_ids[1])
            .unwrap();
        let mut host_layer_hashes = Vec::with_capacity(num_layers);
        for layer_idx in 0..num_layers {
            host_hidden = runtime
                .eval_layer_hidden_state(layer_idx, &host_hidden, None, 1, &mut caches)
                .unwrap();
            let layer_bits = host_hidden
                .iter()
                .map(|word| (*word as u32) << 16)
                .collect::<Vec<_>>();
            host_layer_hashes.push(format!("0x{:016X}", fnv1a64_u32_words(&layer_bits)));
        }
        let host_hidden_bits = host_hidden
            .iter()
            .map(|word| (*word as u32) << 16)
            .collect::<Vec<_>>();
        let exact_hidden_bits = exact
            .final_hidden_bf16_words
            .iter()
            .map(|word| (*word as u32) << 16)
            .collect::<Vec<_>>();
        let exact_layer_hashes = exact
            .layers
            .iter()
            .map(|layer| {
                format!(
                    "0x{:016X}",
                    fnv1a64_u32_words(layer.layer_output_bits().unwrap())
                )
            })
            .collect::<Vec<_>>();
        let host_next = runtime.greedy_token_from_hidden(&host_hidden).unwrap();
        let first_mismatch = exact_layer_hashes
            .iter()
            .zip(host_layer_hashes.iter())
            .position(|(exact_hash, host_hash)| exact_hash != host_hash);
        println!("exact_layer_hashes={exact_layer_hashes:?}");
        println!("host_layer_hashes={host_layer_hashes:?}");
        println!("first_mismatch_layer={first_mismatch:?}");
        println!(
            "exact_final_hidden_fnv1a64=0x{:016X} host_final_hidden_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&exact_hidden_bits),
            fnv1a64_u32_words(&host_hidden_bits),
        );
        println!(
            "exact_next_token_id={} host_next_token_id={} host_next_token_logit={}",
            exact.next_token.token_id, host_next.token_id, host_next.logit
        );
        assert_eq!(host_hidden.len(), exact.final_hidden_bf16_words.len());
    }

    #[test]
    #[ignore]
    fn host_layer0_reports_against_exact_cached_stage_hashes() {
        let exact = run_two_token_prompt(default_model_path(), "say hi").unwrap();
        let exact_layer0 = &exact.layers[0];
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let runtime = Arc::unwrap_or_clone(runtime);
        let config = &runtime.weights.snapshot.config.text_config;
        let layer_idx = 0usize;
        let layer_type = config.layer_types.get(layer_idx).unwrap();
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
        let q_heads_per_kv = q_head_count / k_head_count;
        let rope_params = if layer_type == "full_attention" {
            &config.rope_parameters.full_attention
        } else {
            &config.rope_parameters.sliding_attention
        };
        let rope_rotary_dim = if let Some(partial_factor) = rope_params.partial_rotary_factor {
            (head_dim as f32 * partial_factor).round() as usize
        } else {
            head_dim
        };
        let rope = RopeSpec {
            head_dim,
            rotary_dim: rope_rotary_dim,
            base: rope_params.rope_theta,
        };
        let names = TextLayerTensorNames::for_layer(layer_idx, attention_k_eq_v);
        let q_norm_weights = runtime
            .weights
            .read_bf16_tensor_words(names.q.norm_weight_name.as_deref().unwrap())
            .unwrap();
        let k_norm_weights = runtime
            .weights
            .read_bf16_tensor_words(names.k.norm_weight_name.as_deref().unwrap())
            .unwrap();

        let hash_f32 = |values: &[f32]| {
            let bits = values.iter().copied().map(f32::to_bits).collect::<Vec<_>>();
            fnv1a64_u32_words(&bits)
        };
        let hash_opt_bits =
            |bits: Option<&[u32]>| bits.map(fnv1a64_u32_words).expect("missing exact bits");

        let mut caches = GemmaKvCacheSet::<f32>::new(runtime.kv_layout.clone()).unwrap();
        let mut prefill_hidden = runtime
            .weights
            .embed_token_bf16_words(exact.prompt_token_ids[0])
            .unwrap();
        let prefill_input_norm = rms_norm_weighted_tensor(
            &runtime.weights,
            &prefill_hidden,
            &names.input_norm_weight_name,
        )
        .unwrap();
        let prefill_input_norm_words = prefill_input_norm
            .iter()
            .copied()
            .map(super::f32_to_bf16_word)
            .collect::<Vec<_>>();
        let prefill_k_raw = quantized_matmul_tensor(
            &runtime.weights,
            &prefill_input_norm_words,
            &names.k.weight_name,
            &names.k.scales_name,
            &names.k.biases_name,
        )
        .unwrap();
        let mut prefill_k =
            rms_norm_rows_weighted_f32(&prefill_k_raw, k_head_count, head_dim, &k_norm_weights)
                .unwrap();
        apply_rope_rows_in_place(&mut prefill_k, k_head_count, rope, 0).unwrap();
        let prefill_v_raw = if attention_k_eq_v {
            prefill_k_raw
        } else {
            quantized_matmul_tensor(
                &runtime.weights,
                &prefill_input_norm_words,
                &names.v.weight_name,
                &names.v.scales_name,
                &names.v.biases_name,
            )
            .unwrap()
        };
        let prefill_v =
            rms_norm_rows_no_scale_f32(&prefill_v_raw, v_head_count, head_dim, config.rms_norm_eps)
                .unwrap();
        let layer_cache = caches.cache_for_layer_mut(layer_idx).unwrap();
        layer_cache
            .update_and_fetch(
                single_token_tensor(k_head_count, head_dim, prefill_k.clone())
                    .unwrap()
                    .view(),
                single_token_tensor(v_head_count, head_dim, prefill_v.clone())
                    .unwrap()
                    .view(),
            )
            .unwrap();
        prefill_hidden = runtime
            .eval_layer_hidden_state(layer_idx, &prefill_hidden, None, 0, &mut caches)
            .unwrap();

        let decode_hidden = runtime
            .weights
            .embed_token_bf16_words(exact.prompt_token_ids[1])
            .unwrap();
        let decode_input_norm = rms_norm_weighted_tensor(
            &runtime.weights,
            &decode_hidden,
            &names.input_norm_weight_name,
        )
        .unwrap();
        let decode_input_norm_words = decode_input_norm
            .iter()
            .copied()
            .map(super::f32_to_bf16_word)
            .collect::<Vec<_>>();
        let decode_q_raw = quantized_matmul_tensor(
            &runtime.weights,
            &decode_input_norm_words,
            &names.q.weight_name,
            &names.q.scales_name,
            &names.q.biases_name,
        )
        .unwrap();
        let mut decode_q =
            rms_norm_rows_weighted_f32(&decode_q_raw, q_head_count, head_dim, &q_norm_weights)
                .unwrap();
        apply_rope_rows_in_place(&mut decode_q, q_head_count, rope, 1).unwrap();
        let decode_k_raw = quantized_matmul_tensor(
            &runtime.weights,
            &decode_input_norm_words,
            &names.k.weight_name,
            &names.k.scales_name,
            &names.k.biases_name,
        )
        .unwrap();
        let mut decode_k =
            rms_norm_rows_weighted_f32(&decode_k_raw, k_head_count, head_dim, &k_norm_weights)
                .unwrap();
        apply_rope_rows_in_place(&mut decode_k, k_head_count, rope, 1).unwrap();
        let decode_v_raw = if attention_k_eq_v {
            decode_k_raw
        } else {
            quantized_matmul_tensor(
                &runtime.weights,
                &decode_input_norm_words,
                &names.v.weight_name,
                &names.v.scales_name,
                &names.v.biases_name,
            )
            .unwrap()
        };
        let decode_v =
            rms_norm_rows_no_scale_f32(&decode_v_raw, v_head_count, head_dim, config.rms_norm_eps)
                .unwrap();
        let layer_cache = caches.cache_for_layer_mut(layer_idx).unwrap();
        layer_cache
            .update_and_fetch(
                single_token_tensor(k_head_count, head_dim, decode_k.clone())
                    .unwrap()
                    .view(),
                single_token_tensor(v_head_count, head_dim, decode_v.clone())
                    .unwrap()
                    .view(),
            )
            .unwrap();
        let attention_out = compute_attention_output_f32(
            &decode_q,
            layer_cache,
            q_head_count,
            q_heads_per_kv,
            head_dim,
        )
        .unwrap();
        let attention_out_words = attention_out
            .iter()
            .copied()
            .map(super::f32_to_bf16_word)
            .collect::<Vec<_>>();
        let attention_oproj = quantized_matmul_tensor(
            &runtime.weights,
            &attention_out_words,
            &names.o.weight_name,
            &names.o.scales_name,
            &names.o.biases_name,
        )
        .unwrap();

        println!(
            "prefill_input_norm host=0x{:016X} exact=0x{:016X}",
            hash_f32(&prefill_input_norm),
            fnv1a64_u32_words(&exact_layer0.prefill_input_norm_bits),
        );
        println!(
            "prefill_k host=0x{:016X} exact=0x{:016X}",
            hash_f32(&prefill_k),
            fnv1a64_u32_words(&exact_layer0.prefill_k_bits),
        );
        println!(
            "prefill_v_proj host=0x{:016X} exact=0x{:016X}",
            hash_f32(&prefill_v_raw),
            fnv1a64_u32_words(&exact_layer0.prefill_v_proj_bits),
        );
        println!(
            "prefill_v host=0x{:016X} exact=0x{:016X}",
            hash_f32(&prefill_v),
            fnv1a64_u32_words(&exact_layer0.prefill_v_bits),
        );
        println!(
            "decode_input_norm host=0x{:016X} exact=0x{:016X}",
            hash_f32(&decode_input_norm),
            fnv1a64_u32_words(&exact_layer0.decode_input_norm_bits),
        );
        println!(
            "decode_q host=0x{:016X} exact=0x{:016X}",
            hash_f32(&decode_q),
            fnv1a64_u32_words(&exact_layer0.decode_q_bits),
        );
        println!(
            "decode_k host=0x{:016X} exact=0x{:016X}",
            hash_f32(&decode_k),
            fnv1a64_u32_words(&exact_layer0.decode_k_bits),
        );
        println!(
            "decode_v_proj host=0x{:016X} exact=0x{:016X}",
            hash_f32(&decode_v_raw),
            fnv1a64_u32_words(&exact_layer0.decode_v_proj_bits),
        );
        println!(
            "decode_v host=0x{:016X} exact=0x{:016X}",
            hash_f32(&decode_v),
            fnv1a64_u32_words(&exact_layer0.decode_v_bits),
        );
        println!(
            "attention_out host=0x{:016X} exact=0x{:016X}",
            hash_f32(&attention_out),
            fnv1a64_u32_words(&exact_layer0.attention_out_bits),
        );
        println!(
            "attention_oproj host=0x{:016X} exact=0x{:016X}",
            hash_f32(&attention_oproj),
            hash_opt_bits(exact_layer0.attention_oproj_bits.as_deref()),
        );
        assert_eq!(decode_q.len(), exact_layer0.decode_q_bits.len());
    }

    #[test]
    fn lazy_plan_formats_gemma4_user_turn_prompt() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let tokenizer_config = &runtime.weights.snapshot.tokenizer_config;
        let plan = lazy_text_plan(
            default_model_path(),
            "say hi",
            GemmaTextGenerationOptions {
                max_new_tokens: 4,
                prompt_format: GemmaPromptFormat::Gemma4UserTurn,
            },
        );
        let formatted = plan.eval_formatted_prompt_text().unwrap();
        assert!(formatted.starts_with(&format!(
            "{}{}user\n",
            tokenizer_config.bos_token, tokenizer_config.sot_token
        )));
        assert!(formatted.contains("say hi"));
        assert!(formatted.contains(&format!("{}model\n", tokenizer_config.sot_token)));
        assert!(formatted.ends_with(&format!(
            "{}thought\n{}",
            tokenizer_config.soc_token, tokenizer_config.eoc_token
        )));
    }

    #[test]
    #[ignore]
    fn lazy_plan_materializes_generation_prefixes_incrementally() {
        let plan = lazy_text_plan(
            default_model_path(),
            "say hi",
            GemmaTextGenerationOptions {
                max_new_tokens: 4,
                prompt_format: GemmaPromptFormat::Gemma4UserTurn,
            },
        );
        let prefix1 = plan.eval_generated_token_ids_up_to(1).unwrap();
        let prefix2 = plan.eval_generated_token_ids_up_to(2).unwrap();
        let output = plan.eval_generate().unwrap();

        assert!(prefix1.len() <= 1);
        assert!(prefix2.len() <= 2);
        assert_eq!(&*prefix1, &prefix2[..prefix1.len()]);
        assert_eq!(&*prefix2, &output.generated_token_ids[..prefix2.len()]);
    }

    #[test]
    #[ignore]
    fn exact_cursor_matches_two_token_exact_path() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let exact = run_two_token_ids(default_model_path(), [30_468, 5_631], 0, 1).unwrap();
        let mut cursor = runtime
            .start_generation_cursor(
                Arc::<[u32]>::from(exact.prompt_token_ids.clone()),
                Some(1),
            )
            .unwrap();
        cursor.ensure_generated(1).unwrap();
        assert_eq!(cursor.generated_token_ids(), [exact.next_token.token_id]);
    }

    #[test]
    #[ignore]
    fn generation_cursor_defers_prompt_prefill_until_demanded() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let prompt_token_ids = Arc::<[u32]>::from(vec![30_468, 5_631]);
        let mut cursor = runtime
            .start_generation_cursor(prompt_token_ids.clone(), Some(1))
            .unwrap();
        assert_eq!(cursor.processed_prompt_tokens(), 0);
        assert_eq!(cursor.position(), 0);
        assert!(!cursor.has_pending_next());

        cursor.ensure_generated(0).unwrap();
        assert_eq!(cursor.processed_prompt_tokens(), 0);
        assert_eq!(cursor.position(), 0);
        assert!(!cursor.has_pending_next());

        cursor.ensure_generated(1).unwrap();
        assert_eq!(cursor.processed_prompt_tokens(), prompt_token_ids.len());
        assert_eq!(cursor.generated_token_ids(), [11]);
    }

    #[test]
    #[ignore]
    fn generation_graph_materializes_prefix_nodes_incrementally() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let prompt_token_ids = Arc::<[u32]>::from(vec![30_468, 5_631]);
        let graph = runtime
            .start_generation_graph(prompt_token_ids.clone(), Some(2))
            .unwrap();

        let prefix1 = graph.generated_token_ids_up_to(1).unwrap();
        let prefix2 = graph.generated_token_ids_up_to(2).unwrap();
        let final_snapshot = graph.finish_snapshot().unwrap();

        assert!(prefix1.len() <= 1);
        assert!(prefix2.len() <= 2);
        assert_eq!(&*prefix1, &prefix2[..prefix1.len()]);
        assert_eq!(&*prefix2, final_snapshot.generated_token_ids.as_ref());
        assert_eq!(
            final_snapshot.processed_prompt_tokens,
            prompt_token_ids.len()
        );
        assert!(final_snapshot.position >= prompt_token_ids.len());
        assert!(
            final_snapshot.has_pending_next
                || final_snapshot.stop_reason.is_some()
                || !final_snapshot.generated_token_ids.is_empty()
        );
    }

    #[test]
    #[ignore]
    fn exact_backend_head_matches_two_token_exact_hidden() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let exact = run_two_token_ids(default_model_path(), [30_468, 5_631], 0, 1).unwrap();
        let exact_backend = runtime.exact_backend().unwrap();
        let mut backend = exact_backend.lock().unwrap();
        let next = backend
            .greedy_token_from_hidden_words(&exact.final_hidden_bf16_words)
            .unwrap();
        println!(
            "exact_hidden_next_token_id={} exact_hidden_next_token_logit={}",
            next.token_id, next.logit
        );
        assert_eq!(next.token_id, exact.next_token.token_id);
    }

    #[test]
    #[ignore]
    fn exact_backend_device_head_matches_shared_logits_scan() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let exact = run_two_token_ids(default_model_path(), [30_468, 5_631], 0, 1).unwrap();
        let exact_backend = runtime.exact_backend().unwrap();
        let mut backend = exact_backend.lock().unwrap();
        let (device, shared) = backend
            .compare_greedy_token_paths_from_hidden_words(&exact.final_hidden_bf16_words)
            .unwrap();
        println!(
            "device_token_id={} device_logit={} shared_token_id={} shared_logit={}",
            device.token_id, device.logit, shared.token_id, shared.logit
        );
        assert_eq!(device.token_id, shared.token_id);
        assert_eq!(device.logit, shared.logit);
    }

    #[test]
    #[ignore]
    fn lazy_plan_generates_text_end_to_end() {
        let plan = lazy_text_plan(
            default_model_path(),
            "say hi",
            GemmaTextGenerationOptions {
                max_new_tokens: 8,
                prompt_format: GemmaPromptFormat::Gemma4UserTurn,
            },
        );
        let output = plan.eval_generate().unwrap();
        println!(
            "prompt_ids={:?} generated_ids={:?} generated_text={:?} stop_reason={:?}",
            output.prompt_token_ids,
            output.generated_token_ids,
            output.generated_text,
            output.stop_reason,
        );
        assert!(!output.prompt_token_ids.is_empty());
        assert!(matches!(
            output.stop_reason,
            GemmaStopReason::MaxNewTokens | GemmaStopReason::EosToken(_)
        ));
    }

    #[test]
    #[ignore]
    fn formatted_say_hi_greedy_prefix_matches_local_mlx() {
        let output = lazy_text_plan(
            default_model_path(),
            "say hi",
            GemmaTextGenerationOptions {
                max_new_tokens: 8,
                prompt_format: GemmaPromptFormat::Gemma4UserTurn,
            },
        )
        .eval_generate()
        .unwrap();

        let expected_prompt_token_ids =
            [2, 105, 2364, 107, 30_468, 5_631, 106, 107, 105, 4_368, 107];
        let expected_generated_token_ids = [100, 45_518, 107, 101, 10_979, 236_888, 2_088, 740];

        println!(
            "prompt_ids={:?} generated_ids={:?} generated_text={:?} stop_reason={:?}",
            output.prompt_token_ids,
            output.generated_token_ids,
            output.generated_text,
            output.stop_reason,
        );

        assert_eq!(output.prompt_token_ids.as_ref(), expected_prompt_token_ids);
        assert_eq!(
            output.generated_token_ids.as_ref(),
            expected_generated_token_ids
        );
        assert_eq!(output.stop_reason, GemmaStopReason::MaxNewTokens);
    }

    #[test]
    #[ignore]
    fn poem_prompt_32_token_prefix_matches_local_mlx() {
        let output = lazy_text_plan(
            default_model_path(),
            "Write a poem in exactly 10 lines about unified memory, lazy evaluation, and metal at midnight. Keep each line concise.",
            GemmaTextGenerationOptions {
                max_new_tokens: 32,
                prompt_format: GemmaPromptFormat::Gemma4UserTurn,
            },
        )
        .eval_generate()
        .unwrap();

        let expected_prompt_token_ids = [
            2, 105, 2364, 107, 6974, 496, 27355, 528, 7121, 236743, 236770, 236771, 4463, 1003,
            44623, 6571, 236764, 31770, 12207, 236764, 532, 6211, 657, 38735, 236761, 17608, 1546,
            1757, 63510, 236761, 106, 107, 105, 4368, 107,
        ];
        let expected_generated_token_ids = [
            4, 2, 2, 4, 2, 4, 4, 4, 2, 13, 36, 40, 20, 2, 4, 2, 13, 2, 9, 17, 16, 20, 20, 34, 2, 2,
            11, 2, 5, 2, 9, 2,
        ];

        println!(
            "prompt_ids={:?} generated_ids={:?} generated_text={:?} stop_reason={:?}",
            output.prompt_token_ids,
            output.generated_token_ids,
            output.generated_text,
            output.stop_reason,
        );

        assert_eq!(output.prompt_token_ids.as_ref(), expected_prompt_token_ids);
        assert_eq!(
            output.generated_token_ids.as_ref(),
            expected_generated_token_ids
        );
        assert_eq!(output.stop_reason, GemmaStopReason::MaxNewTokens);
    }

    #[test]
    #[ignore]
    fn formatted_say_hi_exact_backend_hash_manifest() {
        let runtime = Arc::unwrap_or_clone(GemmaTextRuntimeSession::load(&default_model_path()).unwrap());
        let formatted_prompt = runtime.format_prompt_text("say hi", GemmaPromptFormat::Gemma4UserTurn);
        let prompt_token_ids = runtime.tokenize_prompt(&formatted_prompt).unwrap();
        let num_layers = runtime.weights.snapshot.config.text_config.num_hidden_layers as usize;
        let mut token_hidden_words = prompt_token_ids
            .iter()
            .map(|&token_id| runtime.weights.embed_token_bf16_words(token_id).unwrap())
            .collect::<Vec<_>>();
        let exact_backend = runtime.exact_backend().unwrap();
        let mut backend = exact_backend.lock().unwrap();

        for layer_idx in 0..num_layers {
            backend.reset_kv_caches();
            let mut next_token_hidden_words = Vec::with_capacity(token_hidden_words.len());
            for (position, input_words) in token_hidden_words.iter().enumerate() {
                let hidden_words = backend
                    .eval_layer_hidden_state(layer_idx, input_words, position)
                    .unwrap();
                let hidden_bits = hidden_words
                    .iter()
                    .copied()
                    .map(bf16_word_to_f32)
                    .map(f32::to_bits)
                    .collect::<Vec<_>>();
                println!(
                    "token_position={} layer_idx={} hidden_fnv1a64=0x{:016X}",
                    position,
                    layer_idx,
                    fnv1a64_u32_words(&hidden_bits)
                );
                next_token_hidden_words.push(hidden_words);
            }
            token_hidden_words = next_token_hidden_words;
        }
    }

    #[test]
    #[ignore]
    fn formatted_say_hi_exact_prefill_final_hidden() {
        let runtime = Arc::unwrap_or_clone(GemmaTextRuntimeSession::load(&default_model_path()).unwrap());
        let formatted_prompt = runtime.format_prompt_text("say hi", GemmaPromptFormat::Gemma4UserTurn);
        let prompt_token_ids = runtime.tokenize_prompt(&formatted_prompt).unwrap();
        let exact_backend = runtime.exact_backend().unwrap();
        let mut backend = exact_backend.lock().unwrap();
        let final_hidden_words = backend
            .prefill_prompt_hidden_words_from_token_ids(prompt_token_ids.as_ref(), 0)
            .unwrap();
        let final_hidden_bits = final_hidden_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .map(f32::to_bits)
            .collect::<Vec<_>>();
        let next_token = backend.greedy_token_from_hidden_words(&final_hidden_words).unwrap();
        println!("prompt_ids={:?}", prompt_token_ids);
        println!(
            "final_hidden_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&final_hidden_bits)
        );
        println!(
            "top1_token_id={} top1_logit={}",
            next_token.token_id, next_token.logit
        );
    }
