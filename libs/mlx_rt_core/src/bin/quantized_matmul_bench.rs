use makepad_mlx_rt_core::{
    fnv1a64_u32_words, gemma4_qproj_case_input_bf16_words, MlxSafetensorsHeader,
    GEMMA4_QPROJ_CASE_INNER_DIM, GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64,
};
use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

const DEFAULT_WARMUP_ITERS: usize = 10;
const DEFAULT_BENCH_ITERS: usize = 50;
const WEIGHT_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.weight";
const SCALES_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.scales";
const BIASES_NAME: &str = "language_model.model.layers.0.self_attn.q_proj.biases";

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn usage() {
    eprintln!(
        "Usage: cargo run --release -p makepad-mlx-rt-core --bin quantized_matmul_bench -- [model.safetensors] [--warmup N] [--iters N]"
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut model_path = default_model_path();
    let mut warmup_iters = DEFAULT_WARMUP_ITERS;
    let mut bench_iters = DEFAULT_BENCH_ITERS;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--warmup" => {
                let value = args.next().ok_or("--warmup expects a value")?;
                warmup_iters = value.parse::<usize>()?;
            }
            "--iters" => {
                let value = args.next().ok_or("--iters expects a value")?;
                bench_iters = value.parse::<usize>()?;
            }
            "-h" | "--help" => {
                usage();
                return Ok(());
            }
            _ if arg.starts_with("--") => {
                return Err(format!("unknown option {}", arg).into());
            }
            _ => {
                model_path = PathBuf::from(arg);
            }
        }
    }

    if bench_iters == 0 {
        return Err("--iters must be greater than zero".into());
    }

    let header = MlxSafetensorsHeader::load(&model_path)?;
    let x = gemma4_qproj_case_input_bf16_words(GEMMA4_QPROJ_CASE_INNER_DIM);

    for _ in 0..warmup_iters {
        let _ = header.affine_quantized_matmul_t_f32(
            &x,
            WEIGHT_NAME,
            SCALES_NAME,
            BIASES_NAME,
            64,
            4,
        )?;
    }

    let start = Instant::now();
    let mut last_out = Vec::new();
    for _ in 0..bench_iters {
        last_out = header.affine_quantized_matmul_t_f32(
            &x,
            WEIGHT_NAME,
            SCALES_NAME,
            BIASES_NAME,
            64,
            4,
        )?;
    }
    let elapsed = start.elapsed();

    let out_bits = last_out
        .iter()
        .map(|value| value.to_bits())
        .collect::<Vec<_>>();
    let hash = fnv1a64_u32_words(&out_bits);
    if hash != GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64 {
        return Err(format!(
            "output hash mismatch: got 0x{hash:016X} expected 0x{GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64:016X}"
        )
        .into());
    }

    let avg_ns = elapsed.as_secs_f64() * 1e9 / bench_iters as f64;
    let avg_us = avg_ns / 1e3;
    let qmm_per_s = bench_iters as f64 / elapsed.as_secs_f64();

    println!("backend=rust-host");
    println!("model_path={}", model_path.display());
    println!("warmup_iters={warmup_iters}");
    println!("bench_iters={bench_iters}");
    println!("input_len={GEMMA4_QPROJ_CASE_INNER_DIM}");
    println!("total_ns={}", elapsed.as_nanos());
    println!("avg_ns={avg_ns:.0}");
    println!("avg_us={avg_us:.3}");
    println!("qmm_per_s={qmm_per_s:.3}");
    println!("full_row_fnv1a64=0x{hash:016X}");
    print!("first16_f32_bits=");
    for (index, bits) in out_bits.iter().take(16).enumerate() {
        if index != 0 {
            print!(",");
        }
        print!("0x{bits:08X}");
    }
    println!();

    Ok(())
}
