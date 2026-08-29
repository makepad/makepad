//! CLI for the H3 DiT safetensors -> pruned-Q4_K GGUF quantizer, plus the
//! verification gate that proves an emitted file loads through the exact
//! `h3_quant` path the `*-q4-24g` tiers run.
//!
//! ```text
//! h3_quant_gguf quantize --src <transformer_shard_dir> --out <file.gguf>
//!                        [--threads N] [--label <provenance>] [--stride N]
//! h3_quant_gguf verify   --gguf <file.gguf> [--src <transformer_shard_dir>]
//! ```
//!
//! `verify` exit codes: 0 = every gate passed, 2 = a gate failed. With
//! `--src` it additionally deep-compares dequantized rows, the BF16/F32
//! passthroughs and the folded AdaLN path against the bf16 source.

use makepad_ai_h3::error::Result;
use makepad_ai_h3::h3::{H3ShardedWeights, H3_DEPTH, H3_TIME_EMBED_DIM};
use makepad_ai_h3::h3_quant::{
    H3GgufWeights, H3QuantComponent, GGML_TYPE_BF16, GGML_TYPE_F32, GGML_TYPE_Q4_K,
};
use makepad_ai_h3::h3_quant_writer::{
    silu_temb_grid, write_fasth3_dit_q4_gguf, FastH3QuantOptions, ADALN_GRID, ADALN_RANK,
};
use makepad_ai_common::quant::get_rows_ggml_bytes_cpu;
use std::path::PathBuf;
use std::process::ExitCode;

struct Args {
    values: Vec<String>,
}

impl Args {
    fn get(&self, flag: &str) -> Option<String> {
        self.values
            .iter()
            .position(|a| a == flag)
            .and_then(|i| self.values.get(i + 1).cloned())
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let args = Args { values: args.collect() };
    let outcome = match mode.as_str() {
        "quantize" => run_quantize(&args),
        "verify" => run_verify(&args),
        _ => {
            eprintln!(
                "usage: h3_quant_gguf quantize --src <dir> --out <file.gguf> [--threads N] \
                 [--label STR] [--stride N]\n       h3_quant_gguf verify --gguf <file.gguf> \
                 [--src <dir>]"
            );
            return ExitCode::from(1);
        }
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("FAIL: {err}");
            ExitCode::from(2)
        }
    }
}

fn run_quantize(args: &Args) -> Result<()> {
    let src_dir = required_path(args, "--src")?;
    let out = required_path(args, "--out")?;
    let mut options = FastH3QuantOptions::default();
    if let Some(threads) = args.get("--threads") {
        options.threads = threads.parse().unwrap_or(options.threads);
    }
    if let Some(stride) = args.get("--stride") {
        options.error_sample_stride = stride.parse().unwrap_or(options.error_sample_stride);
    }
    options.source_label = args
        .get("--label")
        .unwrap_or_else(|| src_dir.display().to_string());
    println!(
        "quantize: src {} -> {} ({} threads)",
        src_dir.display(),
        out.display(),
        options.threads
    );
    let started = std::time::Instant::now();
    let src = H3ShardedWeights::load(&src_dir)?;
    let report = write_fasth3_dit_q4_gguf(&src, &out, &options, &mut |line| {
        println!("  {line}");
    })?;
    println!("== quantize report ==");
    println!("out: {}", report.out_path.display());
    println!(
        "tensors: {} (q4_k {}, bf16 {}, f32 {}), payload {} bytes",
        report.tensor_count,
        report.q4k_tensors,
        report.bf16_tensors,
        report.f32_tensors,
        report.total_bytes
    );
    println!(
        "adaln: rank {} grid {} rel_l2 {:.6e} max_row_rel {:.6e} dmd_max_rel {:.6e}",
        report.adaln.rank,
        report.adaln.grid,
        report.adaln.rel_l2,
        report.adaln.max_row_rel,
        report.adaln.dmd_max_rel
    );
    println!("q4_k mean rel rmse (sampled): {:.6e}", report.mean_q4k_rel_rmse);
    for (name, rel) in &report.worst_q4k_rel_rmse {
        println!("q4_k worst: {name} rel rmse {rel:.6e}");
    }
    println!("skipped sources: {}", report.skipped_sources.len());
    println!("wall: {:.1}s", started.elapsed().as_secs_f64());
    Ok(())
}

fn gate(condition: bool, label: &str) -> Result<()> {
    if condition {
        println!("PASS: {label}");
        Ok(())
    } else {
        Err(makepad_ai_h3::DiffusionError::model(format!("gate failed: {label}")))
    }
}

fn rel_rmse(actual: &[f32], expected: &[f32]) -> f64 {
    let mut err2 = 0.0f64;
    let mut ref2 = 0.0f64;
    for (a, e) in actual.iter().zip(expected) {
        err2 += ((*a - *e) as f64).powi(2);
        ref2 += (*e as f64).powi(2);
    }
    if ref2 == 0.0 {
        return if err2 == 0.0 { 0.0 } else { f64::INFINITY };
    }
    (err2 / ref2).sqrt()
}

fn run_verify(args: &Args) -> Result<()> {
    let path = required_path(args, "--gguf")?;
    println!("verify: loading {} through H3GgufWeights (Dit)", path.display());
    let gguf = H3GgufWeights::load(&path, H3QuantComponent::Dit)?;
    let names = gguf.tensor_names();
    println!("LOAD OK: {} canonical tensors, {} disk bytes", names.len(), gguf.total_disk_bytes());
    let curve = gguf
        .adaln_curve()
        .ok_or_else(|| makepad_ai_h3::DiffusionError::model("no AdaLN curve in file"))?
        .clone();
    gate(
        curve.dim == ADALN_RANK && curve.grid == ADALN_GRID,
        &format!("adaln curve is rank {} over {} grid points", curve.dim, curve.grid),
    )?;
    let (mut q4k, mut bf16, mut f32c, mut other) = (0usize, 0usize, 0usize, 0usize);
    for name in &names {
        match gguf.linear(name).map(|l| l.ggml_type) {
            Some(GGML_TYPE_Q4_K) => q4k += 1,
            Some(GGML_TYPE_BF16) => bf16 += 1,
            Some(GGML_TYPE_F32) | None => f32c += 1,
            Some(_) => other += 1,
        }
    }
    println!("linear types: q4_k {q4k}, bf16 {bf16}, raw/vec {f32c}, other {other}");
    gate(other == 0, "no unexpected linear payload types")?;
    // 50 blocks x (qkv 3 + out + fc1 + fc2) + 2 refiner blocks x 6.
    gate(q4k == 52 * 6, &format!("q4_k linear count {q4k} == 312"))?;
    // Rows decode and are finite through the loader's own row path.
    let probe = "transformer_blocks.0.attn.to_v.weight";
    let row = gguf.tensor_row_f32(probe, 7)?;
    gate(
        row.iter().all(|v| v.is_finite()) && row.len() == 5376,
        &format!("{probe} row 7 decodes to {} finite values", row.len()),
    )?;

    let Some(src_dir) = args.get("--src").map(PathBuf::from) else {
        println!("(no --src: structural gates only)");
        return Ok(());
    };
    println!("deep compare vs {}", src_dir.display());
    let src = H3ShardedWeights::load(&src_dir)?;

    // --- Q4_K rows through the loader's fused-QKV split ------------------
    let sample_rows = [0u64, 1, 1000, 7167];
    for layer in [0usize, 24, H3_DEPTH - 1] {
        for part in ["to_q", "to_k", "to_v"] {
            let name = format!("transformer_blocks.{layer}.attn.{part}.weight");
            let mut worst = 0.0f64;
            for row in sample_rows {
                let got = gguf.tensor_row_f32(&name, row)?;
                let want = src.tensor_row_f32(&name, row)?;
                worst = worst.max(rel_rmse(&got, &want));
            }
            println!("  {name}: worst sampled row rel rmse {worst:.4e}");
            gate(worst < 0.08, &format!("{name} row error {worst:.4e} < 8e-2"))?;
        }
        let name = format!("transformer_blocks.{layer}.attn.to_out.0.weight");
        let got = gguf.tensor_row_f32(&name, 99)?;
        let want = src.tensor_row_f32(&name, 99)?;
        let rel = rel_rmse(&got, &want);
        println!("  {name}: row 99 rel rmse {rel:.4e}");
        gate(rel < 0.08, &format!("{name} row error {rel:.4e} < 8e-2"))?;
        let name = format!("transformer_blocks.{layer}.ff.net.2.weight");
        let got = gguf.tensor_row_f32(&name, 4242)?;
        let want = src.tensor_row_f32(&name, 4242)?;
        let rel = rel_rmse(&got, &want);
        println!("  {name}: row 4242 rel rmse {rel:.4e}");
        gate(rel < 0.08, &format!("{name} row error {rel:.4e} < 8e-2"))?;
    }

    // --- fc1: the swapped gated projection, via the loader's payload path
    // (linear_payload is what the CUDA cache uploads; it must give back the
    // CANONICAL [value|gate] row order) ------------------------------------
    {
        let name = "transformer_blocks.0.ff.net.0.proj.weight";
        let linear = gguf.linear(name).unwrap().clone();
        let payload = gguf.linear_payload(name)?;
        let rows: Vec<i32> = vec![0, 14336, 28671];
        let got = get_rows_ggml_bytes_cpu(&payload, linear.ggml_type, linear.k, linear.n, &rows)
            .ok_or_else(|| makepad_ai_h3::DiffusionError::model("fc1 payload dequant failed"))?;
        let mut worst = 0.0f64;
        for (slot, row) in rows.iter().enumerate() {
            let want = src.tensor_row_f32(name, *row as u64)?;
            let got_row = &got[slot * linear.k..(slot + 1) * linear.k];
            worst = worst.max(rel_rmse(got_row, &want));
        }
        println!("  {name}: worst payload row rel rmse {worst:.4e} (value|gate order)");
        gate(worst < 0.08, &format!("fc1 swapped payload error {worst:.4e} < 8e-2"))?;
    }

    // --- BF16 passthroughs decode bit-identically -------------------------
    for name in [
        "proj_in.weight",
        "audio_proj_in.weight",
        "context_embedder.weight",
        "proj_out.weight",
        "audio_proj_out.weight",
    ] {
        let got = gguf.tensor_f32(name)?;
        let want = src.tensor_f32(name)?;
        gate(got == want, &format!("{name} BF16 passthrough is bit-exact"))?;
    }
    for name in [
        "transformer_blocks.0.norm1.weight",
        "transformer_blocks.49.attn.norm_q.weight",
        "token_refiner.final_norm.weight",
        "norm_out.linear.bias",
        "transformer_blocks.0.adaln_proj.linear.bias",
        "proj_in.bias",
    ] {
        let got = gguf.tensor_f32(name)?;
        let want = src.tensor_f32(name)?;
        gate(got == want, &format!("{name} F32 vector is exact"))?;
    }

    // --- AdaLN: folded weight x curve == full weight x silu(temb) --------
    {
        println!("  adaln functional check (this recomputes the temb grid)...");
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
        let l1_w = src.tensor_f32("time_embedder.linear_1.weight")?;
        let l1_b = src.tensor_f32("time_embedder.linear_1.bias")?;
        let l2_w = src.tensor_f32("time_embedder.linear_2.weight")?;
        let l2_b = src.tensor_f32("time_embedder.linear_2.bias")?;
        let grid = silu_temb_grid(&l1_w, &l1_b, &l2_w, &l2_b, threads);
        let mut worst = 0.0f64;
        for (weight_name, bias_len) in [
            ("transformer_blocks.0.adaln_proj.linear.weight", 96_768usize),
            ("norm_out.linear.weight", 10_752usize),
        ] {
            let folded = gguf.tensor_f32(weight_name)?;
            gate(
                folded.len() == bias_len * ADALN_RANK,
                &format!("{weight_name} folded to [{bias_len}, {ADALN_RANK}]"),
            )?;
            let full = src.tensor_f32(weight_name)?;
            for grid_row in [0usize, 256, 512, 768, 1023, 1024] {
                let t = grid_row as f32 / (ADALN_GRID - 1) as f32;
                let coefficients = curve.temb(t);
                let m_row = &grid[grid_row * H3_TIME_EMBED_DIM..(grid_row + 1) * H3_TIME_EMBED_DIM];
                let mut err2 = 0.0f64;
                let mut ref2 = 0.0f64;
                // Sample the output coordinates on a fixed stride.
                for j in (0..bias_len).step_by(37) {
                    let mut direct = 0.0f64;
                    let full_row = &full[j * H3_TIME_EMBED_DIM..(j + 1) * H3_TIME_EMBED_DIM];
                    for (w, m) in full_row.iter().zip(m_row) {
                        direct += *w as f64 * *m as f64;
                    }
                    let mut via = 0.0f64;
                    for r in 0..ADALN_RANK {
                        via += folded[j * ADALN_RANK + r] as f64 * coefficients[r] as f64;
                    }
                    err2 += (via - direct).powi(2);
                    ref2 += direct.powi(2);
                }
                let rel = if ref2 > 0.0 { (err2 / ref2).sqrt() } else { 0.0 };
                worst = worst.max(rel);
                println!(
                    "  adaln {weight_name} t={t:.4}: sampled rel l2 {rel:.4e}"
                );
            }
        }
        gate(
            worst < 0.02,
            &format!("adaln folded path matches the full MLP path ({worst:.4e} < 2e-2)"),
        )?;
    }
    println!("ALL GATES PASSED");
    Ok(())
}

fn required_path(args: &Args, flag: &str) -> Result<PathBuf> {
    args.get(flag)
        .map(PathBuf::from)
        .filter(|p: &PathBuf| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            makepad_ai_h3::DiffusionError::workflow(format!("missing required {flag} <path>"))
        })
}
