    use super::{
        bf16_word_to_f32, fnv1a64_u32_words, gemma4_qproj_case_input_bf16_words, MlxDType,
        MlxIndexedSafetensors, MlxModelSnapshot, MlxSafetensorsHeader, MlxTokenizer,
        GEMMA4_QPROJ_CASE_INNER_DIM, GEMMA4_QPROJ_CASE_OUTPUT_DIM,
        GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64,
    };
    use std::path::PathBuf;

    fn local_model_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/models/gemma-4-26b-mlx")
    }

    fn local_model_shard_1() -> PathBuf {
        local_model_dir().join("model-00001-of-00003.safetensors")
    }

    #[test]
    fn loads_local_gemma4_mlx_snapshot() {
        let snapshot = MlxModelSnapshot::load(local_model_dir()).unwrap();

        assert_eq!(
            snapshot.config.architectures,
            vec!["Gemma4ForConditionalGeneration".to_string()]
        );
        assert_eq!(snapshot.config.model_type, "gemma4");
        assert_eq!(snapshot.config.text_config.num_hidden_layers, 30);
        assert_eq!(snapshot.config.vision_config.num_hidden_layers, 27);
        assert_eq!(snapshot.config.quantization.bits, 4);
        assert_eq!(snapshot.config.quantization.group_size, 64);
        assert_eq!(snapshot.config.quantization.mode, "affine");
        assert_eq!(snapshot.processor_config.image_processor.size.height, 224);
        assert_eq!(snapshot.processor_config.image_processor.size.width, 224);
        assert_eq!(
            snapshot.tokenizer_config.model_max_length,
            1000000000000000019884624838656u128
        );
        assert_eq!(snapshot.weight_index.metadata.total_size, 15_335_574_684);
        assert_eq!(snapshot.unique_weight_shards().len(), 3);
        assert_eq!(
            snapshot
                .weight_index
                .weight_map
                .get("language_model.model.layers.0.self_attn.q_proj.weight")
                .map(String::as_str),
            Some("model-00001-of-00003.safetensors")
        );
        assert_eq!(
            snapshot
                .weight_index
                .weight_map
                .get("embed_vision.embedding_projection.weight")
                .map(String::as_str),
            Some("model-00003-of-00003.safetensors")
        );
    }

    #[test]
    fn reads_local_safetensors_header_without_touching_payload() {
        let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();

        assert_eq!(header.header_len, 64_065);
        assert_eq!(
            header.metadata.get("format").map(String::as_str),
            Some("mlx")
        );
        assert_eq!(header.tensors.len(), 488);

        let embed_weight = header
            .tensor("language_model.model.embed_tokens.weight")
            .unwrap();
        assert_eq!(embed_weight.dtype, MlxDType::U32);
        assert_eq!(embed_weight.shape, vec![262_144, 352]);
        assert_eq!(embed_weight.data_offsets, [3_116_339_724, 3_485_438_476]);
        assert_eq!(
            embed_weight.data_len_bytes(),
            embed_weight.expected_len_bytes()
        );

        let q_proj = header
            .tensor("language_model.model.layers.0.self_attn.q_proj.weight")
            .unwrap();
        assert_eq!(q_proj.dtype, MlxDType::U32);
        assert_eq!(q_proj.shape, vec![4_096, 352]);
        assert_eq!(q_proj.data_offsets, [3_612_518_924, 3_618_286_092]);
        assert_eq!(q_proj.data_len_bytes(), q_proj.expected_len_bytes());

        let q_proj_scales = header
            .tensor("language_model.model.layers.0.self_attn.q_proj.scales")
            .unwrap();
        assert_eq!(q_proj_scales.dtype, MlxDType::BF16);
        assert_eq!(q_proj_scales.shape, vec![4_096, 44]);
        assert_eq!(q_proj_scales.data_offsets, [2_536_973_576, 2_537_334_024]);
        assert_eq!(
            q_proj_scales.file_offsets(header.payload_base_offset())[0],
            2_537_037_649
        );
    }

    #[test]
    fn indexed_safetensors_resolves_late_layer_to_correct_shard() {
        let indexed = MlxIndexedSafetensors::load(local_model_dir()).unwrap();

        assert_eq!(
            indexed
                .shard_name_for_tensor("language_model.model.layers.29.self_attn.q_proj.weight")
                .unwrap(),
            "model-00003-of-00003.safetensors"
        );

        let header = indexed
            .header_for_tensor("language_model.model.layers.29.self_attn.q_proj.weight")
            .unwrap();
        assert!(header.path.ends_with("model-00003-of-00003.safetensors"));

        let entry = indexed
            .tensor("language_model.model.layers.29.self_attn.q_proj.weight")
            .unwrap();
        assert_eq!(entry.dtype, MlxDType::U32);
        assert_eq!(entry.shape, vec![8_192, 352]);
    }

    #[test]
    fn loads_local_tokenizer_metadata() {
        let tokenizer = MlxTokenizer::load(local_model_dir()).unwrap();

        assert!(tokenizer.vocab_size() > 260_000);
        assert!(tokenizer.merge_count() > 500_000);
        assert_eq!(tokenizer.token_to_id("<bos>"), Some(2));
        assert_eq!(tokenizer.token_to_id("<eos>"), Some(1));
        assert_eq!(tokenizer.token_to_id("<|video|>"), Some(258_884));
        assert_eq!(tokenizer.token_to_id("say"), Some(30_468));
        assert_eq!(tokenizer.token_to_id("▁hi"), Some(5_631));
        assert_eq!(tokenizer.token_to_id("▁("), Some(568));
        assert_eq!(tokenizer.id_to_token(2), Some("<bos>"));
        assert_eq!(tokenizer.id_to_token(568), Some("▁("));
    }

    #[test]
    fn tokenizer_encodes_and_decodes_simple_phrase() {
        let tokenizer = MlxTokenizer::load(local_model_dir()).unwrap();

        assert_eq!(tokenizer.encode("say hi").unwrap(), vec![30_468, 5_631]);
        assert_eq!(tokenizer.encode(" hi").unwrap(), vec![5_631]);
        assert_eq!(tokenizer.decode(&[30_468, 5_631]).unwrap(), "say hi");
        assert_eq!(tokenizer.decode(&[1_879, 5_631]).unwrap(), " say hi");
    }

    #[test]
    fn embeds_and_norms_local_text_token_rows() {
        let weights = MlxIndexedSafetensors::load(local_model_dir()).unwrap();

        let embed = weights.embed_token_bf16_words(30_468).unwrap();
        assert_eq!(embed.len(), 2_816);

        let final_norm = weights.final_text_norm_bf16_words(&embed).unwrap();
        assert_eq!(final_norm.len(), 2_816);
    }

    #[test]
    #[ignore]
    fn embed_rows_report_hashes_for_two_token_prompt() {
        let weights = MlxIndexedSafetensors::load(local_model_dir()).unwrap();
        for token_id in [30_468u32, 5_631u32] {
            let bits = weights
                .embed_token_bf16_words(token_id)
                .unwrap()
                .into_iter()
                .map(|word| (word as u32) << 16)
                .collect::<Vec<_>>();
            println!(
                "token_id={} embed_fnv1a64=0x{:016X}",
                token_id,
                fnv1a64_u32_words(&bits)
            );
            println!(
                "token_id={} embed_first16_f32_bits={}",
                token_id,
                bits.iter()
                    .take(16)
                    .map(|bits| format!("0x{bits:08X}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    }

    #[test]
    fn reads_local_tensor_payload_words() {
        let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();

        let q_proj_weight = header
            .read_u32_tensor_words("language_model.model.layers.0.self_attn.q_proj.weight")
            .unwrap();
        assert_eq!(
            &q_proj_weight[..8],
            &[
                2_259_126_473,
                1_283_001_501,
                2_291_701_430,
                1_970_953_151,
                1_283_929_482,
                2_027_333_543,
                934_918_473,
                3_033_893_010,
            ]
        );

        let q_proj_scales = header
            .read_bf16_tensor_words("language_model.model.layers.0.self_attn.q_proj.scales")
            .unwrap();
        assert_eq!(
            &q_proj_scales[..16],
            &[
                15_321, 48_110, 48_135, 48_112, 15_290, 48_057, 15_308, 15_307, 15_254, 15_260,
                15_397, 15_275, 15_300, 15_297, 48_099, 15_299,
            ]
        );
    }

    #[test]
    fn dequantizes_one_local_q_proj_row_matches_mlx_oracle() {
        let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();
        let row = header
            .affine_dequantize_row_f32(
                "language_model.model.layers.0.self_attn.q_proj.weight",
                "language_model.model.layers.0.self_attn.q_proj.scales",
                "language_model.model.layers.0.self_attn.q_proj.biases",
                0,
                64,
                4,
            )
            .unwrap();
        assert_eq!(row.len(), 2_816);
        assert_eq!(
            fnv1a64_u32_words(&row.iter().map(|value| value.to_bits()).collect::<Vec<_>>()),
            0x2D44_4223_7EE7_C10F
        );
        assert_eq!(
            &row[..16]
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            &[
                0x3BD9_0000,
                0x3CD9_0000,
                0x0000_0000,
                0x0000_0000,
                0xBBD9_0000,
                0x3C59_0000,
                0xBC59_0000,
                0x0000_0000,
                0x3D08_0000,
                0x3BD9_0000,
                0x3CD9_0000,
                0xBD59_0000,
                0x3BD9_0000,
                0xBBD9_0000,
                0x3CD9_0000,
                0xBCD9_0000,
            ]
        );
    }

    #[test]
    fn quantized_matmul_one_local_q_proj_case_matches_mlx_oracle() {
        let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();
        let x = gemma4_qproj_case_input_bf16_words(GEMMA4_QPROJ_CASE_INNER_DIM);
        assert_eq!(
            x[..16]
                .iter()
                .copied()
                .map(bf16_word_to_f32)
                .map(f32::to_bits)
                .collect::<Vec<_>>(),
            vec![
                0xBF80_0000,
                0xBF40_0000,
                0xBF00_0000,
                0xBE80_0000,
                0x0000_0000,
                0x3E80_0000,
                0x3F00_0000,
                0x3F40_0000,
                0x3F80_0000,
                0x3F00_0000,
                0x0000_0000,
                0xBF00_0000,
                0xBF80_0000,
                0x3E00_0000,
                0x3EC0_0000,
                0x3F20_0000,
            ]
        );

        let out = header
            .affine_quantized_matmul_t_f32(
                &x,
                "language_model.model.layers.0.self_attn.q_proj.weight",
                "language_model.model.layers.0.self_attn.q_proj.scales",
                "language_model.model.layers.0.self_attn.q_proj.biases",
                64,
                4,
            )
            .unwrap();
        assert_eq!(out.len(), GEMMA4_QPROJ_CASE_OUTPUT_DIM);
        let out_bits = out.iter().map(|value| value.to_bits()).collect::<Vec<_>>();
        assert_eq!(
            &out_bits[..16],
            &[
                0xBF59_0000,
                0x4029_0000,
                0x3F3B_0000,
                0xBF63_0000,
                0x3DAF_0000,
                0xBF51_0000,
                0xBF49_0000,
                0x3FCE_0000,
                0x3D8B_0000,
                0xBEB9_0000,
                0x3F0F_0000,
                0x3E8E_0000,
                0x3DF2_0000,
                0x3E80_0000,
                0x3E89_0000,
                0xC022_0000,
            ]
        );
        assert_eq!(
            fnv1a64_u32_words(&out_bits),
            GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64
        );
    }

    #[test]
    fn rms_norm_one_local_layer0_input_case_matches_mlx_gpu_oracle() {
        let header = MlxSafetensorsHeader::load(local_model_shard_1()).unwrap();
        let x = gemma4_qproj_case_input_bf16_words(2_816);
        let out = header
            .rms_norm_weighted_f32(
                &x,
                "language_model.model.layers.0.input_layernorm.weight",
                1e-6,
            )
            .unwrap();
        assert_eq!(out.len(), 2_816);
        let out_bits = out.iter().map(|value| value.to_bits()).collect::<Vec<_>>();
        assert_eq!(
            &out_bits[..16],
            &[
                0xC0A2_0000,
                0xC080_0000,
                0xC033_0000,
                0xBFBC_0000,
                0x0000_0000,
                0x3FB6_0000,
                0x402F_0000,
                0x40E3_0000,
                0x4126_0000,
                0x4041_0000,
                0x0000_0000,
                0xC048_0000,
                0xC11C_0000,
                0x3F6C_0000,
                0x4081_0000,
                0x40A4_0000,
            ]
        );
        assert_eq!(fnv1a64_u32_words(&out_bits), 0xBF5E_A05B_53DF_E923);
    }
