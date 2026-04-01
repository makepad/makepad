use std::time::Instant;

use makepad_ggml::{
    backend::metal::{
        compile_prepared_graph, execute_compiled_graph, is_available as metal_available,
        BufferStorageMode, MetalDeviceFeatures, MetalGraphTensorWrite, MetalRuntime,
    },
    TensorType,
};
use makepad_llama::{
    compile_attention_block_metal, compile_attention_decode_metal,
    compile_delta_net_recurrent_decode_metal, compile_logits_probe_metal,
    compile_hybrid_decode_metal, compile_moe_ffn_metal,
    execute_attention_block_graph_metal_cached, execute_attention_decode_graph_metal_cached,
    execute_delta_net_recurrent_decode_graph_metal_cached,
    execute_hybrid_decode_graph_metal_cached,
    execute_moe_ffn_graph_metal_cached,
    execute_logits_probe_graph_metal, execute_logits_probe_graph_metal_cached,
    execute_logits_probe_metal, prepare_attention_block_graph, prepare_attention_decode_graph,
    prepare_delta_net_recurrent_decode_graph, prepare_hybrid_decode_graph,
    prepare_logits_probe_graph, prepare_moe_ffn_graph,
    qwen35moe_attention_block_layout, qwen35moe_attention_decode_spec,
    qwen35moe_delta_net_recurrent_decode_spec, qwen35moe_first_attention_block_spec,
    qwen35moe_first_moe_ffn_spec, qwen35moe_first_recurrent_block_spec,
    qwen35moe_hybrid_decode_spec, qwen35moe_moe_ffn_layout, qwen35moe_recurrent_block_layout, GraphBatch,
    HybridCacheLayout, HybridCacheShape, HybridCacheTypes, LlamaModel, LogitsProbeInput,
    ModelLayerRole,
};

fn main() {
    let mut args = std::env::args();
    let _exe = args.next();
    let path = match args.next() {
        Some(path) => path,
        None => {
            eprintln!("usage: llama-load <model.gguf>");
            std::process::exit(2);
        }
    };

    match run(&path) {
        Ok(()) => {}
        Err(err) => {
            eprintln!("llama-load failed: {}", err);
            std::process::exit(1);
        }
    }
}

fn run(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let model = LlamaModel::load(path)?;
    model.validate_layout()?;
    let plan = model.execution_plan()?;
    let qwen35moe = model.qwen35moe_tensors()?;
    let layout = qwen35moe.weight_layout()?;
    let (attention_layer, attention_spec) = qwen35moe_first_attention_block_spec(&model)?;
    let (moe_layer, moe_spec) = qwen35moe_first_moe_ffn_spec(&model)?;
    let (recurrent_layer, _recurrent_spec) = qwen35moe_first_recurrent_block_spec(&model)?;
    let attention_decode_spec = qwen35moe_attention_decode_spec(
        &model,
        attention_layer,
        8,
        1,
        TensorType::F16,
        TensorType::F16,
    )?;
    let recurrent_decode_spec = qwen35moe_delta_net_recurrent_decode_spec(
        &model,
        recurrent_layer,
        1,
        TensorType::F32,
        TensorType::F32,
    )?;
    let hybrid_decode_spec = qwen35moe_hybrid_decode_spec(
        &model,
        8,
        1,
        TensorType::F16,
        TensorType::F16,
        TensorType::F32,
        TensorType::F32,
    )?;
    let attention_layout = qwen35moe_attention_block_layout(&model, attention_layer)?;
    let moe_layout = qwen35moe_moe_ffn_layout(&model, moe_layer)?;
    let recurrent_layout = qwen35moe_recurrent_block_layout(&model, recurrent_layer)?;
    let probe_layout = qwen35moe_probe_layout(&qwen35moe)?;
    let probe_loaded = probe_layout.allocate_and_load(&model.gguf)?;
    let cache_layout = plan.hybrid_cache.as_ref().map(|template| {
        HybridCacheLayout::new(template.materialize(
            HybridCacheShape {
                n_ctx_seq: 4096,
                n_seq_max: 8,
            },
            HybridCacheTypes {
                attention_k_type: TensorType::F16,
                attention_v_type: TensorType::F16,
                recurrent_r_type: TensorType::F32,
                recurrent_s_type: TensorType::F32,
            },
        ))
    }).transpose()?;
    let hybrid_decode_cache_layout = plan
        .hybrid_cache
        .as_ref()
        .map(|template| {
            HybridCacheLayout::new(template.materialize(
                HybridCacheShape {
                    n_ctx_seq: 8,
                    n_seq_max: 1,
                },
                HybridCacheTypes {
                    attention_k_type: TensorType::F16,
                    attention_v_type: TensorType::F16,
                    recurrent_r_type: TensorType::F32,
                    recurrent_s_type: TensorType::F32,
                },
            ))
        })
        .transpose()?;
    let mut tail_probe_loaded = plan
        .tail_probe
        .weights
        .allocate_and_load_with_extra(&model.gguf, plan.tail_probe.extra_activation_bytes)?;
    let (tail_probe_graph, tail_probe_prepared) = prepare_logits_probe_graph(
        &mut tail_probe_loaded.ctx,
        &tail_probe_loaded.tensor_ids,
        &plan.tail_probe.spec,
        GraphBatch {
            n_tokens: 1,
            n_outputs: 1,
        },
        MetalDeviceFeatures::default(),
    )
    .map_err(|err| std::io::Error::other(format!("prepare_logits_probe_graph: {err}")))?;
    let mut attention_loaded = attention_layout.allocate_and_load_with_extra(&model.gguf, 32 << 20)?;
    let (attention_graph, attention_prepared) = prepare_attention_block_graph(
        &mut attention_loaded.ctx,
        &attention_loaded.tensor_ids,
        &attention_spec,
        1,
        MetalDeviceFeatures::default(),
    )
    .map_err(|err| std::io::Error::other(format!("prepare_attention_block_graph: {err}")))?;
    let mut attention_decode_loaded =
        attention_layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let (attention_decode_graph, attention_decode_prepared) = prepare_attention_decode_graph(
        &mut attention_decode_loaded.ctx,
        &attention_decode_loaded.tensor_ids,
        &attention_decode_spec,
        1,
        MetalDeviceFeatures::default(),
    )
    .map_err(|err| std::io::Error::other(format!("prepare_attention_decode_graph: {err}")))?;
    let mut recurrent_decode_loaded =
        recurrent_layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let (recurrent_decode_graph, recurrent_decode_prepared) =
        prepare_delta_net_recurrent_decode_graph(
            &mut recurrent_decode_loaded.ctx,
            &recurrent_decode_loaded.tensor_ids,
            &recurrent_decode_spec,
            1,
            MetalDeviceFeatures::default(),
        )
        .map_err(|err| {
            std::io::Error::other(format!("prepare_delta_net_recurrent_decode_graph: {err}"))
        })?;
    let mut moe_loaded = moe_layout.allocate_and_load_with_extra(&model.gguf, 64 << 20)?;
    let (moe_graph, moe_prepared) = prepare_moe_ffn_graph(
        &mut moe_loaded.ctx,
        &moe_loaded.tensor_ids,
        &moe_spec,
        1,
        MetalDeviceFeatures::default(),
    )
    .map_err(|err| std::io::Error::other(format!("prepare_moe_ffn_graph: {err}")))?;

    println!("path: {}", model.gguf.path.display());
    println!("version: {}", model.gguf.version);
    println!("alignment: {}", model.gguf.alignment);
    println!("data_offset: {}", model.gguf.data_offset);
    println!("file_size: {}", model.gguf.file_size);
    println!("n_kv: {}", model.gguf.kv.len());
    println!("n_tensors: {}", model.gguf.tensors.len());
    println!("architecture: {}", model.architecture.name());
    if let Some(name) = &model.general.name {
        println!("general.name: {}", name);
    }
    if let Some(model_type) = &model.general.model_type {
        println!("general.type: {}", model_type);
    }
    if let Some(file_type) = model.general.file_type {
        println!("general.file_type: {}", file_type);
    }
    if let Some(qv) = model.general.quantization_version {
        println!("general.quantization_version: {}", qv);
    }

    if let Some(cfg) = &model.qwen35moe {
        println!("qwen35moe.block_count: {}", cfg.block_count);
        println!("qwen35moe.context_length: {}", cfg.context_length);
        println!("qwen35moe.embedding_length: {}", cfg.embedding_length);
        println!(
            "qwen35moe.attention.head_count: {}",
            cfg.attention_head_count
        );
        println!(
            "qwen35moe.attention.head_count_kv: {}",
            cfg.attention_head_count_kv
        );
        println!(
            "qwen35moe.rope.dimension_sections: {:?}",
            cfg.rope_dimension_sections
        );
        println!("qwen35moe.rope.freq_base: {}", cfg.rope_freq_base);
        println!("qwen35moe.expert_count: {}", cfg.expert_count);
        println!("qwen35moe.expert_used_count: {}", cfg.expert_used_count);
        println!(
            "qwen35moe.full_attention_interval: {}",
            cfg.full_attention_interval
        );
    }
    println!("qwen35moe.unique_tensors: {}", qwen35moe.unique_tensor_count());
    println!("qwen35moe.total_tensor_bytes: {}", qwen35moe.total_tensor_bytes());
    println!("qwen35moe.layout_total_bytes: {}", layout.total_bytes);
    println!(
        "execution.inventory_unique_tensors: {}",
        plan.inventory.unique_tensor_count()
    );
    println!(
        "execution.inventory_total_tensor_bytes: {}",
        plan.inventory.total_tensor_bytes()
    );
    println!("execution.full_weight_bytes: {}", plan.full_weights.total_bytes);
    println!("execution.layer_count: {}", plan.layer_count());
    println!(
        "execution.attention_layers: {}",
        plan.inventory.count_layers_with_role(ModelLayerRole::Attention)
    );
    println!(
        "execution.recurrent_layers: {}",
        plan.inventory.count_layers_with_role(ModelLayerRole::Recurrent)
    );
    println!("qwen35moe.probe_layout_total_bytes: {}", probe_layout.total_bytes);
    println!("qwen35moe.probe_tensors: {}", probe_loaded.tensor_ids.len());
    println!("qwen35moe.attention_block_layer: {}", attention_layer);
    println!(
        "qwen35moe.attention_block_weight_bytes: {}",
        attention_layout.total_bytes
    );
    println!(
        "qwen35moe.attention_block_tensors: {}",
        attention_loaded.tensor_ids.len()
    );
    println!("qwen35moe.recurrent_block_layer: {}", recurrent_layer);
    println!(
        "qwen35moe.recurrent_block_weight_bytes: {}",
        recurrent_layout.total_bytes
    );
    println!(
        "qwen35moe.recurrent_block_tensors: {}",
        recurrent_decode_loaded.tensor_ids.len()
    );
    println!("qwen35moe.moe_ffn_layer: {}", moe_layer);
    println!("qwen35moe.moe_ffn_weight_bytes: {}", moe_layout.total_bytes);
    println!("qwen35moe.moe_ffn_tensors: {}", moe_loaded.tensor_ids.len());
    if let Some(cache_layout) = &cache_layout {
        println!("qwen35moe.cache_total_bytes: {}", cache_layout.total_bytes);
        println!(
            "qwen35moe.cache_attention_layers: {}",
            cache_layout.spec.attention_layers.len()
        );
        println!(
            "qwen35moe.cache_recurrent_layers: {}",
            cache_layout.spec.recurrent_layers.len()
        );
    }
    if let Some(cache_layout) = &hybrid_decode_cache_layout {
        println!(
            "qwen35moe.hybrid_decode_cache_total_bytes: {}",
            cache_layout.total_bytes
        );
    }
    println!(
        "qwen35moe.tail_probe_weight_bytes: {}",
        tail_probe_loaded.ctx.used_mem()
    );
    println!(
        "qwen35moe.tail_probe_nodes: {}",
        tail_probe_graph.graph.n_nodes()
    );
    println!(
        "qwen35moe.tail_probe_main_buffer_size: {}",
        tail_probe_prepared.main_buffer_size
    );
    println!(
        "qwen35moe.tail_probe_tail_buffer_size: {}",
        tail_probe_prepared.tail_buffer_size
    );
    println!(
        "qwen35moe.attention_block_nodes: {}",
        attention_graph.graph.n_nodes()
    );
    println!(
        "qwen35moe.attention_block_main_buffer_size: {}",
        attention_prepared.main_buffer_size
    );
    println!(
        "qwen35moe.attention_block_tail_buffer_size: {}",
        attention_prepared.tail_buffer_size
    );
    println!(
        "qwen35moe.attention_decode_nodes: {}",
        attention_decode_graph.graph.n_nodes()
    );
    println!(
        "qwen35moe.attention_decode_main_buffer_size: {}",
        attention_decode_prepared.main_buffer_size
    );
    println!(
        "qwen35moe.attention_decode_tail_buffer_size: {}",
        attention_decode_prepared.tail_buffer_size
    );
    println!(
        "qwen35moe.recurrent_decode_nodes: {}",
        recurrent_decode_graph.graph.n_nodes()
    );
    println!(
        "qwen35moe.recurrent_decode_main_buffer_size: {}",
        recurrent_decode_prepared.main_buffer_size
    );
    println!(
        "qwen35moe.recurrent_decode_tail_buffer_size: {}",
        recurrent_decode_prepared.tail_buffer_size
    );
    println!("qwen35moe.moe_ffn_nodes: {}", moe_graph.graph.n_nodes());
    println!(
        "qwen35moe.moe_ffn_main_buffer_size: {}",
        moe_prepared.main_buffer_size
    );
    println!(
        "qwen35moe.moe_ffn_tail_buffer_size: {}",
        moe_prepared.tail_buffer_size
    );
    println!("metal.compat_available: {}", metal_available());
    println!("qwen35moe.layer_count: {}", qwen35moe.layers.len());
    let attention_layers = qwen35moe
        .layers
        .iter()
        .filter(|layer| layer.kind.name() == "attention")
        .count();
    let recurrent_layers = qwen35moe.layers.len() - attention_layers;
    println!("qwen35moe.attention_layers: {}", attention_layers);
    println!("qwen35moe.recurrent_layers: {}", recurrent_layers);
    if let Some(layer0) = qwen35moe.layers.first() {
        println!("qwen35moe.layer0.kind: {}", layer0.kind.name());
        println!(
            "qwen35moe.layer0.moe_merged_gate_up: {}",
            layer0.moe.uses_merged_gate_up()
        );
    }

    if let Some(first) = model.gguf.tensors.first() {
        println!(
            "tensor.first: {} type={} dims={:?} size_bytes={} offset={}",
            first.name,
            first.tensor_type.name(),
            first.dimensions,
            first.size_bytes,
            first.offset
        );
    }

    if let Some(last) = model.gguf.tensors.last() {
        println!(
            "tensor.last: {} type={} dims={:?} size_bytes={} offset={}",
            last.name,
            last.tensor_type.name(),
            last.dimensions,
            last.size_bytes,
            last.offset
        );
    }

    println!(
        "tensor.sample: {}",
        model.gguf.tensor_summary("blk.0.attn_qkv.weight")?
    );
    println!(
        "tensor.sample: {}",
        model.gguf.tensor_summary("blk.39.post_attention_norm.weight")?
    );

    if metal_available() {
        let zero_embed = vec![0.0f32; plan.embedding_length as usize];
        let moe_embed = (0..plan.embedding_length as usize)
            .map(|index| ((index % 31) as f32 - 15.0) * 0.01)
            .collect::<Vec<_>>();
        let token_ids = [0_i32];
        let positions = [0_i32];
        let hybrid_decode_extra = hybrid_decode_cache_layout
            .as_ref()
            .map(|layout| layout.total_bytes)
            .unwrap_or(0)
            .saturating_add(256 << 20);
        let mut hybrid_decode_loaded = plan
            .full_weights
            .allocate_and_load_with_extra(&model.gguf, hybrid_decode_extra)?;
        let (hybrid_decode_graph, hybrid_decode_prepared) = prepare_hybrid_decode_graph(
            &mut hybrid_decode_loaded.ctx,
            &hybrid_decode_loaded.tensor_ids,
            &hybrid_decode_spec,
            1,
            MetalDeviceFeatures::default(),
        )
        .map_err(|err| std::io::Error::other(format!("prepare_hybrid_decode_graph: {err}")))?;
        println!(
            "qwen35moe.hybrid_decode_nodes: {}",
            hybrid_decode_graph.graph.n_nodes()
        );
        println!(
            "qwen35moe.hybrid_decode_main_buffer_size: {}",
            hybrid_decode_prepared.main_buffer_size
        );
        println!(
            "qwen35moe.hybrid_decode_tail_buffer_size: {}",
            hybrid_decode_prepared.tail_buffer_size
        );

        match compile_attention_block_metal(&mut attention_loaded, &attention_spec, 1) {
            Ok(compiled) => {
                println!("attention_block_compiled.ok: true");
                match execute_attention_block_graph_metal_cached(
                    &compiled,
                    &attention_loaded,
                    LogitsProbeInput::TokenIds(&token_ids),
                    &positions,
                ) {
                    Ok(run) => {
                        println!("attention_block_cached.hidden_size: {}", run.hidden_size);
                        println!("attention_block_cached.n_tokens: {}", run.n_tokens);
                        println!("attention_block_cached.hidden_len: {}", run.hidden.len());
                        if let Some(first) = run.hidden.first() {
                            println!("attention_block_cached.hidden0: {}", first);
                        }
                    }
                    Err(err) => {
                        println!("attention_block_cached.error: {}", err);
                    }
                }
            }
            Err(err) => {
                println!("attention_block_compiled.error: {}", err);
            }
        }

        match compile_moe_ffn_metal(&mut moe_loaded, &moe_spec, 1) {
            Ok(compiled) => {
                println!("moe_ffn_compiled.ok: true");
                match execute_moe_ffn_graph_metal_cached(
                    &compiled,
                    &moe_loaded,
                    LogitsProbeInput::EmbeddingsF32 {
                        data: &moe_embed,
                        n_tokens: 1,
                    },
                ) {
                    Ok(run) => {
                        println!("moe_ffn.hidden_size: {}", run.hidden_size);
                        println!("moe_ffn.n_tokens: {}", run.n_tokens);
                        println!("moe_ffn.hidden_len: {}", run.hidden.len());
                        if let Some(first) = run.hidden.first() {
                            println!("moe_ffn.hidden0: {}", first);
                        }
                        println!(
                            "moe_ffn.selected_experts: {:?}",
                            &run.selected_experts
                                [..run.expert_used_count.min(run.selected_experts.len())]
                        );
                    }
                    Err(err) => {
                        println!("moe_ffn.error: {}", err);
                    }
                }
            }
            Err(err) => {
                println!("moe_ffn_compiled.error: {}", err);
            }
        }

        match compile_attention_decode_metal(&mut attention_decode_loaded, &attention_decode_spec, 1) {
            Ok(compiled) => {
                println!("attention_decode_compiled.ok: true");
                match execute_attention_decode_graph_metal_cached(
                    &compiled,
                    &mut attention_decode_loaded,
                    LogitsProbeInput::TokenIds(&[0_i32]),
                    &[0_i32],
                    1,
                ) {
                    Ok(run) => {
                        println!("attention_decode.step0.hidden_size: {}", run.hidden_size);
                        println!("attention_decode.step0.n_tokens: {}", run.n_tokens);
                        println!("attention_decode.step0.hidden_len: {}", run.hidden.len());
                        if let Some(first) = run.hidden.first() {
                            println!("attention_decode.step0.hidden0: {}", first);
                        }
                    }
                    Err(err) => {
                        println!("attention_decode.step0.error: {}", err);
                    }
                }
                match execute_attention_decode_graph_metal_cached(
                    &compiled,
                    &mut attention_decode_loaded,
                    LogitsProbeInput::TokenIds(&[1_i32]),
                    &[1_i32],
                    2,
                ) {
                    Ok(run) => {
                        println!("attention_decode.step1.hidden_size: {}", run.hidden_size);
                        println!("attention_decode.step1.n_tokens: {}", run.n_tokens);
                        println!("attention_decode.step1.hidden_len: {}", run.hidden.len());
                        if let Some(first) = run.hidden.first() {
                            println!("attention_decode.step1.hidden0: {}", first);
                        }
                    }
                    Err(err) => {
                        println!("attention_decode.step1.error: {}", err);
                    }
                }
            }
            Err(err) => {
                println!("attention_decode_compiled.error: {}", err);
            }
        }

        match compile_delta_net_recurrent_decode_metal(
            &mut recurrent_decode_loaded,
            &recurrent_decode_spec,
            1,
        ) {
            Ok(compiled) => {
                println!("recurrent_decode_compiled.ok: true");
                match execute_delta_net_recurrent_decode_graph_metal_cached(
                    &compiled,
                    &mut recurrent_decode_loaded,
                    LogitsProbeInput::TokenIds(&[0_i32]),
                ) {
                    Ok(run) => {
                        println!("recurrent_decode.step0.hidden_size: {}", run.hidden_size);
                        println!("recurrent_decode.step0.n_tokens: {}", run.n_tokens);
                        println!("recurrent_decode.step0.hidden_len: {}", run.hidden.len());
                        if let Some(first) = run.hidden.first() {
                            println!("recurrent_decode.step0.hidden0: {}", first);
                            if first.is_nan() {
                                dump_recurrent_debug(
                                    &model.gguf,
                                    &recurrent_layout,
                                    &recurrent_decode_spec,
                                    &[0_i32],
                                    &[
                                        "recur_decode.input_norm",
                                        "recur_decode.beta",
                                        "recur_decode.gate",
                                        "recur_decode.conv_output",
                                        "recur_decode.delta",
                                        "recur_decode.output_view",
                                        "recur_decode.output_rms",
                                        "recur_decode.output_norm",
                                        "recur_decode.z_silu",
                                        "recur_decode.gated_output",
                                        "recur_decode.final_output",
                                        "recur_decode.output",
                                    ],
                                )?;
                            }
                        }
                    }
                    Err(err) => {
                        println!("recurrent_decode.step0.error: {}", err);
                    }
                }
                match execute_delta_net_recurrent_decode_graph_metal_cached(
                    &compiled,
                    &mut recurrent_decode_loaded,
                    LogitsProbeInput::TokenIds(&[1_i32]),
                ) {
                    Ok(run) => {
                        println!("recurrent_decode.step1.hidden_size: {}", run.hidden_size);
                        println!("recurrent_decode.step1.n_tokens: {}", run.n_tokens);
                        println!("recurrent_decode.step1.hidden_len: {}", run.hidden.len());
                        if let Some(first) = run.hidden.first() {
                            println!("recurrent_decode.step1.hidden0: {}", first);
                        }
                    }
                    Err(err) => {
                        println!("recurrent_decode.step1.error: {}", err);
                    }
                }
            }
            Err(err) => {
                println!("recurrent_decode_compiled.error: {}", err);
            }
        }

        let hybrid_compile_started = Instant::now();
        match compile_hybrid_decode_metal(&mut hybrid_decode_loaded, &hybrid_decode_spec, 1) {
            Ok(compiled) => {
                println!("hybrid_decode_compiled.ok: true");
                println!(
                    "hybrid_decode.compile_ms: {}",
                    hybrid_compile_started.elapsed().as_millis()
                );
                let step0_started = Instant::now();
                match execute_hybrid_decode_graph_metal_cached(
                    &compiled,
                    &mut hybrid_decode_loaded,
                    LogitsProbeInput::TokenIds(&[0_i32]),
                    &[0_i32],
                    1,
                ) {
                    Ok(run) => {
                        println!(
                            "hybrid_decode.step0.ms: {}",
                            step0_started.elapsed().as_millis()
                        );
                        println!("hybrid_decode.step0.hidden_size: {}", run.hidden_size);
                        println!("hybrid_decode.step0.n_tokens: {}", run.n_tokens);
                        println!("hybrid_decode.step0.hidden_len: {}", run.hidden.len());
                        println!("hybrid_decode.step0.vocab_size: {}", run.vocab_size);
                        println!("hybrid_decode.step0.logits_len: {}", run.logits.len());
                        if let Some(first) = run.hidden.first() {
                            println!("hybrid_decode.step0.hidden0: {}", first);
                        }
                        if let Some(first) = run.logits.first() {
                            println!("hybrid_decode.step0.logit0: {}", first);
                        }
                        if let Some((layer, experts)) = run.selected_experts.first() {
                            println!(
                                "hybrid_decode.step0.layer{}_experts: {:?}",
                                layer, experts
                            );
                        }
                    }
                    Err(err) => {
                        println!("hybrid_decode.step0.error: {}", err);
                    }
                }
                let step1_started = Instant::now();
                match execute_hybrid_decode_graph_metal_cached(
                    &compiled,
                    &mut hybrid_decode_loaded,
                    LogitsProbeInput::TokenIds(&[1_i32]),
                    &[1_i32],
                    2,
                ) {
                    Ok(run) => {
                        println!(
                            "hybrid_decode.step1.ms: {}",
                            step1_started.elapsed().as_millis()
                        );
                        println!("hybrid_decode.step1.hidden_size: {}", run.hidden_size);
                        println!("hybrid_decode.step1.n_tokens: {}", run.n_tokens);
                        println!("hybrid_decode.step1.hidden_len: {}", run.hidden.len());
                        println!("hybrid_decode.step1.vocab_size: {}", run.vocab_size);
                        println!("hybrid_decode.step1.logits_len: {}", run.logits.len());
                        if let Some(first) = run.hidden.first() {
                            println!("hybrid_decode.step1.hidden0: {}", first);
                        }
                        if let Some(first) = run.logits.first() {
                            println!("hybrid_decode.step1.logit0: {}", first);
                        }
                        if let Some((layer, experts)) = run.selected_experts.first() {
                            println!(
                                "hybrid_decode.step1.layer{}_experts: {:?}",
                                layer, experts
                            );
                        }
                        let bench_started = Instant::now();
                        let mut bench_last_hidden0 = None;
                        let mut bench_last_logit0 = None;
                        let mut bench_steps = 0usize;
                        for step in 2_i32..8_i32 {
                            let run = execute_hybrid_decode_graph_metal_cached(
                                &compiled,
                                &mut hybrid_decode_loaded,
                                LogitsProbeInput::TokenIds(&[step]),
                                &[step],
                                usize::try_from(step + 1).unwrap(),
                            )?;
                            bench_steps += 1;
                            bench_last_hidden0 = run.hidden.first().copied();
                            bench_last_logit0 = run.logits.first().copied();
                        }
                        let bench_elapsed = bench_started.elapsed();
                        println!("hybrid_decode.bench_steps: {}", bench_steps);
                        println!("hybrid_decode.bench_ms: {}", bench_elapsed.as_millis());
                        println!(
                            "hybrid_decode.bench_tok_s: {:.3}",
                            bench_steps as f64 / bench_elapsed.as_secs_f64()
                        );
                        if let Some(value) = bench_last_hidden0 {
                            println!("hybrid_decode.bench_last_hidden0: {}", value);
                        }
                        if let Some(value) = bench_last_logit0 {
                            println!("hybrid_decode.bench_last_logit0: {}", value);
                        }
                    }
                    Err(err) => {
                        println!("hybrid_decode.step1.error: {}", err);
                    }
                }
            }
            Err(err) => {
                println!("hybrid_decode_compiled.error: {}", err);
            }
        }

        match compile_logits_probe_metal(
            &mut tail_probe_loaded,
            &plan.tail_probe.spec,
            GraphBatch {
                n_tokens: 1,
                n_outputs: 1,
            },
        ) {
            Ok(compiled) => {
                println!("tail_probe_compiled.ok: true");
                match execute_logits_probe_graph_metal_cached(
                    &compiled,
                    &tail_probe_loaded,
                    LogitsProbeInput::EmbeddingsF32 {
                        data: &zero_embed,
                        n_tokens: 1,
                    },
                    &[0],
                ) {
                    Ok(run) => {
                        println!("tail_probe_cached.outputs: {}", run.n_outputs);
                        println!("tail_probe_cached.vocab_size: {}", run.vocab_size);
                        println!("tail_probe_cached.logits_len: {}", run.logits.len());
                        if let Some(first) = run.logits.first() {
                            println!("tail_probe_cached.logit0: {}", first);
                        }
                    }
                    Err(err) => {
                        println!("tail_probe_cached.error: {}", err);
                    }
                }
            }
            Err(err) => {
                println!("tail_probe_compiled.error: {}", err);
            }
        }

        match execute_logits_probe_graph_metal(
            &mut tail_probe_loaded,
            &plan.tail_probe.spec,
            LogitsProbeInput::EmbeddingsF32 {
                data: &zero_embed,
                n_tokens: 1,
            },
            &[0],
        ) {
            Ok(run) => {
                println!("tail_probe_graph.outputs: {}", run.n_outputs);
                println!("tail_probe_graph.vocab_size: {}", run.vocab_size);
                println!("tail_probe_graph.logits_len: {}", run.logits.len());
                if let Some(first) = run.logits.first() {
                    println!("tail_probe_graph.logit0: {}", first);
                }
            }
            Err(err) => {
                println!("tail_probe_graph.error: {}", err);
            }
        }

        match execute_logits_probe_metal(
            &tail_probe_loaded,
            &plan.tail_probe.spec,
            LogitsProbeInput::EmbeddingsF32 {
                data: &zero_embed,
                n_tokens: 1,
            },
            &[0],
        ) {
            Ok(run) => {
                println!("tail_probe_eager.outputs: {}", run.n_outputs);
                println!("tail_probe_eager.vocab_size: {}", run.vocab_size);
                println!("tail_probe_eager.logits_len: {}", run.logits.len());
                if let Some(first) = run.logits.first() {
                    println!("tail_probe_eager.logit0: {}", first);
                }
            }
            Err(err) => {
                println!("tail_probe_eager.error: {}", err);
            }
        }
    }

    Ok(())
}

fn qwen35moe_probe_layout(
    qwen35moe: &makepad_llama::Qwen35MoeTensors,
) -> Result<makepad_llama::GgufWeightLayout, Box<dyn std::error::Error>> {
    let recurrent = qwen35moe
        .layers
        .iter()
        .find_map(|layer| layer.recurrent.as_ref().map(|recurrent| (layer, recurrent)))
        .ok_or_else(|| std::io::Error::other("qwen35moe probe could not find a recurrent layer"))?;
    let attention = qwen35moe
        .layers
        .iter()
        .find_map(|layer| layer.attention.as_ref().map(|attention| (layer, attention)))
        .ok_or_else(|| std::io::Error::other("qwen35moe probe could not find an attention layer"))?;

    Ok(makepad_llama::GgufWeightLayout::from_tensors(vec![
        qwen35moe.globals.output_norm.clone(),
        recurrent.0.attn_norm.clone(),
        recurrent.0.post_attention_norm.clone(),
        recurrent.1.ssm_dt.clone(),
        recurrent.1.ssm_norm.clone(),
        attention.0.attn_norm.clone(),
        attention.0.post_attention_norm.clone(),
        attention.1.wq.clone(),
        attention.1.attn_q_norm.clone(),
        attention.1.wo.clone(),
    ])?)
}

fn dump_recurrent_debug(
    gguf: &makepad_llama::GgufFile,
    layout: &makepad_llama::GgufWeightLayout,
    spec: &makepad_llama::DeltaNetRecurrentDecodeSpec,
    token_ids: &[i32],
    tensor_names: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut loaded = layout.allocate_and_load_with_extra(gguf, 64 << 20)?;
    let (_, prepared) = prepare_delta_net_recurrent_decode_graph(
        &mut loaded.ctx,
        &loaded.tensor_ids,
        spec,
        token_ids.len(),
        MetalDeviceFeatures::default(),
    )?;
    let runtime = MetalRuntime::new()?;
    let compiled = compile_prepared_graph(
        &runtime,
        &loaded.ctx,
        &prepared,
        BufferStorageMode::Private,
        BufferStorageMode::Private,
    )?;
    let input_id = loaded
        .ctx
        .get_tensor("recur_decode.inp_tokens")
        .ok_or_else(|| std::io::Error::other("missing recur_decode.inp_tokens"))?;
    let output_ids = tensor_names
        .iter()
        .filter_map(|name| loaded.ctx.get_tensor(name).map(|id| ((*name).to_string(), id)))
        .collect::<Vec<_>>();

    let execution = execute_compiled_graph(
        &runtime,
        &loaded.ctx,
        &compiled,
        &[MetalGraphTensorWrite {
            tensor_id: input_id,
            bytes: unsafe {
                std::slice::from_raw_parts(
                    token_ids.as_ptr() as *const u8,
                    std::mem::size_of_val(token_ids),
                )
            },
        }],
        &output_ids.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
    )?;

    for (name, id) in output_ids {
        if let Some(tensor) = loaded.ctx.tensor(id) {
            println!(
                "recurrent_debug.{}.meta: ty={} rank={} ne={:?} nb={:?} contiguous={} view={}",
                name,
                tensor.desc.ty.name(),
                tensor.desc.layout.rank(),
                &tensor.ne[..tensor.desc.layout.rank()],
                &tensor.nb[..tensor.desc.layout.rank()],
                tensor.is_contiguous(),
                tensor.is_view(),
            );
        }
        if let Some(bytes) = execution.outputs.get(&id) {
            println!("recurrent_debug.{}.bytes: {}", name, bytes.len());
            let mut first = f32::NAN;
            let mut nan_count = 0usize;
            for (index, chunk) in bytes.chunks_exact(4).enumerate() {
                let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if index == 0 {
                    first = value;
                }
                if value.is_nan() {
                    nan_count += 1;
                }
            }
            println!("recurrent_debug.{}.first: {}", name, first);
            println!("recurrent_debug.{}.nan_count: {}", name, nan_count);
        }
    }

    Ok(())
}
