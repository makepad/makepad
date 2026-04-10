    use super::*;
    use crate::{MlxRopeAttentionParameters, MlxTextRopeParameters};

    #[test]
    fn layout_reuses_last_concrete_cache_of_the_same_attention_kind() -> Result<()> {
        let config = sample_text_config(
            &[
                "full_attention",
                "sliding_attention",
                "full_attention",
                "sliding_attention",
                "full_attention",
                "sliding_attention",
            ],
            2,
        );
        let layout = GemmaKvCacheLayout::from_text_config(&config, 1)?;

        assert_eq!(layout.first_kv_shared_layer_idx, 4);
        assert_eq!(layout.layer_idx_to_cache_idx, vec![0, 1, 2, 3, 2, 3]);
        assert_eq!(layout.first_full_cache_idx, Some(0));
        assert_eq!(layout.first_sliding_cache_idx, Some(1));
        assert_eq!(layout.cache_specs.len(), 4);
        assert_eq!(layout.cache_specs[0].attention, GemmaAttentionKind::Full);
        assert_eq!(layout.cache_specs[1].attention, GemmaAttentionKind::Sliding);
        assert!(layout.is_kv_shared_layer(4)?);
        assert!(!layout.is_kv_shared_layer(3)?);
        Ok(())
    }

    #[test]
    fn layout_uses_global_kv_shape_for_full_attention_layers() -> Result<()> {
        let mut config = sample_text_config(
            &["sliding_attention", "full_attention", "sliding_attention"],
            0,
        );
        config.attention_k_eq_v = true;
        config.head_dim = 4;
        config.global_head_dim = 8;
        config.num_key_value_heads = 2;
        config.num_global_key_value_heads = 1;
        config.max_position_embeddings = 32;
        config.sliding_window = 6;

        let layout = GemmaKvCacheLayout::from_text_config(&config, 1)?;

        assert_eq!(layout.cache_specs.len(), 3);
        assert_eq!(
            layout.cache_specs[0],
            GemmaKvCacheSpec::new(GemmaAttentionKind::Sliding, 1, 2, 4, 6)?
        );
        assert_eq!(
            layout.cache_specs[1],
            GemmaKvCacheSpec::new(GemmaAttentionKind::Full, 1, 1, 8, 32)?
        );
        assert_eq!(
            layout.cache_specs[2],
            GemmaKvCacheSpec::new(GemmaAttentionKind::Sliding, 1, 2, 4, 6)?
        );
        assert_eq!(layout.cache_spec_for_layer(1)?, &layout.cache_specs[1]);
        Ok(())
    }

    #[test]
    fn full_cache_prefill_then_decode_preserves_head_layout() -> Result<()> {
        let spec = GemmaKvCacheSpec::new(GemmaAttentionKind::Full, 1, 2, 3, 8)?;
        let mut cache = GemmaKvCache::<i32>::new(spec)?;

        let prefill_keys = test_tensor(
            KvTensorShape {
                batch_size: 1,
                kv_head_count: 2,
                seq_len: 2,
                head_dim: 3,
            },
            0,
        )?;
        let prefill_values = test_tensor(prefill_keys.shape(), 1000)?;
        cache.update_and_fetch(prefill_keys.view(), prefill_values.view())?;

        let decode_keys = test_tensor(
            KvTensorShape {
                batch_size: 1,
                kv_head_count: 2,
                seq_len: 1,
                head_dim: 3,
            },
            200,
        )?;
        let decode_values = test_tensor(decode_keys.shape(), 1200)?;
        let state = cache.update_and_fetch(decode_keys.view(), decode_values.view())?;

        assert_eq!(state.stored_tokens(), 3);
        assert_eq!(state.start_position(), 0);
        assert_eq!(state.offset(), 3);
        assert_eq!(state.keys.row(0, 0, 0)?, &[0, 1, 2]);
        assert_eq!(state.keys.row(0, 0, 1)?, &[10, 11, 12]);
        assert_eq!(state.keys.row(0, 0, 2)?, &[200, 201, 202]);
        assert_eq!(state.keys.row(0, 1, 0)?, &[100, 101, 102]);
        assert_eq!(state.keys.row(0, 1, 2)?, &[300, 301, 302]);
        assert_eq!(state.values.row(0, 1, 2)?, &[1300, 1301, 1302]);
        Ok(())
    }

    #[test]
    fn sliding_cache_keeps_only_the_last_window_and_tracks_absolute_positions() -> Result<()> {
        let spec = GemmaKvCacheSpec::new(GemmaAttentionKind::Sliding, 1, 1, 2, 3)?;
        let mut cache = GemmaKvCache::<i32>::new(spec)?;

        let first = test_tensor(
            KvTensorShape {
                batch_size: 1,
                kv_head_count: 1,
                seq_len: 2,
                head_dim: 2,
            },
            0,
        )?;
        cache.update_and_fetch(first.view(), first.view())?;

        let second = test_tensor(
            KvTensorShape {
                batch_size: 1,
                kv_head_count: 1,
                seq_len: 2,
                head_dim: 2,
            },
            100,
        )?;
        let state = cache.update_and_fetch(second.view(), second.view())?;

        assert_eq!(state.stored_tokens(), 3);
        assert_eq!(state.start_position(), 1);
        assert_eq!(state.offset(), 4);
        assert_eq!(state.keys.row(0, 0, 0)?, &[10, 11]);
        assert_eq!(state.keys.row(0, 0, 1)?, &[100, 101]);
        assert_eq!(state.keys.row(0, 0, 2)?, &[110, 111]);
        Ok(())
    }

    #[test]
    fn decode_step_reads_match_expected_batch_head_layout() -> Result<()> {
        let spec = GemmaKvCacheSpec::new(GemmaAttentionKind::Full, 2, 2, 2, 4)?;
        let mut cache = GemmaKvCache::<i32>::new(spec)?;

        let decode = test_tensor(
            KvTensorShape {
                batch_size: 2,
                kv_head_count: 2,
                seq_len: 1,
                head_dim: 2,
            },
            500,
        )?;
        let state = cache.update_and_fetch(decode.view(), decode.view())?;

        assert_eq!(state.keys.row(0, 0, 0)?, &[500, 501]);
        assert_eq!(state.keys.row(0, 1, 0)?, &[600, 601]);
        assert_eq!(state.keys.row(1, 0, 0)?, &[1500, 1501]);
        assert_eq!(state.keys.row(1, 1, 0)?, &[1600, 1601]);
        Ok(())
    }

    fn sample_text_config(layer_types: &[&str], num_kv_shared_layers: u32) -> MlxTextConfig {
        MlxTextConfig {
            attention_bias: false,
            attention_dropout: 0.0,
            attention_k_eq_v: false,
            bos_token_id: 2,
            dtype: "bfloat16".to_owned(),
            enable_moe_block: true,
            eos_token_id: 1,
            final_logit_softcapping: 0.0,
            global_head_dim: 4,
            head_dim: 4,
            hidden_activation: "gelu_pytorch_tanh".to_owned(),
            hidden_size: 16,
            hidden_size_per_layer_input: 0,
            initializer_range: 0.02,
            intermediate_size: 32,
            layer_types: layer_types.iter().map(|item| (*item).to_owned()).collect(),
            max_position_embeddings: 16,
            model_type: "gemma4".to_owned(),
            moe_intermediate_size: 32,
            num_attention_heads: 4,
            num_experts: 8,
            num_global_key_value_heads: 1,
            num_hidden_layers: layer_types.len() as u32,
            num_key_value_heads: 2,
            num_kv_shared_layers,
            pad_token_id: 0,
            rms_norm_eps: 1e-6,
            rope_parameters: MlxTextRopeParameters {
                full_attention: MlxRopeAttentionParameters {
                    partial_rotary_factor: None,
                    rope_theta: 10_000.0,
                    rope_type: "default".to_owned(),
                },
                sliding_attention: MlxRopeAttentionParameters {
                    partial_rotary_factor: None,
                    rope_theta: 10_000.0,
                    rope_type: "default".to_owned(),
                },
            },
            sliding_window: 4,
            tie_word_embeddings: true,
            top_k_experts: 2,
            use_bidirectional_attention: "never".to_owned(),
            use_cache: true,
            use_double_wide_mlp: false,
            vocab_size: 256,
            vocab_size_per_layer_input: 0,
        }
    }

    fn test_tensor(shape: KvTensorShape, base: i32) -> Result<KvTensor<i32>> {
        let mut data = Vec::with_capacity(shape.element_count()?);
        for batch in 0..shape.batch_size {
            for head in 0..shape.kv_head_count {
                for token in 0..shape.seq_len {
                    for dim in 0..shape.head_dim {
                        data.push(
                            base + (batch as i32) * 1000
                                + (head as i32) * 100
                                + (token as i32) * 10
                                + dim as i32,
                        );
                    }
                }
            }
        }
        KvTensor::from_vec(shape, data)
    }
