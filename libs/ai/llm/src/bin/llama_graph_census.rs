//! Dump the exact node list of the hybrid-decode graph for a model: per
//! node the op, output/src tensor types, extents, strides, contiguity and
//! view-ness, plus a deduplicated (op, type-signature, layout-signature)
//! summary. This is the executable contract a new execution backend must
//! cover; the CUDA executor's dispatch table and fail-closed validation
//! list are derived from (and tested against) this census.
//!
//! Usage:
//!   llama-graph-census <model.gguf> [--n-tokens N] [--n-outputs N]
//!       [--key-count N] [--max-context N] [--summary-only]

use std::collections::BTreeSet;
use std::path::PathBuf;

use makepad_ai_llm::{TensorId, TensorType};
use makepad_ai_llm::runtime::{
    allocate_hybrid_shared_cache_tensors, build_hybrid_decode_graph_with_attention_key_count,
};
use makepad_ai_llm::{LlamaModel, LlamaSessionConfig};

struct Args {
    model_path: PathBuf,
    n_tokens: usize,
    n_outputs: usize,
    key_count: Option<usize>,
    max_context: Option<u32>,
    summary_only: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let model_path = PathBuf::from(args.next().ok_or("usage: llama-graph-census <model.gguf>")?);
    let mut out = Args {
        model_path,
        n_tokens: 1,
        n_outputs: 1,
        key_count: None,
        max_context: Some(8192),
        summary_only: false,
    };
    while let Some(arg) = args.next() {
        let mut take = |name: &str| -> Result<String, String> {
            args.next().ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--n-tokens" => out.n_tokens = take("--n-tokens")?.parse().map_err(|e| format!("{e}"))?,
            "--n-outputs" => {
                out.n_outputs = take("--n-outputs")?.parse().map_err(|e| format!("{e}"))?
            }
            "--key-count" => {
                out.key_count = Some(take("--key-count")?.parse().map_err(|e| format!("{e}"))?)
            }
            "--max-context" => {
                out.max_context = Some(take("--max-context")?.parse().map_err(|e| format!("{e}"))?)
            }
            "--summary-only" => out.summary_only = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(out)
}

fn ty_name(ty: TensorType) -> &'static str {
    ty.name()
}

fn main() {
    if let Err(err) = run() {
        eprintln!("llama-graph-census failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let model = LlamaModel::load(&args.model_path).map_err(|e| format!("{e:?}"))?;
    let plan = model.execution_plan().map_err(|e| format!("{e:?}"))?;
    let config = LlamaSessionConfig::default();
    let max_context = args
        .max_context
        .unwrap_or_else(|| model.context_length().unwrap_or(8192));
    let spec = model
        .hybrid_decode_spec(
            max_context,
            1,
            config.attention_k_type,
            config.attention_v_type,
            config.recurrent_r_type,
            config.recurrent_s_type,
        )
        .map_err(|e| format!("{e:?}"))?;

    // Graph shape only needs tensor descriptors, not weight bytes: allocate
    // the arena without reading the file.
    let mut weights = plan
        .full_weights
        .allocate_context_with_extra(28 << 30)
        .map_err(|e| format!("{e:?}"))?;
    let shared_cache =
        allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, &spec)
            .map_err(|e| format!("{e:?}"))?;
    let key_count = args.key_count.unwrap_or(max_context as usize);
    let decode = build_hybrid_decode_graph_with_attention_key_count(
        &mut weights.ctx,
        &weights.tensor_ids,
        &spec,
        Some(&shared_cache),
        args.n_tokens,
        args.n_outputs,
        key_count,
    )
    .map_err(|e| format!("{e:?}"))?;

    let ctx = &weights.ctx;
    let tensors = ctx.tensors();
    let describe = |id: TensorId| -> String {
        let t = &tensors[id];
        format!(
            "{}[{} ne={:?} nb={:?}{}{}{}]",
            id,
            ty_name(t.desc.ty),
            &t.ne[..],
            &t.nb[..],
            if t.is_contiguous() { " cont" } else { "" },
            if t.is_view() { " view" } else { "" },
            if t.data_offset.is_some() { " resident" } else { "" },
        )
    };

    let mut summary: BTreeSet<String> = BTreeSet::new();
    let mut op_counts: std::collections::BTreeMap<String, usize> = Default::default();
    println!(
        "graph: arch={} n_tokens={} n_outputs={} key_count={} nodes={}",
        model.architecture.name(),
        args.n_tokens,
        args.n_outputs,
        key_count,
        decode.graph.nodes.len()
    );
    for (index, &node_id) in decode.graph.nodes.iter().enumerate() {
        let node = &tensors[node_id];
        let op = node.op;
        let mut line = format!("{index:5} {op:?} -> {}", describe(node_id));
        let mut sig_srcs = Vec::new();
        for src in node.src.iter().flatten() {
            line.push_str(&format!(" src {}", describe(*src)));
            let s = &tensors[*src];
            sig_srcs.push(format!(
                "{}{}{}",
                ty_name(s.desc.ty),
                if s.is_contiguous() { "" } else { "/strided" },
                if s.is_view() { "/view" } else { "" },
            ));
        }
        if !args.summary_only {
            println!("{line}");
        }
        let params = node.op_params;
        let has_params = params.iter().any(|&p| p != 0);
        summary.insert(format!(
            "{:?} out={}{} srcs=[{}]{}",
            op,
            ty_name(node.desc.ty),
            if node.is_contiguous() { "" } else { "/strided" },
            sig_srcs.join(", "),
            if has_params { " +params" } else { "" },
        ));
        *op_counts.entry(format!("{op:?}")).or_default() += 1;
    }
    println!("\n== op counts ==");
    for (op, count) in &op_counts {
        println!("{count:5} {op}");
    }
    println!("\n== unique (op, types, layout) signatures: {} ==", summary.len());
    for sig in &summary {
        println!("{sig}");
    }
    Ok(())
}
