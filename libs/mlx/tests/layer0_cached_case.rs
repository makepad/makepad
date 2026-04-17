use super::{
    bf16_round_to_f32, bf16_word_to_f32, bf16_words_from_f32_bits, bytes_from_bf16_words,
    compile_default_pipeline, default_model_path, exact_qproj_layout, print_cached_artifacts,
    read_bf16_buffer_bits, read_exact_kv_cache_tensor_bits, read_f32_file_as_bf16_words,
    run_layer_plan, run_layer_plan_from_sequence, run_layer_plan_with_session,
    run_layer_plan_with_session_from_sequence, run_layer_sequence, run_layer_sequence_from_inputs,
    write_bf16_words_as_f32_file, CachedLayerInputs, CachedLayerSequenceInputs,
    ExactMetalQprojLayout, ExactMetalTextRuntimeSession, Layer0CachedArtifacts, Layer0CachedPlan,
    Layer0CachedStage, LayerExecutionSession, LayerTensorNames, MlxAffineQprojRowArgs,
    MlxIndexedSafetensors, DECODE_ROPE_OFFSET, NORM_LEN, PREFILL_ROPE_OFFSET,
};
use crate::fnv1a64_u32_words;
use makepad_ggml::backend::metal::{BufferStorageMode, MetalBufferBindingRef, MetalSize};
use std::env::temp_dir;
use std::path::PathBuf;
use std::{fs, io::Write};

fn u4_group_terms_row_totals(
    x_bf16_words: &[u16],
    weights: &[u32],
    scales: &[u16],
    biases: &[u16],
    row: usize,
    weight_stride_words: usize,
    qparams_per_row: usize,
) -> (u32, u32) {
    const VALUES_PER_THREAD: usize = 8;
    const BLOCK_SIZE: usize = VALUES_PER_THREAD * 32;
    const GROUPS_PER_BLOCK: usize = BLOCK_SIZE / 64;

    let row_weight_base = row * weight_stride_words;
    let row_qparam_base = row * qparams_per_row;
    let mut total_plain = 0.0f32;
    let mut total_groupbf16 = 0.0f32;

    for block in 0..(x_bf16_words.len() / BLOCK_SIZE) {
        let block_weight_base = block * (BLOCK_SIZE / 8);
        let block_x_base = block * BLOCK_SIZE;
        for group in 0..GROUPS_PER_BLOCK {
            let scale =
                bf16_word_to_f32(scales[row_qparam_base + block * GROUPS_PER_BLOCK + group]);
            let bias = bf16_word_to_f32(biases[row_qparam_base + block * GROUPS_PER_BLOCK + group]);
            let mut group_sum = 0.0f32;
            let mut group_accum = 0.0f32;

            for lane in 0..8 {
                let lane_in_block = group * 8 + lane;
                let lane_x_base = block_x_base + lane_in_block * VALUES_PER_THREAD;
                let w = weights[row_weight_base + block_weight_base + lane_in_block];

                let mut x_thread = [0.0f32; VALUES_PER_THREAD];
                let mut lane_sum = 0.0f32;
                for i in (0..VALUES_PER_THREAD).step_by(4) {
                    let x0 = bf16_word_to_f32(x_bf16_words[lane_x_base + i]);
                    let x1 = bf16_word_to_f32(x_bf16_words[lane_x_base + i + 1]);
                    let x2 = bf16_word_to_f32(x_bf16_words[lane_x_base + i + 2]);
                    let x3 = bf16_word_to_f32(x_bf16_words[lane_x_base + i + 3]);
                    lane_sum += x0 + x1 + x2 + x3;
                    x_thread[i] = x0;
                    x_thread[i + 1] = x1 / 16.0;
                    x_thread[i + 2] = x2 / 256.0;
                    x_thread[i + 3] = x3 / 4096.0;
                }

                let ws0 = (w & 0xFFFF) as u16;
                let ws1 = (w >> 16) as u16;
                let lane_accum = x_thread[0] * ((ws0 & 0x000F) as f32)
                    + x_thread[1] * ((ws0 & 0x00F0) as f32)
                    + x_thread[2] * ((ws0 & 0x0F00) as f32)
                    + x_thread[3] * ((ws0 & 0xF000) as f32)
                    + x_thread[4] * ((ws1 & 0x000F) as f32)
                    + x_thread[5] * ((ws1 & 0x00F0) as f32)
                    + x_thread[6] * ((ws1 & 0x0F00) as f32)
                    + x_thread[7] * ((ws1 & 0xF000) as f32);

                group_sum += lane_sum;
                group_accum += lane_accum;
            }

            total_plain += scale * group_accum + bias * group_sum;
            total_groupbf16 +=
                bf16_round_to_f32(scale * group_accum) + bf16_round_to_f32(bias * group_sum);
        }
    }

    (
        bf16_round_to_f32(total_plain).to_bits(),
        bf16_round_to_f32(total_groupbf16).to_bits(),
    )
}

fn empty_artifacts() -> Layer0CachedArtifacts {
    Layer0CachedArtifacts {
        backend_name: "test".to_string(),
        model_path: PathBuf::from("test.safetensors"),
        layer_idx: 0,
        selected_stage: None,
        prefill_rope_offset: PREFILL_ROPE_OFFSET,
        decode_rope_offset: DECODE_ROPE_OFFSET,
        q_head_count: 0,
        k_head_count: 0,
        v_head_count: 0,
        q_heads_per_kv: 0,
        head_dim: 0,
        prefill_input_norm_bits: Vec::new(),
        prefill_v_proj_bits: Vec::new(),
        prefill_q_bits: Vec::new(),
        prefill_k_bits: Vec::new(),
        prefill_v_bits: Vec::new(),
        decode_input_norm_bits: Vec::new(),
        decode_v_proj_bits: Vec::new(),
        decode_q_bits: Vec::new(),
        decode_k_bits: Vec::new(),
        decode_v_bits: Vec::new(),
        full_k_bits: Vec::new(),
        full_v_bits: Vec::new(),
        attention_score_bits: Vec::new(),
        attention_prob_bits: Vec::new(),
        attention_out_bits: Vec::new(),
        attention_oproj_bits: None,
        post_attention_norm_bits: None,
        post_attention_residual_bits: None,
        pre_feedforward_norm_bits: None,
        dense_gate_bits: None,
        dense_up_bits: None,
        dense_geglu_bits: None,
        dense_down_bits: None,
        router_output: None,
        moe_expert_gate_bits: None,
        moe_expert_up_bits: None,
        moe_expert_geglu_bits: None,
        moe_expert_down_bits: None,
        post_ffn_norm1_bits: None,
        moe_expert_out_bits: None,
        moe_post_ffn_norm2_bits: None,
        moe_merge_bits: None,
        prefill_post_ffn_residual_bits: None,
        post_ffn_residual_bits: None,
    }
}

fn device_qproj_row_bits(
    session: &mut LayerExecutionSession,
    input_words: &[u16],
    layout: ExactMetalQprojLayout,
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    let runtime = session.runtime.clone();
    let x_buf = runtime.create_buffer_with_bytes(
        &bytes_from_bf16_words(input_words),
        BufferStorageMode::Private,
    )?;
    let out_buf = runtime.create_buffer(layout.out_len() * 2, BufferStorageMode::Private)?;
    let pipeline = compile_default_pipeline(&runtime, "kernel_mlx_affine_qproj_row_bf16")?;
    let args = MlxAffineQprojRowArgs {
        n_in: u32::try_from(input_words.len())?,
        weight_words_per_row: layout.weight_words_per_row,
        qparams_per_row: layout.qparams_per_row,
        out_rows: layout.out_rows,
    };
    let bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &x_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &session.private_weight_buffer(weight_name)?,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &session.private_weight_buffer(scales_name)?,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &session.private_weight_buffer(biases_name)?,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &out_buf,
            offset_bytes: 0,
        },
    ];
    let threads_per_threadgroup = MetalSize {
        width: 256,
        height: 1,
        depth: 1,
    };
    let threadgroups = MetalSize {
        width: (layout.out_len() as u64).div_ceil(threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };
    runtime.begin_command_batch()?;
    runtime.dispatch_compute(
        &pipeline,
        super::bytes_of(&args),
        &bindings,
        &[],
        threadgroups,
        threads_per_threadgroup,
    )?;
    runtime.end_command_batch()?;
    runtime.wait_idle()?;
    read_bf16_buffer_bits(&runtime, &out_buf, layout.out_len())
}

fn write_f32_bits_file(path: &PathBuf, bits: &[u32]) {
    let mut bytes = Vec::with_capacity(bits.len() * size_of::<u32>());
    for word in bits {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

#[derive(Debug)]
struct FormattedSayHiLayerOutputs {
    layer_idx: usize,
    step1_prefill_hash: u64,
    step1_decode_hash: u64,
    step2_decode_hash: u64,
    step1_prefill_f32_path: PathBuf,
    step1_decode_f32_path: PathBuf,
    step2_decode_f32_path: PathBuf,
}

fn post_ffn_only_plan() -> Layer0CachedPlan {
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::PostFfnResidual);
    plan
}

fn write_formatted_say_hi_outputs_through_layer(
    last_layer_idx: usize,
) -> Vec<FormattedSayHiLayerOutputs> {
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let mut token2_words = weights.embed_token_bf16_words(2).unwrap();
    let mut token105_words = weights.embed_token_bf16_words(105).unwrap();
    let mut token2364_words = weights.embed_token_bf16_words(2_364).unwrap();
    let tmp_dir = temp_dir();
    let mut outputs = Vec::with_capacity(last_layer_idx + 1);

    for layer_idx in 0..=last_layer_idx {
        let step1 = run_layer_plan_with_session(
            &mut session,
            layer_idx,
            CachedLayerInputs {
                prefill_input_words: token2_words.clone(),
                decode_input_words: token105_words.clone(),
                prefill_rope_offset: 0,
                decode_rope_offset: 1,
                validate_against_oracle: false,
            },
            post_ffn_only_plan(),
        )
        .unwrap();
        let step2 = run_layer_plan_with_session_from_sequence(
            &mut session,
            layer_idx,
            CachedLayerSequenceInputs {
                prefill_input_words_list: vec![token2_words.clone(), token105_words.clone()],
                decode_input_words: token2364_words.clone(),
                prefill_rope_offset: 0,
                decode_rope_offset: 2,
                validate_against_oracle: false,
            },
            post_ffn_only_plan(),
        )
        .unwrap();

        let step1_prefill_words = step1
            .prefill_layer_output_bf16_words()
            .expect("missing step1 prefill output");
        let step1_decode_words = step1
            .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
            .expect("missing step1 decode output");
        let step2_decode_words = step2
            .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
            .expect("missing step2 decode output");

        let step1_prefill_f32_path =
            tmp_dir.join(format!("gemma_say_hi_layer{layer_idx}_prefill_2_f32.bin"));
        let step1_decode_f32_path =
            tmp_dir.join(format!("gemma_say_hi_layer{layer_idx}_decode_105_f32.bin"));
        let step2_decode_f32_path =
            tmp_dir.join(format!("gemma_say_hi_layer{layer_idx}_decode_2364_f32.bin"));
        write_bf16_words_as_f32_file(&step1_prefill_f32_path, &step1_prefill_words).unwrap();
        write_bf16_words_as_f32_file(&step1_decode_f32_path, &step1_decode_words).unwrap();
        write_bf16_words_as_f32_file(&step2_decode_f32_path, &step2_decode_words).unwrap();

        let entry = FormattedSayHiLayerOutputs {
            layer_idx,
            step1_prefill_hash: fnv1a64_u32_words(
                step1
                    .prefill_layer_output_bits()
                    .expect("missing step1 prefill bits"),
            ),
            step1_decode_hash: fnv1a64_u32_words(
                step1
                    .layer_output_bits()
                    .expect("missing step1 decode bits"),
            ),
            step2_decode_hash: fnv1a64_u32_words(
                step2
                    .layer_output_bits()
                    .expect("missing step2 decode bits"),
            ),
            step1_prefill_f32_path,
            step1_decode_f32_path,
            step2_decode_f32_path,
        };
        outputs.push(entry);

        token2_words = step1_prefill_words;
        token105_words = step1_decode_words;
        token2364_words = step2_decode_words;
    }

    outputs
}

fn rms_norm_unweighted_rows_f32(x: &[f32], head_dim: usize, eps: f32) -> Vec<f32> {
    assert!(head_dim != 0);
    assert_eq!(x.len() % head_dim, 0);
    let mut out = Vec::with_capacity(x.len());
    for row in x.chunks_exact(head_dim) {
        let mut mean_square = 0.0f32;
        for value in row {
            mean_square += value * value;
        }
        mean_square /= head_dim as f32;
        let inv_rms = 1.0f32 / (mean_square + eps).sqrt();
        for value in row {
            out.push(bf16_round_to_f32(*value * inv_rms));
        }
    }
    out
}

fn formatted_say_hi_prompt_token_ids() -> Vec<u32> {
    vec![2, 105, 2364, 107, 30_468, 5_631, 106, 107, 105, 4_368, 107]
}

fn teacher_forced_prompt_hidden_words_through_layer(last_layer_idx: usize) -> Vec<Vec<u16>> {
    let prompt_token_ids = formatted_say_hi_prompt_token_ids();
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let weights = session.weights.clone();
    let mut token_hidden_words = prompt_token_ids
        .iter()
        .map(|token_id| weights.embed_token_bf16_words(*token_id).unwrap())
        .collect::<Vec<_>>();

    for layer_idx in 0..=last_layer_idx {
        let mut next_token_hidden_words = vec![Vec::new(); prompt_token_ids.len()];
        for pos in 0..(prompt_token_ids.len() - 1) {
            let artifacts = run_layer_plan_with_session_from_sequence(
                &mut session,
                layer_idx,
                CachedLayerSequenceInputs {
                    prefill_input_words_list: token_hidden_words[..=pos].to_vec(),
                    decode_input_words: token_hidden_words[pos + 1].clone(),
                    prefill_rope_offset: 0,
                    decode_rope_offset: i32::try_from(pos + 1).unwrap(),
                    validate_against_oracle: false,
                },
                post_ffn_only_plan(),
            )
            .unwrap();

            let prefill_output_words = artifacts
                .prefill_layer_output_bf16_words()
                .expect("missing teacher-forced prefill output");
            let decode_output_words = artifacts
                .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
                .expect("missing teacher-forced decode output");

            if pos == 0 {
                next_token_hidden_words[pos] = prefill_output_words;
            } else {
                assert_eq!(
                        next_token_hidden_words[pos], prefill_output_words,
                        "teacher-forced layer {} token {} prefill output disagreed with prior decode path",
                        layer_idx, pos
                    );
            }
            next_token_hidden_words[pos + 1] = decode_output_words;
        }
        token_hidden_words = next_token_hidden_words;
    }

    token_hidden_words
}

fn write_teacher_forced_hidden_inputs_for_oracle(
    input_layer_idx: usize,
    decode_token_position: usize,
) {
    let token_hidden_words = teacher_forced_prompt_hidden_words_through_layer(input_layer_idx);
    let tmp_dir = temp_dir();
    let mut prefill_paths = Vec::new();
    for token_position in 0..decode_token_position {
        let path = tmp_dir.join(format!(
            "gemma_say_hi_layer{input_layer_idx}_token{token_position}_f32.bin"
        ));
        write_bf16_words_as_f32_file(&path, &token_hidden_words[token_position]).unwrap();
        println!(
            "token_position={} hidden_fnv1a64=0x{:016X} f32_path={}",
            token_position,
            fnv1a64_u32_words(
                &token_hidden_words[token_position]
                    .iter()
                    .copied()
                    .map(bf16_word_to_f32)
                    .map(f32::to_bits)
                    .collect::<Vec<_>>()
            ),
            path.display()
        );
        prefill_paths.push(path);
    }

    let decode_path = tmp_dir.join(format!(
        "gemma_say_hi_layer{input_layer_idx}_token{decode_token_position}_f32.bin"
    ));
    write_bf16_words_as_f32_file(&decode_path, &token_hidden_words[decode_token_position]).unwrap();
    println!(
        "token_position={} hidden_fnv1a64=0x{:016X} f32_path={}",
        decode_token_position,
        fnv1a64_u32_words(
            &token_hidden_words[decode_token_position]
                .iter()
                .copied()
                .map(bf16_word_to_f32)
                .map(f32::to_bits)
                .collect::<Vec<_>>()
        ),
        decode_path.display()
    );
    println!(
        "prefill_input_f32_files={}",
        prefill_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    println!("decode_input_f32_file={}", decode_path.display());
}

fn teacher_forced_prompt_step_artifacts(
    layer_idx: usize,
    input_layer_idx: usize,
    decode_token_position: usize,
) -> Layer0CachedArtifacts {
    let token_hidden_words = teacher_forced_prompt_hidden_words_through_layer(input_layer_idx);
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    run_layer_plan_with_session_from_sequence(
        &mut session,
        layer_idx,
        CachedLayerSequenceInputs {
            prefill_input_words_list: token_hidden_words[..decode_token_position].to_vec(),
            decode_input_words: token_hidden_words[decode_token_position].clone(),
            prefill_rope_offset: 0,
            decode_rope_offset: i32::try_from(decode_token_position).unwrap(),
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap()
}

fn teacher_env_usize(name: &str) -> usize {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing {name}"))
        .parse()
        .unwrap_or_else(|_| panic!("invalid {name}"))
}

#[test]
fn post_ffn_residual_plan_pulls_in_full_cached_layer_path() {
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::PostFfnResidual);

    for stage in [
        Layer0CachedStage::AttentionOproj,
        Layer0CachedStage::PostAttentionResidual,
        Layer0CachedStage::PreFeedforwardNorm,
        Layer0CachedStage::DenseGate,
        Layer0CachedStage::DenseUp,
        Layer0CachedStage::DenseGeGlu,
        Layer0CachedStage::DenseDown,
        Layer0CachedStage::PostFfnNorm1,
        Layer0CachedStage::Router,
        Layer0CachedStage::MoeExpertGate,
        Layer0CachedStage::MoeExpertUp,
        Layer0CachedStage::MoeExpertGeGlu,
        Layer0CachedStage::MoeExpertDown,
        Layer0CachedStage::MoeExpertOut,
        Layer0CachedStage::MoePostFfnNorm2,
        Layer0CachedStage::MoeMerge,
        Layer0CachedStage::PostFfnResidual,
    ] {
        assert!(plan.requires(stage), "missing dependency for {stage:?}");
    }
}

#[test]
fn display_stage_matches_previous_priority_order() {
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::Router);
    plan.require_stage(Layer0CachedStage::DenseUp);

    assert_eq!(plan.display_stage(), Some(Layer0CachedStage::DenseUp));
}

#[test]
fn evaluation_order_is_dependency_first() {
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::MoeMerge);

    assert_eq!(
        plan.evaluation_order().last().copied(),
        Some(Layer0CachedStage::MoeMerge)
    );
    assert!(plan.requires(Layer0CachedStage::PostFfnNorm1));
    assert!(plan.requires(Layer0CachedStage::MoePostFfnNorm2));
}

#[test]
fn artifacts_expose_stage_bits_and_bf16_words() {
    let mut artifacts = empty_artifacts();
    artifacts.post_ffn_residual_bits = Some(vec![0x3F80_0000, 0xC020_0000]);

    assert_eq!(
        artifacts.layer_output_bits(),
        Some([0x3F80_0000, 0xC020_0000].as_slice())
    );
    assert_eq!(
        artifacts.bf16_words_for_stage(Layer0CachedStage::PostFfnResidual),
        Some(vec![0x3F80, 0xC020])
    );
    assert_eq!(
        artifacts.tensor_bits_for_stage(Layer0CachedStage::Router),
        None
    );
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_v_path_matches_local_mlx_math() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let text_config = &weights.snapshot.config.text_config;
    let layer_idx = 28usize;
    let layer_type = text_config.layer_types.get(layer_idx).unwrap();
    let attention_k_eq_v = text_config.attention_k_eq_v && layer_type == "full_attention";
    let layer_names = LayerTensorNames::for_layer(layer_idx, attention_k_eq_v);

    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        layer_idx,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let expected_input_norm = weights
        .header_for_tensor(&layer_names.input_norm_weight_name)
        .unwrap()
        .rms_norm_weighted_f32(
            &read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path).unwrap(),
            &layer_names.input_norm_weight_name,
            weights.snapshot.config.text_config.rms_norm_eps,
        )
        .unwrap();
    let expected_input_norm_bits = expected_input_norm
        .iter()
        .copied()
        .map(f32::to_bits)
        .collect::<Vec<_>>();
    let decode_h_words = bf16_words_from_f32_bits(&artifacts.decode_input_norm_bits);
    let expected_v_proj = weights
        .header_for_tensor(&layer_names.v.weight_name)
        .unwrap()
        .affine_quantized_matmul_t_f32(
            &decode_h_words,
            &layer_names.v.weight_name,
            &layer_names.v.scales_name,
            &layer_names.v.biases_name,
            weights.snapshot.config.quantization.group_size as u64,
            weights.snapshot.config.quantization.bits,
        )
        .unwrap();
    let expected_k_proj = weights
        .header_for_tensor(&layer_names.k.weight_name)
        .unwrap()
        .affine_quantized_matmul_t_f32(
            &decode_h_words,
            &layer_names.k.weight_name,
            &layer_names.k.scales_name,
            &layer_names.k.biases_name,
            weights.snapshot.config.quantization.group_size as u64,
            weights.snapshot.config.quantization.bits,
        )
        .unwrap();
    let expected_v_proj_bits = expected_v_proj
        .iter()
        .copied()
        .map(f32::to_bits)
        .collect::<Vec<_>>();
    let expected_k_proj_bits = expected_k_proj
        .iter()
        .copied()
        .map(f32::to_bits)
        .collect::<Vec<_>>();
    let expected_v_norm_bits = rms_norm_unweighted_rows_f32(
        &expected_v_proj,
        artifacts.head_dim,
        weights.snapshot.config.text_config.rms_norm_eps,
    )
    .into_iter()
    .map(f32::to_bits)
    .collect::<Vec<_>>();
    eprintln!(
        "layer28_step1 decode_input_norm expected=0x{:016X} actual=0x{:016X}",
        fnv1a64_u32_words(&expected_input_norm_bits),
        fnv1a64_u32_words(&artifacts.decode_input_norm_bits),
    );
    eprintln!(
        "layer28_step1 decode_k_proj expected=0x{:016X}",
        fnv1a64_u32_words(&expected_k_proj_bits),
    );
    eprintln!(
        "layer28_step1 decode_v_proj expected=0x{:016X} actual=0x{:016X}",
        fnv1a64_u32_words(&expected_v_proj_bits),
        fnv1a64_u32_words(&artifacts.decode_v_proj_bits),
    );
    eprintln!(
        "layer28_step1 decode_v_proj expected_first16={}",
        expected_v_proj_bits
            .iter()
            .take(16)
            .map(|bits| format!("0x{bits:08X}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    eprintln!(
        "layer28_step1 decode_v_proj actual_first16={}",
        artifacts
            .decode_v_proj_bits
            .iter()
            .take(16)
            .map(|bits| format!("0x{bits:08X}"))
            .collect::<Vec<_>>()
            .join(",")
    );

    assert_eq!(
        fnv1a64_u32_words(&expected_input_norm_bits),
        fnv1a64_u32_words(&artifacts.decode_input_norm_bits),
        "layer 28 step1 decode_input_norm diverged from local MLX math"
    );
    assert_eq!(
        fnv1a64_u32_words(&expected_v_proj_bits),
        fnv1a64_u32_words(&artifacts.decode_v_proj_bits),
        "layer 28 step1 decode_v_proj diverged from local MLX math"
    );
    assert_eq!(
        fnv1a64_u32_words(&expected_v_norm_bits),
        fnv1a64_u32_words(&artifacts.decode_v_bits),
        "layer 28 step1 decode_v_norm diverged from local MLX math"
    );
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_v_row665_term_models() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let text_config = &weights.snapshot.config.text_config;
    let layer_idx = 28usize;
    let layer_type = text_config.layer_types.get(layer_idx).unwrap();
    let attention_k_eq_v = text_config.attention_k_eq_v && layer_type == "full_attention";
    let layer_names = LayerTensorNames::for_layer(layer_idx, attention_k_eq_v);

    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        layer_idx,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let v_weight_entry = weights.tensor(&layer_names.v.weight_name).unwrap();
    let v_scales_entry = weights.tensor(&layer_names.v.scales_name).unwrap();
    let v_weights = weights
        .header_for_tensor(&layer_names.v.weight_name)
        .unwrap()
        .read_u32_tensor_words(&layer_names.v.weight_name)
        .unwrap();
    let v_scales = weights
        .read_bf16_tensor_words(&layer_names.v.scales_name)
        .unwrap();
    let v_biases = weights
        .read_bf16_tensor_words(&layer_names.v.biases_name)
        .unwrap();
    let decode_h_words = bf16_words_from_f32_bits(&artifacts.decode_input_norm_bits);
    let row = 665usize;
    let (plain_bits, groupbf16_bits) = u4_group_terms_row_totals(
        &decode_h_words,
        &v_weights,
        &v_scales,
        &v_biases,
        row,
        v_weight_entry.shape[1] as usize,
        v_scales_entry.shape[1] as usize,
    );

    eprintln!(
        "layer28_step1 row665 actual=0x{:08X} plain_seq=0x{:08X} groupbf16_seq=0x{:08X}",
        artifacts.decode_v_proj_bits[row], plain_bits, groupbf16_bits,
    );
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_device_qproj_matches_device_qmv() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let text_config = &weights.snapshot.config.text_config;
    let layer_idx = 28usize;
    let layer_type = text_config.layer_types.get(layer_idx).unwrap();
    let attention_k_eq_v = text_config.attention_k_eq_v && layer_type == "full_attention";
    let layer_names = LayerTensorNames::for_layer(layer_idx, attention_k_eq_v);
    let v_weight_entry = weights.tensor(&layer_names.v.weight_name).unwrap();
    let v_scales_entry = weights.tensor(&layer_names.v.scales_name).unwrap();
    let v_layout = exact_qproj_layout(
        v_weight_entry.shape[1] as u32,
        v_scales_entry.shape[1] as u32,
        u32::try_from(v_weight_entry.shape[0]).unwrap(),
        weights.snapshot.config.quantization.bits,
    );

    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        layer_idx,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let decode_h_words = bf16_words_from_f32_bits(&artifacts.decode_input_norm_bits);
    let qproj_bits = device_qproj_row_bits(
        &mut session,
        &decode_h_words,
        v_layout,
        &layer_names.v.weight_name,
        &layer_names.v.scales_name,
        &layer_names.v.biases_name,
    )
    .unwrap();

    eprintln!(
        "layer28_step1 device_qproj expected=0x{:016X} qmv=0x{:016X}",
        fnv1a64_u32_words(&qproj_bits),
        fnv1a64_u32_words(&artifacts.decode_v_proj_bits),
    );
    eprintln!(
        "layer28_step1 device_qproj first16={}",
        qproj_bits
            .iter()
            .take(16)
            .map(|bits| format!("0x{bits:08X}"))
            .collect::<Vec<_>>()
            .join(",")
    );

    assert_eq!(
        fnv1a64_u32_words(&qproj_bits),
        fnv1a64_u32_words(&artifacts.decode_v_proj_bits),
        "layer 28 step1 device qproj row kernel diverged from qmv output"
    );
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_writes_rust_decode_v_proj_file() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        28,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let out_path = temp_dir().join("rust_layer28_step1_decode_v_proj_f32.bin");
    write_f32_bits_file(&out_path, &artifacts.decode_v_proj_bits);
    println!("rust_decode_v_proj_f32_path={}", out_path.display());
    println!(
        "rust_decode_v_proj_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.decode_v_proj_bits)
    );
    assert_eq!(artifacts.decode_v_proj_bits.len(), 2048);
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_writes_rust_decode_v_norm_file() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        28,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let out_path = temp_dir().join("rust_layer28_step1_decode_v_norm_f32.bin");
    write_f32_bits_file(&out_path, &artifacts.decode_v_bits);
    println!("rust_decode_v_norm_f32_path={}", out_path.display());
    println!(
        "rust_decode_v_norm_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.decode_v_bits)
    );
    assert_eq!(artifacts.decode_v_bits.len(), 2048);
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_writes_rust_post_ffn_residual_file() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        28,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let out_bits = artifacts
        .post_ffn_residual_bits
        .as_ref()
        .expect("missing post-ffn residual bits");
    let out_path = temp_dir().join("rust_layer28_step1_post_ffn_residual_f32.bin");
    write_f32_bits_file(&out_path, out_bits);
    println!("rust_post_ffn_residual_f32_path={}", out_path.display());
    println!(
        "rust_post_ffn_residual_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(out_bits)
    );
    assert_eq!(out_bits.len(), 2816);
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_writes_rust_attention_output_file() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        28,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let out_path = temp_dir().join("rust_layer28_step1_attention_output_f32.bin");
    write_f32_bits_file(&out_path, &artifacts.attention_out_bits);
    println!("rust_attention_output_f32_path={}", out_path.display());
    println!(
        "rust_attention_output_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_out_bits)
    );
    assert_eq!(artifacts.attention_out_bits.len(), 8192);
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_writes_rust_full_v_cache_file() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        28,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let out_path = temp_dir().join("rust_layer28_step1_full_v_cache_f32.bin");
    write_f32_bits_file(&out_path, &artifacts.full_v_bits);
    println!("rust_full_v_cache_f32_path={}", out_path.display());
    println!(
        "rust_full_v_cache_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.full_v_bits)
    );
    assert_eq!(artifacts.full_v_bits.len(), 4096);
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_writes_rust_attention_scores_file() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        28,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let out_path = temp_dir().join("rust_layer28_step1_attention_scores_f32.bin");
    write_f32_bits_file(&out_path, &artifacts.attention_score_bits);
    println!("rust_attention_scores_f32_path={}", out_path.display());
    println!(
        "rust_attention_scores_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_score_bits)
    );
    assert_eq!(artifacts.attention_score_bits.len(), 32);
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_writes_rust_attention_probs_file() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        28,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let out_path = temp_dir().join("rust_layer28_step1_attention_probs_f32.bin");
    write_f32_bits_file(&out_path, &artifacts.attention_prob_bits);
    println!("rust_attention_probs_f32_path={}", out_path.display());
    println!(
        "rust_attention_probs_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_prob_bits)
    );
    assert_eq!(artifacts.attention_prob_bits.len(), 32);
}

#[test]
#[ignore]
fn formatted_say_hi_layer28_step1_writes_rust_attention_oproj_file() {
    let outputs = write_formatted_say_hi_outputs_through_layer(27);
    let layer27 = outputs.last().expect("missing layer 27 outputs");
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path).unwrap();
    let artifacts = run_layer_plan_with_session_from_sequence(
        &mut session,
        28,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![read_f32_file_as_bf16_words(
                &layer27.step1_prefill_f32_path,
            )
            .unwrap()],
            decode_input_words: read_f32_file_as_bf16_words(&layer27.step1_decode_f32_path)
                .unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        post_ffn_only_plan(),
    )
    .unwrap();

    let out_bits = artifacts
        .attention_oproj_bits
        .as_ref()
        .expect("missing attention oproj bits");
    let out_path = temp_dir().join("rust_layer28_step1_attention_oproj_f32.bin");
    write_f32_bits_file(&out_path, out_bits);
    println!("rust_attention_oproj_f32_path={}", out_path.display());
    println!(
        "rust_attention_oproj_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(out_bits)
    );
    assert_eq!(out_bits.len(), 2816);
}

#[test]
#[ignore]
fn layer0_to_layer1_hidden_state_handoff_executes() {
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::PostFfnResidual);
    let outputs = run_layer_sequence(default_model_path(), &[0, 1], plan).unwrap();
    assert_eq!(outputs.len(), 2);
    assert!(outputs[0].prefill_layer_output_bits().is_some());
    assert!(outputs[1].prefill_layer_output_bits().is_some());
    assert!(outputs[1].layer_output_bits().is_some());
}

#[test]
#[ignore]
fn all_30_text_layers_execute_from_synthetic_hidden_state_handoff() {
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::PostFfnResidual);
    let layer_indices = (0usize..30).collect::<Vec<_>>();
    let outputs = run_layer_sequence(default_model_path(), &layer_indices, plan).unwrap();
    assert_eq!(outputs.len(), 30);
    assert!(outputs[29].prefill_layer_output_bits().is_some());
    assert!(outputs[29].layer_output_bits().is_some());
}

#[test]
#[ignore]
fn layer0_real_two_token_inputs_report_exact_cached_stage_hashes() {
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::PostFfnResidual);
    let outputs = run_layer_sequence_from_inputs(
        model_path,
        &[0],
        CachedLayerInputs {
            prefill_input_words: weights.embed_token_bf16_words(30_468).unwrap(),
            decode_input_words: weights.embed_token_bf16_words(5_631).unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        plan,
    )
    .unwrap();
    assert_eq!(outputs.len(), 1);
    print_cached_artifacts(&outputs[0]);
}

#[test]
#[ignore]
fn single_prefill_sequence_path_matches_single_prefill_plan() {
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::PostFfnResidual);
    let single = run_layer_plan(
        model_path.clone(),
        0,
        CachedLayerInputs {
            prefill_input_words: weights.embed_token_bf16_words(30_468).unwrap(),
            decode_input_words: weights.embed_token_bf16_words(5_631).unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        plan,
    )
    .unwrap();
    let sequence = run_layer_plan_from_sequence(
        model_path,
        0,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![weights.embed_token_bf16_words(30_468).unwrap()],
            decode_input_words: weights.embed_token_bf16_words(5_631).unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        plan,
    )
    .unwrap();
    assert_eq!(sequence.prefill_k_bits, single.prefill_k_bits);
    assert_eq!(sequence.full_k_bits, single.full_k_bits);
    assert_eq!(sequence.full_v_bits, single.full_v_bits);
    assert_eq!(sequence.layer_output_bits(), single.layer_output_bits());
}

#[test]
#[ignore]
fn layer0_formatted_say_hi_prefix_step_matches_local_mlx_hash() {
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::PostFfnResidual);
    let artifacts = run_layer_plan_from_sequence(
        model_path,
        0,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![
                weights.embed_token_bf16_words(2).unwrap(),
                weights.embed_token_bf16_words(105).unwrap(),
            ],
            decode_input_words: weights.embed_token_bf16_words(2_364).unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 2,
            validate_against_oracle: false,
        },
        plan,
    )
    .unwrap();
    assert_eq!(
        fnv1a64_u32_words(&artifacts.decode_q_bits),
        0x9B1CAF70FB269479
    );
    assert_eq!(
        fnv1a64_u32_words(&artifacts.full_k_bits),
        0xF4944920E989FCF5
    );
    assert_eq!(
        fnv1a64_u32_words(
            artifacts
                .layer_output_bits()
                .expect("missing post-ffn residual bits for formatted prompt step"),
        ),
        0xA062311D5B7C20A4
    );
}

#[test]
#[ignore]
fn formatted_say_hi_first_prefix_step_writes_layer0_outputs_for_layer1_oracle() {
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::PostFfnResidual);
    let artifacts = run_layer_plan(
        model_path,
        0,
        CachedLayerInputs {
            prefill_input_words: weights.embed_token_bf16_words(2).unwrap(),
            decode_input_words: weights.embed_token_bf16_words(105).unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 1,
            validate_against_oracle: false,
        },
        plan,
    )
    .unwrap();
    let prefill_words = artifacts
        .prefill_layer_output_bf16_words()
        .expect("missing layer-0 prefill output");
    let decode_words = artifacts
        .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
        .expect("missing layer-0 decode output");
    let prefill_path = temp_dir().join("gemma_say_hi_layer0_prefill_2_f32.bin");
    let decode_path = temp_dir().join("gemma_say_hi_layer0_decode_105_f32.bin");
    write_bf16_words_as_f32_file(&prefill_path, &prefill_words).unwrap();
    write_bf16_words_as_f32_file(&decode_path, &decode_words).unwrap();
    println!("prefill_f32_path={}", prefill_path.display());
    println!("decode_f32_path={}", decode_path.display());
    println!(
        "prefill_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(
            artifacts
                .prefill_layer_output_bits()
                .expect("missing layer-0 prefill bits"),
        )
    );
    println!(
        "decode_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(
            artifacts
                .layer_output_bits()
                .expect("missing layer-0 decode bits"),
        )
    );
    assert_eq!(prefill_words.len(), NORM_LEN);
    assert_eq!(decode_words.len(), NORM_LEN);
}

#[test]
#[ignore]
fn formatted_say_hi_second_prefix_step_writes_layer0_decode_output_for_layer1_oracle() {
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::PostFfnResidual);
    let artifacts = run_layer_plan_from_sequence(
        model_path,
        0,
        CachedLayerSequenceInputs {
            prefill_input_words_list: vec![
                weights.embed_token_bf16_words(2).unwrap(),
                weights.embed_token_bf16_words(105).unwrap(),
            ],
            decode_input_words: weights.embed_token_bf16_words(2_364).unwrap(),
            prefill_rope_offset: 0,
            decode_rope_offset: 2,
            validate_against_oracle: false,
        },
        plan,
    )
    .unwrap();
    let decode_words = artifacts
        .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
        .expect("missing layer-0 decode output for token 2364");
    let decode_path = temp_dir().join("gemma_say_hi_layer0_decode_2364_f32.bin");
    write_bf16_words_as_f32_file(&decode_path, &decode_words).unwrap();
    println!("decode_f32_path={}", decode_path.display());
    println!(
        "decode_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(
            artifacts
                .layer_output_bits()
                .expect("missing layer-0 decode bits for token 2364"),
        )
    );
    assert_eq!(decode_words.len(), NORM_LEN);
}

#[test]
#[ignore]
fn formatted_say_hi_prefix_step_writes_outputs_through_text_tower() {
    let outputs = write_formatted_say_hi_outputs_through_layer(29);
    let manifest_path = temp_dir().join("gemma_say_hi_layer_hashes.txt");
    let mut manifest = fs::File::create(&manifest_path).unwrap();
    for entry in &outputs {
        writeln!(
                manifest,
                "layer={} step1_prefill_fnv1a64=0x{:016X} step1_decode105_fnv1a64=0x{:016X} step2_decode2364_fnv1a64=0x{:016X}",
                entry.layer_idx,
                entry.step1_prefill_hash,
                entry.step1_decode_hash,
                entry.step2_decode_hash,
            )
            .unwrap();
        println!(
                "layer={} step1_prefill_fnv1a64=0x{:016X} step1_decode105_fnv1a64=0x{:016X} step2_decode2364_fnv1a64=0x{:016X}",
                entry.layer_idx,
                entry.step1_prefill_hash,
                entry.step1_decode_hash,
                entry.step2_decode_hash,
            );
        println!(
            "layer={} step1_prefill_f32_path={}",
            entry.layer_idx,
            entry.step1_prefill_f32_path.display()
        );
        println!(
            "layer={} step1_decode105_f32_path={}",
            entry.layer_idx,
            entry.step1_decode_f32_path.display()
        );
        println!(
            "layer={} step2_decode2364_f32_path={}",
            entry.layer_idx,
            entry.step2_decode_f32_path.display()
        );
    }
    println!("manifest_path={}", manifest_path.display());
    assert_eq!(outputs.len(), 30);
}

#[test]
#[ignore]
fn formatted_say_hi_full_prompt_teacher_forced_hash_manifest() {
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let num_layers = weights.snapshot.config.text_config.num_hidden_layers as usize;

    for layer_idx in 0..num_layers {
        let token_hidden_words = teacher_forced_prompt_hidden_words_through_layer(layer_idx);
        for (position, hidden_words) in token_hidden_words.iter().enumerate() {
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
        }
    }
}

#[test]
#[ignore]
fn formatted_say_hi_layer6_token8_writes_layer5_hidden_inputs_for_oracle() {
    write_teacher_forced_hidden_inputs_for_oracle(5, 8);
}

#[test]
#[ignore]
fn formatted_say_hi_layer17_token8_writes_layer16_hidden_inputs_for_oracle() {
    write_teacher_forced_hidden_inputs_for_oracle(16, 8);
}

#[test]
#[ignore]
fn formatted_say_hi_layer29_token7_writes_layer28_hidden_inputs_for_oracle() {
    write_teacher_forced_hidden_inputs_for_oracle(28, 7);
}

#[test]
#[ignore]
fn formatted_say_hi_layer17_token8_writes_rust_attention_output_file() {
    let artifacts = teacher_forced_prompt_step_artifacts(17, 16, 8);
    let out_path = temp_dir().join("rust_layer17_token8_attention_output_f32.bin");
    write_f32_bits_file(&out_path, &artifacts.attention_out_bits);
    println!("rust_attention_output_f32_path={}", out_path.display());
    println!(
        "rust_attention_output_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_out_bits)
    );
    assert_eq!(artifacts.attention_out_bits.len(), 8192);
}

#[test]
#[ignore]
fn teacher_forced_prompt_hidden_inputs_for_oracle_from_env() {
    let input_layer_idx = teacher_env_usize("MAKEPAD_TEACHER_INPUT_LAYER");
    let decode_token_position = teacher_env_usize("MAKEPAD_TEACHER_DECODE_POS");
    write_teacher_forced_hidden_inputs_for_oracle(input_layer_idx, decode_token_position);
}

#[test]
#[ignore]
fn teacher_forced_prompt_attention_output_file_from_env() {
    let layer_idx = teacher_env_usize("MAKEPAD_TEACHER_LAYER");
    let input_layer_idx = teacher_env_usize("MAKEPAD_TEACHER_INPUT_LAYER");
    let decode_token_position = teacher_env_usize("MAKEPAD_TEACHER_DECODE_POS");
    let artifacts =
        teacher_forced_prompt_step_artifacts(layer_idx, input_layer_idx, decode_token_position);
    let out_path = temp_dir().join(format!(
        "rust_layer{layer_idx}_token{decode_token_position}_attention_output_f32.bin"
    ));
    write_f32_bits_file(&out_path, &artifacts.attention_out_bits);
    println!("rust_attention_output_f32_path={}", out_path.display());
    println!(
        "rust_attention_output_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&artifacts.attention_out_bits)
    );
    assert_eq!(artifacts.attention_out_bits.len(), 8192);
}

#[test]
#[ignore]
fn formatted_say_hi_full_prompt_teacher_forced_plan_matches_exact_backend() {
    let prompt_token_ids = formatted_say_hi_prompt_token_ids();
    let model_path = default_model_path();
    let mut session = LayerExecutionSession::load(model_path.clone()).unwrap();
    let weights = session.weights.clone();
    let num_layers = weights.snapshot.config.text_config.num_hidden_layers as usize;
    let mut token_hidden_words = prompt_token_ids
        .iter()
        .map(|token_id| weights.embed_token_bf16_words(*token_id).unwrap())
        .collect::<Vec<_>>();

    for layer_idx in 0..num_layers {
        let mut next_token_hidden_words = vec![Vec::new(); prompt_token_ids.len()];
        for pos in 0..(prompt_token_ids.len() - 1) {
            let artifacts = run_layer_plan_with_session_from_sequence(
                &mut session,
                layer_idx,
                CachedLayerSequenceInputs {
                    prefill_input_words_list: token_hidden_words[..=pos].to_vec(),
                    decode_input_words: token_hidden_words[pos + 1].clone(),
                    prefill_rope_offset: 0,
                    decode_rope_offset: i32::try_from(pos + 1).unwrap(),
                    validate_against_oracle: false,
                },
                post_ffn_only_plan(),
            )
            .unwrap();

            let prefill_output_words = artifacts
                .prefill_layer_output_bf16_words()
                .expect("missing teacher-forced prefill output");
            let decode_output_words = artifacts
                .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
                .expect("missing teacher-forced decode output");

            if pos == 0 {
                next_token_hidden_words[pos] = prefill_output_words;
            } else {
                assert_eq!(
                        next_token_hidden_words[pos], prefill_output_words,
                        "teacher-forced layer {} token {} prefill output disagreed with prior decode path",
                        layer_idx, pos
                    );
            }
            next_token_hidden_words[pos + 1] = decode_output_words;
        }
        token_hidden_words = next_token_hidden_words;
    }

    let explicit_final_hidden_words = token_hidden_words
        .last()
        .expect("missing explicit final hidden")
        .clone();
    let explicit_final_hidden_bits = read_bf16_buffer_bits(
        &session.runtime,
        &session
            .runtime
            .create_buffer_with_bytes(
                &bytes_from_bf16_words(&explicit_final_hidden_words),
                BufferStorageMode::Shared,
            )
            .unwrap(),
        explicit_final_hidden_words.len(),
    )
    .unwrap();
    let explicit_final_norm_words = weights
        .final_text_norm_bf16_words(&explicit_final_hidden_words)
        .unwrap();
    let explicit_next = weights
        .tied_text_logits_top1_f32(&explicit_final_norm_words)
        .unwrap();

    let mut backend = ExactMetalTextRuntimeSession::load(model_path).unwrap();
    backend.reset_kv_caches().unwrap();
    let mut backend_final_hidden_words = Vec::new();
    for (position, token_id) in prompt_token_ids.iter().copied().enumerate() {
        backend_final_hidden_words = backend
            .eval_token_hidden_state_from_token_id(token_id, position)
            .unwrap();
    }
    let backend_next = backend
        .greedy_token_from_hidden_words(&backend_final_hidden_words)
        .unwrap();

    println!(
        "explicit_final_hidden_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(&explicit_final_hidden_bits)
    );
    println!(
        "backend_final_hidden_fnv1a64=0x{:016X}",
        fnv1a64_u32_words(
            &backend_final_hidden_words
                .iter()
                .copied()
                .map(|word| (bf16_word_to_f32(word)).to_bits())
                .collect::<Vec<_>>()
        )
    );
    println!(
        "explicit_next_token_id={} backend_next_token_id={}",
        explicit_next.token_id, backend_next.token_id
    );

    assert_eq!(explicit_final_hidden_words, backend_final_hidden_words);
    assert_eq!(explicit_next.token_id, backend_next.token_id);
}

#[test]
#[ignore]
fn exact_runtime_reuses_cached_layer_workspace() {
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let mut runtime = ExactMetalTextRuntimeSession::load(model_path).unwrap();
    let token0 = weights.embed_token_bf16_words(30_468).unwrap();
    let token1 = weights.embed_token_bf16_words(5_631).unwrap();

    runtime.eval_layer_hidden_state(0, &token0, 0).unwrap();
    assert_eq!(runtime.layer_workspaces.len(), 1);
    let workspace_ptr = runtime
        .layer_workspaces
        .get(&0)
        .map(|workspace| workspace as *const _)
        .unwrap();

    runtime.eval_layer_hidden_state(0, &token1, 1).unwrap();
    assert_eq!(runtime.layer_workspaces.len(), 1);
    let workspace_ptr_after = runtime
        .layer_workspaces
        .get(&0)
        .map(|workspace| workspace as *const _)
        .unwrap();
    assert_eq!(workspace_ptr, workspace_ptr_after);
}

#[test]
#[ignore]
fn formatted_say_hi_token0_layer0_active_stage_hashes() {
    let model_path = default_model_path();
    let mut runtime = ExactMetalTextRuntimeSession::load(model_path).unwrap();
    runtime.reset_kv_caches().unwrap();

    let workspace = runtime.layer_workspace(0).unwrap();
    let input_buffer = runtime.token_input_buffer().unwrap();
    let output_buffer = workspace.buffers.post_ffn_residual_out.clone();
    runtime
        .dequantize_token_embedding_into_buffer(2, &input_buffer)
        .unwrap();
    runtime
        .eval_layer_hidden_state_core(0, None, Some(&input_buffer), Some(&output_buffer), 0, false)
        .unwrap();
    let workspace = runtime.layer_workspace(0).unwrap();
    let metal = runtime.session.runtime.clone();

    let print_bits = |stage: &str, bits: &[u32]| {
        println!(
            "stage_hash token_position=0 layer_idx=0 stage={} fnv1a64=0x{:016X}",
            stage,
            fnv1a64_u32_words(bits)
        );
    };
    let print_buffer =
        |stage: &str, buffer: &makepad_ggml::backend::metal::MetalBuffer, len: usize| {
            let bits = read_bf16_buffer_bits(&metal, buffer, len).unwrap();
            print_bits(stage, &bits);
        };

    let input_bits = read_bf16_buffer_bits(&metal, &input_buffer, NORM_LEN).unwrap();
    print_bits("input", &input_bits);
    print_buffer("input_norm", &workspace.buffers.h, NORM_LEN);
    print_buffer("q", &workspace.buffers.q_rope, workspace.q_proj.out_len());
    print_buffer("k", &workspace.buffers.k_rope, workspace.k_proj.out_len());
    print_buffer("v", &workspace.buffers.v_norm, workspace.k_proj.out_len());

    {
        let kv_cache = runtime.kv_cache_for_layer(0).unwrap();
        let full_k_bits = read_exact_kv_cache_tensor_bits(
            &metal,
            &kv_cache,
            kv_cache
                .key_buffer()
                .expect("stage hash dump expects bf16 key cache storage"),
        )
        .unwrap();
        let full_v_bits =
            read_exact_kv_cache_tensor_bits(&metal, &kv_cache, &kv_cache.value_buffer).unwrap();
        print_bits("full_k", &full_k_bits);
        print_bits("full_v", &full_v_bits);
    }

    print_buffer(
        "attention_output",
        &workspace.buffers.attn_out,
        workspace.q_proj.out_len(),
    );
    print_buffer(
        "attention_oproj",
        &workspace.buffers.o_proj_out,
        workspace.o_proj.out_len(),
    );
    print_buffer(
        "post_attention_residual",
        &workspace.buffers.residual_out,
        workspace.post_attention_norm_len,
    );
    print_buffer(
        "pre_feedforward_norm",
        &workspace.buffers.pre_feedforward_norm_out,
        workspace.pre_feedforward_norm_len,
    );
    print_buffer(
        "dense_down",
        &workspace.buffers.mlp_down_out,
        workspace.mlp_down.out_len(),
    );
    print_buffer(
        "post_ffn_norm1",
        &workspace.buffers.post_feedforward_norm1_out,
        workspace.post_feedforward_norm1_len,
    );
    print_buffer(
        "router_scaled",
        &workspace.buffers.router_scaled_out,
        workspace.post_attention_norm_len,
    );
    print_buffer(
        "moe_pre_ffn_norm2",
        &workspace.buffers.pre_feedforward_norm2_out,
        workspace.pre_feedforward_norm2_len,
    );
    print_buffer(
        "expert_scores",
        &workspace.buffers.router_proj_out,
        workspace.router_proj.out_len(),
    );
    print_buffer(
        "router_probs",
        &workspace.buffers.router_probs_out,
        workspace.router_proj.out_len(),
    );
    print_buffer(
        "moe_expert_out",
        &workspace.buffers.moe_weighted_out,
        workspace.post_feedforward_norm2_len,
    );
    print_buffer(
        "moe_post_ffn_norm2",
        &workspace.buffers.moe_post_ffn_norm2_out,
        workspace.post_feedforward_norm2_len,
    );
    print_buffer(
        "moe_merge",
        &workspace.buffers.moe_merge_out,
        workspace.post_feedforward_norm1_len,
    );
    print_buffer(
        "post_feedforward_norm",
        &workspace.buffers.post_feedforward_norm_out,
        workspace.post_feedforward_norm_len,
    );
    print_buffer(
        "layer_output",
        &workspace.buffers.post_ffn_residual_out,
        workspace.post_feedforward_norm1_len,
    );
}

#[test]
#[ignore]
fn exact_token_embedding_matches_cpu_dequant() {
    let model_path = default_model_path();
    let model_root = super::model_root_dir(&model_path).unwrap();
    let weights = MlxIndexedSafetensors::load(&model_root).unwrap();
    let mut runtime = ExactMetalTextRuntimeSession::load(model_path).unwrap();

    let token_id = 2u32;
    let expected_words = weights.embed_token_bf16_words(token_id).unwrap();
    let expected_bits = expected_words
        .iter()
        .copied()
        .map(|word| (word as u32) << 16)
        .collect::<Vec<_>>();

    let input_buffer = runtime.token_input_buffer().unwrap();
    runtime
        .dequantize_token_embedding_into_buffer(token_id, &input_buffer)
        .unwrap();
    let got_bits = read_bf16_buffer_bits(
        &runtime.session.runtime,
        &input_buffer,
        expected_words.len(),
    )
    .unwrap();

    println!(
        "token_id={} expected_fnv1a64=0x{:016X} got_fnv1a64=0x{:016X}",
        token_id,
        fnv1a64_u32_words(&expected_bits),
        fnv1a64_u32_words(&got_bits)
    );
    if got_bits != expected_bits {
        for idx in 0..16.min(expected_words.len()) {
            let expected_word = expected_words[idx];
            let got_word = (got_bits[idx] >> 16) as u16;
            println!(
                "idx={} expected=0x{:04X} ({:.7}) got=0x{:04X} ({:.7})",
                idx,
                expected_word,
                super::bf16_word_to_f32(expected_word),
                got_word,
                super::bf16_word_to_f32(got_word),
            );
        }
    }
    assert_eq!(got_bits, expected_bits);
}
