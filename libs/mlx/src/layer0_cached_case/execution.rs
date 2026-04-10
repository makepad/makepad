pub fn run_cli() -> Result<(), Box<dyn Error>> {
    let mut model_path = default_model_path();
    let mut layer_idx = 0usize;
    let mut plan = Layer0CachedPlan::new();
    let mut prefill_token_ids = Vec::<u32>::new();
    let mut decode_token_id = None;
    let mut prefill_input_f32_files = Vec::<PathBuf>::new();
    let mut decode_input_f32_file = None;
    let mut write_prefill_layer_output_f32_file = None;
    let mut write_layer_output_f32_file = None;
    let mut prefill_rope_offset = PREFILL_ROPE_OFFSET;
    let mut decode_rope_offset = DECODE_ROPE_OFFSET;
    let mut args_iter = env::args().skip(1);
    while let Some(arg) = args_iter.next() {
        if let Some(stage) = Layer0CachedStage::from_cli_flag(arg.as_str()) {
            plan.require_stage(stage);
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => {
                eprintln!(
                    "Usage: metal_qkv_attention_output_cached_row [model.safetensors] [--layer N] [--prefill-token ID] [--prefill-tokens ID,ID,...] [--decode-token ID] [--prefill-input-f32-file PATH] [--prefill-input-f32-files PATH,PATH,...] [--decode-input-f32-file PATH] [--write-prefill-layer-output-f32-file PATH] [--write-layer-output-f32-file PATH] [--prefill-position N] [--decode-position N] [--oproj] [--residual] [--pre-ffn-norm] [--dense-gate] [--dense-up] [--dense-geglu] [--dense-down] [--post-ffn-norm1] [--router] [--moe-expert-gate] [--moe-expert-up] [--moe-expert-geglu] [--moe-expert-down] [--moe-expert-out] [--moe-post-ffn-norm2] [--moe-merge] [--post-ffn-residual|--layer-output]"
                );
                return Ok(());
            }
            "--layer" => {
                let value = args_iter.next().ok_or("--layer expects a value")?;
                layer_idx = value.parse()?;
            }
            "--prefill-token" => {
                let value = args_iter.next().ok_or("--prefill-token expects a value")?;
                prefill_token_ids.push(value.parse()?);
            }
            "--prefill-tokens" => {
                let value = args_iter.next().ok_or("--prefill-tokens expects a value")?;
                for token in value.split(',').filter(|token| !token.is_empty()) {
                    prefill_token_ids.push(token.parse()?);
                }
            }
            "--decode-token" => {
                let value = args_iter.next().ok_or("--decode-token expects a value")?;
                decode_token_id = Some(value.parse()?);
            }
            "--prefill-input-f32-file" => {
                let value = args_iter
                    .next()
                    .ok_or("--prefill-input-f32-file expects a value")?;
                prefill_input_f32_files.push(PathBuf::from(value));
            }
            "--prefill-input-f32-files" => {
                let value = args_iter
                    .next()
                    .ok_or("--prefill-input-f32-files expects a value")?;
                for path in value.split(',').filter(|path| !path.is_empty()) {
                    prefill_input_f32_files.push(PathBuf::from(path));
                }
            }
            "--decode-input-f32-file" => {
                let value = args_iter
                    .next()
                    .ok_or("--decode-input-f32-file expects a value")?;
                decode_input_f32_file = Some(PathBuf::from(value));
            }
            "--write-prefill-layer-output-f32-file" => {
                let value = args_iter
                    .next()
                    .ok_or("--write-prefill-layer-output-f32-file expects a value")?;
                write_prefill_layer_output_f32_file = Some(PathBuf::from(value));
            }
            "--write-layer-output-f32-file" => {
                let value = args_iter
                    .next()
                    .ok_or("--write-layer-output-f32-file expects a value")?;
                write_layer_output_f32_file = Some(PathBuf::from(value));
            }
            "--prefill-position" => {
                let value = args_iter
                    .next()
                    .ok_or("--prefill-position expects a value")?;
                prefill_rope_offset = value.parse()?;
            }
            "--decode-position" => {
                let value = args_iter
                    .next()
                    .ok_or("--decode-position expects a value")?;
                decode_rope_offset = value.parse()?;
            }
            _ if arg.starts_with("--") => {
                return Err(format!("unknown option {arg}").into());
            }
            _ => {
                model_path = PathBuf::from(arg);
            }
        }
    }

    let using_file_inputs = !prefill_input_f32_files.is_empty() || decode_input_f32_file.is_some();
    let using_token_inputs = !prefill_token_ids.is_empty() || decode_token_id.is_some();
    if using_file_inputs && using_token_inputs {
        return Err("choose either token-id inputs or f32 hidden-state inputs, not both".into());
    }

    let artifacts = if !using_file_inputs && !using_token_inputs {
        run_layer_plan(
            model_path,
            layer_idx,
            CachedLayerInputs::synthetic_case(),
            plan,
        )?
    } else if using_file_inputs {
        let decode_input_f32_file = decode_input_f32_file
            .ok_or("--decode-input-f32-file must be provided with f32 hidden-state inputs")?;
        if prefill_input_f32_files.is_empty() {
            return Err("at least one --prefill-input-f32-file is required".into());
        }
        let prefill_input_words_list = prefill_input_f32_files
            .iter()
            .map(|path| read_f32_file_as_bf16_words(path))
            .collect::<Result<Vec<_>, _>>()?;
        let decode_input_words = read_f32_file_as_bf16_words(&decode_input_f32_file)?;
        run_layer_plan_from_sequence(
            model_path,
            layer_idx,
            CachedLayerSequenceInputs {
                prefill_input_words_list,
                decode_input_words,
                prefill_rope_offset,
                decode_rope_offset,
                validate_against_oracle: false,
            },
            plan,
        )?
    } else {
        let decode_token_id = decode_token_id.ok_or(
            "--decode-token must be provided when using --prefill-token or --prefill-tokens",
        )?;
        let mut session = LayerExecutionSession::load(model_path.clone())?;
        let prefill_input_words_list = prefill_token_ids
            .iter()
            .copied()
            .map(|token_id| session.weights.embed_token_bf16_words(token_id))
            .collect::<Result<Vec<_>, _>>()?;
        if prefill_input_words_list.is_empty() {
            return Err("at least one prefill token is required".into());
        }
        let decode_input_words = session.weights.embed_token_bf16_words(decode_token_id)?;
        run_layer_plan_with_session_from_sequence(
            &mut session,
            layer_idx,
            CachedLayerSequenceInputs {
                prefill_input_words_list,
                decode_input_words,
                prefill_rope_offset,
                decode_rope_offset,
                validate_against_oracle: false,
            },
            plan,
        )?
    };
    print_cached_artifacts(&artifacts);
    if let Some(path) = write_prefill_layer_output_f32_file {
        let prefill_words = artifacts.prefill_layer_output_bf16_words().ok_or(
            "--write-prefill-layer-output-f32-file requires prefill post-ffn residual output",
        )?;
        write_bf16_words_as_f32_file(&path, &prefill_words)?;
        println!("prefill_layer_output_f32_file={}", path.display());
    }
    if let Some(path) = write_layer_output_f32_file {
        let decode_words = artifacts
            .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
            .ok_or("--write-layer-output-f32-file requires post-ffn residual output")?;
        write_bf16_words_as_f32_file(&path, &decode_words)?;
        println!("layer_output_f32_file={}", path.display());
    }
    Ok(())
}

pub fn run_plan(
    model_path: PathBuf,
    plan: Layer0CachedPlan,
) -> Result<Layer0CachedArtifacts, Box<dyn Error>> {
    run_layer_plan(model_path, 0, CachedLayerInputs::synthetic_case(), plan)
}

pub fn run_layer_plan(
    model_path: PathBuf,
    layer_idx: usize,
    inputs: CachedLayerInputs,
    plan: Layer0CachedPlan,
) -> Result<Layer0CachedArtifacts, Box<dyn Error>> {
    let mut session = LayerExecutionSession::load(model_path)?;
    run_layer_plan_with_session(&mut session, layer_idx, inputs, plan)
}

pub fn run_layer_plan_from_sequence(
    model_path: PathBuf,
    layer_idx: usize,
    inputs: CachedLayerSequenceInputs,
    plan: Layer0CachedPlan,
) -> Result<Layer0CachedArtifacts, Box<dyn Error>> {
    let mut session = LayerExecutionSession::load(model_path)?;
    run_layer_plan_with_session_from_sequence(&mut session, layer_idx, inputs, plan)
}

fn run_layer_plan_with_session(
    session: &mut LayerExecutionSession,
    layer_idx: usize,
    inputs: CachedLayerInputs,
    plan: Layer0CachedPlan,
) -> Result<Layer0CachedArtifacts, Box<dyn Error>> {
    run_layer_plan_with_session_from_sequence(
        session,
        layer_idx,
        CachedLayerSequenceInputs::from_single(inputs),
        plan,
    )
}

fn run_layer_plan_with_session_from_sequence(
    session: &mut LayerExecutionSession,
    layer_idx: usize,
    inputs: CachedLayerSequenceInputs,
    plan: Layer0CachedPlan,
) -> Result<Layer0CachedArtifacts, Box<dyn Error>> {
    let validate_oproj = plan.requires(Layer0CachedStage::AttentionOproj);
    let validate_residual = plan.requires(Layer0CachedStage::PostAttentionResidual);
    let validate_pre_ffn_norm = plan.requires(Layer0CachedStage::PreFeedforwardNorm);
    let validate_dense_gate = plan.requires(Layer0CachedStage::DenseGate);
    let validate_dense_up = plan.requires(Layer0CachedStage::DenseUp);
    let validate_dense_geglu = plan.requires(Layer0CachedStage::DenseGeGlu);
    let validate_dense_down = plan.requires(Layer0CachedStage::DenseDown);
    let validate_post_ffn_norm1 = plan.requires(Layer0CachedStage::PostFfnNorm1);
    let validate_router = plan.requires(Layer0CachedStage::Router);
    let validate_moe_expert_gate = plan.requires(Layer0CachedStage::MoeExpertGate);
    let validate_moe_expert_up = plan.requires(Layer0CachedStage::MoeExpertUp);
    let validate_moe_expert_geglu = plan.requires(Layer0CachedStage::MoeExpertGeGlu);
    let validate_moe_expert_down = plan.requires(Layer0CachedStage::MoeExpertDown);
    let validate_moe_expert_out = plan.requires(Layer0CachedStage::MoeExpertOut);
    let validate_moe_post_ffn_norm2 = plan.requires(Layer0CachedStage::MoePostFfnNorm2);
    let validate_moe_merge = plan.requires(Layer0CachedStage::MoeMerge);
    let validate_post_ffn_residual = plan.requires(Layer0CachedStage::PostFfnResidual);

    let model_path = session.model_path.clone();
    let weights = session.weights.clone();
    let runtime = session.runtime.clone();
    let layer_type = weights
        .snapshot
        .config
        .text_config
        .layer_types
        .get(layer_idx)
        .ok_or_else(|| format!("missing text layer type for layer {layer_idx}"))?;
    let attention_k_eq_v =
        weights.snapshot.config.text_config.attention_k_eq_v && layer_type == "full_attention";
    let layer_names = LayerTensorNames::for_layer(layer_idx, attention_k_eq_v);

    let q_weight_entry = weights
        .tensor(&layer_names.q.weight_name)
        .map_err(|_| "missing q projection weight entry")?;
    let q_scales_entry = weights
        .tensor(&layer_names.q.scales_name)
        .map_err(|_| "missing q projection scales entry")?;
    let q_norm_weight_entry = weights
        .tensor(
            layer_names
                .q
                .norm_weight_name
                .as_deref()
                .ok_or("missing q norm weight name")?,
        )
        .map_err(|_| "missing q norm weight entry")?;
    let k_weight_entry = weights
        .tensor(&layer_names.k.weight_name)
        .map_err(|_| "missing k projection weight entry")?;
    let k_scales_entry = weights
        .tensor(&layer_names.k.scales_name)
        .map_err(|_| "missing k projection scales entry")?;
    let k_norm_weight_entry = weights
        .tensor(
            layer_names
                .k
                .norm_weight_name
                .as_deref()
                .ok_or("missing k norm weight name")?,
        )
        .map_err(|_| "missing k norm weight entry")?;
    let v_weight_entry = weights
        .tensor(&layer_names.v.weight_name)
        .map_err(|_| "missing v projection weight entry")?;
    let v_scales_entry = weights
        .tensor(&layer_names.v.scales_name)
        .map_err(|_| "missing v projection scales entry")?;
    let o_weight_entry = if validate_oproj {
        Some(
            weights
                .tensor(&layer_names.o.weight_name)
                .map_err(|_| "missing o_proj weight entry")?,
        )
    } else {
        None
    };
    let o_scales_entry = if validate_oproj {
        Some(
            weights
                .tensor(&layer_names.o.scales_name)
                .map_err(|_| "missing o_proj scales entry")?,
        )
    } else {
        None
    };
    let post_attention_norm_weight_entry = if validate_residual {
        Some(
            weights
                .tensor(&layer_names.post_attention_norm_weight_name)
                .map_err(|_| "missing post-attention norm weight entry")?,
        )
    } else {
        None
    };
    let pre_feedforward_norm_weight_entry = if validate_pre_ffn_norm {
        Some(
            weights
                .tensor(&layer_names.pre_feedforward_norm_weight_name)
                .map_err(|_| "missing pre-feedforward norm weight entry")?,
        )
    } else {
        None
    };
    let pre_feedforward_norm2_weight_entry = if validate_moe_expert_gate {
        Some(
            weights
                .tensor(&layer_names.pre_feedforward_norm2_weight_name)
                .map_err(|_| "missing pre-feedforward norm2 weight entry")?,
        )
    } else {
        None
    };
    let post_feedforward_norm1_weight_entry = if validate_post_ffn_norm1 {
        Some(
            weights
                .tensor(&layer_names.post_feedforward_norm1_weight_name)
                .map_err(|_| "missing post-feedforward norm1 weight entry")?,
        )
    } else {
        None
    };
    let post_feedforward_norm2_weight_entry = if validate_moe_post_ffn_norm2 {
        Some(
            weights
                .tensor(&layer_names.post_feedforward_norm2_weight_name)
                .map_err(|_| "missing post-feedforward norm2 weight entry")?,
        )
    } else {
        None
    };
    let mlp_gate_weight_entry = if validate_dense_gate {
        Some(
            weights
                .tensor(&layer_names.mlp_gate_weight_name)
                .map_err(|_| "missing mlp gate_proj weight entry")?,
        )
    } else {
        None
    };
    let mlp_gate_scales_entry = if validate_dense_gate {
        Some(
            weights
                .tensor(&layer_names.mlp_gate_scales_name)
                .map_err(|_| "missing mlp gate_proj scales entry")?,
        )
    } else {
        None
    };
    let mlp_up_weight_entry = if validate_dense_up {
        Some(
            weights
                .tensor(&layer_names.mlp_up_weight_name)
                .map_err(|_| "missing mlp up_proj weight entry")?,
        )
    } else {
        None
    };
    let mlp_up_scales_entry = if validate_dense_up {
        Some(
            weights
                .tensor(&layer_names.mlp_up_scales_name)
                .map_err(|_| "missing mlp up_proj scales entry")?,
        )
    } else {
        None
    };
    let mlp_down_weight_entry = if validate_dense_down {
        Some(
            weights
                .tensor(&layer_names.mlp_down_weight_name)
                .map_err(|_| "missing mlp down_proj weight entry")?,
        )
    } else {
        None
    };
    let mlp_down_scales_entry = if validate_dense_down {
        Some(
            weights
                .tensor(&layer_names.mlp_down_scales_name)
                .map_err(|_| "missing mlp down_proj scales entry")?,
        )
    } else {
        None
    };
    let router_scale_entry = if validate_router {
        Some(
            weights
                .tensor(&layer_names.router_scale_name)
                .map_err(|_| "missing router scale entry")?,
        )
    } else {
        None
    };
    let router_proj_weight_entry = if validate_router {
        Some(
            weights
                .tensor(&layer_names.router_proj_weight_name)
                .map_err(|_| "missing router proj weight entry")?,
        )
    } else {
        None
    };
    let router_proj_scales_entry = if validate_router {
        Some(
            weights
                .tensor(&layer_names.router_proj_scales_name)
                .map_err(|_| "missing router proj scales entry")?,
        )
    } else {
        None
    };
    let expert_gate_weight_entry = if validate_moe_expert_gate {
        Some(
            weights
                .tensor(&layer_names.expert_gate_weight_name)
                .map_err(|_| "missing expert gate weight entry")?,
        )
    } else {
        None
    };
    let expert_gate_scales_entry = if validate_moe_expert_gate {
        Some(
            weights
                .tensor(&layer_names.expert_gate_scales_name)
                .map_err(|_| "missing expert gate scales entry")?,
        )
    } else {
        None
    };
    let expert_up_weight_entry = if validate_moe_expert_up {
        Some(
            weights
                .tensor(&layer_names.expert_up_weight_name)
                .map_err(|_| "missing expert up weight entry")?,
        )
    } else {
        None
    };
    let expert_up_scales_entry = if validate_moe_expert_up {
        Some(
            weights
                .tensor(&layer_names.expert_up_scales_name)
                .map_err(|_| "missing expert up scales entry")?,
        )
    } else {
        None
    };
    let expert_down_weight_entry = if validate_moe_expert_down {
        Some(
            weights
                .tensor(&layer_names.expert_down_weight_name)
                .map_err(|_| "missing expert down weight entry")?,
        )
    } else {
        None
    };
    let expert_down_scales_entry = if validate_moe_expert_down {
        Some(
            weights
                .tensor(&layer_names.expert_down_scales_name)
                .map_err(|_| "missing expert down scales entry")?,
        )
    } else {
        None
    };

    let q_out_len = usize::try_from(q_weight_entry.shape[0])?;
    let k_out_len = usize::try_from(k_weight_entry.shape[0])?;
    let v_out_len = usize::try_from(v_weight_entry.shape[0])?;
    let o_out_len = if let Some(entry) = o_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let post_attention_norm_len = if let Some(entry) = post_attention_norm_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let pre_feedforward_norm_len = if let Some(entry) = pre_feedforward_norm_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let pre_feedforward_norm2_len = if let Some(entry) = pre_feedforward_norm2_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let post_feedforward_norm1_len = if let Some(entry) = post_feedforward_norm1_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let post_feedforward_norm2_len = if let Some(entry) = post_feedforward_norm2_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let mlp_gate_out_len = if let Some(entry) = mlp_gate_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let mlp_up_out_len = if let Some(entry) = mlp_up_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let mlp_down_out_len = if let Some(entry) = mlp_down_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let router_scale_len = if let Some(entry) = router_scale_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let router_out_len = if let Some(entry) = router_proj_weight_entry {
        usize::try_from(entry.shape[0])?
    } else {
        0
    };
    let expert_gate_out_len = if let Some(entry) = expert_gate_weight_entry {
        usize::try_from(entry.shape[1])?
    } else {
        0
    };
    let expert_up_out_len = if let Some(entry) = expert_up_weight_entry {
        usize::try_from(entry.shape[1])?
    } else {
        0
    };
    let expert_down_out_len = if let Some(entry) = expert_down_weight_entry {
        usize::try_from(entry.shape[1])?
    } else {
        0
    };
    let head_dim = usize::try_from(q_norm_weight_entry.shape[0])?;
    let k_head_dim = usize::try_from(k_norm_weight_entry.shape[0])?;
    if head_dim == 0 || head_dim != k_head_dim {
        return Err(format!("invalid q/k head_dim: q={head_dim} k={k_head_dim}").into());
    }
    if q_out_len % head_dim != 0 || k_out_len % head_dim != 0 || v_out_len % head_dim != 0 {
        return Err(format!(
            "invalid q/k/v head layout: q_out_len={} k_out_len={} v_out_len={} head_dim={}",
            q_out_len, k_out_len, v_out_len, head_dim
        )
        .into());
    }
    let q_head_count = q_out_len / head_dim;
    let k_head_count = k_out_len / head_dim;
    let v_head_count = v_out_len / head_dim;
    if k_head_count == 0 || v_head_count != k_head_count || q_head_count % k_head_count != 0 {
        return Err(format!(
            "invalid grouped-query head layout: q_head_count={} k_head_count={} v_head_count={}",
            q_head_count, k_head_count, v_head_count
        )
        .into());
    }
    let q_heads_per_kv = q_head_count / k_head_count;
    let rope_params = if layer_type == "full_attention" {
        &weights
            .snapshot
            .config
            .text_config
            .rope_parameters
            .full_attention
    } else {
        &weights
            .snapshot
            .config
            .text_config
            .rope_parameters
            .sliding_attention
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
    let rope_base = rope_params.rope_theta as f32;
    let layer_attention_kind = if layer_type == "full_attention" {
        GemmaAttentionKind::Full
    } else {
        GemmaAttentionKind::Sliding
    };
    if validate_residual && post_attention_norm_len != o_out_len {
        return Err(format!(
            "invalid post-attention norm length: got {} expected {}",
            post_attention_norm_len, o_out_len
        )
        .into());
    }
    if validate_pre_ffn_norm && pre_feedforward_norm_len != post_attention_norm_len {
        return Err(format!(
            "invalid pre-feedforward norm length: got {} expected {}",
            pre_feedforward_norm_len, post_attention_norm_len
        )
        .into());
    }
    if let Some(weight_entry) = mlp_gate_weight_entry {
        let mlp_gate_n_in = usize::try_from(weight_entry.shape[1] * 8)?;
        if mlp_gate_n_in != pre_feedforward_norm_len {
            return Err(format!(
                "invalid mlp gate_proj input size: got {} expected {}",
                mlp_gate_n_in, pre_feedforward_norm_len
            )
            .into());
        }
    }
    if let Some(weight_entry) = mlp_up_weight_entry {
        let mlp_up_n_in = usize::try_from(weight_entry.shape[1] * 8)?;
        if mlp_up_n_in != pre_feedforward_norm_len {
            return Err(format!(
                "invalid mlp up_proj input size: got {} expected {}",
                mlp_up_n_in, pre_feedforward_norm_len
            )
            .into());
        }
    }
    if let Some(weight_entry) = mlp_down_weight_entry {
        let mlp_down_n_in = usize::try_from(weight_entry.shape[1] * 8)?;
        if mlp_down_n_in != mlp_gate_out_len {
            return Err(format!(
                "invalid mlp down_proj input size: got {} expected {}",
                mlp_down_n_in, mlp_gate_out_len
            )
            .into());
        }
        if mlp_down_out_len != pre_feedforward_norm_len {
            return Err(format!(
                "invalid mlp down_proj output size: got {} expected {}",
                mlp_down_out_len, pre_feedforward_norm_len
            )
            .into());
        }
    }
    if validate_post_ffn_norm1 && post_feedforward_norm1_len != mlp_down_out_len {
        return Err(format!(
            "invalid post-feedforward norm1 length: got {} expected {}",
            post_feedforward_norm1_len, mlp_down_out_len
        )
        .into());
    }
    if validate_router && router_scale_len != post_attention_norm_len {
        return Err(format!(
            "invalid router scale length: got {} expected {}",
            router_scale_len, post_attention_norm_len
        )
        .into());
    }
    if validate_router && router_out_len < ROUTER_TOP_K {
        return Err(format!(
            "invalid router output length: got {} expected at least {}",
            router_out_len, ROUTER_TOP_K
        )
        .into());
    }
    if validate_moe_expert_gate && pre_feedforward_norm2_len != post_attention_norm_len {
        return Err(format!(
            "invalid pre-feedforward norm2 length: got {} expected {}",
            pre_feedforward_norm2_len, post_attention_norm_len
        )
        .into());
    }
    if let Some(weight_entry) = expert_gate_weight_entry {
        let expert_gate_n_in = usize::try_from(weight_entry.shape[2] * 8)?;
        if expert_gate_n_in != pre_feedforward_norm2_len {
            return Err(format!(
                "invalid expert gate input size: got {} expected {}",
                expert_gate_n_in, pre_feedforward_norm2_len
            )
            .into());
        }
    }
    if let Some(weight_entry) = expert_up_weight_entry {
        let expert_up_n_in = usize::try_from(weight_entry.shape[2] * 8)?;
        if expert_up_n_in != pre_feedforward_norm2_len {
            return Err(format!(
                "invalid expert up input size: got {} expected {}",
                expert_up_n_in, pre_feedforward_norm2_len
            )
            .into());
        }
    }
    if let Some(weight_entry) = expert_down_weight_entry {
        let expert_down_n_in = usize::try_from(weight_entry.shape[2] * 8)?;
        if expert_down_n_in != expert_gate_out_len {
            return Err(format!(
                "invalid expert down input size: got {} expected {}",
                expert_down_n_in, expert_gate_out_len
            )
            .into());
        }
        if expert_down_out_len != pre_feedforward_norm2_len {
            return Err(format!(
                "invalid expert down output size: got {} expected {}",
                expert_down_out_len, pre_feedforward_norm2_len
            )
            .into());
        }
    }
    if validate_moe_post_ffn_norm2 && post_feedforward_norm2_len != expert_down_out_len {
        return Err(format!(
            "invalid post-feedforward norm2 length: got {} expected {}",
            post_feedforward_norm2_len, expert_down_out_len
        )
        .into());
    }

    let CachedLayerSequenceInputs {
        prefill_input_words_list,
        decode_input_words: decode_x_words,
        prefill_rope_offset,
        decode_rope_offset,
        validate_against_oracle,
    } = inputs;
    if prefill_input_words_list.is_empty() {
        return Err("cached layer sequence requires at least one prefill input".into());
    }
    for (prefill_index, prefill_x_words) in prefill_input_words_list.iter().enumerate() {
        if prefill_x_words.len() != NORM_LEN {
            return Err(format!(
                "prefill input length mismatch at index {}: got {} expected {}",
                prefill_index,
                prefill_x_words.len(),
                NORM_LEN
            )
            .into());
        }
    }
    if decode_x_words.len() != NORM_LEN {
        return Err(format!(
            "decode input length mismatch: got {} expected {}",
            decode_x_words.len(),
            NORM_LEN
        )
        .into());
    }
    let kv_capacity = prefill_input_words_list.len() + 1;
    let x_buf = runtime.create_buffer(NORM_LEN * 2, BufferStorageMode::Shared)?;
    let input_norm_weight_buf =
        session.private_weight_buffer(&layer_names.input_norm_weight_name)?;
    let h_buf = runtime.create_buffer(NORM_LEN * 2, BufferStorageMode::Private)?;

    let q_weight_buf = session.private_weight_buffer(&layer_names.q.weight_name)?;
    let q_scales_buf = session.private_weight_buffer(&layer_names.q.scales_name)?;
    let q_biases_buf = session.private_weight_buffer(&layer_names.q.biases_name)?;
    let q_norm_weight_buf = session.private_weight_buffer(
        layer_names
            .q
            .norm_weight_name
            .as_deref()
            .ok_or("missing q norm weight name")?,
    )?;
    let q_proj_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;
    let q_norm_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;
    let q_rope_buf = runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?;

    let k_weight_buf = session.private_weight_buffer(&layer_names.k.weight_name)?;
    let k_scales_buf = session.private_weight_buffer(&layer_names.k.scales_name)?;
    let k_biases_buf = session.private_weight_buffer(&layer_names.k.biases_name)?;
    let k_norm_weight_buf = session.private_weight_buffer(
        layer_names
            .k
            .norm_weight_name
            .as_deref()
            .ok_or("missing k norm weight name")?,
    )?;
    let k_proj_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;
    let k_norm_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;
    let k_rope_buf = runtime.create_buffer(k_out_len * 2, BufferStorageMode::Private)?;

    let v_weight_buf = session.private_weight_buffer(&layer_names.v.weight_name)?;
    let v_scales_buf = session.private_weight_buffer(&layer_names.v.scales_name)?;
    let v_biases_buf = session.private_weight_buffer(&layer_names.v.biases_name)?;
    let v_proj_buf = runtime.create_buffer(v_out_len * 2, BufferStorageMode::Private)?;
    let v_norm_buf = runtime.create_buffer(v_out_len * 2, BufferStorageMode::Private)?;
    let ones_bytes = bytes_from_bf16_words(&vec![0x3F80u16; head_dim]);
    let v_norm_weight_buf =
        runtime.create_buffer_with_bytes(&ones_bytes, BufferStorageMode::Private)?;
    let attention_logits_buf =
        runtime.create_buffer(q_head_count * kv_capacity * 2, BufferStorageMode::Private)?;
    let o_weight_buf =
        optional_private_weight_buffer(session, validate_oproj, &layer_names.o.weight_name)?;
    let o_scales_buf =
        optional_private_weight_buffer(session, validate_oproj, &layer_names.o.scales_name)?;
    let o_biases_buf =
        optional_private_weight_buffer(session, validate_oproj, &layer_names.o.biases_name)?;
    let post_attention_norm_weight_buf = optional_private_weight_buffer(
        session,
        validate_residual,
        &layer_names.post_attention_norm_weight_name,
    )?;
    let pre_feedforward_norm_weight_buf = optional_private_weight_buffer(
        session,
        validate_pre_ffn_norm,
        &layer_names.pre_feedforward_norm_weight_name,
    )?;
    let pre_feedforward_norm2_weight_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_gate,
        &layer_names.pre_feedforward_norm2_weight_name,
    )?;
    let post_feedforward_norm1_weight_buf = optional_private_weight_buffer(
        session,
        validate_post_ffn_norm1,
        &layer_names.post_feedforward_norm1_weight_name,
    )?;
    let post_feedforward_norm2_weight_buf = optional_private_weight_buffer(
        session,
        validate_moe_post_ffn_norm2,
        &layer_names.post_feedforward_norm2_weight_name,
    )?;
    let mlp_gate_weight_buf = optional_private_weight_buffer(
        session,
        validate_dense_gate,
        &layer_names.mlp_gate_weight_name,
    )?;
    let mlp_gate_scales_buf = optional_private_weight_buffer(
        session,
        validate_dense_gate,
        &layer_names.mlp_gate_scales_name,
    )?;
    let mlp_gate_biases_buf = optional_private_weight_buffer(
        session,
        validate_dense_gate,
        &layer_names.mlp_gate_biases_name,
    )?;
    let mlp_up_weight_buf = optional_private_weight_buffer(
        session,
        validate_dense_up,
        &layer_names.mlp_up_weight_name,
    )?;
    let mlp_up_scales_buf = optional_private_weight_buffer(
        session,
        validate_dense_up,
        &layer_names.mlp_up_scales_name,
    )?;
    let mlp_up_biases_buf = optional_private_weight_buffer(
        session,
        validate_dense_up,
        &layer_names.mlp_up_biases_name,
    )?;
    let mlp_down_weight_buf = optional_private_weight_buffer(
        session,
        validate_dense_down,
        &layer_names.mlp_down_weight_name,
    )?;
    let mlp_down_scales_buf = optional_private_weight_buffer(
        session,
        validate_dense_down,
        &layer_names.mlp_down_scales_name,
    )?;
    let mlp_down_biases_buf = optional_private_weight_buffer(
        session,
        validate_dense_down,
        &layer_names.mlp_down_biases_name,
    )?;
    let router_scale_weight_buf =
        optional_private_weight_buffer(session, validate_router, &layer_names.router_scale_name)?;
    let router_proj_weight_buf = optional_private_weight_buffer(
        session,
        validate_router,
        &layer_names.router_proj_weight_name,
    )?;
    let router_proj_scales_buf = optional_private_weight_buffer(
        session,
        validate_router,
        &layer_names.router_proj_scales_name,
    )?;
    let router_proj_biases_buf = optional_private_weight_buffer(
        session,
        validate_router,
        &layer_names.router_proj_biases_name,
    )?;
    let router_per_expert_scale_buf = optional_private_weight_buffer(
        session,
        validate_router,
        &layer_names.router_per_expert_scale_name,
    )?;
    let expert_gate_weight_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_gate,
        &layer_names.expert_gate_weight_name,
    )?;
    let expert_gate_scales_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_gate,
        &layer_names.expert_gate_scales_name,
    )?;
    let expert_gate_biases_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_gate,
        &layer_names.expert_gate_biases_name,
    )?;
    let expert_up_weight_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_up,
        &layer_names.expert_up_weight_name,
    )?;
    let expert_up_scales_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_up,
        &layer_names.expert_up_scales_name,
    )?;
    let expert_up_biases_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_up,
        &layer_names.expert_up_biases_name,
    )?;
    let expert_down_weight_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_down,
        &layer_names.expert_down_weight_name,
    )?;
    let expert_down_scales_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_down,
        &layer_names.expert_down_scales_name,
    )?;
    let expert_down_biases_buf = optional_private_weight_buffer(
        session,
        validate_moe_expert_down,
        &layer_names.expert_down_biases_name,
    )?;
    let attention_probs_buf =
        Some(runtime.create_buffer(q_head_count * kv_capacity * 2, BufferStorageMode::Private)?);
    let attn_out_buf = Some(runtime.create_buffer(q_out_len * 2, BufferStorageMode::Private)?);
    let o_proj_out_buf = if validate_oproj {
        Some(runtime.create_buffer(o_out_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let post_attention_norm_out_buf = if validate_residual {
        Some(runtime.create_buffer(post_attention_norm_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let residual_out_buf = if validate_residual {
        Some(runtime.create_buffer(post_attention_norm_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let pre_feedforward_norm_out_buf = if validate_pre_ffn_norm {
        Some(runtime.create_buffer(pre_feedforward_norm_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_gate_out_buf = if validate_dense_gate {
        Some(runtime.create_buffer(mlp_gate_out_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_up_out_buf = if validate_dense_up {
        Some(runtime.create_buffer(mlp_up_out_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let geglu_out_buf = if validate_dense_geglu {
        Some(runtime.create_buffer(mlp_gate_out_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let mlp_down_out_buf = if validate_dense_down {
        Some(runtime.create_buffer(mlp_down_out_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let router_scaled_out_buf = if validate_router {
        Some(runtime.create_buffer(post_attention_norm_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let router_proj_out_buf = if validate_router {
        Some(runtime.create_buffer(router_out_len * 2, BufferStorageMode::Shared)?)
    } else {
        None
    };
    let router_probs_out_buf = if validate_router {
        Some(runtime.create_buffer(router_out_len * 2, BufferStorageMode::Shared)?)
    } else {
        None
    };
    let pre_feedforward_norm2_out_buf = if validate_moe_expert_gate {
        Some(runtime.create_buffer(pre_feedforward_norm2_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let moe_top_k_indices_buf = if validate_router {
        Some(runtime.create_buffer(ROUTER_TOP_K * size_of::<u32>(), BufferStorageMode::Shared)?)
    } else {
        None
    };
    let moe_top_k_weights_buf = if validate_router {
        Some(runtime.create_buffer(ROUTER_TOP_K * size_of::<u16>(), BufferStorageMode::Shared)?)
    } else {
        None
    };
    let expert_gate_out_buf = if validate_moe_expert_gate {
        Some(runtime.create_buffer(
            ROUTER_TOP_K * expert_gate_out_len * 2,
            BufferStorageMode::Private,
        )?)
    } else {
        None
    };
    let expert_up_out_buf = if validate_moe_expert_up {
        Some(runtime.create_buffer(
            ROUTER_TOP_K * expert_up_out_len * 2,
            BufferStorageMode::Private,
        )?)
    } else {
        None
    };
    let expert_geglu_out_buf = if validate_moe_expert_geglu {
        Some(runtime.create_buffer(
            ROUTER_TOP_K * expert_gate_out_len * 2,
            BufferStorageMode::Private,
        )?)
    } else {
        None
    };
    let expert_down_out_buf = if validate_moe_expert_down {
        Some(runtime.create_buffer(
            ROUTER_TOP_K * expert_down_out_len * 2,
            BufferStorageMode::Private,
        )?)
    } else {
        None
    };
    let post_feedforward_norm1_out_buf = if validate_post_ffn_norm1 {
        Some(runtime.create_buffer(post_feedforward_norm1_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let moe_weighted_out_buf = if validate_moe_post_ffn_norm2 {
        Some(runtime.create_buffer(post_feedforward_norm2_len * 2, BufferStorageMode::Shared)?)
    } else {
        None
    };
    let moe_post_ffn_norm2_out_buf = if validate_moe_post_ffn_norm2 {
        Some(runtime.create_buffer(post_feedforward_norm2_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let moe_merge_out_buf = if validate_moe_merge {
        Some(runtime.create_buffer(post_feedforward_norm1_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };
    let post_ffn_residual_out_buf = if validate_post_ffn_residual {
        Some(runtime.create_buffer(post_feedforward_norm1_len * 2, BufferStorageMode::Private)?)
    } else {
        None
    };

    let rms_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rms_norm_row_bf16".to_string(),
        base_name: "kernel_mlx_rms_norm_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let proj_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_affine_qmv_row_bf16".to_string(),
        base_name: "kernel_mlx_affine_qmv_row_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let head_norm_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rms_norm_rows_bf16".to_string(),
        base_name: "kernel_mlx_rms_norm_rows_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let rope_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_rope_single_bf16".to_string(),
        base_name: "kernel_mlx_rope_single_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let attention_logits_seq_pipeline =
        runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_gqa_attention_logits_seq_bf16".to_string(),
            base_name: "kernel_mlx_gqa_attention_logits_seq_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?;
    let attention_softmax_pipeline = runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
        cache_name: "kernel_mlx_softmax_rows_bf16".to_string(),
        base_name: "kernel_mlx_softmax_rows_bf16".to_string(),
        constants: Vec::new(),
        smem_bytes: 0,
        nr0: 0,
        nr1: 0,
        nsg: 0,
    })?;
    let attention_weighted_sum_pipeline =
        runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_gqa_attention_weighted_sum_bf16".to_string(),
            base_name: "kernel_mlx_gqa_attention_weighted_sum_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?;
    let o_proj_fast_pipeline = if validate_oproj {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_affine_qmv_fast_row_bf16".to_string(),
            base_name: "kernel_mlx_affine_qmv_fast_row_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };
    let residual_pipeline = if validate_residual {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_add_row_bf16".to_string(),
            base_name: "kernel_mlx_add_row_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };
    let geglu_pipeline = if validate_dense_geglu || validate_moe_expert_geglu {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_geglu_row_bf16".to_string(),
            base_name: "kernel_mlx_geglu_row_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };
    let router_scale_pipeline = if validate_router {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_router_scale_bf16".to_string(),
            base_name: "kernel_mlx_router_scale_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };
    let router_topk_pipeline = if validate_router {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_router_topk_bf16".to_string(),
            base_name: "kernel_mlx_router_topk_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };
    let selected_expert_proj_pipeline = if validate_moe_expert_gate {
        Some(runtime.get_or_compile_pipeline(&MetalPipelineDescriptor {
            cache_name: "kernel_mlx_affine_qmv_selected_experts_row_bf16".to_string(),
            base_name: "kernel_mlx_affine_qmv_selected_experts_row_bf16".to_string(),
            constants: Vec::new(),
            smem_bytes: 0,
            nr0: 0,
            nr1: 0,
            nsg: 0,
        })?)
    } else {
        None
    };

    let n_reads = 4usize;
    let simd_size = 32usize;
    let rms_threadgroup_needed = NORM_LEN.div_ceil(n_reads);
    let rms_simds_needed = rms_threadgroup_needed.div_ceil(simd_size);
    let rms_threadgroup_size = simd_size * rms_simds_needed;
    let head_norm_threadgroup_needed = head_dim.div_ceil(n_reads);
    let head_norm_simds_needed = head_norm_threadgroup_needed.div_ceil(simd_size);
    let head_norm_threadgroup_size = simd_size * head_norm_simds_needed;

    let rms_args = MlxRmsNormRowArgs {
        n: NORM_LEN as u32,
        eps: EPS,
    };
    let q_proj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: q_weight_entry.shape[1] as u32,
        qparams_per_row: q_scales_entry.shape[1] as u32,
        out_rows: q_out_len as u32,
    };
    let k_proj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: k_weight_entry.shape[1] as u32,
        qparams_per_row: k_scales_entry.shape[1] as u32,
        out_rows: k_out_len as u32,
    };
    let v_proj_args = MlxAffineQprojRowArgs {
        n_in: NORM_LEN as u32,
        weight_words_per_row: v_weight_entry.shape[1] as u32,
        qparams_per_row: v_scales_entry.shape[1] as u32,
        out_rows: v_out_len as u32,
    };
    let o_proj_layout =
        if let (Some(weight_entry), Some(scales_entry)) = (o_weight_entry, o_scales_entry) {
            Some(ExactMetalQprojLayout {
                weight_words_per_row: weight_entry.shape[1] as u32,
                qparams_per_row: scales_entry.shape[1] as u32,
                out_rows: o_out_len as u32,
            })
        } else {
            None
        };
    let o_proj_args = o_proj_layout.map(|layout| layout.row_args(q_out_len as u32));
    let post_attention_norm_args = if validate_residual {
        Some(MlxRmsNormRowArgs {
            n: post_attention_norm_len as u32,
            eps: EPS,
        })
    } else {
        None
    };
    let residual_args = if validate_residual {
        Some(MlxAddRowArgs {
            n: post_attention_norm_len as u32,
        })
    } else {
        None
    };
    let pre_feedforward_norm_args = if validate_pre_ffn_norm {
        Some(MlxRmsNormRowArgs {
            n: pre_feedforward_norm_len as u32,
            eps: EPS,
        })
    } else {
        None
    };
    let router_scale_args = if validate_router {
        Some(MlxRouterScaleArgs {
            n: post_attention_norm_len as u32,
            eps: EPS,
            root_size: bf16_round_to_f32((post_attention_norm_len as f32).powf(-0.5)),
        })
    } else {
        None
    };
    let mlp_gate_args = if let (Some(weight_entry), Some(scales_entry)) =
        (mlp_gate_weight_entry, mlp_gate_scales_entry)
    {
        Some(MlxAffineQprojRowArgs {
            n_in: pre_feedforward_norm_len as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: mlp_gate_out_len as u32,
        })
    } else {
        None
    };
    let mlp_up_args = if let (Some(weight_entry), Some(scales_entry)) =
        (mlp_up_weight_entry, mlp_up_scales_entry)
    {
        Some(MlxAffineQprojRowArgs {
            n_in: pre_feedforward_norm_len as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: mlp_up_out_len as u32,
        })
    } else {
        None
    };
    let geglu_args = if validate_dense_geglu {
        Some(MlxGegluRowArgs {
            n: mlp_gate_out_len as u32,
        })
    } else {
        None
    };
    let mlp_down_args = if let (Some(weight_entry), Some(scales_entry)) =
        (mlp_down_weight_entry, mlp_down_scales_entry)
    {
        Some(MlxAffineQprojRowArgs {
            n_in: mlp_gate_out_len as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: mlp_down_out_len as u32,
        })
    } else {
        None
    };
    let router_proj_args = if let (Some(weight_entry), Some(scales_entry)) =
        (router_proj_weight_entry, router_proj_scales_entry)
    {
        Some(MlxAffineQprojRowArgs {
            n_in: post_attention_norm_len as u32,
            weight_words_per_row: weight_entry.shape[1] as u32,
            qparams_per_row: scales_entry.shape[1] as u32,
            out_rows: router_out_len as u32,
        })
    } else {
        None
    };
    let router_softmax_args = if validate_router {
        Some(MlxSoftmaxRowsArgs {
            row_stride: router_out_len as u32,
            row_count: 1,
            seq_len: router_out_len as u32,
        })
    } else {
        None
    };
    let router_topk_args = if validate_router {
        Some(MlxRouterTopKArgs {
            expert_count: router_out_len as u32,
            top_k: ROUTER_TOP_K as u32,
        })
    } else {
        None
    };
    let pre_feedforward_norm2_args = if validate_moe_expert_gate {
        Some(MlxRmsNormRowArgs {
            n: pre_feedforward_norm2_len as u32,
            eps: EPS,
        })
    } else {
        None
    };
    let expert_gate_selected_args = if let (Some(weight_entry), Some(scales_entry)) =
        (expert_gate_weight_entry, expert_gate_scales_entry)
    {
        Some(MlxAffineSelectedExpertsQprojRowArgs {
            n_in: pre_feedforward_norm2_len as u32,
            weight_words_per_row: weight_entry.shape[2] as u32,
            qparams_per_row: scales_entry.shape[2] as u32,
            out_rows: expert_gate_out_len as u32,
            input_row_stride: 0,
        })
    } else {
        None
    };
    let expert_up_selected_args = if let (Some(weight_entry), Some(scales_entry)) =
        (expert_up_weight_entry, expert_up_scales_entry)
    {
        Some(MlxAffineSelectedExpertsQprojRowArgs {
            n_in: pre_feedforward_norm2_len as u32,
            weight_words_per_row: weight_entry.shape[2] as u32,
            qparams_per_row: scales_entry.shape[2] as u32,
            out_rows: expert_up_out_len as u32,
            input_row_stride: 0,
        })
    } else {
        None
    };
    let moe_expert_geglu_args = if validate_moe_expert_geglu {
        Some(MlxGegluRowArgs {
            n: (ROUTER_TOP_K * expert_gate_out_len) as u32,
        })
    } else {
        None
    };
    let expert_down_selected_args = if let (Some(weight_entry), Some(scales_entry)) =
        (expert_down_weight_entry, expert_down_scales_entry)
    {
        Some(MlxAffineSelectedExpertsQprojRowArgs {
            n_in: expert_gate_out_len as u32,
            weight_words_per_row: weight_entry.shape[2] as u32,
            qparams_per_row: scales_entry.shape[2] as u32,
            out_rows: expert_down_out_len as u32,
            input_row_stride: expert_gate_out_len as u32,
        })
    } else {
        None
    };
    let post_ffn_norm1_args = if validate_post_ffn_norm1 {
        Some(MlxRmsNormRowArgs {
            n: post_feedforward_norm1_len as u32,
            eps: EPS,
        })
    } else {
        None
    };
    let moe_post_ffn_norm2_args = if validate_moe_post_ffn_norm2 {
        Some(MlxRmsNormRowArgs {
            n: post_feedforward_norm2_len as u32,
            eps: EPS,
        })
    } else {
        None
    };
    let q_head_norm_args = MlxRmsNormRowsArgs {
        n: head_dim as u32,
        row_stride: head_dim as u32,
        row_count: q_head_count as u32,
        eps: EPS,
    };
    let k_head_norm_args = MlxRmsNormRowsArgs {
        n: head_dim as u32,
        row_stride: head_dim as u32,
        row_count: k_head_count as u32,
        eps: EPS,
    };
    let v_head_norm_args = MlxRmsNormRowsArgs {
        n: head_dim as u32,
        row_stride: head_dim as u32,
        row_count: v_head_count as u32,
        eps: EPS,
    };

    let rms_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &x_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &input_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &h_buf,
            offset_bytes: 0,
        },
    ];
    let q_proj_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &h_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &q_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &q_scales_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &q_biases_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &q_proj_buf,
            offset_bytes: 0,
        },
    ];
    let q_head_norm_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &q_proj_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &q_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &q_norm_buf,
            offset_bytes: 0,
        },
    ];
    let k_proj_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &h_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &k_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &k_scales_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &k_biases_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &k_proj_buf,
            offset_bytes: 0,
        },
    ];
    let k_head_norm_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &k_proj_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &k_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &k_norm_buf,
            offset_bytes: 0,
        },
    ];
    let v_proj_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &h_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &v_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &v_scales_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 4,
            buffer: &v_biases_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 5,
            buffer: &v_proj_buf,
            offset_bytes: 0,
        },
    ];
    let v_head_norm_bindings = [
        MetalBufferBindingRef {
            index: 1,
            buffer: &v_proj_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 2,
            buffer: &v_norm_weight_buf,
            offset_bytes: 0,
        },
        MetalBufferBindingRef {
            index: 3,
            buffer: &v_norm_buf,
            offset_bytes: 0,
        },
    ];
    let router_scale_bindings = if let (Some(scale_buf), Some(out_buf)) = (
        router_scale_weight_buf.as_ref(),
        router_scaled_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: residual_out_buf
                    .as_ref()
                    .ok_or("missing residual output buffer for router")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: scale_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let mlp_gate_bindings =
        if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
            mlp_gate_weight_buf.as_ref(),
            mlp_gate_scales_buf.as_ref(),
            mlp_gate_biases_buf.as_ref(),
            mlp_gate_out_buf.as_ref(),
        ) {
            Some([
                MetalBufferBindingRef {
                    index: 1,
                    buffer: pre_feedforward_norm_out_buf
                        .as_ref()
                        .ok_or("missing pre-feedforward norm output buffer")?,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: weight_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: scales_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: biases_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: out_buf,
                    offset_bytes: 0,
                },
            ])
        } else {
            None
        };
    let mlp_up_bindings =
        if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
            mlp_up_weight_buf.as_ref(),
            mlp_up_scales_buf.as_ref(),
            mlp_up_biases_buf.as_ref(),
            mlp_up_out_buf.as_ref(),
        ) {
            Some([
                MetalBufferBindingRef {
                    index: 1,
                    buffer: pre_feedforward_norm_out_buf
                        .as_ref()
                        .ok_or("missing pre-feedforward norm output buffer")?,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: weight_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: scales_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: biases_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: out_buf,
                    offset_bytes: 0,
                },
            ])
        } else {
            None
        };
    let geglu_bindings = if let (Some(out_buf), Some(gate_buf), Some(up_buf)) = (
        geglu_out_buf.as_ref(),
        mlp_gate_out_buf.as_ref(),
        mlp_up_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: gate_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: up_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let mlp_down_bindings =
        if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
            mlp_down_weight_buf.as_ref(),
            mlp_down_scales_buf.as_ref(),
            mlp_down_biases_buf.as_ref(),
            mlp_down_out_buf.as_ref(),
        ) {
            Some([
                MetalBufferBindingRef {
                    index: 1,
                    buffer: geglu_out_buf
                        .as_ref()
                        .ok_or("missing geglu output buffer")?,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: weight_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: scales_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: biases_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: out_buf,
                    offset_bytes: 0,
                },
            ])
        } else {
            None
        };
    let router_proj_bindings =
        if let (Some(weight_buf), Some(scales_buf), Some(biases_buf), Some(out_buf)) = (
            router_proj_weight_buf.as_ref(),
            router_proj_scales_buf.as_ref(),
            router_proj_biases_buf.as_ref(),
            router_proj_out_buf.as_ref(),
        ) {
            Some([
                MetalBufferBindingRef {
                    index: 1,
                    buffer: router_scaled_out_buf
                        .as_ref()
                        .ok_or("missing router scaled output buffer")?,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: weight_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: scales_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: biases_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: out_buf,
                    offset_bytes: 0,
                },
            ])
        } else {
            None
        };
    let pre_feedforward_norm2_bindings = if let (Some(weight_buf), Some(out_buf)) = (
        pre_feedforward_norm2_weight_buf.as_ref(),
        pre_feedforward_norm2_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: residual_out_buf
                    .as_ref()
                    .ok_or("missing residual output buffer for pre-feedforward norm2")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let expert_gate_selected_bindings = if let (
        Some(indices_buf),
        Some(weight_buf),
        Some(scales_buf),
        Some(biases_buf),
        Some(out_buf),
    ) = (
        moe_top_k_indices_buf.as_ref(),
        expert_gate_weight_buf.as_ref(),
        expert_gate_scales_buf.as_ref(),
        expert_gate_biases_buf.as_ref(),
        expert_gate_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: pre_feedforward_norm2_out_buf
                    .as_ref()
                    .ok_or("missing pre-feedforward norm2 output buffer")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: indices_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 6,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let expert_up_selected_bindings = if let (
        Some(indices_buf),
        Some(weight_buf),
        Some(scales_buf),
        Some(biases_buf),
        Some(out_buf),
    ) = (
        moe_top_k_indices_buf.as_ref(),
        expert_up_weight_buf.as_ref(),
        expert_up_scales_buf.as_ref(),
        expert_up_biases_buf.as_ref(),
        expert_up_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: pre_feedforward_norm2_out_buf
                    .as_ref()
                    .ok_or("missing pre-feedforward norm2 output buffer")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: indices_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 6,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let moe_expert_geglu_bindings = if let (Some(out_buf), Some(gate_buf), Some(up_buf)) = (
        expert_geglu_out_buf.as_ref(),
        expert_gate_out_buf.as_ref(),
        expert_up_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: gate_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: up_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let expert_down_selected_bindings = if let (
        Some(indices_buf),
        Some(weight_buf),
        Some(scales_buf),
        Some(biases_buf),
        Some(out_buf),
    ) = (
        moe_top_k_indices_buf.as_ref(),
        expert_down_weight_buf.as_ref(),
        expert_down_scales_buf.as_ref(),
        expert_down_biases_buf.as_ref(),
        expert_down_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: expert_geglu_out_buf
                    .as_ref()
                    .ok_or("missing expert geglu output buffer")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: indices_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 6,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let post_ffn_norm1_bindings = if let (Some(weight_buf), Some(out_buf)) = (
        post_feedforward_norm1_weight_buf.as_ref(),
        post_feedforward_norm1_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: mlp_down_out_buf
                    .as_ref()
                    .ok_or("missing dense down output buffer for post-ffn norm1")?,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let moe_post_ffn_norm2_bindings = if let (Some(weight_buf), Some(out_buf), Some(weighted_buf)) = (
        post_feedforward_norm2_weight_buf.as_ref(),
        moe_post_ffn_norm2_out_buf.as_ref(),
        moe_weighted_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: weighted_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let moe_merge_bindings = if let (Some(dense_buf), Some(moe_buf), Some(out_buf)) = (
        post_feedforward_norm1_out_buf.as_ref(),
        moe_post_ffn_norm2_out_buf.as_ref(),
        moe_merge_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: dense_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: moe_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };
    let post_ffn_residual_bindings = if let (Some(base_buf), Some(merge_buf), Some(out_buf)) = (
        residual_out_buf.as_ref(),
        moe_merge_out_buf.as_ref(),
        post_ffn_residual_out_buf.as_ref(),
    ) {
        Some([
            MetalBufferBindingRef {
                index: 1,
                buffer: base_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: merge_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: out_buf,
                offset_bytes: 0,
            },
        ])
    } else {
        None
    };

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
    let q_proj_threadgroups = MetalSize {
        width: 1,
        height: (q_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let q_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let q_head_norm_threadgroups = MetalSize {
        width: q_head_count as u64,
        height: 1,
        depth: 1,
    };
    let q_head_norm_threads_per_threadgroup = MetalSize {
        width: head_norm_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let k_proj_threadgroups = MetalSize {
        width: 1,
        height: (k_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let k_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let k_head_norm_threadgroups = MetalSize {
        width: k_head_count as u64,
        height: 1,
        depth: 1,
    };
    let k_head_norm_threads_per_threadgroup = MetalSize {
        width: head_norm_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let v_proj_threadgroups = MetalSize {
        width: 1,
        height: (v_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let v_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let v_head_norm_threadgroups = MetalSize {
        width: v_head_count as u64,
        height: 1,
        depth: 1,
    };
    let v_head_norm_threads_per_threadgroup = MetalSize {
        width: head_norm_threadgroup_size as u64,
        height: 1,
        depth: 1,
    };
    let o_proj_threadgroups = MetalSize {
        width: 1,
        height: (o_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let o_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let residual_threads_per_threadgroup = MetalSize {
        width: 256,
        height: 1,
        depth: 1,
    };
    let residual_threadgroups = MetalSize {
        width: (post_attention_norm_len as u64).div_ceil(residual_threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };
    let pre_feedforward_norm_threadgroups = MetalSize {
        width: 1,
        height: 1,
        depth: 1,
    };
    let mlp_gate_threadgroups = MetalSize {
        width: 1,
        height: (mlp_gate_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let mlp_gate_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let mlp_up_threadgroups = MetalSize {
        width: 1,
        height: (mlp_up_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let mlp_up_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let geglu_threads_per_threadgroup = MetalSize {
        width: 256,
        height: 1,
        depth: 1,
    };
    let geglu_threadgroups = MetalSize {
        width: (mlp_gate_out_len as u64).div_ceil(geglu_threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };
    let mlp_down_threadgroups = MetalSize {
        width: 1,
        height: (mlp_down_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let mlp_down_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
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
        height: (router_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let router_proj_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
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
    let expert_gate_selected_threadgroups = MetalSize {
        width: ROUTER_TOP_K as u64,
        height: (expert_gate_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let expert_gate_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let expert_up_selected_threadgroups = MetalSize {
        width: ROUTER_TOP_K as u64,
        height: (expert_up_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let expert_up_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };
    let moe_expert_geglu_threadgroups = MetalSize {
        width: ((ROUTER_TOP_K * expert_gate_out_len) as u64)
            .div_ceil(geglu_threads_per_threadgroup.width),
        height: 1,
        depth: 1,
    };
    let expert_down_selected_threadgroups = MetalSize {
        width: ROUTER_TOP_K as u64,
        height: (expert_down_out_len as u64).div_ceil(8),
        depth: 1,
    };
    let expert_down_threads_per_threadgroup = MetalSize {
        width: 32,
        height: 2,
        depth: 1,
    };

    let run_projection =
        |input_words: &[u16],
         rope_offset: i32|
         -> Result<(Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>), Box<dyn Error>> {
            runtime.write_buffer(&x_buf, 0, &bytes_from_bf16_words(input_words))?;

            let q_rope_args = MlxRopeSingleArgs {
                half_dims: rope_half_dims as u32,
                row_stride: head_dim as u32,
                row_count: q_head_count as u32,
                offset: rope_offset,
                scale: ROPE_SCALE,
                base_log2: rope_base.log2(),
            };
            let k_rope_args = MlxRopeSingleArgs {
                half_dims: rope_half_dims as u32,
                row_stride: head_dim as u32,
                row_count: k_head_count as u32,
                offset: rope_offset,
                scale: ROPE_SCALE,
                base_log2: rope_base.log2(),
            };
            let q_rope_bindings = [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &q_norm_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &q_rope_buf,
                    offset_bytes: 0,
                },
            ];
            let k_rope_bindings = [
                MetalBufferBindingRef {
                    index: 1,
                    buffer: &k_norm_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: &k_rope_buf,
                    offset_bytes: 0,
                },
            ];
            let q_rope_threadgroups = MetalSize {
                width: (rope_half_dims as u64).div_ceil(32),
                height: q_head_count as u64,
                depth: 1,
            };
            let q_rope_threads_per_threadgroup = MetalSize {
                width: 32,
                height: 1,
                depth: 1,
            };
            let k_rope_threadgroups = MetalSize {
                width: (rope_half_dims as u64).div_ceil(32),
                height: k_head_count as u64,
                depth: 1,
            };
            let k_rope_threads_per_threadgroup = MetalSize {
                width: 32,
                height: 1,
                depth: 1,
            };

            runtime.begin_command_batch()?;
            runtime.dispatch_compute(
                &rms_pipeline,
                bytes_of(&rms_args),
                &rms_bindings,
                &[],
                rms_threadgroups,
                rms_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &proj_pipeline,
                bytes_of(&q_proj_args),
                &q_proj_bindings,
                &[],
                q_proj_threadgroups,
                q_proj_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &head_norm_pipeline,
                bytes_of(&q_head_norm_args),
                &q_head_norm_bindings,
                &[],
                q_head_norm_threadgroups,
                q_head_norm_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            if rope_half_dims * 2 < head_dim {
                runtime.copy_buffer_range(
                    &q_norm_buf,
                    0,
                    &q_rope_buf,
                    0,
                    q_out_len * size_of::<u16>(),
                )?;
            }
            runtime.dispatch_compute(
                &rope_pipeline,
                bytes_of(&q_rope_args),
                &q_rope_bindings,
                &[],
                q_rope_threadgroups,
                q_rope_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &proj_pipeline,
                bytes_of(&k_proj_args),
                &k_proj_bindings,
                &[],
                k_proj_threadgroups,
                k_proj_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &head_norm_pipeline,
                bytes_of(&k_head_norm_args),
                &k_head_norm_bindings,
                &[],
                k_head_norm_threadgroups,
                k_head_norm_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            if rope_half_dims * 2 < head_dim {
                runtime.copy_buffer_range(
                    &k_norm_buf,
                    0,
                    &k_rope_buf,
                    0,
                    k_out_len * size_of::<u16>(),
                )?;
            }
            runtime.dispatch_compute(
                &rope_pipeline,
                bytes_of(&k_rope_args),
                &k_rope_bindings,
                &[],
                k_rope_threadgroups,
                k_rope_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &proj_pipeline,
                bytes_of(&v_proj_args),
                &v_proj_bindings,
                &[],
                v_proj_threadgroups,
                v_proj_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &head_norm_pipeline,
                bytes_of(&v_head_norm_args),
                &v_head_norm_bindings,
                &[],
                v_head_norm_threadgroups,
                v_head_norm_threads_per_threadgroup,
            )?;
            runtime.end_command_batch()?;
            runtime.wait_idle()?;

            let input_norm_bits =
                decode_bf16_buffer_bits(&runtime.read_buffer(&h_buf, NORM_LEN * 2)?);
            let v_proj_bits =
                decode_bf16_buffer_bits(&runtime.read_buffer(&v_proj_buf, v_out_len * 2)?);
            let q_bits = decode_bf16_buffer_bits(&runtime.read_buffer(&q_rope_buf, q_out_len * 2)?);
            let k_bits = decode_bf16_buffer_bits(&runtime.read_buffer(&k_rope_buf, k_out_len * 2)?);
            let v_bits = decode_bf16_buffer_bits(&runtime.read_buffer(&v_norm_buf, v_out_len * 2)?);
            Ok((input_norm_bits, v_proj_bits, q_bits, k_bits, v_bits))
        };

    let mut kv_cache = ExactMetalKvCache::load(
        &runtime,
        GemmaKvCacheSpec::new(layer_attention_kind, 1, k_head_count, head_dim, kv_capacity)?,
    )?;
    let mut prefill_attention_cache = if validate_post_ffn_residual {
        Some(ExactMetalKvCache::load(
            &runtime,
            GemmaKvCacheSpec::new(layer_attention_kind, 1, k_head_count, head_dim, kv_capacity)?,
        )?)
    } else {
        None
    };
    let mut prefill_input_norm_bits = Vec::new();
    let mut prefill_v_proj_bits = Vec::new();
    let mut prefill_q_bits = Vec::new();
    let mut prefill_k_bits = Vec::new();
    let mut prefill_v_bits = Vec::new();
    let mut prefill_x_words = Vec::new();
    let mut prefill_attention_out_bits = None;
    for (prefill_index, input_words) in prefill_input_words_list.iter().enumerate() {
        let rope_offset = prefill_rope_offset + prefill_index as i32;
        let (
            current_input_norm_bits,
            current_v_proj_bits,
            current_q_bits,
            current_k_bits,
            current_v_bits,
        ) = run_projection(input_words, rope_offset)?;
        kv_cache.append_token_from_buffers(&runtime, &k_rope_buf, &v_norm_buf)?;
        if let Some(cache) = prefill_attention_cache.as_mut() {
            cache.append_token_from_buffers(&runtime, &k_rope_buf, &v_norm_buf)?;
            if prefill_index + 1 == prefill_input_words_list.len() {
                prefill_attention_out_bits = Some(
                    compute_cached_attention_metal(
                        &runtime,
                        &attention_logits_seq_pipeline,
                        &attention_softmax_pipeline,
                        &attention_weighted_sum_pipeline,
                        &q_rope_buf,
                        cache,
                        q_head_count,
                        q_heads_per_kv,
                        head_dim,
                        &attention_logits_buf,
                        attention_probs_buf
                            .as_ref()
                            .ok_or("missing attention probs buffer for prefill attention")?,
                        attn_out_buf
                            .as_ref()
                            .ok_or("missing attention output buffer for prefill attention")?,
                    )?
                    .2,
                );
            }
        }
        if prefill_index + 1 == prefill_input_words_list.len() {
            prefill_input_norm_bits = current_input_norm_bits;
            prefill_v_proj_bits = current_v_proj_bits;
            prefill_q_bits = current_q_bits;
            prefill_k_bits = current_k_bits;
            prefill_v_bits = current_v_bits;
            prefill_x_words = input_words.clone();
        }
    }
    let (decode_input_norm_bits, decode_v_proj_bits, decode_q_bits, decode_k_bits, decode_v_bits) =
        run_projection(&decode_x_words, decode_rope_offset)?;

    kv_cache.append_token_from_buffers(&runtime, &k_rope_buf, &v_norm_buf)?;

    let full_k_bits = read_exact_kv_cache_tensor_bits(&runtime, &kv_cache, &kv_cache.key_buffer)?;
    let full_v_bits = read_exact_kv_cache_tensor_bits(&runtime, &kv_cache, &kv_cache.value_buffer)?;
    let (attention_score_bits, attention_prob_bits, attention_out_bits) =
        compute_cached_attention_metal(
            &runtime,
            &attention_logits_seq_pipeline,
            &attention_softmax_pipeline,
            &attention_weighted_sum_pipeline,
            &q_rope_buf,
            &kv_cache,
            q_head_count,
            q_heads_per_kv,
            head_dim,
            &attention_logits_buf,
            attention_probs_buf
                .as_ref()
                .ok_or("missing attention probs buffer for decode attention")?,
            attn_out_buf
                .as_ref()
                .ok_or("missing attention output buffer for decode attention")?,
        )?;
    let attention_oproj_bits = if validate_oproj {
        let attn_out_buf = attn_out_buf
            .as_ref()
            .ok_or("missing attention output buffer")?;
        let o_proj_out_buf = o_proj_out_buf
            .as_ref()
            .ok_or("missing attention o_proj output buffer")?;
        let o_weight_buf = o_weight_buf
            .as_ref()
            .ok_or("missing o_proj weight buffer")?;
        let o_scales_buf = o_scales_buf
            .as_ref()
            .ok_or("missing o_proj scales buffer")?;
        let o_biases_buf = o_biases_buf
            .as_ref()
            .ok_or("missing o_proj biases buffer")?;
        let o_proj_fast_pipeline = o_proj_fast_pipeline
            .as_ref()
            .ok_or("missing o_proj fast pipeline")?;
        let o_proj_layout = o_proj_layout.as_ref().ok_or("missing o_proj layout")?;
        let o_proj_args = o_proj_args.as_ref().ok_or("missing o_proj args")?;
        let o_proj_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: attn_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: o_weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: o_scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: o_biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: o_proj_out_buf,
                offset_bytes: 0,
            },
        ];
        runtime.begin_command_batch()?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &proj_pipeline,
            o_proj_fast_pipeline,
            *o_proj_layout,
            o_proj_args,
            &o_proj_bindings,
            o_proj_threadgroups,
            o_proj_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(
            &runtime.read_buffer(o_proj_out_buf, o_out_len * 2)?,
        ))
    } else {
        None
    };
    let post_attention_stage_bits = if validate_residual {
        let decode_x_bytes = bytes_from_bf16_words(&decode_x_words);
        let x_buf = &x_buf;
        let o_proj_out_buf = o_proj_out_buf
            .as_ref()
            .ok_or("missing attention o_proj output buffer")?;
        let post_attention_norm_weight_buf = post_attention_norm_weight_buf
            .as_ref()
            .ok_or("missing post-attention norm weight buffer")?;
        let post_attention_norm_out_buf = post_attention_norm_out_buf
            .as_ref()
            .ok_or("missing post-attention norm output buffer")?;
        let residual_out_buf = residual_out_buf
            .as_ref()
            .ok_or("missing residual output buffer")?;
        let residual_pipeline = residual_pipeline
            .as_ref()
            .ok_or("missing residual pipeline")?;
        let post_attention_norm_args = post_attention_norm_args
            .as_ref()
            .ok_or("missing post-attention norm args")?;
        let residual_args = residual_args.as_ref().ok_or("missing residual args")?;
        let pre_feedforward_norm_weight_buf = pre_feedforward_norm_weight_buf.as_ref();
        let pre_feedforward_norm_out_buf = pre_feedforward_norm_out_buf.as_ref();
        let pre_feedforward_norm_args = pre_feedforward_norm_args.as_ref();
        let post_attention_norm_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: o_proj_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: post_attention_norm_weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: post_attention_norm_out_buf,
                offset_bytes: 0,
            },
        ];
        let residual_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: x_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: post_attention_norm_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: residual_out_buf,
                offset_bytes: 0,
            },
        ];
        let pre_feedforward_norm_bindings = if let (Some(weight_buf), Some(out_buf)) = (
            pre_feedforward_norm_weight_buf,
            pre_feedforward_norm_out_buf,
        ) {
            Some([
                MetalBufferBindingRef {
                    index: 1,
                    buffer: residual_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: weight_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: out_buf,
                    offset_bytes: 0,
                },
            ])
        } else {
            None
        };
        runtime.write_buffer(x_buf, 0, &decode_x_bytes)?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(post_attention_norm_args),
            &post_attention_norm_bindings,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            residual_pipeline,
            bytes_of(residual_args),
            &residual_bindings,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        if let (Some(args), Some(bindings)) =
            (pre_feedforward_norm_args, &pre_feedforward_norm_bindings)
        {
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &rms_pipeline,
                bytes_of(args),
                bindings,
                &[],
                pre_feedforward_norm_threadgroups,
                rms_threads_per_threadgroup,
            )?;
        }
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        let post_attention_norm_bits = decode_bf16_buffer_bits(
            &runtime.read_buffer(post_attention_norm_out_buf, post_attention_norm_len * 2)?,
        );
        let residual_bits = decode_bf16_buffer_bits(
            &runtime.read_buffer(residual_out_buf, post_attention_norm_len * 2)?,
        );
        let pre_feedforward_norm_bits = if let Some(out_buf) = pre_feedforward_norm_out_buf {
            Some(decode_bf16_buffer_bits(
                &runtime.read_buffer(out_buf, pre_feedforward_norm_len * 2)?,
            ))
        } else {
            None
        };
        Some((
            post_attention_norm_bits,
            residual_bits,
            pre_feedforward_norm_bits,
        ))
    } else {
        None
    };
    let dense_gate_bits = if validate_dense_gate {
        let mlp_gate_args = mlp_gate_args.as_ref().ok_or("missing mlp gate args")?;
        let mlp_gate_bindings = mlp_gate_bindings
            .as_ref()
            .ok_or("missing mlp gate bindings")?;
        let mlp_gate_out_buf = mlp_gate_out_buf
            .as_ref()
            .ok_or("missing mlp gate output buffer")?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_gate_args),
            mlp_gate_bindings,
            &[],
            mlp_gate_threadgroups,
            mlp_gate_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(
            &runtime.read_buffer(mlp_gate_out_buf, mlp_gate_out_len * 2)?,
        ))
    } else {
        None
    };
    let dense_up_bits = if validate_dense_up {
        let mlp_up_args = mlp_up_args.as_ref().ok_or("missing mlp up args")?;
        let mlp_up_bindings = mlp_up_bindings.as_ref().ok_or("missing mlp up bindings")?;
        let mlp_up_out_buf = mlp_up_out_buf
            .as_ref()
            .ok_or("missing mlp up output buffer")?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_up_args),
            mlp_up_bindings,
            &[],
            mlp_up_threadgroups,
            mlp_up_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(
            &runtime.read_buffer(mlp_up_out_buf, mlp_up_out_len * 2)?,
        ))
    } else {
        None
    };
    let dense_geglu_bits = if validate_dense_geglu {
        let geglu_pipeline = geglu_pipeline.as_ref().ok_or("missing geglu pipeline")?;
        let geglu_args = geglu_args.as_ref().ok_or("missing geglu args")?;
        let geglu_bindings = geglu_bindings.as_ref().ok_or("missing geglu bindings")?;
        let geglu_out_buf = geglu_out_buf
            .as_ref()
            .ok_or("missing geglu output buffer")?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            geglu_pipeline,
            bytes_of(geglu_args),
            geglu_bindings,
            &[],
            geglu_threadgroups,
            geglu_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(
            &runtime.read_buffer(geglu_out_buf, mlp_gate_out_len * 2)?,
        ))
    } else {
        None
    };
    let dense_down_bits = if validate_dense_down {
        let mlp_down_args = mlp_down_args.as_ref().ok_or("missing mlp down args")?;
        let mlp_down_bindings = mlp_down_bindings
            .as_ref()
            .ok_or("missing mlp down bindings")?;
        let mlp_down_out_buf = mlp_down_out_buf
            .as_ref()
            .ok_or("missing mlp down output buffer")?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_down_args),
            mlp_down_bindings,
            &[],
            mlp_down_threadgroups,
            mlp_down_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(
            &runtime.read_buffer(mlp_down_out_buf, mlp_down_out_len * 2)?,
        ))
    } else {
        None
    };
    let router_output = if validate_router {
        let router_scale_pipeline = router_scale_pipeline
            .as_ref()
            .ok_or("missing router scale pipeline")?;
        let router_scale_args = router_scale_args
            .as_ref()
            .ok_or("missing router scale args")?;
        let router_scale_bindings = router_scale_bindings
            .as_ref()
            .ok_or("missing router scale bindings")?;
        let router_proj_args = router_proj_args
            .as_ref()
            .ok_or("missing router proj args")?;
        let router_softmax_args = router_softmax_args
            .as_ref()
            .ok_or("missing router softmax args")?;
        let router_topk_args = router_topk_args
            .as_ref()
            .ok_or("missing router top-k args")?;
        let router_proj_bindings = router_proj_bindings
            .as_ref()
            .ok_or("missing router proj bindings")?;
        let router_topk_pipeline = router_topk_pipeline
            .as_ref()
            .ok_or("missing router top-k pipeline")?;
        let router_scaled_out_buf = router_scaled_out_buf
            .as_ref()
            .ok_or("missing router scaled output buffer")?;
        let router_proj_out_buf = router_proj_out_buf
            .as_ref()
            .ok_or("missing router proj output buffer")?;
        let router_probs_out_buf = router_probs_out_buf
            .as_ref()
            .ok_or("missing router probs output buffer")?;
        let router_per_expert_scale_buf = router_per_expert_scale_buf
            .as_ref()
            .ok_or("missing router per-expert scale buffer")?;
        let moe_top_k_indices_buf = moe_top_k_indices_buf
            .as_ref()
            .ok_or("missing moe top-k indices buffer")?;
        let moe_top_k_weights_buf = moe_top_k_weights_buf
            .as_ref()
            .ok_or("missing moe top-k weights buffer")?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            router_scale_pipeline,
            bytes_of(router_scale_args),
            router_scale_bindings,
            &[],
            router_scale_threadgroups,
            router_scale_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(router_proj_args),
            router_proj_bindings,
            &[],
            router_proj_threadgroups,
            router_proj_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &attention_softmax_pipeline,
            bytes_of(router_softmax_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: router_proj_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: router_probs_out_buf,
                    offset_bytes: 0,
                },
            ],
            &[],
            router_softmax_threadgroups,
            mlx_softmax_threads_per_threadgroup(
                router_out_len,
                attention_softmax_pipeline.max_threads_per_threadgroup,
            )?,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            router_topk_pipeline,
            bytes_of(router_topk_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: router_proj_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: router_probs_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: router_per_expert_scale_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: moe_top_k_indices_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: moe_top_k_weights_buf,
                    offset_bytes: 0,
                },
            ],
            &[],
            router_topk_threadgroups,
            router_topk_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(read_router_output_from_device(
            &runtime,
            router_scaled_out_buf,
            router_proj_out_buf,
            router_probs_out_buf,
            moe_top_k_indices_buf,
            moe_top_k_weights_buf,
            post_attention_norm_len,
            router_out_len,
            ROUTER_TOP_K,
        )?)
    } else {
        None
    };
    let (moe_expert_gate_bits, moe_expert_up_bits, moe_expert_geglu_bits, moe_expert_down_bits) =
        if validate_moe_expert_gate {
            let router_output = router_output
                .as_ref()
                .ok_or("missing router output for moe expert gate")?;
            let pre_feedforward_norm2_args = pre_feedforward_norm2_args
                .as_ref()
                .ok_or("missing pre-feedforward norm2 args")?;
            let pre_feedforward_norm2_bindings = pre_feedforward_norm2_bindings
                .as_ref()
                .ok_or("missing pre-feedforward norm2 bindings")?;
            let selected_expert_proj_pipeline = selected_expert_proj_pipeline
                .as_ref()
                .ok_or("missing selected expert projection pipeline")?;
            let expert_gate_selected_args = expert_gate_selected_args
                .as_ref()
                .ok_or("missing expert gate selected args")?;
            let expert_gate_selected_bindings = expert_gate_selected_bindings
                .as_ref()
                .ok_or("missing expert gate selected bindings")?;
            let moe_top_k_indices_buf = moe_top_k_indices_buf
                .as_ref()
                .ok_or("missing moe top-k indices buffer")?;
            let expert_gate_out_buf = expert_gate_out_buf
                .as_ref()
                .ok_or("missing expert gate output buffer")?;
            let expert_up_selected_args = expert_up_selected_args.as_ref();
            let expert_up_selected_bindings = expert_up_selected_bindings.as_ref();
            let expert_up_out_buf = expert_up_out_buf.as_ref();
            let moe_expert_geglu_args = moe_expert_geglu_args.as_ref();
            let moe_expert_geglu_bindings = moe_expert_geglu_bindings.as_ref();
            let expert_geglu_out_buf = expert_geglu_out_buf.as_ref();
            let expert_down_selected_args = expert_down_selected_args.as_ref();
            let expert_down_selected_bindings = expert_down_selected_bindings.as_ref();
            let expert_down_out_buf = expert_down_out_buf.as_ref();
            let geglu_pipeline = geglu_pipeline.as_ref();
            let mut top_k_index_bytes = Vec::with_capacity(ROUTER_TOP_K * size_of::<u32>());
            for &index in &router_output.top_k_indices {
                top_k_index_bytes.extend_from_slice(&index.to_le_bytes());
            }
            runtime.write_buffer(moe_top_k_indices_buf, 0, &top_k_index_bytes)?;
            runtime.begin_command_batch()?;
            runtime.dispatch_compute(
                &rms_pipeline,
                bytes_of(pre_feedforward_norm2_args),
                pre_feedforward_norm2_bindings,
                &[],
                rms_threadgroups,
                rms_threads_per_threadgroup,
            )?;
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                selected_expert_proj_pipeline,
                bytes_of(expert_gate_selected_args),
                expert_gate_selected_bindings,
                &[],
                expert_gate_selected_threadgroups,
                expert_gate_threads_per_threadgroup,
            )?;
            if let (Some(args), Some(bindings)) =
                (expert_up_selected_args, expert_up_selected_bindings)
            {
                runtime.memory_barrier_buffers()?;
                runtime.dispatch_compute(
                    selected_expert_proj_pipeline,
                    bytes_of(args),
                    bindings,
                    &[],
                    expert_up_selected_threadgroups,
                    expert_up_threads_per_threadgroup,
                )?;
            }
            if let (Some(pipeline), Some(args), Some(bindings)) = (
                geglu_pipeline,
                moe_expert_geglu_args,
                moe_expert_geglu_bindings,
            ) {
                runtime.memory_barrier_buffers()?;
                runtime.dispatch_compute(
                    pipeline,
                    bytes_of(args),
                    bindings,
                    &[],
                    moe_expert_geglu_threadgroups,
                    geglu_threads_per_threadgroup,
                )?;
            }
            if let (Some(args), Some(bindings)) =
                (expert_down_selected_args, expert_down_selected_bindings)
            {
                runtime.memory_barrier_buffers()?;
                runtime.dispatch_compute(
                    selected_expert_proj_pipeline,
                    bytes_of(args),
                    bindings,
                    &[],
                    expert_down_selected_threadgroups,
                    expert_down_threads_per_threadgroup,
                )?;
            }
            runtime.end_command_batch()?;
            runtime.wait_idle()?;
            let gate_bits = Some(decode_bf16_buffer_bits(
                &runtime
                    .read_buffer(expert_gate_out_buf, ROUTER_TOP_K * expert_gate_out_len * 2)?,
            ));
            let up_bits = if let Some(out_buf) = expert_up_out_buf {
                Some(decode_bf16_buffer_bits(&runtime.read_buffer(
                    out_buf,
                    ROUTER_TOP_K * expert_up_out_len * 2,
                )?))
            } else {
                None
            };
            let geglu_bits = if let Some(out_buf) = expert_geglu_out_buf {
                Some(decode_bf16_buffer_bits(&runtime.read_buffer(
                    out_buf,
                    ROUTER_TOP_K * expert_gate_out_len * 2,
                )?))
            } else {
                None
            };
            let down_bits = if let Some(out_buf) = expert_down_out_buf {
                Some(decode_bf16_buffer_bits(&runtime.read_buffer(
                    out_buf,
                    ROUTER_TOP_K * expert_down_out_len * 2,
                )?))
            } else {
                None
            };
            (gate_bits, up_bits, geglu_bits, down_bits)
        } else {
            (None, None, None, None)
        };
    let (
        post_ffn_norm1_bits,
        moe_expert_out_bits,
        moe_post_ffn_norm2_bits,
        moe_merge_bits,
        post_ffn_residual_bits,
    ) = if validate_post_ffn_norm1
        || validate_moe_expert_out
        || validate_moe_post_ffn_norm2
        || validate_moe_merge
        || validate_post_ffn_residual
    {
        let weighted_bits = if validate_moe_expert_out
            || validate_moe_post_ffn_norm2
            || validate_moe_merge
            || validate_post_ffn_residual
        {
            Some(moe_weighted_expert_out_bits(
                moe_expert_down_bits
                    .as_ref()
                    .ok_or("missing moe expert down output for weighted expert reduction")?,
                &router_output
                    .as_ref()
                    .ok_or("missing router output for weighted expert reduction")?
                    .top_k_weights_bits,
                expert_down_out_len,
            )?)
        } else {
            None
        };

        let post_ffn_norm1_args = post_ffn_norm1_args.as_ref();
        let post_ffn_norm1_bindings = post_ffn_norm1_bindings.as_ref();
        let moe_post_ffn_norm2_args = moe_post_ffn_norm2_args.as_ref();
        let moe_post_ffn_norm2_bindings = moe_post_ffn_norm2_bindings.as_ref();
        let moe_merge_bindings = moe_merge_bindings.as_ref();
        let post_ffn_residual_bindings = post_ffn_residual_bindings.as_ref();

        if let (Some(bits), Some(weighted_buf)) = (&weighted_bits, moe_weighted_out_buf.as_ref()) {
            let weighted_words = bf16_words_from_f32_bits(bits);
            let weighted_bytes = bytes_from_bf16_words(&weighted_words);
            runtime.write_buffer(weighted_buf, 0, &weighted_bytes)?;
        }

        runtime.begin_command_batch()?;
        if let (Some(args), Some(bindings)) = (post_ffn_norm1_args, post_ffn_norm1_bindings) {
            runtime.dispatch_compute(
                &rms_pipeline,
                bytes_of(args),
                bindings,
                &[],
                rms_threadgroups,
                rms_threads_per_threadgroup,
            )?;
        }
        if let (Some(args), Some(bindings)) = (moe_post_ffn_norm2_args, moe_post_ffn_norm2_bindings)
        {
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                &rms_pipeline,
                bytes_of(args),
                bindings,
                &[],
                rms_threadgroups,
                rms_threads_per_threadgroup,
            )?;
        }
        if let Some(bindings) = moe_merge_bindings {
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                residual_pipeline
                    .as_ref()
                    .ok_or("missing add pipeline for moe merge")?,
                bytes_of(
                    residual_args
                        .as_ref()
                        .ok_or("missing add args for moe merge")?,
                ),
                bindings,
                &[],
                residual_threadgroups,
                residual_threads_per_threadgroup,
            )?;
        }
        if let Some(bindings) = post_ffn_residual_bindings {
            runtime.memory_barrier_buffers()?;
            runtime.dispatch_compute(
                residual_pipeline
                    .as_ref()
                    .ok_or("missing add pipeline for post-ffn residual")?,
                bytes_of(
                    residual_args
                        .as_ref()
                        .ok_or("missing add args for post-ffn residual")?,
                ),
                bindings,
                &[],
                residual_threadgroups,
                residual_threads_per_threadgroup,
            )?;
        }
        runtime.end_command_batch()?;
        runtime.wait_idle()?;

        let post_ffn_norm1_bits = if let Some(out_buf) = post_feedforward_norm1_out_buf.as_ref() {
            Some(decode_bf16_buffer_bits(
                &runtime.read_buffer(out_buf, post_feedforward_norm1_len * 2)?,
            ))
        } else {
            None
        };
        let moe_post_ffn_norm2_bits = if let Some(out_buf) = moe_post_ffn_norm2_out_buf.as_ref() {
            Some(decode_bf16_buffer_bits(
                &runtime.read_buffer(out_buf, post_feedforward_norm2_len * 2)?,
            ))
        } else {
            None
        };
        let moe_merge_bits = if let Some(out_buf) = moe_merge_out_buf.as_ref() {
            Some(decode_bf16_buffer_bits(
                &runtime.read_buffer(out_buf, post_feedforward_norm1_len * 2)?,
            ))
        } else {
            None
        };
        let post_ffn_residual_bits = if let Some(out_buf) = post_ffn_residual_out_buf.as_ref() {
            Some(decode_bf16_buffer_bits(
                &runtime.read_buffer(out_buf, post_feedforward_norm1_len * 2)?,
            ))
        } else {
            None
        };
        (
            post_ffn_norm1_bits,
            weighted_bits,
            moe_post_ffn_norm2_bits,
            moe_merge_bits,
            post_ffn_residual_bits,
        )
    } else {
        (None, None, None, None, None)
    };
    let prefill_post_ffn_residual_bits = if let Some(prefill_attention_out_bits) =
        &prefill_attention_out_bits
    {
        let attn_out_buf = attn_out_buf
            .as_ref()
            .ok_or("missing attention output buffer for prefill tail")?;
        let o_proj_out_buf = o_proj_out_buf
            .as_ref()
            .ok_or("missing attention o_proj output buffer for prefill tail")?;
        let o_weight_buf = o_weight_buf
            .as_ref()
            .ok_or("missing o_proj weight buffer for prefill tail")?;
        let o_scales_buf = o_scales_buf
            .as_ref()
            .ok_or("missing o_proj scales buffer for prefill tail")?;
        let o_biases_buf = o_biases_buf
            .as_ref()
            .ok_or("missing o_proj biases buffer for prefill tail")?;
        let o_proj_fast_pipeline = o_proj_fast_pipeline
            .as_ref()
            .ok_or("missing o_proj fast pipeline for prefill tail")?;
        let o_proj_layout = o_proj_layout
            .as_ref()
            .ok_or("missing o_proj layout for prefill tail")?;
        let o_proj_args = o_proj_args
            .as_ref()
            .ok_or("missing o_proj args for prefill tail")?;
        let post_attention_norm_weight_buf = post_attention_norm_weight_buf
            .as_ref()
            .ok_or("missing post-attention norm weight buffer for prefill tail")?;
        let post_attention_norm_out_buf = post_attention_norm_out_buf
            .as_ref()
            .ok_or("missing post-attention norm output buffer for prefill tail")?;
        let residual_out_buf = residual_out_buf
            .as_ref()
            .ok_or("missing residual output buffer for prefill tail")?;
        let residual_pipeline = residual_pipeline
            .as_ref()
            .ok_or("missing add pipeline for prefill tail")?;
        let post_attention_norm_args = post_attention_norm_args
            .as_ref()
            .ok_or("missing post-attention norm args for prefill tail")?;
        let residual_args = residual_args
            .as_ref()
            .ok_or("missing residual args for prefill tail")?;
        let pre_feedforward_norm_weight_buf = pre_feedforward_norm_weight_buf
            .as_ref()
            .ok_or("missing pre-feedforward norm weight buffer for prefill tail")?;
        let pre_feedforward_norm_out_buf = pre_feedforward_norm_out_buf
            .as_ref()
            .ok_or("missing pre-feedforward norm output buffer for prefill tail")?;
        let pre_feedforward_norm_args = pre_feedforward_norm_args
            .as_ref()
            .ok_or("missing pre-feedforward norm args for prefill tail")?;
        let mlp_gate_args = mlp_gate_args
            .as_ref()
            .ok_or("missing mlp gate args for prefill tail")?;
        let mlp_gate_bindings = mlp_gate_bindings
            .as_ref()
            .ok_or("missing mlp gate bindings for prefill tail")?;
        let mlp_up_args = mlp_up_args
            .as_ref()
            .ok_or("missing mlp up args for prefill tail")?;
        let mlp_up_bindings = mlp_up_bindings
            .as_ref()
            .ok_or("missing mlp up bindings for prefill tail")?;
        let geglu_pipeline = geglu_pipeline
            .as_ref()
            .ok_or("missing geglu pipeline for prefill tail")?;
        let geglu_args = geglu_args
            .as_ref()
            .ok_or("missing geglu args for prefill tail")?;
        let geglu_bindings = geglu_bindings
            .as_ref()
            .ok_or("missing geglu bindings for prefill tail")?;
        let mlp_down_args = mlp_down_args
            .as_ref()
            .ok_or("missing mlp down args for prefill tail")?;
        let mlp_down_bindings = mlp_down_bindings
            .as_ref()
            .ok_or("missing mlp down bindings for prefill tail")?;
        let router_scale_pipeline = router_scale_pipeline
            .as_ref()
            .ok_or("missing router scale pipeline for prefill tail")?;
        let router_scale_args = router_scale_args
            .as_ref()
            .ok_or("missing router scale args for prefill tail")?;
        let router_scale_bindings = router_scale_bindings
            .as_ref()
            .ok_or("missing router scale bindings for prefill tail")?;
        let router_proj_args = router_proj_args
            .as_ref()
            .ok_or("missing router proj args for prefill tail")?;
        let router_proj_bindings = router_proj_bindings
            .as_ref()
            .ok_or("missing router proj bindings for prefill tail")?;
        let router_scaled_out_buf = router_scaled_out_buf
            .as_ref()
            .ok_or("missing router scaled output buffer for prefill tail")?;
        let router_proj_out_buf = router_proj_out_buf
            .as_ref()
            .ok_or("missing router proj output buffer for prefill tail")?;
        let router_probs_out_buf = router_probs_out_buf
            .as_ref()
            .ok_or("missing router probs output buffer for prefill tail")?;
        let router_per_expert_scale_buf = router_per_expert_scale_buf
            .as_ref()
            .ok_or("missing router per-expert scale buffer for prefill tail")?;
        let router_softmax_args = router_softmax_args
            .as_ref()
            .ok_or("missing router softmax args for prefill tail")?;
        let router_topk_args = router_topk_args
            .as_ref()
            .ok_or("missing router top-k args for prefill tail")?;
        let router_topk_pipeline = router_topk_pipeline
            .as_ref()
            .ok_or("missing router top-k pipeline for prefill tail")?;
        let pre_feedforward_norm2_args = pre_feedforward_norm2_args
            .as_ref()
            .ok_or("missing pre-feedforward norm2 args for prefill tail")?;
        let pre_feedforward_norm2_bindings = pre_feedforward_norm2_bindings
            .as_ref()
            .ok_or("missing pre-feedforward norm2 bindings for prefill tail")?;
        let selected_expert_proj_pipeline = selected_expert_proj_pipeline
            .as_ref()
            .ok_or("missing selected expert projection pipeline for prefill tail")?;
        let expert_gate_selected_args = expert_gate_selected_args
            .as_ref()
            .ok_or("missing expert gate args for prefill tail")?;
        let expert_gate_selected_bindings = expert_gate_selected_bindings
            .as_ref()
            .ok_or("missing expert gate bindings for prefill tail")?;
        let expert_up_selected_args = expert_up_selected_args
            .as_ref()
            .ok_or("missing expert up args for prefill tail")?;
        let expert_up_selected_bindings = expert_up_selected_bindings
            .as_ref()
            .ok_or("missing expert up bindings for prefill tail")?;
        let moe_expert_geglu_args = moe_expert_geglu_args
            .as_ref()
            .ok_or("missing expert geglu args for prefill tail")?;
        let moe_expert_geglu_bindings = moe_expert_geglu_bindings
            .as_ref()
            .ok_or("missing expert geglu bindings for prefill tail")?;
        let expert_down_selected_args = expert_down_selected_args
            .as_ref()
            .ok_or("missing expert down args for prefill tail")?;
        let expert_down_selected_bindings = expert_down_selected_bindings
            .as_ref()
            .ok_or("missing expert down bindings for prefill tail")?;
        let moe_top_k_indices_buf = moe_top_k_indices_buf
            .as_ref()
            .ok_or("missing moe top-k index buffer for prefill tail")?;
        let moe_top_k_weights_buf = moe_top_k_weights_buf
            .as_ref()
            .ok_or("missing moe top-k weight buffer for prefill tail")?;
        let expert_down_out_buf = expert_down_out_buf
            .as_ref()
            .ok_or("missing expert down output buffer for prefill tail")?;
        let post_ffn_norm1_args = post_ffn_norm1_args
            .as_ref()
            .ok_or("missing post-ffn norm1 args for prefill tail")?;
        let post_ffn_norm1_bindings = post_ffn_norm1_bindings
            .as_ref()
            .ok_or("missing post-ffn norm1 bindings for prefill tail")?;
        let moe_post_ffn_norm2_args = moe_post_ffn_norm2_args
            .as_ref()
            .ok_or("missing moe post-ffn norm2 args for prefill tail")?;
        let moe_post_ffn_norm2_bindings = moe_post_ffn_norm2_bindings
            .as_ref()
            .ok_or("missing moe post-ffn norm2 bindings for prefill tail")?;
        let moe_merge_bindings = moe_merge_bindings
            .as_ref()
            .ok_or("missing moe merge bindings for prefill tail")?;
        let post_ffn_residual_bindings = post_ffn_residual_bindings
            .as_ref()
            .ok_or("missing post-ffn residual bindings for prefill tail")?;
        let moe_weighted_out_buf = moe_weighted_out_buf
            .as_ref()
            .ok_or("missing weighted moe output buffer for prefill tail")?;
        let post_ffn_residual_out_buf = post_ffn_residual_out_buf
            .as_ref()
            .ok_or("missing post-ffn residual output buffer for prefill tail")?;

        let prefill_x_bytes = bytes_from_bf16_words(&prefill_x_words);
        let prefill_attn_out_words = bf16_words_from_f32_bits(prefill_attention_out_bits);
        let prefill_attn_out_bytes = bytes_from_bf16_words(&prefill_attn_out_words);
        let o_proj_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: attn_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: o_weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: o_scales_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 4,
                buffer: o_biases_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 5,
                buffer: o_proj_out_buf,
                offset_bytes: 0,
            },
        ];
        let post_attention_norm_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: o_proj_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: post_attention_norm_weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: post_attention_norm_out_buf,
                offset_bytes: 0,
            },
        ];
        let residual_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: &x_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: post_attention_norm_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: residual_out_buf,
                offset_bytes: 0,
            },
        ];
        let pre_feedforward_norm_bindings = [
            MetalBufferBindingRef {
                index: 1,
                buffer: residual_out_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 2,
                buffer: pre_feedforward_norm_weight_buf,
                offset_bytes: 0,
            },
            MetalBufferBindingRef {
                index: 3,
                buffer: pre_feedforward_norm_out_buf,
                offset_bytes: 0,
            },
        ];

        runtime.write_buffer(&x_buf, 0, &prefill_x_bytes)?;
        runtime.write_buffer(attn_out_buf, 0, &prefill_attn_out_bytes)?;
        runtime.begin_command_batch()?;
        dispatch_exact_mlx_qmv_row(
            &runtime,
            &proj_pipeline,
            o_proj_fast_pipeline,
            *o_proj_layout,
            o_proj_args,
            &o_proj_bindings,
            o_proj_threadgroups,
            o_proj_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(post_attention_norm_args),
            &post_attention_norm_bindings,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            residual_pipeline,
            bytes_of(residual_args),
            &residual_bindings,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(pre_feedforward_norm_args),
            &pre_feedforward_norm_bindings,
            &[],
            pre_feedforward_norm_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_gate_args),
            mlp_gate_bindings,
            &[],
            mlp_gate_threadgroups,
            mlp_gate_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_up_args),
            mlp_up_bindings,
            &[],
            mlp_up_threadgroups,
            mlp_up_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            geglu_pipeline,
            bytes_of(geglu_args),
            geglu_bindings,
            &[],
            geglu_threadgroups,
            geglu_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(mlp_down_args),
            mlp_down_bindings,
            &[],
            mlp_down_threadgroups,
            mlp_down_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            router_scale_pipeline,
            bytes_of(router_scale_args),
            router_scale_bindings,
            &[],
            router_scale_threadgroups,
            router_scale_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &proj_pipeline,
            bytes_of(router_proj_args),
            router_proj_bindings,
            &[],
            router_proj_threadgroups,
            router_proj_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &attention_softmax_pipeline,
            bytes_of(router_softmax_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: router_proj_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: router_probs_out_buf,
                    offset_bytes: 0,
                },
            ],
            &[],
            router_softmax_threadgroups,
            mlx_softmax_threads_per_threadgroup(
                router_out_len,
                attention_softmax_pipeline.max_threads_per_threadgroup,
            )?,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            router_topk_pipeline,
            bytes_of(router_topk_args),
            &[
                MetalBufferBindingRef {
                    index: 1,
                    buffer: router_proj_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 2,
                    buffer: router_probs_out_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 3,
                    buffer: router_per_expert_scale_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 4,
                    buffer: moe_top_k_indices_buf,
                    offset_bytes: 0,
                },
                MetalBufferBindingRef {
                    index: 5,
                    buffer: moe_top_k_weights_buf,
                    offset_bytes: 0,
                },
            ],
            &[],
            router_topk_threadgroups,
            router_topk_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;

        let router_output = read_router_output_from_device(
            &runtime,
            router_scaled_out_buf,
            router_proj_out_buf,
            router_probs_out_buf,
            moe_top_k_indices_buf,
            moe_top_k_weights_buf,
            post_attention_norm_len,
            router_out_len,
            ROUTER_TOP_K,
        )?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(pre_feedforward_norm2_args),
            pre_feedforward_norm2_bindings,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            selected_expert_proj_pipeline,
            bytes_of(expert_gate_selected_args),
            expert_gate_selected_bindings,
            &[],
            expert_gate_selected_threadgroups,
            expert_gate_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            selected_expert_proj_pipeline,
            bytes_of(expert_up_selected_args),
            expert_up_selected_bindings,
            &[],
            expert_up_selected_threadgroups,
            expert_up_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            geglu_pipeline,
            bytes_of(moe_expert_geglu_args),
            moe_expert_geglu_bindings,
            &[],
            moe_expert_geglu_threadgroups,
            geglu_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            selected_expert_proj_pipeline,
            bytes_of(expert_down_selected_args),
            expert_down_selected_bindings,
            &[],
            expert_down_selected_threadgroups,
            expert_down_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;

        let expert_down_bits = decode_bf16_buffer_bits(
            &runtime.read_buffer(expert_down_out_buf, ROUTER_TOP_K * expert_down_out_len * 2)?,
        );
        let weighted_bits = moe_weighted_expert_out_bits(
            &expert_down_bits,
            &router_output.top_k_weights_bits,
            expert_down_out_len,
        )?;
        let weighted_words = bf16_words_from_f32_bits(&weighted_bits);
        let weighted_bytes = bytes_from_bf16_words(&weighted_words);
        runtime.write_buffer(moe_weighted_out_buf, 0, &weighted_bytes)?;
        runtime.begin_command_batch()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(post_ffn_norm1_args),
            post_ffn_norm1_bindings,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            &rms_pipeline,
            bytes_of(moe_post_ffn_norm2_args),
            moe_post_ffn_norm2_bindings,
            &[],
            rms_threadgroups,
            rms_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            residual_pipeline,
            bytes_of(residual_args),
            moe_merge_bindings,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        runtime.memory_barrier_buffers()?;
        runtime.dispatch_compute(
            residual_pipeline,
            bytes_of(residual_args),
            post_ffn_residual_bindings,
            &[],
            residual_threadgroups,
            residual_threads_per_threadgroup,
        )?;
        runtime.end_command_batch()?;
        runtime.wait_idle()?;
        Some(decode_bf16_buffer_bits(&runtime.read_buffer(
            post_ffn_residual_out_buf,
            post_feedforward_norm1_len * 2,
        )?))
    } else {
        None
    };

    if layer_idx == 0 && validate_against_oracle && prefill_input_words_list.len() == 1 {
        validate_hash_and_prefix(
            "prefill_k_cache",
            &prefill_k_bits,
            EXPECTED_PREFILL_K_CACHE_HASH,
            &EXPECTED_PREFILL_K_CACHE_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "prefill_v_cache",
            &prefill_v_bits,
            EXPECTED_PREFILL_V_CACHE_HASH,
            &EXPECTED_PREFILL_V_CACHE_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "decode_q_rope",
            &decode_q_bits,
            EXPECTED_DECODE_Q_ROPE_HASH,
            &EXPECTED_DECODE_Q_ROPE_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "decode_k_rope",
            &decode_k_bits,
            EXPECTED_DECODE_K_ROPE_HASH,
            &EXPECTED_DECODE_K_ROPE_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "decode_v_norm",
            &decode_v_bits,
            EXPECTED_DECODE_V_NORM_HASH,
            &EXPECTED_DECODE_V_NORM_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "full_k_cache",
            &full_k_bits,
            EXPECTED_FULL_K_CACHE_HASH,
            &EXPECTED_FULL_K_CACHE_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "full_v_cache",
            &full_v_bits,
            EXPECTED_FULL_V_CACHE_HASH,
            &EXPECTED_FULL_V_CACHE_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "attention_scores",
            &attention_score_bits,
            EXPECTED_ATTENTION_SCORES_HASH,
            &EXPECTED_ATTENTION_SCORES_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "attention_probs",
            &attention_prob_bits,
            EXPECTED_ATTENTION_PROBS_HASH,
            &EXPECTED_ATTENTION_PROBS_FIRST16_BITS,
        )?;
        validate_hash_and_prefix(
            "attention_output",
            &attention_out_bits,
            EXPECTED_ATTENTION_OUTPUT_HASH,
            &EXPECTED_ATTENTION_OUTPUT_FIRST16_BITS,
        )?;
        if let Some(attention_oproj_bits) = &attention_oproj_bits {
            validate_hash_and_prefix(
                "attention_oproj",
                attention_oproj_bits,
                EXPECTED_ATTENTION_OPROJ_HASH,
                &EXPECTED_ATTENTION_OPROJ_FIRST16_BITS,
            )?;
        }
        if let Some((post_attention_norm_bits, residual_bits, pre_feedforward_norm_bits)) =
            &post_attention_stage_bits
        {
            validate_hash_and_prefix(
                "attention_post_attn_norm",
                post_attention_norm_bits,
                EXPECTED_POST_ATTENTION_NORM_HASH,
                &EXPECTED_POST_ATTENTION_NORM_FIRST16_BITS,
            )?;
            validate_hash_and_prefix(
                "attention_post_attn_residual",
                residual_bits,
                EXPECTED_POST_ATTENTION_RESIDUAL_HASH,
                &EXPECTED_POST_ATTENTION_RESIDUAL_FIRST16_BITS,
            )?;
            if let Some(pre_feedforward_norm_bits) = pre_feedforward_norm_bits {
                validate_hash_and_prefix(
                    "attention_pre_ffn_norm",
                    pre_feedforward_norm_bits,
                    EXPECTED_PRE_FEEDFORWARD_NORM_HASH,
                    &EXPECTED_PRE_FEEDFORWARD_NORM_FIRST16_BITS,
                )?;
            }
        }
        if let Some(dense_gate_bits) = &dense_gate_bits {
            validate_hash_and_prefix(
                "attention_pre_ffn_gate",
                dense_gate_bits,
                EXPECTED_PRE_FEEDFORWARD_GATE_HASH,
                &EXPECTED_PRE_FEEDFORWARD_GATE_FIRST16_BITS,
            )?;
        }
        if let Some(dense_up_bits) = &dense_up_bits {
            validate_hash_and_prefix(
                "attention_pre_ffn_up",
                dense_up_bits,
                EXPECTED_PRE_FEEDFORWARD_UP_HASH,
                &EXPECTED_PRE_FEEDFORWARD_UP_FIRST16_BITS,
            )?;
        }
        if let Some(dense_geglu_bits) = &dense_geglu_bits {
            validate_hash_and_prefix(
                "attention_pre_ffn_geglu",
                dense_geglu_bits,
                EXPECTED_PRE_FEEDFORWARD_GEGLU_HASH,
                &EXPECTED_PRE_FEEDFORWARD_GEGLU_FIRST16_BITS,
            )?;
        }
        if let Some(dense_down_bits) = &dense_down_bits {
            validate_hash_and_prefix(
                "attention_pre_ffn_down",
                dense_down_bits,
                EXPECTED_PRE_FEEDFORWARD_DOWN_HASH,
                &EXPECTED_PRE_FEEDFORWARD_DOWN_FIRST16_BITS,
            )?;
        }
        if let Some(router_output) = &router_output {
            validate_hash_and_prefix(
                "router_scaled",
                &router_output.router_scaled_bits,
                EXPECTED_ROUTER_SCALED_HASH,
                &EXPECTED_ROUTER_SCALED_FIRST16_BITS,
            )?;
            validate_hash_and_prefix(
                "router_expert_scores",
                &router_output.expert_scores_bits,
                EXPECTED_ROUTER_EXPERT_SCORES_HASH,
                &EXPECTED_ROUTER_EXPERT_SCORES_FIRST16_BITS,
            )?;
            validate_hash_and_prefix(
                "router_probs",
                &router_output.router_probs_bits,
                EXPECTED_ROUTER_PROBS_HASH,
                &EXPECTED_ROUTER_PROBS_FIRST16_BITS,
            )?;
            validate_hash_and_prefix(
                "router_topk_indices",
                &router_output.top_k_indices,
                EXPECTED_ROUTER_TOPK_INDICES_HASH,
                &EXPECTED_ROUTER_TOPK_INDICES,
            )?;
            validate_hash_and_prefix(
                "router_topk_weights",
                &router_output.top_k_weights_bits,
                EXPECTED_ROUTER_TOPK_WEIGHTS_HASH,
                &EXPECTED_ROUTER_TOPK_WEIGHTS_FIRST8_BITS,
            )?;
        }
        if let Some(moe_expert_gate_bits) = &moe_expert_gate_bits {
            validate_hash_and_prefix(
                "attention_moe_expert_gate",
                moe_expert_gate_bits,
                EXPECTED_MOE_EXPERT_GATE_HASH,
                &EXPECTED_MOE_EXPERT_GATE_FIRST16_BITS,
            )?;
        }
        if let Some(moe_expert_up_bits) = &moe_expert_up_bits {
            validate_hash_and_prefix(
                "attention_moe_expert_up",
                moe_expert_up_bits,
                EXPECTED_MOE_EXPERT_UP_HASH,
                &EXPECTED_MOE_EXPERT_UP_FIRST16_BITS,
            )?;
        }
        if let Some(moe_expert_geglu_bits) = &moe_expert_geglu_bits {
            validate_hash_and_prefix(
                "attention_moe_expert_geglu",
                moe_expert_geglu_bits,
                EXPECTED_MOE_EXPERT_GEGLU_HASH,
                &EXPECTED_MOE_EXPERT_GEGLU_FIRST16_BITS,
            )?;
        }
        if let Some(moe_expert_down_bits) = &moe_expert_down_bits {
            validate_hash_and_prefix(
                "attention_moe_expert_down",
                moe_expert_down_bits,
                EXPECTED_MOE_EXPERT_DOWN_HASH,
                &EXPECTED_MOE_EXPERT_DOWN_FIRST16_BITS,
            )?;
        }
        if let Some(post_ffn_norm1_bits) = &post_ffn_norm1_bits {
            validate_hash_and_prefix(
                "attention_post_ffn_norm1",
                post_ffn_norm1_bits,
                EXPECTED_POST_FFN_NORM1_HASH,
                &EXPECTED_POST_FFN_NORM1_FIRST16_BITS,
            )?;
        }
        if let Some(moe_expert_out_bits) = &moe_expert_out_bits {
            validate_hash_and_prefix(
                "attention_moe_expert_out",
                moe_expert_out_bits,
                EXPECTED_MOE_EXPERT_OUT_HASH,
                &EXPECTED_MOE_EXPERT_OUT_FIRST16_BITS,
            )?;
        }
        if let Some(moe_post_ffn_norm2_bits) = &moe_post_ffn_norm2_bits {
            validate_hash_and_prefix(
                "attention_moe_post_ffn_norm2",
                moe_post_ffn_norm2_bits,
                EXPECTED_MOE_POST_FFN_NORM2_HASH,
                &EXPECTED_MOE_POST_FFN_NORM2_FIRST16_BITS,
            )?;
        }
        if let Some(moe_merge_bits) = &moe_merge_bits {
            validate_hash_and_prefix(
                "attention_moe_merge",
                moe_merge_bits,
                EXPECTED_MOE_MERGE_HASH,
                &EXPECTED_MOE_MERGE_FIRST16_BITS,
            )?;
        }
        if let Some(post_ffn_residual_bits) = &post_ffn_residual_bits {
            validate_hash_and_prefix(
                "attention_post_ffn_residual",
                post_ffn_residual_bits,
                EXPECTED_POST_FFN_RESIDUAL_HASH,
                &EXPECTED_POST_FFN_RESIDUAL_FIRST16_BITS,
            )?;
        }
    }

    let (post_attention_norm_bits, post_attention_residual_bits, pre_feedforward_norm_bits) =
        match post_attention_stage_bits {
            Some((post_attention_norm_bits, residual_bits, pre_feedforward_norm_bits)) => (
                Some(post_attention_norm_bits),
                Some(residual_bits),
                pre_feedforward_norm_bits,
            ),
            None => (None, None, None),
        };

    Ok(Layer0CachedArtifacts {
        backend_name: runtime.backend_info().name.to_string(),
        model_path,
        layer_idx,
        selected_stage: plan.display_stage(),
        prefill_rope_offset,
        decode_rope_offset,
        q_head_count,
        k_head_count,
        v_head_count,
        q_heads_per_kv,
        head_dim,
        prefill_input_norm_bits,
        prefill_v_proj_bits,
        prefill_q_bits,
        prefill_k_bits,
        prefill_v_bits,
        decode_input_norm_bits,
        decode_v_proj_bits,
        decode_q_bits,
        decode_k_bits,
        decode_v_bits,
        full_k_bits,
        full_v_bits,
        attention_score_bits,
        attention_prob_bits,
        attention_out_bits,
        attention_oproj_bits,
        post_attention_norm_bits,
        post_attention_residual_bits,
        pre_feedforward_norm_bits,
        dense_gate_bits,
        dense_up_bits,
        dense_geglu_bits,
        dense_down_bits,
        router_output,
        moe_expert_gate_bits,
        moe_expert_up_bits,
        moe_expert_geglu_bits,
        moe_expert_down_bits,
        post_ffn_norm1_bits,
        moe_expert_out_bits,
        moe_post_ffn_norm2_bits,
        moe_merge_bits,
        prefill_post_ffn_residual_bits,
        post_ffn_residual_bits,
    })
}

pub fn run_layer_sequence(
    model_path: PathBuf,
    layer_indices: &[usize],
    plan: Layer0CachedPlan,
) -> Result<Vec<Layer0CachedArtifacts>, Box<dyn Error>> {
    run_layer_sequence_from_inputs(
        model_path,
        layer_indices,
        CachedLayerInputs::synthetic_case(),
        plan,
    )
}

pub fn run_layer_sequence_from_inputs(
    model_path: PathBuf,
    layer_indices: &[usize],
    mut inputs: CachedLayerInputs,
    plan: Layer0CachedPlan,
) -> Result<Vec<Layer0CachedArtifacts>, Box<dyn Error>> {
    let mut session = LayerExecutionSession::load(model_path)?;
    let mut outputs = Vec::with_capacity(layer_indices.len());
    for &layer_idx in layer_indices {
        let artifacts = run_layer_plan_with_session(&mut session, layer_idx, inputs, plan)?;
        let next_prefill = artifacts
            .prefill_layer_output_bf16_words()
            .ok_or("layer sequence requires prefill post-ffn residual output")?;
        let next_decode = artifacts
            .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
            .ok_or("layer sequence requires decode post-ffn residual output")?;
        inputs = CachedLayerInputs {
            prefill_input_words: next_prefill,
            decode_input_words: next_decode,
            prefill_rope_offset: artifacts.prefill_rope_offset,
            decode_rope_offset: artifacts.decode_rope_offset,
            validate_against_oracle: false,
        };
        outputs.push(artifacts);
    }
    Ok(outputs)
}

