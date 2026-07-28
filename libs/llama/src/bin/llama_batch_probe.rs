//! Compare last-step logits across prefill batchings of the same prompt.
//!
//! Sequential (batch 1) is the known-good reference; any batching must give
//! the same final logits. Prints max-abs-diff and top-5 tokens per batching.
//!
//! Usage: llama-batch-probe <model.gguf> [--tokens N] [--batches 1,2,3,4]

use makepad_ggml::TensorType;
use makepad_llama::{
    compile_delta_net_recurrent_decode_metal, execute_delta_net_recurrent_decode_graph_metal_cached,
    qwen35_delta_net_recurrent_decode_spec, qwen35_recurrent_block_layout, LlamaModel,
    LlamaSession, LlamaSessionConfig, LlamaVocab, LogitsProbeInput,
};

/// Tensors tapped per execution, in dataflow order. `per_token` is the value
/// count per token for tokenwise comparison; `None` compares whole buffers.
const DN_TAPS: &[(&str, &str)] = &[
    ("recur_decode.qkv_mixed", "qkv_mixed"),
    ("recur_decode.conv_input", "conv_input"),
    ("recur_decode.conv_raw", "conv_raw"),
    ("recur_decode.conv_output", "conv_output"),
    ("recur_decode.beta", "beta"),
    ("recur_decode.gate", "gate"),
    ("recur_decode.gated_output", "gated_output"),
];

/// Run the first delta-net block alone through the real compile path:
/// n tokens in one batched execution vs one-at-a-time with carried state,
/// comparing tapped intermediates stage by stage.
fn dn_block_probe(model: &LlamaModel, vocab: &LlamaVocab, n_tokens: usize) {
    let prompt = "The quick brown fox jumps over the lazy dog while seventeen zebras watch";
    let mut token_ids = vocab.tokenize(prompt, true, true).expect("tokenize");
    token_ids.truncate(n_tokens);
    let n_tokens = token_ids.len();
    eprintln!("dn probe tokens ({n_tokens}): {token_ids:?}");
    const EXTRA: usize = 512 << 20;

    let layout = qwen35_recurrent_block_layout(model, 0).expect("layout");
    let spec = qwen35_delta_net_recurrent_decode_spec(model, 0, 1, TensorType::F32, TensorType::F32)
        .expect("spec");

    let tap_ids = |loaded: &makepad_llama::LoadedGgufWeights| -> Vec<(usize, &'static str)> {
        DN_TAPS
            .iter()
            .filter_map(|(name, label)| loaded.ctx.get_tensor(name).map(|id| (id, *label)))
            .collect()
    };

    // Sequential reference: fresh weights, n_tokens single-token executions,
    // collecting taps per step.
    let mut seq_taps: Vec<Vec<Vec<f32>>> = Vec::new(); // [step][tap][values]
    let mut sequential_last = Vec::new();
    {
        let mut loaded = layout
            .allocate_and_load_with_extra(&model.gguf, EXTRA)
            .expect("load seq");
        let compiled =
            compile_delta_net_recurrent_decode_metal(&mut loaded, &spec, 1).expect("compile seq");
        let ids = tap_ids(&loaded);
        for &token in &token_ids {
            let (run, taps) = compiled
                .execute_with_taps(
                    &mut loaded.ctx,
                    LogitsProbeInput::TokenIds(&[token]),
                    &ids.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                )
                .expect("exec seq");
            seq_taps.push(taps);
            sequential_last = run.hidden;
        }
    }

    // Batched: fresh weights again (fresh conv/S state), one execution.
    let mut loaded = layout
        .allocate_and_load_with_extra(&model.gguf, EXTRA)
        .expect("load batch");
    let compiled = compile_delta_net_recurrent_decode_metal(&mut loaded, &spec, n_tokens)
        .expect("compile batch");
    // Execution-order audit: for every compiled node (in run order), print
    // its output byte range; flag any node whose OUTPUT intersects the
    // ssm_conv output's range — including views, whose bindings resolve into
    // their sources.
    {
        let graph = compiled.compiled_graph();
        let conv_out_id = loaded.ctx.get_tensor("recur_decode.conv_output").unwrap();
        let conv_binding = graph.bindings.get(&conv_out_id).unwrap();
        let conv_range = (
            conv_binding.offset_bytes,
            conv_binding.offset_bytes + conv_binding.size_bytes,
        );
        eprintln!(
            "  conv_output range: [{}..{}] ({} bytes)",
            conv_range.0,
            conv_range.1,
            conv_binding.size_bytes
        );
        let mut conv_seen = false;
        for (order, node) in graph.nodes.iter().enumerate() {
            let Some(binding) = graph.bindings.get(&node.node_id) else {
                continue;
            };
            let name = loaded
                .ctx
                .tensor(node.node_id)
                .and_then(|tensor| tensor.name().map(|name| name.to_string()))
                .unwrap_or_else(|| format!("tensor{}", node.node_id));
            if node.node_id == conv_out_id {
                conv_seen = true;
                eprintln!("  node {order}: ssm_conv output itself ({name})");
                continue;
            }
            let start = binding.offset_bytes;
            let end = binding.offset_bytes + binding.size_bytes;
            if start < conv_range.1 && end > conv_range.0 {
                eprintln!(
                    "  node {order}{}: {name} output [{start}..{end}] INTERSECTS conv_output (view={})",
                    if conv_seen { " (AFTER conv)" } else { " (before conv)" },
                    binding.is_view
                );
            }
        }
    }

    let ids = tap_ids(&loaded);
    let (run, batch_taps) = compiled
        .execute_with_taps(
            &mut loaded.ctx,
            LogitsProbeInput::TokenIds(&token_ids),
            &ids.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        )
        .expect("exec batch");

    // Stage-by-stage, token-by-token comparison. Sequential step i's tap holds
    // one token's values; the batched tap holds n_tokens of them. conv_input
    // is special: time-major with a 3-sample prefix, so token i's window in
    // the batched buffer sits at rows i..i+4 of each channel column.
    for (tap_index, (_, label)) in ids.iter().enumerate() {
        let mut worst: (f32, usize) = (0.0, 0);
        for step in 0..n_tokens {
            let seq_values = &seq_taps[step][tap_index];
            let batch_values = &batch_taps[tap_index];
            let diff = if *label == "conv_input" {
                // seq: [4, 8192] per step; batch: [3+n, 8192].
                let d_inner = seq_values.len() / 4;
                let batch_rows = 3 + n_tokens;
                let mut max_diff = 0.0f32;
                for c in 0..d_inner {
                    for k in 0..4 {
                        let seq_v = seq_values[c * 4 + k];
                        let batch_v = batch_values[c * batch_rows + step + k];
                        max_diff = max_diff.max((seq_v - batch_v).abs());
                    }
                }
                max_diff
            } else {
                let per_token = seq_values.len();
                let batch_slice = &batch_values[step * per_token..(step + 1) * per_token];
                batch_slice
                    .iter()
                    .zip(seq_values.iter())
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0f32, f32::max)
            };
            if diff > worst.0 {
                worst = (diff, step);
            }
        }
        eprintln!(
            "  tap {label:<14} worst token {}: max_abs_diff = {:.6}",
            worst.1, worst.0
        );
        if *label == "conv_output" && worst.0 > 1.0e-3 {
            for step in 0..n_tokens {
                let seq_values = &seq_taps[step][tap_index];
                let per_token = seq_values.len();
                let batch_values = &batch_taps[tap_index];
                let batch_slice = &batch_values[step * per_token..(step + 1) * per_token];
                let bad: Vec<usize> = batch_slice
                    .iter()
                    .zip(seq_values.iter())
                    .enumerate()
                    .filter(|(_, (a, b))| (*a - *b).abs() > 1.0e-3)
                    .map(|(c, _)| c)
                    .collect();
                let head: Vec<usize> = bad.iter().copied().take(8).collect();
                let tail: Vec<usize> = bad.iter().rev().take(4).rev().copied().collect();
                eprintln!(
                    "    conv_output token {step}: {} bad channels of {per_token}, first {head:?} last {tail:?}",
                    bad.len()
                );
            }
        }
    }

    let hidden_size = run.hidden_size;
    let batched_last = &run.hidden[(n_tokens - 1) * hidden_size..];
    let mut max_diff = 0.0f32;
    for (a, b) in batched_last.iter().zip(sequential_last.iter()) {
        max_diff = max_diff.max((a - b).abs());
    }
    eprintln!("dn block n_tokens={n_tokens}: last-token hidden max_abs_diff = {max_diff:.6}");

    // Forensics on one corrupted output: reconstruct the conv from the tapped
    // input + weights on the CPU, then test misread patterns against what the
    // GPU actually produced. conv_output tap is post-silu, so compare through
    // silu(x) = x/(1+exp(-x)).
    let conv_in_index = ids.iter().position(|(_, l)| *l == "conv_input").unwrap();
    let conv_out_index = ids.iter().position(|(_, l)| *l == "conv_output").unwrap();
    let weight_id = loaded
        .tensor_ids
        .iter()
        .find(|(name, _)| name.ends_with("ssm_conv1d.weight"))
        .map(|(_, id)| *id)
        .expect("conv weight");
    let weight_tensor = loaded.ctx.tensor(weight_id).expect("weight tensor");
    eprintln!(
        "  weight dims {:?} type {:?}",
        &weight_tensor.ne[..2],
        weight_tensor.desc.ty
    );
    let weights = {
        // Weights live in the context image; read them straight out.
        let (run2, taps) = compiled
            .execute_with_taps(
                &mut loaded.ctx,
                LogitsProbeInput::TokenIds(&token_ids),
                &[weight_id],
            )
            .expect("weight fetch");
        let _ = run2;
        taps.into_iter().next().unwrap()
    };
    let conv_in = &batch_taps[conv_in_index];
    let conv_out = &batch_taps[conv_out_index];
    let rows = 3 + n_tokens;
    let silu = |x: f32| x / (1.0 + (-x).exp());
    for &(token, channel) in &[(0usize, 1025usize), (1, 1024), (3, 4096)] {
        let window = |t: isize, c: isize| -> f32 {
            let c = c.rem_euclid(8192) as usize;
            let t = t.rem_euclid(rows as isize) as usize;
            conv_in[c * rows + t]
        };
        let dot = |t_off: isize, c_off: isize| -> f32 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += window(token as isize + k + t_off, channel as isize + c_off)
                    * weights[(channel as isize + c_off).rem_euclid(8192) as usize * 4 + k as usize];
            }
            silu(sum)
        };
        let actual = conv_out[token * 8192 + channel];
        eprintln!(
            "  t{token} c{channel}: gpu={actual:.6} expect={:.6} | shifts t+1={:.6} t-1={:.6} c+1={:.6} c-1={:.6} c+2048={:.6}",
            dot(0, 0),
            dot(1, 0),
            dot(-1, 0),
            dot(0, 1),
            dot(0, -1),
            dot(0, 2048),
        );
    }
}

fn top5(logits: &[f32]) -> Vec<(usize, f32)> {
    let mut indexed: Vec<(usize, f32)> = logits.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| b.1.total_cmp(&a.1));
    indexed.truncate(5);
    indexed
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let model_path = args.get(1).cloned().unwrap_or_else(|| {
        "local/models/Qwen3.5-4B-Q5_K_M.gguf".to_string()
    });
    let mut n_tokens = 8usize;
    let mut batches = vec![1usize, 2, 3, 4, 8];
    let mut iter = args.iter().skip(2);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--tokens" => n_tokens = iter.next().unwrap().parse().unwrap(),
            "--batches" => {
                batches = iter
                    .next()
                    .unwrap()
                    .split(',')
                    .map(|value| value.parse().unwrap())
                    .collect()
            }
            "--dn" => {}
            other => panic!("unknown arg {other}"),
        }
    }

    let dn_mode = args.iter().any(|arg| arg == "--dn");

    let model = LlamaModel::load(&model_path).expect("model");
    let vocab = LlamaVocab::from_model(&model).expect("vocab");

    if dn_mode {
        return dn_block_probe(&model, &vocab, n_tokens);
    }
    let long_prompt = "The quick brown fox jumps over the lazy dog while seventeen zebras watch. "
        .repeat(12);
    let mut token_ids = vocab.tokenize(&long_prompt, true, true).expect("tokenize");
    token_ids.truncate(n_tokens);
    eprintln!("prompt tokens: {}", token_ids.len());

    let run = |prefill_batch_size: usize| -> Vec<f32> {
        let mut session = LlamaSession::from_model(
            &model,
            LlamaSessionConfig {
                max_context: Some(256),
                prefill_batch_size,
                ..LlamaSessionConfig::default()
            },
        )
        .expect("session");
        session.append_tokens(&token_ids).expect("prefill");
        session.last_logits().expect("logits").to_vec()
    };

    let reference = run(1);
    eprintln!("batch 1 top5: {:?}", top5(&reference));
    for &batch in &batches {
        if batch == 1 {
            continue;
        }
        let logits = run(batch);
        let mut max_diff = 0.0f32;
        for (a, b) in logits.iter().zip(reference.iter()) {
            max_diff = max_diff.max((a - b).abs());
        }
        eprintln!(
            "batch {batch}: max_abs_diff vs batch1 = {max_diff:.6}, top5: {:?}",
            top5(&logits)
        );
    }
}
