//! Isolated Metal `kernel_mul_mm_q4_K` vs the MLX 1.6ms number.
//! Same shape as the Flux hidden linear: M=256, K=3072, N=3072.
//!
//! Reports three clocks:
//! - wall+copy: input write + encode + GPU + output readback
//! - gpu-wall:  encode + GPU only (resident buffers, no host copies)
//! - gpu-ts:    MTLCommandBuffer GPUStartTime/GPUEndTime

use makepad_ggml::backend::metal::{
    compile_graph_session, prepare_graph, BufferStorageMode, MetalGraphTensorWrite,
};
use makepad_ggml::quant::{block_size, f32_to_f16, GGML_TYPE_Q4_K, QK_K};
use makepad_ggml::{BufferUsage, Context, Graph, InitParams, TensorId, TensorType, GGML_MEM_ALIGN};
use std::time::Instant;

fn fill_q4k(bytes: &mut [u8]) {
    let bs = block_size(GGML_TYPE_Q4_K);
    for (i, block) in bytes.chunks_exact_mut(bs).enumerate() {
        block.fill(0);
        let d = f32_to_f16(0.05).to_le_bytes();
        block[0..2].copy_from_slice(&d);
        block[4] = 8;
        block[8] = 1;
        for (j, b) in block[16..].iter_mut().enumerate() {
            *b = ((i + j) as u8).wrapping_mul(17);
        }
    }
}

fn summarize(times: &mut [f64]) -> (f64, f64, f64) {
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (times[times.len() / 2], times[0], times[times.len() - 1])
}

fn bench_session(
    label: &str,
    ctx: &Context,
    session: &makepad_ggml::backend::metal::MetalGraphSession,
    input: TensorId,
    out: TensorId,
    a_bytes_le: &[u8],
    m: usize,
    k: usize,
    n: usize,
    chain: usize,
    warmup: u32,
    iters: u32,
) -> Result<(), String> {
    let writes = [MetalGraphTensorWrite {
        tensor_id: input,
        bytes: a_bytes_le,
    }];

    for _ in 0..warmup {
        let _ = session.execute(ctx, &writes, &[out])?;
    }

    let mut wall_copy = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        let t0 = Instant::now();
        let _ = session.execute(ctx, &writes, &[out])?;
        wall_copy.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let (copy_med, copy_min, copy_max) = summarize(&mut wall_copy);

    // Resident GPU-only: inputs already in the shared buffer. Empty writes /
    // empty outputs skip host memcpy and readback; wait_idle waits the GPU.
    for _ in 0..warmup {
        let _ = session.execute(ctx, &[], &[])?;
        session.runtime().wait_idle()?;
    }
    let mut gpu_wall = Vec::with_capacity(iters as usize);
    let mut gpu_ts = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        session.runtime().reset_counters();
        let t0 = Instant::now();
        let _ = session.execute(ctx, &[], &[])?;
        session.runtime().wait_idle()?;
        gpu_wall.push(t0.elapsed().as_secs_f64() * 1000.0);
        gpu_ts.push(session.runtime().counters().gpu_elapsed_ns as f64 / 1e6);
    }
    let (gpu_med, gpu_min, gpu_max) = summarize(&mut gpu_wall);
    let (ts_med, ts_min, ts_max) = summarize(&mut gpu_ts);

    let flops = 2.0 * m as f64 * k as f64 * n as f64 * chain as f64;
    eprintln!(
        "ggml mul_mm_q4_K {label} m={m} k={k} n={n} chain={chain} nodes={}",
        session.compiled().nodes.len()
    );
    eprintln!(
        "  wall+copy  med={copy_med:.3} ms  tflops={:.2}  min={copy_min:.3} max={copy_max:.3}",
        flops / (copy_med * 1e9)
    );
    eprintln!(
        "  gpu-wall   med={gpu_med:.3} ms  tflops={:.2}  min={gpu_min:.3} max={gpu_max:.3}",
        flops / (gpu_med * 1e9)
    );
    eprintln!(
        "  gpu-ts     med={ts_med:.3} ms  tflops={:.2}  min={ts_min:.3} max={ts_max:.3}",
        flops / (ts_med.max(1e-9) * 1e9)
    );
    if chain == 1 {
        eprintln!(
            "  vs MLX isolated 1.62ms: official gpu-wall is {:.2}x",
            gpu_med / 1.62
        );
    } else {
        eprintln!(
            "  vs MLX 228-seq 0.315s: official gpu-wall is {:.2}x ({:.3}s)",
            (gpu_med / 1000.0) / 0.315,
            gpu_med / 1000.0
        );
        eprintln!(
            "  amortized per GEMM gpu-wall={:.3} ms (Flux 256/4 used 16.8ms = step/228)",
            gpu_med / chain as f64
        );
    }
    Ok(())
}

fn bench_one(m: usize, k: usize, n: usize, chain: usize, warmup: u32, iters: u32) -> Result<(), String> {
    if k % QK_K != 0 {
        return Err(format!("K={k} is not a multiple of {QK_K}"));
    }
    if chain > 1 && n != k {
        return Err(format!("chain={chain} needs n==k so C can feed the next A (n={n} k={k})"));
    }
    let w_bytes = (n * k / QK_K) * block_size(GGML_TYPE_Q4_K);
    let a_bytes = m * k * 4;
    let c_bytes = m * n * 4 * chain;
    let extra = 8 << 20;
    let mut ctx = Context::new(InitParams {
        mem_size: ggml_pad(w_bytes + a_bytes + c_bytes + extra, GGML_MEM_ALIGN),
        mem_buffer: None,
        no_alloc: false,
    });
    let weight = ctx
        .new_tensor_2d(TensorType::Q4K, k as i64, n as i64, BufferUsage::Weights)
        .map_err(|e| e)?;
    {
        let dst = ctx.tensor_data_mut(weight).map_err(|e| e)?;
        fill_q4k(dst);
    }
    let input = ctx
        .new_tensor_2d(TensorType::F32, k as i64, m as i64, BufferUsage::Activations)
        .map_err(|e| e)?;
    let ones = vec![1.0f32; m * k];
    ctx.write_tensor_data(input, &f32s(&ones))?;
    let mut cur = input;
    let mut out = input;
    for _ in 0..chain {
        out = ctx.mul_mat(weight, cur, BufferUsage::Activations)?;
        cur = out;
    }
    let mut graph = Graph::new();
    graph.build_forward_expand(&ctx, out).map_err(|e| e)?;

    let runtime = makepad_ggml::backend::metal::MetalRuntime::new()?;
    let prepared = prepare_graph(&ctx, &graph, runtime.features())?;
    let session = compile_graph_session(
        &ctx,
        &prepared,
        BufferStorageMode::Shared,
        BufferStorageMode::Shared,
    )?;
    let a_bytes_le = f32s(&ones);
    let label = if chain == 1 { "isolated" } else { "chained" };
    bench_session(
        label, &ctx, &session, input, out, &a_bytes_le, m, k, n, chain, warmup, iters,
    )
}

fn f32s(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn ggml_pad(n: usize, align: usize) -> usize {
    n.div_ceil(align) * align
}

fn main() {
    let shapes = [
        (256usize, 3072usize, 3072usize),
        (256, 3072, 9216),
        (256, 3072, 12288),
        (128, 3072, 3072),
    ];
    for (m, k, n) in shapes {
        if let Err(err) = bench_one(m, k, n, 1, 5, 15) {
            eprintln!("FAIL m={m} k={k} n={n}: {err}");
        }
    }
    // Fair 228-seq compare vs MLX's 0.315s for 228 hidden_sq GEMMs.
    if let Err(err) = bench_one(256, 3072, 3072, 228, 2, 4) {
        eprintln!("FAIL chain228: {err}");
    }
}
