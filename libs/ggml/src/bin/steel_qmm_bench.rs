//! Isolated MLX steel `affine_qmm_t` vs the 1.62ms MLX oracle.
//! F16-resident A/C, optional F32 staging (pack_a + unpack_c).

use makepad_ggml::backend::metal::bench_steel_isolated;

fn report(label: &str, m: usize, k: usize, n: usize, chain: u32, r: makepad_ggml::backend::metal::SteelBenchResult) {
    let gemms = chain.max(1) as f64;
    let per = r.gpu_wall_ms / gemms;
    let flops = 2.0 * m as f64 * k as f64 * n as f64 * gemms;
    eprintln!(
        "steel {label} m={m} k={k} n={n} chain={chain}  wall={:.3} ms  per={:.3} ms  gpu-ts={:.3}  tflops={:.2}  min={:.3} max={:.3}",
        r.gpu_wall_ms,
        per,
        r.gpu_ts_ms,
        flops / (r.gpu_wall_ms * 1e9),
        r.wall_min,
        r.wall_max
    );
    if chain == 1 {
        eprintln!("  vs MLX isolated 1.62ms: {:.2}x", per / 1.62);
    } else {
        eprintln!(
            "  vs MLX 228-seq 0.315s: {:.2}x ({:.3}s)",
            (r.gpu_wall_ms / 1000.0) / 0.315,
            r.gpu_wall_ms / 1000.0
        );
    }
}

fn main() {
    let shapes = [
        ("hidden_sq", 256usize, 3072usize, 3072usize),
        ("qkv", 256, 3072, 9216),
        ("mlp_up", 256, 3072, 12288),
        ("mlp_down", 256, 12288, 3072),
        ("joint_down", 512, 15360, 3072),
        ("joint_wide", 512, 3072, 21504),
    ];
    for (name, m, k, n) in shapes {
        match bench_steel_isolated(m, k, n, 5, 15, 1, false) {
            Ok(r) => report(&format!("{name} steel-only"), m, k, n, 1, r),
            Err(err) => eprintln!("FAIL {name} steel-only: {err}"),
        }
        match bench_steel_isolated(m, k, n, 5, 15, 1, true) {
            Ok(r) => report(&format!("{name} +staging"), m, k, n, 1, r),
            Err(err) => eprintln!("FAIL {name} +staging: {err}"),
        }
    }
    match bench_steel_isolated(256, 3072, 3072, 2, 4, 228, false) {
        Ok(r) => report("hidden_sq x228 steel-only", 256, 3072, 3072, 228, r),
        Err(err) => eprintln!("FAIL chain228 steel-only: {err}"),
    }
    match bench_steel_isolated(256, 3072, 3072, 2, 4, 228, true) {
        Ok(r) => report("hidden_sq x228 +staging", 256, 3072, 3072, 228, r),
        Err(err) => eprintln!("FAIL chain228 +staging: {err}"),
    }
}
