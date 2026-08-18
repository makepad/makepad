//! Isolated official `kernel_flash_attn_ext_*_dk128_dv128` at Flux shapes.

use makepad_ggml::backend::metal::{
    compile_graph_session, prepare_graph, BufferStorageMode, MetalGraphTensorWrite,
};
use makepad_ggml::{BufferUsage, Context, Graph, InitParams, TensorType, GGML_MEM_ALIGN};
use std::time::Instant;

fn summarize(times: &mut [f64]) -> (f64, f64, f64) {
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (times[times.len() / 2], times[0], times[times.len() - 1])
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

fn bench_flash(
    label: &str,
    tokens: usize,
    heads: usize,
    dim: usize,
    ty: TensorType,
    chain: usize,
    warmup: u32,
    iters: u32,
) -> Result<(), String> {
    let elems = dim * tokens * heads;
    let q_bytes = elems * ty.scalar_size_bytes().unwrap_or(4);
    let extra = 16 << 20;
    let mut ctx = Context::new(InitParams {
        mem_size: ggml_pad(q_bytes * 3 * (chain + 1) + extra, GGML_MEM_ALIGN),
        mem_buffer: None,
        no_alloc: false,
    });
    // Flux after permute: [dk, tokens, heads]
    let q = ctx
        .new_tensor_3d(ty, dim as i64, tokens as i64, heads as i64, BufferUsage::Activations)
        .map_err(|e| e)?;
    let k = ctx
        .new_tensor_3d(ty, dim as i64, tokens as i64, heads as i64, BufferUsage::Activations)
        .map_err(|e| e)?;
    let v = ctx
        .new_tensor_3d(ty, dim as i64, tokens as i64, heads as i64, BufferUsage::Activations)
        .map_err(|e| e)?;
    let ones = vec![0.01f32; elems];
    let bytes = f32s(&ones);
    if ty == TensorType::F32 {
        ctx.write_tensor_data(q, &bytes)?;
        ctx.write_tensor_data(k, &bytes)?;
        ctx.write_tensor_data(v, &bytes)?;
    } else {
        let f16 = f32_bytes_to_f16(&bytes);
        ctx.write_tensor_data(q, &f16)?;
        ctx.write_tensor_data(k, &f16)?;
        ctx.write_tensor_data(v, &f16)?;
    }
    let scale = 1.0 / (dim as f32).sqrt();
    let mut out = ctx.flash_attn_ext(q, k, v, None, scale, 0.0, 0.0, BufferUsage::Activations)?;
    ctx.flash_attn_ext_set_prec(out, makepad_ggml::Prec::F32)?;
    for _ in 1..chain {
        out = ctx.flash_attn_ext(q, k, v, None, scale, 0.0, 0.0, BufferUsage::Activations)?;
        ctx.flash_attn_ext_set_prec(out, makepad_ggml::Prec::F32)?;
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

    let writes: Vec<MetalGraphTensorWrite> = vec![];
    for _ in 0..warmup {
        let _ = session.execute(&ctx, &writes, &[])?;
        session.runtime().wait_idle()?;
    }
    let mut gpu_wall = Vec::with_capacity(iters as usize);
    let mut gpu_ts = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        session.runtime().reset_counters();
        let t0 = Instant::now();
        let _ = session.execute(&ctx, &writes, &[])?;
        session.runtime().wait_idle()?;
        gpu_wall.push(t0.elapsed().as_secs_f64() * 1000.0);
        gpu_ts.push(session.runtime().counters().gpu_elapsed_ns as f64 / 1e6);
    }
    let (wall_med, wall_min, wall_max) = summarize(&mut gpu_wall);
    let (ts_med, ts_min, ts_max) = summarize(&mut gpu_ts);
    eprintln!(
        "flash {label} ty={} T={tokens} H={heads} D={dim} chain={chain} nodes={}",
        ty.name(),
        session.compiled().nodes.len()
    );
    eprintln!(
        "  gpu-wall med={wall_med:.3} ms  min={wall_min:.3} max={wall_max:.3}  per={:.3} ms",
        wall_med / chain as f64
    );
    eprintln!(
        "  gpu-ts   med={ts_med:.3} ms  min={ts_min:.3} max={ts_max:.3}  per={:.3} ms",
        ts_med / chain as f64
    );
    if chain == 57 {
        eprintln!(
            "  57 flashes ~= {:.3}s of a 3.54s Flux step ({:.0}%)",
            wall_med / 1000.0,
            100.0 * wall_med / 3540.0
        );
    }
    Ok(())
}

fn f32_bytes_to_f16(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(4) {
        let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        out.extend_from_slice(&makepad_ggml::quant::f32_to_f16(v).to_le_bytes());
    }
    out
}

fn main() {
    let cases = [
        ("joint512", 512usize, 24usize, 128usize, TensorType::F32),
        ("img256", 256, 24, 128, TensorType::F32),
        ("joint512_f16", 512, 24, 128, TensorType::F16),
        ("img256_f16", 256, 24, 128, TensorType::F16),
    ];
    for (label, t, h, d, ty) in cases {
        if let Err(err) = bench_flash(label, t, h, d, ty, 1, 5, 15) {
            eprintln!("FAIL {label}: {err}");
        }
    }
    if let Err(err) = bench_flash("joint512_x57", 512, 24, 128, TensorType::F32, 57, 2, 4) {
        eprintln!("FAIL chain57: {err}");
    }
    if let Err(err) = bench_flash("joint512_f16_x57", 512, 24, 128, TensorType::F16, 57, 2, 4) {
        eprintln!("FAIL chain57f16: {err}");
    }
}